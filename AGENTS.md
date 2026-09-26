# Project policy

- Keep credentials, personal data and runtime state outside Git.
- Preserve existing user changes. Write documentation and CLI text in English.
- Keep Cargo artifacts inside the repository; `/tmp` may be small on this host.
- Run `project-check fast` after meaningful changes and `project-check full`
  before handoff. Checks are declared in `.project-checks.json` and never run on
  their own.
- Missing tools or offline dependencies are environment blockers, not failures.
- Do not stage, commit, push, publish or deploy without explicit authorization.

# Repository notes

- harnesh must stay agent-agnostic and host-agnostic: no agent packaging,
  host paths, credentials or personal configuration in this repository.
  Consumers map their agents to adapters (see `examples/bridge.nix`).
- tmux is the only runtime state (`@harnesh_*` pane options). Do not add
  state files, databases or daemons.
- `src/config.rs` is the single definition of a valid config; the Nix module
  validates generated files with the binary instead of duplicating rules.
- Integration tests (`tests/cockpit.rs`) drive a real tmux server on an
  isolated socket and must keep working without a UTF-8 locale (Nix sandbox).
