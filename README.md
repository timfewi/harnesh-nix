# harnesh

**harness + dash**: an agent-agnostic tmux cockpit. Run several coding agents
side by side in live panes, switch between them from one dashboard, and steer
them by hand or from another agent acting as supervisor.

```
┌ control ──────────────────────────────┐  ┌ agents (tiled) ─────────────────┐
│ harnesh dash       │ editor (nvim)    │  │ codex-1        │ claude-1       │
│ name  agent  state │                  │  │                │                │
│ fix   Codex running├──────────────────┤  ├────────────────┼────────────────┤
│ docs  Claude exited│ tests / lazygit  │  │ opencode-1     │ pi-1           │
└───────────────────────────────────────┘  └─────────────────────────────────┘
```

- **Any agent.** An adapter is just a command: `argv` to start an interactive
  session and an optional `promptArgv` with a `{prompt}` placeholder. Claude
  Code, Codex, OpenCode, Gemini CLI, Pi, Aider, or your own wrapper all work
  the same way.
- **Stateless.** tmux is the only state. Each pane carries `@harnesh_*`
  options (role, name, adapter, task, worktree); harnesh stores nothing else, and
  `harnesh down` removes everything.
- **Scriptable.** Core dashboard actions are also CLI commands; `list --json`
  exposes the panes so another agent can supervise them through its shell tool.
- **Parallel-safe.** `--worktree` starts an agent in its own git worktree, so
  agents never edit the same checkout.

## Quick start

```sh
nix run github:timfewi/harnesh-nix -- up        # create the cockpit and attach
```

Without a config file, harnesh offers presets for `claude`, `codex`,
`opencode`, `gemini`, `pi` and `aider` when those commands are on `PATH`.

The first screen gives a three-step quick start: `n` starts an agent, `j`/`k`
selects it, `Enter` opens its **real tmux pane**. From any pane,
`harnesh home` returns to the dashboard. For a one-keystroke return, optionally
add this binding to your own `tmux.conf` (harnesh never changes global keys):

```tmux
bind-key H run-shell 'harnesh home'  # prefix + H
```

Use `harnesh --config PATH --socket NAME home` instead when the cockpit uses an
explicit config or tmux socket. `?` opens the keyboard guide and settings;
scroll it with `j/k` in a small terminal. `a` there toggles short animations
for this dashboard. `Space` expands a compact
session row; `h` collapses it. `5j`, `gg`, `G`, `Ctrl-U`/`Ctrl-D`, and
`PageUp`/`PageDown` navigate quickly. `t` edits a task description, `s` sends
input, `i` interrupts, `x` confirms a stop, and `q` exits the dashboard.

Sessions show a colored name, task excerpt and the process's observed
`running`/`exited` status. The capacity gauge shows live agents versus
`maxAgents`; it is **not** a progress or readiness indicator. The dashboard
does not capture or mirror agent output. Open the real pane to interact with an
agent; use the explicit `harnesh capture` CLI command when a script needs text.
Colors are deterministic for the same configured adapter set. The terminal
respects `NO_COLOR`; unset it to enable the accents.

To keep the cockpit inside Neovim today, open `:terminal harnesh up` in a
Neovim tab or split. When Neovim itself runs in the same tmux server, `up`
switches that client instead of attaching a nested one. A native Neovim buffer
can be built on the existing `list --json` and action commands later; no
agent-specific integration is required for the current switcher.

Task summaries default to a compact excerpt of `--prompt`. Use `--task` for an
explicit description, or `t` / `harnesh task <pane> "description"` to update one
without sending input to the agent. Empty text clears the description. Summaries
normalize whitespace and are limited to 240 characters; they are local excerpts,
not AI-generated interpretations. Existing panes without task metadata show a
hint to add it. Task text lives only in the pane's `@harnesh_task` option and is
included in `harnesh list --json`.

```sh
harnesh spawn codex --prompt "Fix the login retry regression and add a test" --task "Fix login retries"
harnesh task codex-1 "Checking login regression tests"
```

## CLI

| Command                                                                       | Purpose                                                |
| ----------------------------------------------------------------------------- | ------------------------------------------------------ |
| `harnesh up [--detached]`                                                     | Create the session (dashboard + tool panes) and attach |
| `harnesh spawn <adapter> [--name N] [--cwd DIR] [--prompt TEXT] [--task TEXT] [--worktree]` | Start an agent pane; prints JSON                       |
| `harnesh list [--json]`                                                       | Panes with role, adapter, state, cwd and worktree      |
| `harnesh capture <pane> [--lines N]`                                          | Last lines of a pane's screen                          |
| `harnesh send <pane> [TEXT\|-] [--no-enter]`                                  | Bracketed paste, then Enter; `-` reads stdin           |
| `harnesh task <pane> TEXT` | Set or clear the task description without sending input |
| `harnesh interrupt <pane>` / `harnesh stop <pane>`                            | Ctrl-C / kill the pane                                 |
| `harnesh focus <pane>` / `harnesh home` | Switch to a real pane / return to the dashboard |
| `harnesh down`                                                                | Kill the cockpit session                               |
| `harnesh adapters [--json]` / `harnesh config`                                | Inspect the effective configuration                    |

`<pane>` is a pane name or tmux id (`%3`). Global flags: `--config PATH`
(`HARNESH_CONFIG`) and `--socket NAME` (`HARNESH_TMUX_SOCKET`, an isolated
`tmux -L` server). Panes inherit `HARNESH_SESSION`, and agents also get
`HARNESH_AGENT`, so a nested `harnesh` drives the same cockpit.

A `--worktree` agent works in `<repo>.<name>` next to the repository on branch
`harnesh/<name>`. harnesh never removes worktrees; merge or delete them
yourself.

## Configuration

harnesh reads the first of `--config`, `$XDG_CONFIG_HOME/harnesh/config.json`
(or `~/.config/harnesh/config.json`) and `/etc/harnesh/config.json`:

```json
{
  "version": 1,
  "session": "harnesh",
  "maxAgents": 6,
  "detectPresets": false,
  "adapters": {
    "codex": {
      "label": "Codex",
      "argv": ["codex"],
      "promptArgv": ["codex", "{prompt}"]
    },
    "review": { "argv": ["claude", "--model", "opus"] }
  },
  "tools": [
    {
      "name": "editor",
      "argv": ["nvim", "--listen", "/tmp/harnesh-nvim.sock"]
    },
    { "name": "git", "argv": ["lazygit"] }
  ]
}
```

Unknown fields, a `promptArgv` without `{prompt}`, invalid names and duplicate
tools are rejected. `detectPresets` (default `true` without a file) adds the
built-in presets for commands found on `PATH`; configured adapters win.

## NixOS module

```nix
{
  inputs.harnesh.url = "github:timfewi/harnesh-nix";

  outputs = { nixpkgs, harnesh, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      modules = [
        harnesh.nixosModules.default
        {
          programs.harnesh = {
            enable = true;
            adapters.codex = { argv = [ "codex" ]; promptArgv = [ "codex" "{prompt}" ]; };
            tools = [ { name = "editor"; argv = [ "nvim" ]; } ];
          };
        }
      ];
    };
  };
}
```

The module writes `/etc/harnesh/config.json` and validates it at build time
with the harnesh binary itself, so Nix and the CLI share one definition of a
valid file. `programs.harnesh.configFile` exposes the generated file.
`overlays.default` adds `pkgs.harnesh`.

### Wiring your agent packaging (bridge)

This repository knows no agent packaging, and your agent packaging need not
know harnesh. Connect the two in the configuration that imports both, by
mapping each agent you install to an adapter. See
[`examples/bridge.nix`](examples/bridge.nix) for a complete module.

## Supervising with an agent

`lib.skills.harnesh-supervisor` is an Agent Skills
directory that teaches any skill-aware agent to split an objective, start one
worktree agent per part, watch, steer and report through the CLI. Point your
harness's skill directory at it, or copy it.

## Development

```sh
nix develop          # cargo, clippy, rustfmt, tmux, nixfmt, statix, deadnix
mkdir -p target/check-tmp
export TMPDIR="$PWD/target/check-tmp"  # keep check-runner Cargo artifacts in the repository
project-check fast   # formatting, lints, unit and tmux integration tests
project-check full   # Nix package/module checks and clippy
```

The integration tests drive a real tmux server on an isolated socket, with
shell loops standing in for agents, so they need no network or credentials.
Requires tmux 3.2 or newer.

## License

MIT
