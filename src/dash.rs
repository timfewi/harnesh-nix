//! Interactive dashboard. It keeps no state beyond the current screen: every
//! refresh re-reads the panes from tmux.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, Gauge, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::ops::{Ctx, SpawnRequest};
use crate::tmux::Pane;

const REFRESH: Duration = Duration::from_millis(1000);
const MOTION: Duration = Duration::from_millis(210);

// Runtime design tokens; see DESIGN.md. Use indexed colors for tmux compatibility.
const BACKGROUND: Color = Color::Indexed(234);
const INK: Color = Color::Indexed(253);
const MUTED: Color = Color::Indexed(248);
const SELECTION: Color = Color::Indexed(237);
const DANGER: Color = Color::Indexed(210);
const HARNESS_PALETTE: [u8; 10] = [81, 215, 150, 183, 221, 117, 211, 159, 179, 111];

fn panel<'a>(title: impl Into<Line<'a>>, color: Color) -> Block<'a> {
    Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(title)
}

/// Deterministic across refreshes and restarts; resolve palette collisions so
/// configured harnesses have distinct colors. Labels also identify every harness.
fn harness_colors<'a>(names: impl Iterator<Item = &'a str>) -> HashMap<String, Color> {
    let palette: Vec<u8> = HARNESS_PALETTE
        .into_iter()
        .chain((16..=231).filter(|c| !HARNESS_PALETTE.contains(c) && readable_cube_color(*c)))
        .collect();
    let mut colors = HashMap::new();
    for name in names {
        let hash = name.bytes().fold(0usize, |h, b| {
            h.wrapping_mul(31).wrapping_add(usize::from(b))
        });
        let start = hash % HARNESS_PALETTE.len();
        let color = (0..palette.len())
            .map(|offset| Color::Indexed(palette[(start + offset) % palette.len()]))
            .find(|c| !colors.values().any(|v| v == c))
            .unwrap_or(INK);
        colors.insert(name.to_owned(), color);
    }
    colors
}

/// xterm cube colors must retain 4.5:1 text contrast on SELECTION (#3a3a3a).
fn readable_cube_color(index: u8) -> bool {
    let cube = usize::from(index - 16);
    let levels: [f64; 6] = [0.0, 95.0, 135.0, 175.0, 215.0, 255.0];
    let linear = |level: usize| {
        let channel = levels[level] / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(cube / 36) + 0.7152 * linear((cube / 6) % 6) + 0.0722 * linear(cube % 6)
        >= 0.366
}

fn excerpt(text: &str, width: usize) -> String {
    // Ratatui measures terminal cell widths, including wide Unicode glyphs.
    let mut value = String::new();
    for c in text.chars() {
        value.push(c);
        if Line::from(value.as_str()).width() > width {
            value.pop();
            while Line::from(value.as_str()).width() >= width && !value.is_empty() {
                value.pop();
            }
            if width > 0 {
                value.push('…');
            }
            break;
        }
    }
    value
}

fn state_label(pane: &Pane) -> String {
    match (pane.dead, pane.exit_status) {
        (false, _) => "running".to_owned(),
        (true, Some(code)) => format!("exited {code}"),
        (true, None) => "exited".to_owned(),
    }
}

enum Mode {
    Normal,
    Help { scroll: usize },
    Send { target: String, text: String },
    Task { target: String, text: String },
    Spawn(usize),
    ConfirmStop { target: String, name: String },
}

struct Dash<'a> {
    ctx: &'a Ctx,
    panes: Vec<Pane>,
    table: TableState,
    mode: Mode,
    message: String,
    colors: HashMap<String, Color>,
    expanded: Option<String>,
    animations: bool,
    selection_at: Instant,
    capacity_from: usize,
    capacity_to: usize,
    capacity_at: Instant,
    count: usize,
    pending_g: bool,
    page_size: usize,
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = Dash::new(ctx).event_loop(&mut terminal);
    ratatui::restore();
    result
}

impl<'a> Dash<'a> {
    fn new(ctx: &'a Ctx) -> Self {
        Self {
            ctx,
            panes: Vec::new(),
            table: TableState::default().with_selected(0),
            mode: Mode::Normal,
            message: String::new(),
            colors: harness_colors(ctx.config.adapters.keys().map(String::as_str)),
            expanded: None,
            animations: true,
            selection_at: Instant::now() - MOTION,
            capacity_from: 0,
            capacity_to: 0,
            capacity_at: Instant::now() - MOTION,
            count: 0,
            pending_g: false,
            page_size: 8,
        }
    }

    fn refresh(&mut self) {
        let selected = self.selected().map(|p| p.id.clone());
        match self.ctx.panes() {
            Ok(panes) => {
                self.panes = panes.into_iter().filter(|p| p.role != "dash").collect();
                let live = self
                    .panes
                    .iter()
                    .filter(|p| p.role == "agent" && !p.dead)
                    .count();
                if live != self.capacity_to {
                    self.capacity_from = self.capacity_to;
                    self.capacity_to = live;
                    self.capacity_at = Instant::now();
                }
                if let Some(index) = self
                    .panes
                    .iter()
                    .position(|p| Some(&p.id) == selected.as_ref())
                {
                    self.table.select(Some(index));
                } else {
                    self.expanded = None;
                }
            }
            Err(err) => {
                self.message = format!("{err:#}");
            }
        }
        let len = self.panes.len();
        let selected = self.table.selected().unwrap_or(0);
        self.table.select(Some(selected.min(len.saturating_sub(1))));
    }

    fn selected(&self) -> Option<&Pane> {
        self.table.selected().and_then(|i| self.panes.get(i))
    }

    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.refresh();
        let mut last = Instant::now();
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            let mut timeout = REFRESH.saturating_sub(last.elapsed());
            if self.animations
                && (self.selection_at.elapsed() < MOTION || self.capacity_at.elapsed() < MOTION)
            {
                timeout = timeout.min(Duration::from_millis(70));
            }
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press && !self.handle(key) => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
            if last.elapsed() >= REFRESH {
                self.refresh();
                last = Instant::now();
            }
        }
    }

    fn report(&mut self, result: Result<String>) {
        self.message = match result {
            Ok(message) => message,
            Err(err) => format!("error: {err:#}"),
        };
        self.refresh();
    }

    /// Returns false when the dashboard should exit.
    fn handle(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return false;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Normal => return self.handle_normal(key),
            Mode::Help { mut scroll } => match key.code {
                KeyCode::Char('a') => {
                    self.animations = !self.animations;
                    self.mode = Mode::Help { scroll };
                }
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Enter => {}
                KeyCode::Down | KeyCode::Char('j') => {
                    scroll = scroll.saturating_add(1);
                    self.mode = Mode::Help { scroll };
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    scroll = scroll.saturating_sub(1);
                    self.mode = Mode::Help { scroll };
                }
                KeyCode::PageDown => {
                    self.mode = Mode::Help {
                        scroll: scroll.saturating_add(5),
                    }
                }
                KeyCode::PageUp => {
                    self.mode = Mode::Help {
                        scroll: scroll.saturating_sub(5),
                    }
                }
                _ => self.mode = Mode::Help { scroll },
            },
            Mode::Task { target, mut text } => match key.code {
                KeyCode::Esc => self.message = "task edit cancelled".to_owned(),
                KeyCode::Enter => match self.ctx.tmux.set_task(&target, &text) {
                    Ok(()) => self.report(Ok("task saved".to_owned())),
                    Err(err) => {
                        self.message = format!("task not saved: {err:#}");
                        self.mode = Mode::Task { target, text };
                    }
                },
                KeyCode::Backspace => {
                    text.pop();
                    self.mode = Mode::Task { target, text };
                }
                KeyCode::Char(c) if !c.is_control() && text.chars().count() < 240 => {
                    text.push(c);
                    self.mode = Mode::Task { target, text };
                }
                _ => self.mode = Mode::Task { target, text },
            },
            Mode::Send { target, mut text } => match key.code {
                KeyCode::Esc => self.message = "send cancelled".to_owned(),
                KeyCode::Enter if !text.is_empty() => {
                    let result = self
                        .ctx
                        .tmux
                        .send(&target, &text, true)
                        .map(|()| format!("sent to {target}"));
                    self.report(result);
                }
                KeyCode::Backspace => {
                    text.pop();
                    self.mode = Mode::Send { target, text };
                }
                KeyCode::Char(c) if !c.is_control() => {
                    text.push(c);
                    self.mode = Mode::Send { target, text };
                }
                _ => self.mode = Mode::Send { target, text },
            },
            Mode::Spawn(index) => {
                let count = self.ctx.config.adapters.len();
                match key.code {
                    KeyCode::Esc => {}
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.mode = Mode::Spawn((index + 1) % count)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.mode = Mode::Spawn((index + count - 1) % count);
                    }
                    KeyCode::Enter => {
                        let adapter = self.ctx.config.adapters.keys().nth(index).cloned();
                        if let Some(adapter) = adapter {
                            let result = std::env::current_dir()
                                .map_err(anyhow::Error::from)
                                .and_then(|cwd| {
                                    self.ctx.spawn(&SpawnRequest {
                                        adapter: &adapter,
                                        name: None,
                                        cwd: &cwd,
                                        prompt: None,
                                        task: None,
                                        worktree: false,
                                    })
                                });
                            match result {
                                Ok(spawned) => {
                                    self.report(Ok(format!(
                                        "started {} in {}",
                                        spawned.name, spawned.pane
                                    )));
                                    if let Some(i) =
                                        self.panes.iter().position(|p| p.id == spawned.pane)
                                    {
                                        self.select_index(i);
                                    }
                                }
                                Err(err) => self.report(Err(err)),
                            }
                        }
                    }
                    _ => self.mode = Mode::Spawn(index),
                }
            }
            Mode::ConfirmStop { target, name } => {
                if key.code == KeyCode::Char('y') {
                    let result = self
                        .ctx
                        .tmux
                        .kill(&target)
                        .map(|()| format!("stopped {name}"));
                    self.report(result);
                } else {
                    self.message = "stop cancelled".to_owned();
                }
            }
        }
        true
    }

    fn select_index(&mut self, index: usize) {
        let next = index.min(self.panes.len().saturating_sub(1));
        if self.table.selected() != Some(next) {
            self.table.select(Some(next));
            self.expanded = None;
            self.selection_at = Instant::now();
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) -> bool {
        let has_selection = self.selected().is_some();
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            let distance = self.page_size.div_ceil(2).max(1) * self.count.max(1);
            match key.code {
                KeyCode::Char('d') => {
                    self.select_index(self.table.selected().unwrap_or(0).saturating_add(distance))
                }
                KeyCode::Char('u') => {
                    self.select_index(self.table.selected().unwrap_or(0).saturating_sub(distance))
                }
                _ => {}
            }
            self.count = 0;
            self.pending_g = false;
            return true;
        }
        if let KeyCode::Char(digit @ '1'..='9') = key.code {
            self.count = self
                .count
                .saturating_mul(10)
                .saturating_add(digit.to_digit(10).unwrap() as usize)
                .min(999);
            return true;
        }
        if self.count > 0 && key.code == KeyCode::Char('0') {
            self.count = self.count.saturating_mul(10).min(999);
            return true;
        }
        if key.code == KeyCode::Char('g') {
            if self.pending_g {
                self.select_index(self.count.saturating_sub(1));
                self.count = 0;
                self.pending_g = false;
            } else {
                self.pending_g = true;
            }
            return true;
        }
        self.pending_g = false;
        let requested = std::mem::take(&mut self.count);
        let count = requested.max(1);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_index(self.table.selected().unwrap_or(0).saturating_add(count));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_index(self.table.selected().unwrap_or(0).saturating_sub(count));
            }
            KeyCode::Char('G') => self.select_index(if requested > 0 {
                requested - 1
            } else {
                self.panes.len().saturating_sub(1)
            }),
            KeyCode::Home => self.select_index(0),
            KeyCode::End => self.select_index(self.panes.len().saturating_sub(1)),
            KeyCode::PageDown => self.select_index(
                self.table
                    .selected()
                    .unwrap_or(0)
                    .saturating_add(self.page_size.saturating_mul(count)),
            ),
            KeyCode::PageUp => self.select_index(
                self.table
                    .selected()
                    .unwrap_or(0)
                    .saturating_sub(self.page_size.saturating_mul(count)),
            ),
            KeyCode::Char('?') => self.mode = Mode::Help { scroll: 0 },
            KeyCode::Char(' ') | KeyCode::Char('l') if has_selection => {
                let id = self.selected().unwrap().id.clone();
                self.expanded = (self.expanded.as_deref() != Some(&id)).then_some(id);
            }
            KeyCode::Char('h') => self.expanded = None,
            KeyCode::Char('t') if has_selection => {
                if let Some(pane) = self.selected() {
                    self.mode = Mode::Task {
                        target: pane.id.clone(),
                        text: pane.task.clone().unwrap_or_default(),
                    };
                }
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('n') if !self.ctx.config.adapters.is_empty() => {
                self.mode = Mode::Spawn(0)
            }
            KeyCode::Char('n') => self.message = "no adapters configured".to_owned(),
            KeyCode::Char('s') if has_selection => {
                self.mode = Mode::Send {
                    target: self.selected().unwrap().id.clone(),
                    text: String::new(),
                }
            }
            KeyCode::Char('x') if has_selection => {
                let pane = self.selected().unwrap();
                self.mode = Mode::ConfirmStop {
                    target: pane.id.clone(),
                    name: pane.name.clone(),
                };
            }
            KeyCode::Char('i') if has_selection => {
                if let Some((id, name)) = self.selected().map(|p| (p.id.clone(), p.name.clone())) {
                    let result = self
                        .ctx
                        .tmux
                        .interrupt(&id)
                        .map(|()| format!("interrupted {name}"));
                    self.report(result);
                }
            }
            KeyCode::Enter if has_selection => {
                if let Some(id) = self.selected().map(|p| p.id.clone()) {
                    let result = self.ctx.tmux.focus(&id).map(|()| String::new());
                    self.report(result);
                }
            }
            _ => {}
        }
        true
    }

    fn color(&self, pane: &Pane) -> Color {
        self.colors.get(&pane.adapter).copied().unwrap_or(MUTED)
    }

    fn draw(&mut self, frame: &mut Frame) {
        frame.render_widget(
            Block::new().style(Style::new().fg(INK).bg(BACKGROUND)),
            frame.area(),
        );
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .areas(frame.area());
        let [list, detail] = if body.width >= 100 {
            Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(body)
        } else {
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(body)
        };
        self.page_size = usize::from(list.height.saturating_sub(2)).max(1);
        let live = self
            .panes
            .iter()
            .filter(|p| p.role == "agent" && !p.dead)
            .count();
        let exited = self
            .panes
            .iter()
            .filter(|p| p.role == "agent" && p.dead)
            .count();
        let gauge_width = if header.width >= 65 { 28 } else { 0 };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(" harnesh  /  {}", self.ctx.session())).bold(),
                Line::from(format!(
                    " {live}/{} running · {exited} exited",
                    self.ctx.config.max_agents
                ))
                .fg(MUTED),
            ]),
            Rect::new(
                header.x,
                header.y,
                header.width.saturating_sub(gauge_width),
                header.height,
            ),
        );
        if gauge_width > 0 {
            frame.render_widget(
                Gauge::default()
                    .ratio(self.capacity_ratio(live))
                    .label(format!("capacity {live}/{}", self.ctx.config.max_agents))
                    .gauge_style(Style::new().fg(Color::Indexed(81)).bg(SELECTION))
                    .use_unicode(true),
                Rect::new(
                    header.right().saturating_sub(gauge_width),
                    header.y + 1,
                    gauge_width,
                    1,
                ),
            );
        }

        let width = usize::from(list.width.saturating_sub(6));
        let rows = self.panes.iter().map(|pane| {
            let color = self.color(pane);
            let name = excerpt(&pane.name, width.min(22));
            let state = state_label(pane);
            let task = excerpt(
                pane.task.as_deref().unwrap_or("No task"),
                width.saturating_sub(Line::from(name.as_str()).width() + state.len() + 5),
            );
            let expanded = self.expanded.as_deref() == Some(&pane.id);
            let mut lines = vec![Line::from(vec![
                Span::styled(name, Style::new().fg(color).bold()),
                Span::raw(format!(" · {state}  ")),
                Span::styled(task, Style::new().fg(MUTED)),
            ])];
            if expanded {
                lines.push(Line::from(format!("  {} · {}", pane.adapter, pane.id)).fg(color));
                lines.push(
                    Line::from(format!("  {}", excerpt(&pane.cwd, width.saturating_sub(2))))
                        .fg(MUTED),
                );
            }
            Row::new([Text::from(lines)]).height(if expanded { 3 } else { 1 })
        });
        let selected = self
            .table
            .selected()
            .map_or(0, |i| i + 1)
            .min(self.panes.len());
        let table = Table::new(rows, [Constraint::Fill(1)])
            .row_highlight_style(Style::new().bg(self.selection_color()))
            .highlight_symbol("▎ ")
            .block(panel(
                format!(" Sessions {selected}/{} ", self.panes.len()),
                MUTED,
            ));
        frame.render_stateful_widget(table, list, &mut self.table);
        if self.panes.iter().all(|pane| pane.role != "agent") {
            frame.render_widget(
                Paragraph::new(
                    "No agents yet. Press n to start one.\nPress ? for the quick guide.",
                )
                .wrap(Wrap { trim: false }),
                panel("", MUTED).inner(list),
            );
        }
        self.draw_detail(frame, detail);
        let help = if self.message.is_empty() {
            "Space expand · ? guide / settings · q quit"
        } else {
            &self.message
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("j/k select · Enter open live pane · n new agent"),
                Line::from(help).fg(MUTED),
            ]),
            footer,
        );
        self.draw_mode(frame);
    }

    fn selection_color(&self) -> Color {
        if !self.animations {
            return SELECTION;
        }
        match self.selection_at.elapsed().as_millis() {
            0..=69 => Color::Indexed(239),
            70..=139 => Color::Indexed(238),
            _ => SELECTION,
        }
    }

    fn capacity_ratio(&self, live: usize) -> f64 {
        let visible = if self.animations && self.capacity_at.elapsed() < MOTION {
            let fraction = self.capacity_at.elapsed().as_secs_f64() / MOTION.as_secs_f64();
            self.capacity_from as f64
                + (self.capacity_to as f64 - self.capacity_from as f64) * fraction
        } else {
            live as f64
        };
        (visible / self.ctx.config.max_agents.max(1) as f64).clamp(0.0, 1.0)
    }

    fn draw_detail(&self, frame: &mut Frame, area: Rect) {
        let Some(pane) = self.selected() else {
            frame.render_widget(
                Paragraph::new("1. Press n to start an agent.\n2. Select it with j/k.\n3. Press Enter to open its live pane.\n\n? opens all shortcuts and settings.")
                    .block(panel(" Quick start ", MUTED)).wrap(Wrap { trim: false }),
                area,
            );
            return;
        };
        let color = self.color(pane);
        let task = pane
            .task
            .as_deref()
            .unwrap_or("No task description. Press t to add one.");
        let meta = Paragraph::new(vec![
            Line::from(task).bold(),
            Line::from(""),
            Line::from(format!(
                "{} · {} · {}",
                pane.id,
                state_label(pane),
                pane.command
            ))
            .fg(MUTED),
            Line::from(format!("Directory  {}", pane.cwd)).fg(MUTED),
            Line::from(format!("Window     {}", pane.window)).fg(MUTED),
            Line::from(format!(
                "Worktree   {}",
                pane.worktree.as_deref().unwrap_or("none")
            ))
            .fg(MUTED),
            Line::from(""),
            Line::from("Enter opens the real agent pane.").fg(color),
            Line::from("No terminal output is mirrored here.").fg(MUTED),
        ])
        .wrap(Wrap { trim: false })
        .block(panel(format!(" {} ", pane.name), color));
        frame.render_widget(meta, area);
    }

    fn draw_mode(&self, frame: &mut Frame) {
        let (title, text, color) = match &self.mode {
            Mode::Normal => return,
            Mode::Help { .. } => (
                " Help & settings · j/k scroll · Esc close ",
                format!(
                    "QUICK START\n  n  start an agent · j/k  select · Enter  open live pane\n\nNAVIGATION\n  j/k  move · 5j  move five · gg/G  first/last\n  Ctrl-u/d  half page · PgUp/PgDn  page\n  Space/l  expand · h  collapse\n\nACTIONS\n  s  send · t  task · i  interrupt · x  stop\n  r  refresh · q  quit\n\nSETTINGS (this dashboard only)\n  a  animations: {}\n\nThe agent terminal is never mirrored here.",
                    if self.animations { "on" } else { "off" }
                ),
                INK,
            ),
            Mode::Send { text, .. } => (
                " Send input · Enter submit · Esc cancel ",
                format!("{text}▏"),
                INK,
            ),
            Mode::Task { text, .. } => {
                (" Task · Enter save · Esc cancel ", format!("{text}▏"), INK)
            }
            Mode::Spawn(index) => {
                let choices = self
                    .ctx
                    .config
                    .adapters
                    .keys()
                    .enumerate()
                    .map(|(i, name)| format!("{} {name}", if i == *index { ">" } else { " " }))
                    .collect::<Vec<_>>()
                    .join("\n");
                (
                    " New agent · j/k choose · Enter start · Esc cancel ",
                    choices,
                    INK,
                )
            }
            Mode::ConfirmStop { name, .. } => (
                " Stop session ",
                format!("Stop {name} and terminate its process?\n\ny stop · any other key cancels"),
                DANGER,
            ),
        };
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(76);
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        let line_count = paragraph.line_count(width.saturating_sub(2));
        let height = (line_count as u16)
            .saturating_add(2)
            .min(area.height.saturating_sub(2));
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let scroll = match self.mode {
            Mode::Help { scroll } => {
                scroll.min(line_count.saturating_sub(usize::from(height.saturating_sub(2))))
            }
            Mode::Spawn(i) => i.saturating_sub(usize::from(height.saturating_sub(3))),
            _ => line_count.saturating_sub(usize::from(height.saturating_sub(2))),
        };
        frame.render_widget(Clear, popup);
        frame.render_widget(
            paragraph
                .scroll((scroll.min(usize::from(u16::MAX)) as u16, 0))
                .style(Style::new().fg(INK).bg(SELECTION))
                .block(panel(title, color)),
            popup,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, dead: bool) -> Pane {
        Pane {
            id: id.to_owned(),
            role: "agent".to_owned(),
            name: id.to_owned(),
            adapter: String::new(),
            started: None,
            worktree: None,
            window: String::new(),
            dead,
            exit_status: dead.then_some(1),
            cwd: String::new(),
            command: String::new(),
            task: None,
        }
    }

    fn rendered(dash: &mut Dash<'_>, width: u16, height: u16) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| dash.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn compact_sessions_switch_without_mirroring_terminal_output() {
        let ctx = Ctx {
            tmux: crate::tmux::Tmux::default(),
            config: crate::config::Config::default(),
            config_path: None,
        };
        let mut dash = Dash::new(&ctx);
        let mut first = pane("alpha", false);
        first.task = Some("Repair login".to_owned());
        let mut second = pane("beta", false);
        second.task = Some("Review tests".to_owned());
        dash.panes = vec![first, second];
        for (width, height) in [(140, 36), (80, 24)] {
            let screen = rendered(&mut dash, width, height);
            for value in ["Repair login", "Review tests", "capacity", "open live pane"] {
                assert!(screen.contains(value), "missing {value}:\n{screen}");
            }
            assert!(!screen.contains("output line"));
        }
        dash.handle(KeyCode::Char(' ').into());
        assert_eq!(dash.expanded.as_deref(), Some("alpha"));
        dash.handle_normal(KeyCode::Down.into());
        assert_eq!(dash.selected().unwrap().id, "beta");
        assert!(dash.expanded.is_none());
        dash.handle_normal(KeyCode::Down.into());
        assert_eq!(dash.selected().unwrap().id, "beta");
        for (width, height) in [(40, 12), (12, 5), (1, 1)] {
            rendered(&mut dash, width, height);
        }
    }

    #[test]
    fn colors_are_distinct_and_unicode_excerpts_fit_terminal_cells() {
        let names = [
            "aider", "claude", "codex", "gemini", "opencode", "pi", "review", "custom",
        ];
        let colors = harness_colors(names.into_iter());
        assert_eq!(colors, harness_colors(names.into_iter()));
        for (a, color) in &colors {
            assert!(!colors.iter().any(|(b, other)| a != b && color == other));
        }
        assert_eq!(excerpt("界界界", 5), "界界…");
        assert_eq!(excerpt("text", 0), "");
    }

    #[test]
    fn help_and_pinned_dialogs_render_after_selection_changes() {
        let ctx = Ctx {
            tmux: crate::tmux::Tmux::default(),
            config: crate::config::Config::default(),
            config_path: None,
        };
        let mut dash = Dash::new(&ctx);
        dash.panes = vec![pane("alpha", false), pane("beta", false)];
        dash.handle(KeyCode::Char('?').into());
        assert!(rendered(&mut dash, 100, 30).contains("QUICK START"));
        assert!(rendered(&mut dash, 40, 12).contains("QUICK START"));
        dash.handle(KeyCode::PageDown.into());
        assert!(matches!(dash.mode, Mode::Help { scroll: 5 }));
        dash.handle(KeyCode::Char('a').into());
        assert!(!dash.animations);
        assert!(rendered(&mut dash, 100, 30).contains("animations: off"));
        dash.handle(KeyCode::Esc.into());
        assert!(matches!(dash.mode, Mode::Normal));

        dash.handle(KeyCode::Char('s').into());
        dash.handle(KeyCode::Char('界').into());
        dash.select_index(1);
        assert!(matches!(&dash.mode, Mode::Send { target, text }
            if target == "alpha" && text == "界"));
        let screen = rendered(&mut dash, 100, 30);
        // Wide glyphs occupy a second terminal cell in the captured buffer.
        assert!(screen.contains("界 ▏"), "{screen}");
        dash.handle(KeyCode::Esc.into());

        dash.handle(KeyCode::Char('x').into());
        dash.select_index(0);
        let screen = rendered(&mut dash, 100, 30);
        assert!(screen.contains("Stop beta and terminate its process?"));
        assert!(!screen.contains("Stop alpha and terminate its process?"));
        dash.handle(KeyCode::Esc.into());
        assert!(matches!(dash.mode, Mode::Normal));
    }

    #[test]
    fn empty_and_task_editor_states_remain_keyboard_accessible() {
        let ctx = Ctx {
            tmux: crate::tmux::Tmux::default(),
            config: crate::config::Config::default(),
            config_path: None,
        };
        let mut dash = Dash::new(&ctx);
        assert!(rendered(&mut dash, 80, 24).contains("Press n"));
        dash.panes.push(pane("alpha", false));
        dash.table.select(Some(0));
        dash.handle_normal(KeyCode::Char('t').into());
        dash.handle(KeyEvent::new(KeyCode::Char('界'), KeyModifiers::NONE));
        assert!(
            matches!(&dash.mode, Mode::Task { target, text } if target == "alpha" && text == "界")
        );
        assert!(rendered(&mut dash, 80, 24).contains("Enter save"));
        dash.handle(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(dash.mode, Mode::Normal));
        assert!(dash.panes[0].task.is_none());
    }

    #[test]
    fn vim_navigation_and_observed_process_state() {
        let ctx = Ctx {
            tmux: crate::tmux::Tmux::default(),
            config: crate::config::Config::default(),
            config_path: None,
        };
        let mut dash = Dash::new(&ctx);
        dash.panes = (0..30)
            .map(|n| pane(&format!("agent-{n}"), n == 29))
            .collect();
        dash.handle(KeyCode::Char('2').into());
        dash.handle(KeyCode::Char('0').into());
        dash.handle(KeyCode::Char('j').into());
        assert_eq!(dash.table.selected(), Some(20));
        dash.handle(KeyCode::Char('g').into());
        dash.handle(KeyCode::Char('g').into());
        assert_eq!(dash.table.selected(), Some(0));
        dash.handle(KeyCode::Char('G').into());
        assert_eq!(dash.table.selected(), Some(29));
        assert_eq!(state_label(dash.selected().unwrap()), "exited 1");
        dash.handle(KeyCode::Char('1').into());
        dash.handle(KeyCode::Char('G').into());
        assert_eq!(dash.table.selected(), Some(0));
        dash.handle(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(dash.table.selected().unwrap() > 0);
        assert_eq!(state_label(&pane("live", false)), "running");
        dash.capacity_from = 0;
        dash.capacity_to = 3;
        dash.capacity_at = Instant::now();
        assert!(dash.capacity_ratio(3) < 0.5);
        dash.animations = false;
        assert_eq!(dash.capacity_ratio(3), 0.5);
    }
}
