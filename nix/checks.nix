{
  self,
  nixpkgs,
  system,
}:
let
  inherit (nixpkgs) lib;
  pkgs = nixpkgs.legacyPackages.${system};
  inherit (self.packages.${system}) harnesh;

  evaluate =
    module:
    (lib.nixosSystem {
      inherit system;
      modules = [
        self.nixosModules.default
        { system.stateVersion = "26.05"; }
        module
      ];
    }).config;

  configured = evaluate {
    programs.harnesh = {
      enable = true;
      session = "cockpit";
      maxAgents = 4;
      detectPresets = false;
      adapters.agent = {
        label = "Some agent";
        argv = [ "/opt/agent" ];
        promptArgv = [
          "/opt/agent"
          "--prompt"
          "{prompt}"
        ];
      };
      adapters.plain.argv = [ "plain-agent" ];
      tools = [
        {
          name = "editor";
          argv = [ "nvim" ];
        }
      ];
    };
  };
  written = builtins.fromJSON (
    builtins.unsafeDiscardStringContext (builtins.readFile configured.programs.harnesh.configFile)
  );

  # The binary rejects promptArgv without {prompt}; the module must surface
  # that as a build failure instead of shipping a broken file.
  invalid =
    (evaluate {
      programs.harnesh = {
        enable = true;
        adapters.broken = {
          argv = [ "agent" ];
          promptArgv = [ "agent" ];
        };
      };
    }).programs.harnesh.configFile;
in
{
  # Unit tests plus the tmux integration tests run in the package's checkPhase.
  inherit harnesh;

  clippy = harnesh.overrideAttrs (old: {
    pname = "harnesh-clippy";
    nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
    buildPhase = "cargo clippy --all-targets --offline -- -D warnings";
    doCheck = false;
    installPhase = "touch $out";
    postInstall = "";
  });

  rustfmt =
    pkgs.runCommand "harnesh-rustfmt"
      {
        nativeBuildInputs = [
          pkgs.cargo
          pkgs.rustfmt
        ];
      }
      ''
        cd ${harnesh.src}
        cargo fmt --all -- --check
        touch $out
      '';

  module =
    assert
      written == {
        version = 1;
        session = "cockpit";
        maxAgents = 4;
        detectPresets = false;
        adapters = {
          agent = {
            label = "Some agent";
            argv = [ "/opt/agent" ];
            promptArgv = [
              "/opt/agent"
              "--prompt"
              "{prompt}"
            ];
          };
          plain.argv = [ "plain-agent" ];
        };
        tools = [
          {
            name = "editor";
            argv = [ "nvim" ];
          }
        ];
      };
    assert
      configured.environment.etc."harnesh/config.json".source == configured.programs.harnesh.configFile;
    assert lib.elem harnesh configured.environment.systemPackages;
    assert !((evaluate { }).environment.etc ? "harnesh/config.json");
    pkgs.runCommand "harnesh-module" { } ''
      ${lib.getExe harnesh} --config ${configured.programs.harnesh.configFile} adapters \
        | grep -q '^agent	Some agent	prompt	/opt/agent$'
      touch $out
    '';

  module-rejects-invalid =
    pkgs.runCommand "harnesh-module-rejects-invalid"
      {
        failure = pkgs.testers.testBuildFailure invalid;
      }
      ''
        grep -q 'promptArgv must contain {prompt}' $failure/testBuildFailure.log
        touch $out
      '';

  # The documented bridge example must keep evaluating against the module.
  bridge-example =
    let
      bridged = evaluate {
        imports = [ ../examples/bridge.nix ];
        options.my.agents = lib.mkOption { type = lib.types.attrsOf lib.types.anything; };
        config.my.agents = {
          alpha = {
            enable = true;
            package = pkgs.writeShellScriptBin "alpha" "";
            promptFlag = "--prompt";
          };
          beta = {
            enable = true;
            package = pkgs.writeShellScriptBin "beta" "";
            promptFlag = null;
          };
          off = {
            enable = false;
            package = pkgs.hello;
            promptFlag = null;
          };
        };
      };
      alpha = lib.getExe bridged.my.agents.alpha.package;
      beta = lib.getExe bridged.my.agents.beta.package;
    in
    assert
      bridged.programs.harnesh.adapters.alpha.promptArgv == [
        alpha
        "--prompt"
        "{prompt}"
      ];
    assert
      bridged.programs.harnesh.adapters.beta.promptArgv == [
        beta
        "{prompt}"
      ];
    assert !(bridged.programs.harnesh.adapters ? off);
    pkgs.runCommand "harnesh-bridge-example" { } ''
      test -s ${bridged.programs.harnesh.configFile}
      touch $out
    '';

  skill = pkgs.runCommand "harnesh-skill" { } ''
    skill=${self.lib.skills.harnesh-supervisor}/SKILL.md
    head -1 $skill | grep -qx -- '---'
    grep -qx 'name: harnesh-supervisor' $skill
    grep -q '^description: .\{40,\}' $skill
    touch $out
  '';
}
