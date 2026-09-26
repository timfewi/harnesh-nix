//! Operations shared by the CLI and the dashboard.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::config::{Config, valid_name};
use crate::tmux::{AGENTS_WINDOW, CONTROL_WINDOW, NewPane, Pane, Tmux};

pub struct Ctx {
    pub tmux: Tmux,
    pub config: Config,
    /// Explicit config path, forwarded to panes so a nested `harnesh` sees the
    /// same configuration.
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spawned {
    pub pane: String,
    pub name: String,
    pub adapter: String,
    pub cwd: String,
    pub worktree: Option<String>,
}

pub struct SpawnRequest<'a> {
    pub adapter: &'a str,
    pub name: Option<&'a str>,
    pub cwd: &'a Path,
    pub prompt: Option<&'a str>,
    pub worktree: bool,
}

impl Ctx {
    pub fn session(&self) -> &str {
        &self.config.session
    }

    /// Environment every harnesh pane receives, so agents inside can drive the
    /// same cockpit with plain `harnesh` commands.
    pub fn pane_env(&self) -> Vec<(String, String)> {
        let mut env = vec![("HARNESH_SESSION".to_owned(), self.session().to_owned())];
        if let Some(socket) = &self.tmux.socket {
            env.push(("HARNESH_TMUX_SOCKET".to_owned(), socket.clone()));
        }
        if let Some(path) = &self.config_path {
            env.push(("HARNESH_CONFIG".to_owned(), path.display().to_string()));
        }
        env
    }

    pub fn panes(&self) -> Result<Vec<Pane>> {
        self.tmux.list(self.session())
    }

    /// Creates the session with the dashboard and tool panes if it is absent.
    /// Returns whether it was created.
    pub fn ensure_session(&self, cwd: &Path) -> Result<bool> {
        if self.tmux.has_session(self.session())? {
            return Ok(false);
        }
        let exe = std::env::current_exe().context("cannot locate the harnesh executable")?;
        let dash = vec![exe.display().to_string(), "dash".to_owned()];
        let env = self.pane_env();
        let cwd = cwd.display().to_string();
        self.tmux.create_session(
            self.session(),
            &NewPane {
                role: "dash",
                name: "dash",
                adapter: "",
                cwd: &cwd,
                argv: &dash,
                env: &env,
                worktree: None,
            },
        )?;
        for tool in &self.config.tools {
            self.tmux.open_pane(
                self.session(),
                CONTROL_WINDOW,
                &NewPane {
                    role: "tool",
                    name: &tool.name,
                    adapter: "",
                    cwd: &cwd,
                    argv: &tool.argv,
                    env: &env,
                    worktree: None,
                },
            )?;
        }
        Ok(true)
    }

    pub fn spawn(&self, request: &SpawnRequest<'_>) -> Result<Spawned> {
        let adapter = self.config.adapters.get(request.adapter).with_context(|| {
            let known: Vec<&str> = self.config.adapters.keys().map(String::as_str).collect();
            format!(
                "unknown adapter {:?}; configured: {}",
                request.adapter,
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            )
        })?;
        let argv = adapter.command(request.adapter, request.prompt)?;
        self.ensure_session(request.cwd)?;
        // Names address panes in every command, so they must be unique across
        // agents, tools and the dashboard.
        let panes = self.panes()?;
        let name = match request.name {
            Some(name) => {
                if !valid_name(name) {
                    bail!("invalid agent name {name:?} (use letters, digits, '-', '_', '.')");
                }
                if panes.iter().any(|p| p.name == name) {
                    bail!("a pane named {name:?} already exists");
                }
                name.to_owned()
            }
            None => next_name(request.adapter, &panes),
        };
        let live = panes
            .iter()
            .filter(|p| p.role == "agent" && !p.dead)
            .count();
        if live >= self.config.max_agents {
            bail!(
                "{live} agents are running (maxAgents = {}); stop one first",
                self.config.max_agents
            );
        }
        let (cwd, worktree) = if request.worktree {
            let path = add_worktree(request.cwd, &name)?;
            (path.clone(), Some(path.display().to_string()))
        } else {
            (request.cwd.to_owned(), None)
        };
        let cwd = cwd.display().to_string();
        let mut env = self.pane_env();
        env.push(("HARNESH_AGENT".to_owned(), name.clone()));
        let pane = self.tmux.open_pane(
            self.session(),
            AGENTS_WINDOW,
            &NewPane {
                role: "agent",
                name: &name,
                adapter: request.adapter,
                cwd: &cwd,
                argv: &argv,
                env: &env,
                worktree: worktree.as_deref(),
            },
        )?;
        Ok(Spawned {
            pane,
            name,
            adapter: request.adapter.to_owned(),
            cwd,
            worktree,
        })
    }

    /// Resolves a pane id (`%3`) or a harnesh pane name.
    pub fn resolve(&self, target: &str) -> Result<Pane> {
        resolve_in(self.panes()?, target, self.session())
    }
}

pub fn resolve_in(panes: Vec<Pane>, target: &str, session: &str) -> Result<Pane> {
    let mut matches: Vec<Pane> = panes
        .into_iter()
        .filter(|p| p.id == target || p.name == target)
        .collect();
    match matches.len() {
        0 => bail!("no harnesh pane {target:?} in session {session:?}; see `harnesh list`"),
        1 => Ok(matches.remove(0)),
        _ => bail!("{target:?} is ambiguous; use the pane id from `harnesh list`"),
    }
}

pub fn next_name(adapter: &str, agents: &[Pane]) -> String {
    (1..)
        .map(|n| format!("{adapter}-{n}"))
        .find(|candidate| agents.iter().all(|p| &p.name != candidate))
        .expect("an unused name exists")
}

fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("cannot run git")
}

/// Adds `<parent>/<repo>.<name>` on branch `harnesh/<name>` so parallel agents
/// never edit the same checkout. The worktree is left in place on exit.
pub fn add_worktree(cwd: &Path, name: &str) -> Result<PathBuf> {
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        bail!(
            "--worktree needs a git repository, but {} is not one",
            cwd.display()
        );
    }
    let top = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    let repo = top
        .file_name()
        .context("repository root has no name")?
        .to_string_lossy()
        .into_owned();
    let parent = top.parent().context("repository root has no parent")?;
    let path = parent.join(format!("{repo}.{name}"));
    if path.exists() {
        bail!("worktree path {} already exists", path.display());
    }
    let branch = format!("harnesh/{name}");
    let exists = git(
        &top,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )?
    .status
    .success();
    let path_arg = path.display().to_string();
    let args: Vec<&str> = if exists {
        vec!["worktree", "add", &path_arg, &branch]
    } else {
        vec!["worktree", "add", "-b", &branch, &path_arg]
    };
    let output = git(&top, &args)?;
    if !output.status.success() {
        bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, name: &str) -> Pane {
        Pane {
            id: id.to_owned(),
            role: "agent".to_owned(),
            name: name.to_owned(),
            adapter: "codex".to_owned(),
            started: None,
            worktree: None,
            window: AGENTS_WINDOW.to_owned(),
            dead: false,
            exit_status: None,
            cwd: "/".to_owned(),
            command: "codex".to_owned(),
        }
    }

    #[test]
    fn names_count_up_past_used_names() {
        let agents = [pane("%1", "codex-1"), pane("%2", "codex-3")];
        assert_eq!(next_name("codex", &agents), "codex-2");
        assert_eq!(next_name("claude", &agents), "claude-1");
    }

    #[test]
    fn resolves_by_id_or_name_and_rejects_ambiguity() {
        let panes = || vec![pane("%1", "a"), pane("%2", "b"), pane("%3", "%1")];
        assert_eq!(resolve_in(panes(), "b", "s").unwrap().id, "%2");
        assert_eq!(resolve_in(panes(), "%2", "s").unwrap().name, "b");
        assert!(resolve_in(panes(), "%1", "s").is_err());
        assert!(resolve_in(panes(), "zzz", "s").is_err());
    }
}
