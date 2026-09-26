# NixOS module: installs harnesh and writes /etc/harnesh/config.json.
#
# The module knows no agent. Consumers (a host configuration or a bridge
# module) declare adapters as plain argv lists, which keeps this repository
# free of any dependency on a particular agent packaging.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib) mkEnableOption mkOption types;
  cfg = config.programs.harnesh;
  name = types.strMatching "[A-Za-z0-9._-]{1,64}";
  argv = types.nonEmptyListOf types.str;

  adapterType = types.submodule {
    options = {
      label = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Name shown in the dashboard; defaults to the adapter name.";
      };
      argv = mkOption {
        type = argv;
        example = [ "codex" ];
        description = "Command that starts an interactive agent session.";
      };
      promptArgv = mkOption {
        type = types.nullOr argv;
        default = null;
        example = [
          "codex"
          "{prompt}"
        ];
        description = ''
          Command that starts a session with an initial prompt. Every argument
          containing `{prompt}` receives the prompt text. Without it,
          `harnesh spawn --prompt` is refused for this adapter.
        '';
      };
    };
  };

  toolType = types.submodule {
    options = {
      name = mkOption {
        type = name;
        description = "Pane name, unique among tools.";
      };
      argv = mkOption {
        type = argv;
        description = "Command run in the pane next to the dashboard.";
      };
    };
  };

  settings = {
    version = 1;
    inherit (cfg)
      session
      maxAgents
      detectPresets
      tools
      ;
    adapters = lib.mapAttrs (
      _: adapter: lib.filterAttrs (_: value: value != null) adapter
    ) cfg.adapters;
  };

  # harnesh validates the file itself at build time, so the Nix module and the
  # binary share one definition of a valid configuration.
  configFile =
    pkgs.runCommand "harnesh-config.json"
      {
        nativeBuildInputs = [ cfg.package ];
        passAsFile = [ "json" ];
        json = builtins.toJSON settings;
      }
      ''
        harnesh --config "$jsonPath" config > /dev/null
        cp "$jsonPath" "$out"
      '';
in
{
  options.programs.harnesh = {
    enable = mkEnableOption "harnesh, the agent-agnostic tmux cockpit";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.harnesh;
      defaultText = lib.literalExpression "harnesh-nix.packages.\${system}.harnesh";
      description = "The harnesh package.";
    };

    session = mkOption {
      type = name;
      default = "harnesh";
      description = "tmux session name of the cockpit.";
    };

    maxAgents = mkOption {
      type = types.ints.between 1 32;
      default = 6;
      description = "Maximum number of live agent panes.";
    };

    detectPresets = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Add built-in presets (claude, codex, opencode, gemini, pi, aider) for
        commands found on PATH at runtime. Disable when `adapters` already
        lists every agent, to avoid duplicate entries.
      '';
    };

    adapters = mkOption {
      type = types.attrsOf adapterType;
      default = { };
      example = lib.literalExpression ''
        {
          codex = { argv = [ "codex" ]; promptArgv = [ "codex" "{prompt}" ]; };
        }
      '';
      description = "Agents the cockpit can spawn, keyed by adapter name.";
    };

    tools = mkOption {
      type = types.listOf toolType;
      default = [ ];
      example = lib.literalExpression ''
        [ { name = "editor"; argv = [ "nvim" ]; } ]
      '';
      description = "Panes opened next to the dashboard in the control window.";
    };

    configFile = mkOption {
      type = types.path;
      readOnly = true;
      description = "The generated, validated configuration file.";
    };
  };

  config = lib.mkIf cfg.enable {
    programs.harnesh.configFile = configFile;
    environment.systemPackages = [ cfg.package ];
    environment.etc."harnesh/config.json".source = configFile;
  };
}
