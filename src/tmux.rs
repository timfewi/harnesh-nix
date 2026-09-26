//! tmux is the universal adapter and the only state store: every agent is a
//! pane, and harnesh records its metadata as `@harnesh_*` pane options. Nothing
//! is written outside the tmux server, so killing the session removes all
//! harnesh state.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};
use serde::Serialize;

const FIELD_SEP: char = '\u{1f}';
pub const AGENTS_WINDOW: &str = "agents";
pub const CONTROL_WINDOW: &str = "control";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pane {
    pub id: String,
    pub role: String,
    pub name: String,
    pub adapter: String,
    pub started: Option<u64>,
    pub worktree: Option<String>,
    pub window: String,
    pub dead: bool,
    pub exit_status: Option<i32>,
    pub cwd: String,
    pub command: String,
}

const PANE_FORMAT: &[&str] = &[
    "#{pane_id}",
    "#{@harnesh_role}",
    "#{@harnesh_name}",
    "#{@harnesh_adapter}",
    "#{@harnesh_started}",
    "#{@harnesh_worktree}",
    "#{window_name}",
    "#{pane_dead}",
    "#{pane_dead_status}",
    "#{pane_current_path}",
    "#{pane_current_command}",
];

fn pane_format() -> String {
    PANE_FORMAT.join(&FIELD_SEP.to_string())
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

/// Parses `list-panes` output. Panes without a harnesh role are ignored.
pub fn parse_panes(text: &str) -> Vec<Pane> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(FIELD_SEP).collect();
            if fields.len() != PANE_FORMAT.len() || fields[1].is_empty() {
                return None;
            }
            Some(Pane {
                id: fields[0].to_owned(),
                role: fields[1].to_owned(),
                name: fields[2].to_owned(),
                adapter: fields[3].to_owned(),
                started: fields[4].parse().ok(),
                worktree: non_empty(fields[5]),
                window: fields[6].to_owned(),
                dead: fields[7] == "1",
                exit_status: fields[8].parse().ok(),
                cwd: fields[9].to_owned(),
                command: fields[10].to_owned(),
            })
        })
        .collect()
}

/// POSIX single-quote each argument so tmux's shell runs argv unchanged.
pub fn shell_join(argv: &[String]) -> String {
    let quoted: Vec<String> = argv
        .iter()
        .map(|arg| {
            if !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
            {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', r"'\''"))
            }
        })
        .collect();
    format!("exec {}", quoted.join(" "))
}

/// Last `lines` non-trailing-blank lines of captured pane text.
pub fn tail_lines(text: &str, lines: usize) -> Vec<String> {
    let mut all: Vec<&str> = text.lines().collect();
    while all.last().is_some_and(|line| line.trim().is_empty()) {
        all.pop();
    }
    let start = all.len().saturating_sub(lines);
    all[start..]
        .iter()
        .map(|line| line.trim_end().to_owned())
        .collect()
}

/// A tmux server, optionally isolated with `-L socket`.
#[derive(Debug, Clone, Default)]
pub struct Tmux {
    pub socket: Option<String>,
}

pub struct NewPane<'a> {
    pub role: &'a str,
    pub name: &'a str,
    pub adapter: &'a str,
    pub cwd: &'a str,
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    pub worktree: Option<&'a str>,
}

impl Tmux {
    pub fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        if let Some(socket) = &self.socket {
            command.args(["-L", socket]);
        }
        command
    }

    /// A non-interactive client. `-u` makes tmux treat it as UTF-8: without a
    /// UTF-8 locale, tmux replaces control characters in command output with
    /// `_`, which would corrupt the field separator of `list-panes`.
    fn query(&self) -> Command {
        let mut command = self.command();
        command.arg("-u");
        command
    }

    fn run(&self, args: &[&str]) -> Result<Output> {
        self.query()
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| "cannot run tmux; is it installed and on PATH?".to_string())
    }

    fn checked(&self, args: &[&str]) -> Result<String> {
        let output = self.run(args)?;
        if !output.status.success() {
            bail!(
                "tmux {} failed: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn has_session(&self, session: &str) -> Result<bool> {
        Ok(self
            .run(&["has-session", "-t", &exact(session)])?
            .status
            .success())
    }

    fn has_window(&self, session: &str, window: &str) -> Result<bool> {
        let output = self.run(&[
            "list-windows",
            "-t",
            &exact(session),
            "-F",
            "#{window_name}",
        ])?;
        Ok(output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|name| name == window))
    }

    pub fn list(&self, session: &str) -> Result<Vec<Pane>> {
        if !self.has_session(session)? {
            return Ok(Vec::new());
        }
        let text = self.checked(&[
            "list-panes",
            "-s",
            "-t",
            &exact(session),
            "-F",
            &pane_format(),
        ])?;
        Ok(parse_panes(&text))
    }

    /// Creates the session with the dashboard as its first pane.
    pub fn create_session(&self, session: &str, pane: &NewPane<'_>) -> Result<String> {
        let mut args = vec![
            "new-session".to_owned(),
            "-d".to_owned(),
            "-P".to_owned(),
            "-F".to_owned(),
            "#{pane_id}".to_owned(),
            "-s".to_owned(),
            session.to_owned(),
            "-n".to_owned(),
            CONTROL_WINDOW.to_owned(),
            "-x".to_owned(),
            "240".to_owned(),
            "-y".to_owned(),
            "64".to_owned(),
        ];
        push_common(&mut args, pane);
        self.create(args, pane)
    }

    /// Opens a pane in `window`, creating the window when it does not exist.
    pub fn open_pane(&self, session: &str, window: &str, pane: &NewPane<'_>) -> Result<String> {
        let target = format!("{}:{window}", exact(session));
        let mut args: Vec<String> = if self.has_window(session, window)? {
            [
                "split-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                &target,
            ]
            .map(str::to_owned)
            .to_vec()
        } else {
            let session_target = format!("{}:", exact(session));
            [
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                &session_target,
                "-n",
                window,
            ]
            .map(str::to_owned)
            .to_vec()
        };
        push_common(&mut args, pane);
        let id = self.create(args, pane)?;
        let layout = if window == AGENTS_WINDOW {
            "tiled"
        } else {
            "main-vertical"
        };
        self.checked(&["select-layout", "-t", &target, layout])?;
        Ok(id)
    }

    fn create(&self, args: Vec<String>, pane: &NewPane<'_>) -> Result<String> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let id = self.checked(&refs)?.trim().to_owned();
        if !id.starts_with('%') {
            bail!("tmux did not report a pane id (got {id:?})");
        }
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default()
            .to_string();
        let mut options = vec![
            ("remain-on-exit", "on"),
            ("@harnesh_role", pane.role),
            ("@harnesh_name", pane.name),
            ("@harnesh_adapter", pane.adapter),
            ("@harnesh_started", started.as_str()),
        ];
        if let Some(worktree) = pane.worktree {
            options.push(("@harnesh_worktree", worktree));
        }
        for (key, value) in options {
            self.checked(&["set-option", "-p", "-t", &id, key, value])?;
        }
        self.checked(&["select-pane", "-t", &id, "-T", pane.name])?;
        Ok(id)
    }

    pub fn capture(&self, pane: &str, lines: usize) -> Result<Vec<String>> {
        let start = format!("-{}", lines.max(1));
        let text = self.checked(&["capture-pane", "-p", "-J", "-t", pane, "-S", &start])?;
        Ok(tail_lines(&text, lines))
    }

    /// Delivers text as a bracketed paste, so multi-line prompts arrive as one
    /// message, then optionally submits it with Enter.
    pub fn send(&self, pane: &str, text: &str, submit: bool) -> Result<()> {
        let buffer = format!("harnesh-{}", std::process::id());
        let mut child = self
            .query()
            .args(["load-buffer", "-b", &buffer, "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot run tmux load-buffer")?;
        child
            .stdin
            .take()
            .context("tmux load-buffer has no stdin")?
            .write_all(text.as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!(
                "tmux load-buffer failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        self.checked(&["paste-buffer", "-p", "-d", "-b", &buffer, "-t", pane])?;
        if submit {
            self.checked(&["send-keys", "-t", pane, "Enter"])?;
        }
        Ok(())
    }

    pub fn interrupt(&self, pane: &str) -> Result<()> {
        self.checked(&["send-keys", "-t", pane, "C-c"]).map(drop)
    }

    pub fn kill(&self, pane: &str) -> Result<()> {
        self.checked(&["kill-pane", "-t", pane]).map(drop)
    }

    pub fn focus(&self, pane: &str) -> Result<()> {
        self.checked(&["select-window", "-t", pane])?;
        self.checked(&["select-pane", "-t", pane]).map(drop)
    }

    pub fn kill_session(&self, session: &str) -> Result<()> {
        self.checked(&["kill-session", "-t", &exact(session)])
            .map(drop)
    }
}

fn push_common(args: &mut Vec<String>, pane: &NewPane<'_>) {
    args.push("-c".to_owned());
    args.push(pane.cwd.to_owned());
    for (key, value) in pane.env {
        args.push("-e".to_owned());
        args.push(format!("{key}={value}"));
    }
    args.push(shell_join(pane.argv));
}

/// `=name` makes tmux match the session name exactly instead of by prefix.
fn exact(session: &str) -> String {
    format!("={session}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }

    #[test]
    fn quotes_arguments_for_the_shell() {
        assert_eq!(shell_join(&s(&["codex"])), "exec codex");
        assert_eq!(
            shell_join(&s(&["claude", "fix the bug", "it's", ""])),
            r"exec claude 'fix the bug' 'it'\''s' ''"
        );
        assert_eq!(shell_join(&s(&["a;rm", "$HOME"])), "exec 'a;rm' '$HOME'");
    }

    #[test]
    fn parses_only_harnesh_panes() {
        let sep = FIELD_SEP.to_string();
        let agent = [
            "%3",
            "agent",
            "fix",
            "codex",
            "1700000000",
            "",
            "agents",
            "0",
            "",
            "/repo",
            "codex",
        ]
        .join(&sep);
        let dead = [
            "%4", "tool", "nvim", "", "", "/wt", "control", "1", "2", "/repo", "nvim",
        ]
        .join(&sep);
        let foreign = ["%5", "", "", "", "", "", "other", "0", "", "/", "zsh"].join(&sep);
        let panes = parse_panes(&format!("{agent}\n{dead}\n{foreign}\nbroken\n"));
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].name, "fix");
        assert_eq!(panes[0].started, Some(1_700_000_000));
        assert_eq!(panes[0].worktree, None);
        assert!(!panes[0].dead);
        assert!(panes[1].dead);
        assert_eq!(panes[1].exit_status, Some(2));
        assert_eq!(panes[1].worktree.as_deref(), Some("/wt"));
    }

    #[test]
    fn tails_without_trailing_blank_lines() {
        assert_eq!(tail_lines("a\nb\nc  \n\n  \n", 2), s(&["b", "c"]));
        assert_eq!(tail_lines("", 3), Vec::<String>::new());
        assert_eq!(tail_lines("x\n", 5), s(&["x"]));
    }
}
