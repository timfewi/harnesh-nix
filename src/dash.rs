//! Interactive dashboard. It keeps no state beyond the current screen: every
//! refresh re-reads the panes from tmux.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};
use ratatui::{DefaultTerminal, Frame};

use crate::ops::{Ctx, SpawnRequest};
use crate::tmux::Pane;

const REFRESH: Duration = Duration::from_millis(1000);
const BUSY_WINDOW: Duration = Duration::from_secs(3);
const PREVIEW_LINES: usize = 200;

/// Tracks when each pane's visible output last changed.
#[derive(Default)]
pub struct Activity {
    seen: HashMap<String, (u64, Instant)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Busy,
    Idle(Duration),
    Exited(Option<i32>),
}

impl Activity {
    pub fn observe(&mut self, pane: &str, lines: &[String], now: Instant) {
        let mut hasher = DefaultHasher::new();
        lines.hash(&mut hasher);
        let hash = hasher.finish();
        match self.seen.get_mut(pane) {
            Some((old, changed)) if *old != hash => {
                *old = hash;
                *changed = now;
            }
            Some(_) => {}
            None => {
                self.seen.insert(pane.to_owned(), (hash, now));
            }
        }
    }

    pub fn state(&self, pane: &Pane, now: Instant) -> State {
        if pane.dead {
            return State::Exited(pane.exit_status);
        }
        match self.seen.get(&pane.id) {
            Some((_, changed)) if now.duration_since(*changed) >= BUSY_WINDOW => {
                State::Idle(now.duration_since(*changed))
            }
            _ => State::Busy,
        }
    }

    pub fn retain(&mut self, panes: &[Pane]) {
        self.seen.retain(|id, _| panes.iter().any(|p| &p.id == id));
    }
}

fn describe(state: &State) -> String {
    match state {
        State::Busy => "busy".to_owned(),
        State::Idle(for_) => format!("idle {}s", for_.as_secs()),
        State::Exited(Some(code)) => format!("exited {code}"),
        State::Exited(None) => "exited".to_owned(),
    }
}

enum Mode {
    Normal,
    Send(String),
    Spawn(usize),
    ConfirmStop,
}

struct Dash<'a> {
    ctx: &'a Ctx,
    panes: Vec<Pane>,
    previews: HashMap<String, Vec<String>>,
    activity: Activity,
    table: TableState,
    mode: Mode,
    message: String,
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
            previews: HashMap::new(),
            activity: Activity::default(),
            table: TableState::default().with_selected(0),
            mode: Mode::Normal,
            message: String::new(),
        }
    }

    fn refresh(&mut self) {
        let now = Instant::now();
        match self.ctx.panes() {
            Ok(panes) => {
                self.panes = panes.into_iter().filter(|p| p.role != "dash").collect();
                self.activity.retain(&self.panes);
                self.previews
                    .retain(|id, _| self.panes.iter().any(|p| &p.id == id));
                for pane in &self.panes {
                    if let Ok(lines) = self.ctx.tmux.capture(&pane.id, PREVIEW_LINES) {
                        self.activity.observe(&pane.id, &lines, now);
                        self.previews.insert(pane.id.clone(), lines);
                    }
                }
            }
            Err(err) => self.message = format!("{err:#}"),
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
            let timeout = REFRESH.saturating_sub(last.elapsed());
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press && !self.handle(key) {
                        return Ok(());
                    }
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
            Mode::Normal => return self.handle_normal(key.code),
            Mode::Send(mut text) => match key.code {
                KeyCode::Esc => self.message = "send cancelled".to_owned(),
                KeyCode::Enter if !text.is_empty() => {
                    let target = self.selected().map(|p| (p.id.clone(), p.name.clone()));
                    if let Some((id, name)) = target {
                        let result = self
                            .ctx
                            .tmux
                            .send(&id, &text, true)
                            .map(|()| format!("sent to {name}"));
                        self.report(result);
                    }
                }
                KeyCode::Backspace => {
                    text.pop();
                    self.mode = Mode::Send(text);
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.mode = Mode::Send(text);
                }
                _ => self.mode = Mode::Send(text),
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
                                        worktree: false,
                                    })
                                })
                                .map(|s| format!("started {} in {}", s.name, s.pane));
                            self.report(result);
                        }
                    }
                    _ => self.mode = Mode::Spawn(index),
                }
            }
            Mode::ConfirmStop => {
                if key.code == KeyCode::Char('y') {
                    if let Some((id, name)) =
                        self.selected().map(|p| (p.id.clone(), p.name.clone()))
                    {
                        let result = self.ctx.tmux.kill(&id).map(|()| format!("stopped {name}"));
                        self.report(result);
                    }
                } else {
                    self.message = "stop cancelled".to_owned();
                }
            }
        }
        true
    }

    fn handle_normal(&mut self, code: KeyCode) -> bool {
        let has_selection = self.selected().is_some();
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Down | KeyCode::Char('j') => self.table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.table.select_previous(),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('n') if !self.ctx.config.adapters.is_empty() => {
                self.mode = Mode::Spawn(0)
            }
            KeyCode::Char('n') => self.message = "no adapters configured".to_owned(),
            KeyCode::Char('s') if has_selection => self.mode = Mode::Send(String::new()),
            KeyCode::Char('x') if has_selection => self.mode = Mode::ConfirmStop,
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

    fn draw(&mut self, frame: &mut Frame) {
        let now = Instant::now();
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [list, preview] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .areas(body);

        let agents = self.panes.iter().filter(|p| p.role == "agent");
        let live = agents.clone().filter(|p| !p.dead).count();
        let busy = agents
            .filter(|p| self.activity.state(p, now) == State::Busy)
            .count();
        frame.render_widget(
            Line::from(format!(
                " harnesh · session {} · {live}/{} agents running · {busy} busy",
                self.ctx.session(),
                self.ctx.config.max_agents
            ))
            .bold(),
            header,
        );

        let rows = self.panes.iter().map(|pane| {
            let state = self.activity.state(pane, now);
            let style = match state {
                State::Busy => Style::new().green(),
                State::Idle(_) => Style::new().yellow(),
                State::Exited(_) => Style::new().dark_gray(),
            };
            let kind = if pane.role == "agent" {
                self.ctx
                    .config
                    .adapters
                    .get(&pane.adapter)
                    .map_or(pane.adapter.as_str(), |a| a.display_name(&pane.adapter))
                    .to_owned()
            } else {
                pane.role.clone()
            };
            let place = pane.worktree.as_deref().unwrap_or(&pane.cwd);
            let place = place.rsplit('/').next().unwrap_or(place).to_owned();
            Row::new([
                Cell::from(pane.name.clone()),
                Cell::from(kind),
                Cell::from(describe(&state)).style(style),
                Cell::from(place),
            ])
        });
        let table = Table::new(
            rows,
            [
                Constraint::Fill(2),
                Constraint::Fill(2),
                Constraint::Length(11),
                Constraint::Fill(2),
            ],
        )
        .header(Row::new(["name", "agent", "state", "where"]).style(Style::new().bold()))
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .block(Block::bordered().title(" panes "));
        frame.render_stateful_widget(table, list, &mut self.table);

        let (title, text) = match self.selected() {
            Some(pane) => {
                let height = usize::from(preview.height.saturating_sub(2));
                let lines = self.previews.get(&pane.id).map_or(&[][..], Vec::as_slice);
                let start = lines.len().saturating_sub(height);
                (
                    format!(" {} ({}) ", pane.name, pane.id),
                    lines[start..]
                        .iter()
                        .map(|l| Line::from(l.as_str()))
                        .collect(),
                )
            }
            None => (
                " preview ".to_owned(),
                vec![Line::from("No panes yet. Press n to start an agent.")],
            ),
        };
        frame.render_widget(
            Paragraph::new(text).block(Block::bordered().title(title)),
            preview,
        );

        let status = match &self.mode {
            Mode::Normal if !self.message.is_empty() => self.message.clone(),
            Mode::Normal => {
                "j/k select · enter focus · n new · s send · i interrupt · x stop · q quit"
                    .to_owned()
            }
            Mode::Send(text) => format!("send> {text}█  (enter submit · esc cancel)"),
            Mode::Spawn(index) => {
                let names: Vec<String> = self
                    .ctx
                    .config
                    .adapters
                    .keys()
                    .enumerate()
                    .map(|(i, name)| {
                        if i == *index {
                            format!("[{name}]")
                        } else {
                            name.clone()
                        }
                    })
                    .collect();
                format!(
                    "new agent: {}  (j/k choose · enter start · esc cancel)",
                    names.join(" ")
                )
            }
            Mode::ConfirmStop => format!(
                "stop {}? y to confirm",
                self.selected().map_or("", |p| p.name.as_str())
            ),
        };
        frame.render_widget(Line::from(status).italic(), footer);
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
        }
    }

    #[test]
    fn output_changes_mark_a_pane_busy_until_it_settles() {
        let start = Instant::now();
        let mut activity = Activity::default();
        let lines = vec!["working".to_owned()];
        activity.observe("%1", &lines, start);
        assert_eq!(activity.state(&pane("%1", false), start), State::Busy);

        let later = start + Duration::from_secs(10);
        activity.observe("%1", &lines, later);
        assert_eq!(
            activity.state(&pane("%1", false), later),
            State::Idle(Duration::from_secs(10))
        );

        activity.observe("%1", &["done".to_owned()], later);
        assert_eq!(activity.state(&pane("%1", false), later), State::Busy);
        assert_eq!(
            activity.state(&pane("%1", true), later),
            State::Exited(Some(1))
        );
        assert_eq!(describe(&State::Idle(Duration::from_secs(4))), "idle 4s");
    }

    #[test]
    fn retain_forgets_closed_panes() {
        let now = Instant::now();
        let mut activity = Activity::default();
        activity.observe("%1", &[], now);
        activity.observe("%2", &[], now);
        activity.retain(&[pane("%2", false)]);
        assert!(!activity.seen.contains_key("%1"));
        assert!(activity.seen.contains_key("%2"));
    }
}
