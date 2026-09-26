mod config;
mod dash;
mod ops;
mod tmux;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::ops::{Ctx, SpawnRequest};
use crate::tmux::Tmux;

/// Agent-agnostic tmux cockpit: run several coding agents side by side, watch
/// them live and steer them from a dashboard or from another agent.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Config file (default: $XDG_CONFIG_HOME/harnesh/config.json, then /etc/harnesh/config.json).
    #[arg(long, global = true, env = "HARNESH_CONFIG")]
    config: Option<PathBuf>,
    /// Use an isolated tmux server socket (tmux -L NAME).
    #[arg(long, global = true, env = "HARNESH_TMUX_SOCKET")]
    socket: Option<String>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the cockpit session (dashboard + tool panes) and attach to it.
    Up {
        /// Create the session without attaching.
        #[arg(long)]
        detached: bool,
    },
    /// Run the interactive dashboard in the current terminal.
    Dash,
    /// Start an agent in a new pane of the agents window.
    Spawn {
        /// Adapter name, see `harnesh adapters`.
        adapter: String,
        /// Agent name (default: <adapter>-<n>).
        #[arg(long)]
        name: Option<String>,
        /// Working directory (default: current directory).
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Initial prompt, passed through the adapter's promptArgv.
        #[arg(long)]
        prompt: Option<String>,
        /// Run the agent in a new git worktree next to the repository.
        #[arg(long)]
        worktree: bool,
    },
    /// List harnesh panes.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print the last lines of a pane.
    Capture {
        /// Pane id or name.
        target: String,
        #[arg(long, default_value_t = 60)]
        lines: usize,
    },
    /// Paste text into a pane and submit it (reads stdin when TEXT is omitted).
    Send {
        target: String,
        text: Option<String>,
        /// Paste without pressing Enter.
        #[arg(long)]
        no_enter: bool,
    },
    /// Send Ctrl-C to a pane.
    Interrupt { target: String },
    /// Kill a pane.
    Stop { target: String },
    /// Kill the whole cockpit session.
    Down,
    /// List the configured adapters.
    Adapters {
        #[arg(long)]
        json: bool,
    },
    /// Print the effective configuration and where it came from.
    Config,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("harnesh: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let config_path = cli
        .config
        .as_deref()
        .map(std::path::absolute)
        .transpose()
        .context("cannot resolve --config")?;
    let (config, source) = config::load(config_path.as_deref())?;
    let ctx = Ctx {
        tmux: Tmux { socket: cli.socket },
        config,
        config_path,
    };
    let cwd = std::env::current_dir().context("cannot read the current directory")?;

    match cli.command {
        Cmd::Up { detached } => {
            let created = ctx.ensure_session(&cwd)?;
            if detached {
                println!(
                    "session {} {}",
                    ctx.session(),
                    if created {
                        "created"
                    } else {
                        "already running"
                    }
                );
                return Ok(());
            }
            attach(&ctx)
        }
        Cmd::Dash => dash::run(&ctx),
        Cmd::Spawn {
            adapter,
            name,
            cwd: dir,
            prompt,
            worktree,
        } => {
            let dir = match dir {
                Some(dir) => std::path::absolute(dir)?,
                None => cwd,
            };
            if !dir.is_dir() {
                bail!("{} is not a directory", dir.display());
            }
            let spawned = ctx.spawn(&SpawnRequest {
                adapter: &adapter,
                name: name.as_deref(),
                cwd: &dir,
                prompt: prompt.as_deref(),
                worktree,
            })?;
            println!("{}", serde_json::to_string(&spawned)?);
            Ok(())
        }
        Cmd::List { json } => {
            let panes = ctx.panes()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&panes)?);
            } else {
                for pane in panes.iter().filter(|p| p.role != "dash") {
                    let state = match (pane.dead, pane.exit_status) {
                        (true, Some(code)) => format!("exited {code}"),
                        (true, None) => "exited".to_owned(),
                        (false, _) => "running".to_owned(),
                    };
                    println!(
                        "{}\t{}\t{}\t{}\t{}\t{}",
                        pane.id, pane.role, pane.name, pane.adapter, state, pane.cwd
                    );
                }
            }
            Ok(())
        }
        Cmd::Capture { target, lines } => {
            let pane = ctx.resolve(&target)?;
            for line in ctx.tmux.capture(&pane.id, lines)? {
                println!("{line}");
            }
            Ok(())
        }
        Cmd::Send {
            target,
            text,
            no_enter,
        } => {
            let text = match text {
                Some(text) if text != "-" => text,
                _ => {
                    let mut buffer = String::new();
                    std::io::stdin().read_to_string(&mut buffer)?;
                    buffer.trim_end_matches('\n').to_owned()
                }
            };
            if text.is_empty() {
                bail!("refusing to send empty text");
            }
            let pane = ctx.resolve(&target)?;
            ctx.tmux.send(&pane.id, &text, !no_enter)
        }
        Cmd::Interrupt { target } => ctx.tmux.interrupt(&ctx.resolve(&target)?.id),
        Cmd::Stop { target } => ctx.tmux.kill(&ctx.resolve(&target)?.id),
        Cmd::Down => ctx.tmux.kill_session(ctx.session()),
        Cmd::Adapters { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&ctx.config.adapters)?);
            } else {
                for (name, adapter) in &ctx.config.adapters {
                    let prompt = if adapter.prompt_argv.is_some() {
                        "prompt"
                    } else {
                        "-"
                    };
                    println!(
                        "{name}\t{}\t{prompt}\t{}",
                        adapter.display_name(name),
                        adapter.argv.join(" ")
                    );
                }
            }
            Ok(())
        }
        Cmd::Config => {
            match source {
                config::Source::File(path) => eprintln!("source: {}", path.display()),
                config::Source::BuiltIn => eprintln!("source: built-in defaults"),
            }
            println!("{}", serde_json::to_string_pretty(&ctx.config)?);
            Ok(())
        }
    }
}

/// Switches the current client when already inside tmux on the same server,
/// otherwise attaches a new client.
fn attach(ctx: &Ctx) -> Result<()> {
    let target = format!("={}", ctx.session());
    let inside = std::env::var_os("TMUX").is_some() && ctx.tmux.socket.is_none();
    let mut command = ctx.tmux.command();
    if inside {
        command.args(["switch-client", "-t", &target]);
    } else {
        command
            .env_remove("TMUX")
            .args(["attach-session", "-t", &target]);
    }
    let status = command.status().context("cannot run tmux")?;
    if !status.success() {
        bail!("tmux could not attach to {}", ctx.session());
    }
    Ok(())
}
