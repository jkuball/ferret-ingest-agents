# home-manager: a launchd agent on macOS, a systemd user timer on Linux.
{ config, ... }:
let
  generic = config.flake.modules.generic.ferret-ingest-agents;
in
{
  flake.modules.homeManager.default =
    { config, lib, pkgs, ... }:
    let
      cfg = config.services.ferret-ingest-agents;
    in
    {
      imports = [ generic ];

      config = lib.mkIf cfg.enable (lib.mkMerge [
        (lib.mkIf pkgs.stdenv.hostPlatform.isDarwin {
          launchd.agents.ferret-ingest-agents = {
            enable = true;
            config = {
              ProgramArguments = cfg.command;
              StartInterval = cfg.interval * 60;
              RunAtLoad = true;
              ProcessType = "Background";
              LowPriorityIO = true;
              StandardOutPath = "${config.home.homeDirectory}/Library/Logs/ferret-ingest-agents.log";
              StandardErrorPath = "${config.home.homeDirectory}/Library/Logs/ferret-ingest-agents.log";
            };
          };
        })

        (lib.mkIf pkgs.stdenv.hostPlatform.isLinux {
          systemd.user.services.ferret-ingest-agents = {
            Unit.Description = "Index coding agent sessions into ferret";
            Service = {
              Type = "oneshot";
              ExecStart = lib.escapeShellArgs cfg.command;
              Nice = 10;
              IOSchedulingClass = "idle";
            };
          };
          systemd.user.timers.ferret-ingest-agents = {
            Unit.Description = "Index coding agent sessions into ferret";
            Timer = {
              OnStartupSec = "2min";
              OnUnitActiveSec = "${toString cfg.interval}min";
            };
            Install.WantedBy = [ "timers.target" ];
          };
        })
      ]);
    };
}
