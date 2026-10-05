//! End-to-end check against a real, isolated tmux server. The "agents" are
//! small shell loops, so the test needs no coding agent or network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;

struct Cockpit {
    dir: PathBuf,
    /// Unix socket paths are limited to ~108 bytes, so the tmux directory
    /// lives under the short system temp dir instead of the target dir.
    tmux_dir: PathBuf,
}

impl Cockpit {
    fn new(name: &str) -> Option<Self> {
        if Command::new("tmux").arg("-V").output().is_err() {
            eprintln!("skipping {name}: tmux is not installed");
            return None;
        }
        let dir =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // project-check puts TMPDIR inside the checkout to keep Cargo artifacts
        // local. tmux canonicalizes paths, so even a short symlink to that
        // directory would exceed the Unix socket limit. Only sockets use /tmp.
        let tmux_dir = Path::new("/tmp").join(format!("hn-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&tmux_dir).unwrap();
        let config = serde_json::json!({
            "version": 1,
            "session": "it",
            "maxAgents": 2,
            "adapters": {
                "echo": {
                    "label": "Echo agent",
                    "argv": ["sh", "-c", "echo ready; while read -r l; do echo \"got:$l\"; done"],
                    "promptArgv": ["sh", "-c", "echo 'prompt={prompt}'; while read -r l; do :; done"]
                },
                "plain": { "argv": ["sh", "-c", "while :; do sleep 1; done"] }
            },
            "tools": [{ "name": "notes", "argv": ["sh", "-c", "echo tool-up; exec cat"] }]
        });
        std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
        Some(Self { dir, tmux_dir })
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_harnesh"))
            .args(args)
            .current_dir(&self.dir)
            .env_remove("TMUX")
            // Build sandboxes have no UTF-8 locale; test that case everywhere.
            .env_remove("LANG")
            .env_remove("LC_ALL")
            .env_remove("LC_CTYPE")
            .env("TMUX_TMPDIR", &self.tmux_dir)
            .env("HARNESH_TMUX_SOCKET", "it")
            .env("HARNESH_CONFIG", self.dir.join("config.json"))
            .env("TERM", "xterm-256color")
            .output()
            .unwrap()
    }

    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-u", "-L", "it"])
            .args(args)
            .env_remove("TMUX")
            .env_remove("LANG")
            .env_remove("LC_ALL")
            .env_remove("LC_CTYPE")
            .env("TMUX_TMPDIR", &self.tmux_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "tmux {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "harnesh {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn err(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            !output.status.success(),
            "harnesh {args:?} unexpectedly succeeded"
        );
        String::from_utf8(output.stderr).unwrap()
    }

    fn panes(&self) -> Vec<Value> {
        serde_json::from_str::<Value>(&self.ok(&["list", "--json"]))
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }

    fn wait_for(&self, target: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = self.ok(&["capture", target, "--lines", "200"]);
            if text.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{needle:?} never appeared in {target}:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_dead(&self, name: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let pane = self
                .panes()
                .into_iter()
                .find(|p| p["name"] == name)
                .unwrap();
            if pane["dead"] == true {
                return pane;
            }
            assert!(Instant::now() < deadline, "{name} never exited");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Cockpit {
    fn drop(&mut self) {
        let _ = self.run(&["down"]);
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.tmux_dir);
    }
}

#[test]
fn cockpit_lifecycle() {
    let Some(cockpit) = Cockpit::new("lifecycle") else {
        return;
    };

    assert!(cockpit.ok(&["adapters"]).contains("Echo agent"));
    assert!(cockpit.ok(&["up", "--detached"]).contains("created"));
    assert!(
        cockpit
            .ok(&["up", "--detached"])
            .contains("already running")
    );

    // The dashboard renders inside the session and the tool pane is running.
    cockpit.wait_for("dash", "harnesh  /  it");
    cockpit.wait_for("notes", "tool-up");

    let spawned: Value = serde_json::from_str(&cockpit.ok(&["spawn", "echo"])).unwrap();
    assert_eq!(spawned["name"], "echo-1");
    assert!(spawned["pane"].as_str().unwrap().starts_with('%'));
    cockpit.wait_for("echo-1", "ready");
    // Switching chooses the actual pane; home returns to the dashboard.
    cockpit.ok(&["focus", "echo-1"]);
    assert!(
        cockpit
            .tmux(&[
                "list-windows",
                "-t",
                "it",
                "-F",
                "#{window_name} #{window_active}"
            ])
            .contains("agents 1")
    );
    assert!(
        cockpit
            .tmux(&[
                "list-panes",
                "-t",
                "it:agents",
                "-F",
                "#{pane_id} #{pane_active}"
            ])
            .contains(&format!("{} 1", spawned["pane"].as_str().unwrap()))
    );
    cockpit.ok(&["home"]);
    assert!(
        cockpit
            .tmux(&[
                "list-windows",
                "-t",
                "it",
                "-F",
                "#{window_name} #{window_active}"
            ])
            .contains("control 1")
    );
    let dash_id = cockpit
        .panes()
        .into_iter()
        .find(|p| p["role"] == "dash")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        cockpit
            .tmux(&[
                "list-panes",
                "-t",
                "it:control",
                "-F",
                "#{pane_id} #{pane_active}"
            ])
            .contains(&format!("{dash_id} 1"))
    );

    cockpit.ok(&["send", "echo-1", "hello world"]);
    cockpit.wait_for("echo-1", "got:hello world");
    // Multi-line text arrives line by line through one paste.
    cockpit.ok(&["send", "echo-1", "first\nsecond"]);
    cockpit.wait_for("echo-1", "got:second");
    cockpit.wait_for("echo-1", "got:first");
    // Without Enter the text stays unsubmitted.
    cockpit.ok(&["send", "echo-1", "pending", "--no-enter"]);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!cockpit.ok(&["capture", "echo-1"]).contains("got:pending"));

    // Prompts go through promptArgv; adapters without one refuse a prompt.
    cockpit.ok(&[
        "spawn",
        "echo",
        "--name",
        "asker",
        "--prompt",
        "fix the bug",
    ]);
    cockpit.wait_for("asker", "prompt=fix the bug");
    assert!(
        cockpit
            .err(&["spawn", "plain", "--prompt", "x"])
            .contains("no promptArgv")
    );

    // maxAgents = 2 counts live agents only.
    assert!(cockpit.err(&["spawn", "plain"]).contains("maxAgents = 2"));
    assert!(
        cockpit
            .err(&["spawn", "echo", "--name", "asker"])
            .contains("already exists")
    );
    assert!(
        cockpit
            .err(&["spawn", "echo", "--name", "notes"])
            .contains("already exists")
    );
    assert!(cockpit.err(&["spawn", "nope"]).contains("unknown adapter"));

    // Interrupt ends the shell loop; the pane remains for inspection.
    cockpit.ok(&["interrupt", "asker"]);
    let asker = cockpit.wait_dead("asker");
    assert_eq!(asker["role"], "agent");
    assert!(cockpit.ok(&["list"]).contains("exited"));
    let replaced: Value = serde_json::from_str(&cockpit.ok(&["spawn", "plain"])).unwrap();
    assert_eq!(replaced["name"], "plain-1");

    let names: Vec<String> = cockpit
        .panes()
        .iter()
        .map(|p| {
            format!(
                "{}:{}",
                p["role"].as_str().unwrap(),
                p["name"].as_str().unwrap()
            )
        })
        .collect();
    for expected in [
        "dash:dash",
        "tool:notes",
        "agent:echo-1",
        "agent:asker",
        "agent:plain-1",
    ] {
        assert!(
            names.contains(&expected.to_owned()),
            "{expected} missing from {names:?}"
        );
    }

    cockpit.ok(&["stop", "asker"]);
    assert!(
        cockpit
            .err(&["capture", "asker"])
            .contains("no harnesh pane")
    );
    cockpit.ok(&["down"]);
    assert!(cockpit.panes().is_empty());
}

#[test]
fn worktree_agents_get_their_own_checkout() {
    let Some(cockpit) = Cockpit::new("worktree") else {
        return;
    };
    let repo = cockpit.dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);

    let repo_arg = repo.display().to_string();
    let spawned: Value = serde_json::from_str(&cockpit.ok(&[
        "spawn",
        "echo",
        "--name",
        "wt",
        "--cwd",
        &repo_arg,
        "--worktree",
    ]))
    .unwrap();
    let worktree = cockpit.dir.join("repo.wt");
    assert_eq!(spawned["worktree"], worktree.display().to_string());
    assert!(worktree.join(".git").exists());
    let pane = cockpit
        .panes()
        .into_iter()
        .find(|p| p["name"] == "wt")
        .unwrap();
    assert_eq!(pane["worktree"], worktree.display().to_string());

    cockpit.ok(&["stop", "wt"]);
    let again = cockpit.err(&[
        "spawn",
        "echo",
        "--name",
        "wt",
        "--cwd",
        &repo_arg,
        "--worktree",
    ]);
    assert!(again.contains("already exists"), "{again}");
}

#[test]
fn dashboard_tasks_and_keyboard_workflow() {
    let Some(cockpit) = Cockpit::new("dashboard") else {
        return;
    };
    cockpit.ok(&["up", "--detached"]);
    cockpit.ok(&[
        "spawn",
        "echo",
        "--name",
        "writer",
        "--prompt",
        "fix the bug",
    ]);
    let panes = cockpit.panes();
    let writer = panes.iter().find(|p| p["name"] == "writer").unwrap();
    assert_eq!(writer["task"], "fix the bug");
    let dash = panes.iter().find(|p| p["role"] == "dash").unwrap()["id"]
        .as_str()
        .unwrap();
    cockpit.tmux(&["resize-pane", "-Z", "-t", dash]);
    cockpit.tmux(&["resize-window", "-t", "it:control", "-x", "130", "-y", "36"]);
    cockpit.wait_for("dash", "fix the bug");
    assert!(
        !cockpit
            .ok(&["capture", "dash"])
            .contains("prompt=fix the bug")
    );
    cockpit.ok(&["task", "writer", "Repair\n login\t flow\u{1f} safely"]);
    assert_eq!(
        cockpit
            .panes()
            .iter()
            .find(|p| p["name"] == "writer")
            .unwrap()["task"],
        "Repair login flow safely"
    );
    cockpit.wait_for("dash", "Repair login flow safely");
    cockpit.tmux(&["send-keys", "-t", dash, "?"]);
    cockpit.wait_for("dash", "Help & settings");
    cockpit.tmux(&["send-keys", "-t", dash, "a"]);
    cockpit.wait_for("dash", "animations: off");
    cockpit.tmux(&["send-keys", "-t", dash, "Escape"]);
    // First row is the tool, second row the agent. Edit the existing summary.
    cockpit.tmux(&["send-keys", "-t", dash, "j", "t", "End"]);
    cockpit.wait_for("dash", "Enter save");
    cockpit.tmux(&["send-keys", "-t", dash, "-l", " today"]);
    cockpit.tmux(&["send-keys", "-t", dash, "Enter"]);
    cockpit.wait_for("dash", "task saved");
    assert_eq!(
        cockpit
            .panes()
            .iter()
            .find(|p| p["name"] == "writer")
            .unwrap()["task"],
        "Repair login flow safely today"
    );
    // Cancellation preserves saved metadata.
    cockpit.tmux(&["send-keys", "-t", dash, "t"]);
    cockpit.wait_for("dash", "Enter save");
    cockpit.tmux(&["send-keys", "-t", dash, "-l", " discard"]);
    cockpit.tmux(&["send-keys", "-t", dash, "Escape"]);
    cockpit.wait_for("dash", "task edit cancelled");
    assert_eq!(
        cockpit
            .panes()
            .iter()
            .find(|p| p["name"] == "writer")
            .unwrap()["task"],
        "Repair login flow safely today"
    );
    cockpit.tmux(&["resize-window", "-t", "it:control", "-x", "80", "-y", "24"]);
    cockpit.wait_for("dash", "Repair login flow safely today");
    cockpit.ok(&["task", "writer", ""]);
    assert!(
        cockpit
            .panes()
            .iter()
            .find(|p| p["name"] == "writer")
            .unwrap()["task"]
            .is_null()
    );
    // Explicit descriptions work even for an adapter without prompt support.
    cockpit.ok(&["spawn", "plain", "--task", "Review changes"]);
    assert_eq!(
        cockpit
            .panes()
            .iter()
            .find(|p| p["name"] == "plain-1")
            .unwrap()["task"],
        "Review changes"
    );
    cockpit.ok(&["task", "writer", "#{pane_id} $HOME"]);
    assert_eq!(
        cockpit
            .panes()
            .iter()
            .find(|p| p["name"] == "writer")
            .unwrap()["task"],
        "#{pane_id} $HOME"
    );
}

#[test]
fn switcher_creates_and_opens_new_agent_without_output_mirroring() {
    let Some(cockpit) = Cockpit::new("switcher") else {
        return;
    };
    cockpit.ok(&["up", "--detached"]);
    let dash = cockpit
        .panes()
        .into_iter()
        .find(|p| p["role"] == "dash")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    cockpit.tmux(&["resize-window", "-t", "it:control", "-x", "110", "-y", "30"]);
    cockpit.wait_for("dash", "No agents yet");
    cockpit.tmux(&["send-keys", "-t", &dash, "n", "j", "Enter"]);
    cockpit.wait_for("dash", "plain-1");
    assert!(cockpit.panes().iter().any(|p| p["name"] == "plain-1"));
    cockpit.tmux(&["send-keys", "-t", &dash, "Enter"]);
    // send-keys only queues input; wait for the dashboard to handle Enter.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let selected = cockpit.tmux(&[
            "list-windows",
            "-t",
            "it",
            "-F",
            "#{window_name} #{window_active}",
        ]);
        if selected.contains("agents 1") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "agent window never selected:\n{selected}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    cockpit.ok(&["home"]);
    assert!(
        cockpit
            .tmux(&[
                "list-windows",
                "-t",
                "it",
                "-F",
                "#{window_name} #{window_active}"
            ])
            .contains("control 1")
    );
    cockpit.ok(&["spawn", "echo"]);
    cockpit.wait_for("echo-1", "ready");
    cockpit.wait_for("dash", "echo-1");
    assert!(!cockpit.ok(&["capture", "dash"]).contains("ready"));
}
