# nix-darwin: a launchd daemon that runs as the user whose sessions are indexed.
{ config, ... }:
let
  generic = config.flake.modules.generic.ferret-ingest-agents;
in
{
  flake.modules.darwin.default =
    { config, lib, ... }:
    let
      cfg = config.services.ferret-ingest-agents;
      home = config.users.users.${cfg.user}.home;
    in
    {
      imports = [ generic ];

      options.services.ferret-ingest-agents.user = lib.mkOption {
        type = lib.types.str;
        description = "The user whose sessions are indexed, and who runs the ingester.";
      };

      config = lib.mkIf cfg.enable {
        launchd.daemons.ferret-ingest-agents.serviceConfig = {
          ProgramArguments = cfg.command;
          UserName = cfg.user;
          EnvironmentVariables.HOME = home;
          StartInterval = cfg.interval * 60;
          RunAtLoad = true;
          ProcessType = "Background";
          LowPriorityIO = true;
          StandardOutPath = "${home}/Library/Logs/ferret-ingest-agents.log";
          StandardErrorPath = "${home}/Library/Logs/ferret-ingest-agents.log";
        };
      };
    };
}
