# harnesh

**harness + dash**: an agent-agnostic tmux cockpit. Run several coding agents
side by side in live panes, watch them from one dashboard, and steer them by
hand or from another agent acting as supervisor.

```
┌ control ──────────────────────────────┐  ┌ agents (tiled) ─────────────────┐
│ harnesh dash       │ editor (nvim)    │  │ codex-1        │ claude-1       │
│ name  agent  state │                  │  │                │                │
│ fix   Codex  busy  ├──────────────────┤  ├────────────────┼────────────────┤
│ docs  Claude idle  │ tests / lazygit  │  │ opencode-1     │ pi-1           │
└───────────────────────────────────────┘  └─────────────────────────────────┘
```

- **Any agent.** An adapter is just a command: `argv` to start an interactive
  session and an optional `promptArgv` with a `{prompt}` placeholder. Claude
  Code, Codex, OpenCode, Gemini CLI, Pi, Aider, or your own wrapper all work
  the same way.
- **Stateless.** tmux is the only state. Each pane carries `@harnesh_*`
  options (role, name, adapter, worktree); harnesh stores nothing else, and
  `harnesh down` removes everything.
- **Scriptable.** Every dashboard action is also a CLI command with JSON
  output, so any agent can supervise others through its shell tool.
- **Parallel-safe.** `--worktree` starts an agent in its own git worktree, so
  agents never edit the same checkout.

## Quick start

```sh
nix run github:timfewi/harnesh-nix -- up        # create the cockpit and attach
```

Without a config file, harnesh offers presets for `claude`, `codex`,
`opencode`, `gemini`, `pi` and `aider` when those commands are on `PATH`.

Inside the dashboard: `n` new agent, `j`/`k` select, `enter` jump to the pane,
`s` send a message, `i` interrupt (Ctrl-C), `x` stop, `q` quit.

## CLI

| Command | Purpose |
|---|---|
| `harnesh up [--detached]` | Create the session (dashboard + tool panes) and attach |
| `harnesh spawn <adapter> [--name N] [--cwd DIR] [--prompt TEXT] [--worktree]` | Start an agent pane; prints JSON |
| `harnesh list [--json]` | Panes with role, adapter, state, cwd and worktree |
| `harnesh capture <pane> [--lines N]` | Last lines of a pane's screen |
| `harnesh send <pane> [TEXT\|-] [--no-enter]` | Bracketed paste, then Enter; `-` reads stdin |
| `harnesh interrupt <pane>` / `harnesh stop <pane>` | Ctrl-C / kill the pane |
| `harnesh down` | Kill the cockpit session |
| `harnesh adapters [--json]` / `harnesh config` | Inspect the effective configuration |

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
    "codex": { "label": "Codex", "argv": ["codex"], "promptArgv": ["codex", "{prompt}"] },
    "review": { "argv": ["claude", "--model", "opus"] }
  },
  "tools": [
    { "name": "editor", "argv": ["nvim", "--listen", "/tmp/harnesh-nvim.sock"] },
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
project-check fast   # formatting and lints
nix flake check      # package incl. tmux integration tests, clippy, rustfmt, module tests
```

The integration tests drive a real tmux server on an isolated socket, with
shell loops standing in for agents, so they need no network or credentials.
Requires tmux 3.2 or newer.

## License

MIT
