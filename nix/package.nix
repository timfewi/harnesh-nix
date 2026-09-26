{
  lib,
  rustPlatform,
  makeWrapper,
  tmux,
  git,
}:
let
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../tests
    ];
  };
in
rustPlatform.buildRustPackage {
  pname = "harnesh";
  version = (lib.importTOML ../Cargo.toml).package.version;
  inherit src;
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];
  # The integration tests drive a real, isolated tmux server and create a git
  # worktree, so both are needed while checking.
  nativeCheckInputs = [
    tmux
    git
  ];
  preCheck = ''
    export HOME=$TMPDIR
  '';

  # tmux and git are appended to PATH: a tmux already on the user's PATH wins,
  # so the client always matches the version of the server the user runs.
  postInstall = ''
    wrapProgram $out/bin/harnesh --suffix PATH : ${
      lib.makeBinPath [
        tmux
        git
      ]
    }
  '';

  meta = {
    description = "Agent-agnostic tmux cockpit for running and supervising coding agents";
    homepage = "https://github.com/timfewi/harnesh-nix";
    license = lib.licenses.mit;
    mainProgram = "harnesh";
    platforms = lib.platforms.linux;
  };
}
