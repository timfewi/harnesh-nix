# Formatting and lints.
lint:
    project-check fast

# Full gate: flake checks (package with tmux integration tests, clippy, module tests).
verify:
    project-check full

# Unit and tmux integration tests.
test:
    cargo test

# Run harnesh from the working tree, for example `just run up`.
run *ARGS:
    cargo run -- {{ ARGS }}
