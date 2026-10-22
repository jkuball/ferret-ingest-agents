# The options the home-manager, NixOS and nix-darwin modules share, and what
# they turn into: a config file and the command that runs one ingest.
{ withSystem, ... }:
{
  flake.modules.generic.ferret-ingest-agents =
    { config, lib, pkgs, ... }:
    let
      inherit (lib) mkOption types;
      cfg = config.services.ferret-ingest-agents;

      harness = name: defaultDir: {
        enable = lib.mkEnableOption "indexing ${name} sessions";
        sessionsDir = mkOption {
          type = types.str;
          default = defaultDir;
          description = "Where ${name} keeps its session files. `~` is the user's home.";
        };
        source = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "laptop/agents/${name}";
          description = "The ferret source for its documents; `<host>/agents/${name}` when null.";
        };
      };

      harnessSettings = h: lib.optionalAttrs h.enable (
        { sessions_dir = h.sessionsDir; } // lib.optionalAttrs (h.source != null) { inherit (h) source; }
      );
    in
    {
      options.services.ferret-ingest-agents = {
        enable = lib.mkEnableOption "indexing coding agent sessions into ferret";

        package = mkOption {
          type = types.package;
          default = withSystem pkgs.stdenv.hostPlatform.system ({ config, ... }: config.packages.default);
          defaultText = lib.literalExpression "ferret-ingest-agents from its flake";
        };

        url = mkOption {
          type = types.str;
          example = "https://ferret.example.com";
          description = "The ferret instance.";
        };

        keyFile = mkOption {
          type = types.str;
          example = "/run/secrets/ferret-ingest-agents";
          description = ''
            A file holding a ferret API key with `ingest:write`, read at every run.
            A path string, so the key never enters the Nix store.
          '';
        };

        host = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Names this machine in sources; the short hostname when null.";
        };

        author = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "The person in every turn's authors, next to the models; $USER when null.";
        };

        harnesses = {
          pi = harness "pi" "~/.pi/agent/sessions";
          claude-code = harness "claude-code" "~/.claude/projects";
        };

        include = mkOption {
          type = types.listOf types.str;
          default = [ "**" ];
          description = "Globs on a session's working directory that are indexed.";
        };

        exclude = mkOption {
          type = types.listOf types.str;
          default = [ ];
          example = [ "~/work/**" ];
          description = "Globs on a session's working directory that are left out, even when included.";
        };

        turnTemplate = mkOption {
          type = types.nullOr types.path;
          default = null;
          description = "A minijinja template for turns, replacing the built-in one.";
        };

        concurrency = mkOption {
          type = types.ints.positive;
          default = 4;
          description = "Session files processed at once.";
        };

        interval = mkOption {
          type = types.ints.positive;
          default = 15;
          description = "Minutes between runs.";
        };

        configFile = mkOption {
          type = types.path;
          readOnly = true;
          description = "The generated config file.";
        };

        command = mkOption {
          type = types.listOf types.str;
          readOnly = true;
          description = "The command one run executes.";
        };
      };

      config.services.ferret-ingest-agents = {
        configFile = (pkgs.formats.toml { }).generate "ferret-ingest-agents.toml" (
          {
            inherit (cfg) url include exclude concurrency;
            key_file = cfg.keyFile;
            harnesses = lib.filterAttrs (_: v: v != { }) {
              pi = harnessSettings cfg.harnesses.pi;
              claude-code = harnessSettings cfg.harnesses.claude-code;
            };
          }
          // lib.optionalAttrs (cfg.host != null) { inherit (cfg) host; }
          // lib.optionalAttrs (cfg.author != null) { user = cfg.author; }
          // lib.optionalAttrs (cfg.turnTemplate != null) { turn_template = "${cfg.turnTemplate}"; }
        );
        command = [ "${cfg.package}/bin/ferret-ingest-agents" "--config" "${cfg.configFile}" "run" ];
      };
    };
}
