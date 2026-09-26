# Example bridge module: maps agents installed by some other module to harnesh
# adapters. Keep a module like this in the configuration that imports both, so
# neither harnesh nor the agent packaging depends on the other.
#
# `agents` stands for whatever your agent packaging exposes. Here it is a plain
# attribute set of { enable, package, promptFlag } per agent.
{
  config,
  lib,
  ...
}:
let
  agents = config.my.agents;
  enabled = lib.filterAttrs (_: agent: agent.enable) agents;
in
{
  programs.harnesh = {
    enable = true;
    # Every installed agent is listed explicitly; presets would duplicate them.
    detectPresets = false;
    adapters = lib.mapAttrs (
      _: agent:
      let
        exe = lib.getExe agent.package;
      in
      {
        argv = [ exe ];
        promptArgv = [ exe ] ++ lib.optional (agent.promptFlag != null) agent.promptFlag ++ [ "{prompt}" ];
      }
    ) enabled;
    tools = [
      {
        name = "editor";
        argv = [ "nvim" ];
      }
    ];
  };
}
