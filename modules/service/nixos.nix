# NixOS: a system timer that runs as the user whose sessions are indexed.
{ config, ... }:
let
  generic = config.flake.modules.generic.ferret-ingest-agents;
in
{
  flake.modules.nixos.default =
    { config, lib, ... }:
    let
      cfg = config.services.ferret-ingest-agents;
    in
    {
      imports = [ generic ];

      options.services.ferret-ingest-agents.user = lib.mkOption {
        type = lib.types.str;
        description = "The user whose sessions are indexed, and who runs the ingester.";
      };

      config = lib.mkIf cfg.enable {
        systemd.services.ferret-ingest-agents = {
          description = "Index coding agent sessions into ferret";
          after = [ "network-online.target" ];
          wants = [ "network-online.target" ];
          environment.HOME = config.users.users.${cfg.user}.home;
          serviceConfig = {
            Type = "oneshot";
            User = cfg.user;
            ExecStart = lib.escapeShellArgs cfg.command;
            Nice = 10;
            IOSchedulingClass = "idle";
          };
        };
        systemd.timers.ferret-ingest-agents = {
          description = "Index coding agent sessions into ferret";
          wantedBy = [ "timers.target" ];
          timerConfig = {
            OnBootSec = "2min";
            OnUnitActiveSec = "${toString cfg.interval}min";
          };
        };
      };
    };
}
