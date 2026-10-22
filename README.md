# ferret-ingest-agents

<p><em>Everything you worked out with your coding agent, searchable.</em></p>

ferret-ingest-agents reads the sessions of your coding agents and sends them to
[ferret](https://github.com/thiesgerken/ferret), a personal search index with
hybrid keyword and semantic search and an MCP server. You and your agents can
then find what was decided, tried and built – across every project, machine
and agent – instead of digging through session files.

Supported harnesses:

- [Pi](https://github.com/earendil-works/pi) (`~/.pi/agent/sessions`)
- [Claude Code](https://code.claude.com) (`~/.claude/projects`, subagents included)

## What ends up in ferret

One **document per turn**: a user message and everything up to the next one.
Every session is a ferret **thread**, so search hits collapse to one per
session and the viewer pages through it. A turn reads like this:

````markdown
**2026-10-18 09:00 UTC · User**

Find out whether celld works with my Ceph S3.

---
**2026-10-18 09:01 UTC · Agent (gpt-5.6-sol)**

- **bash** `rg -n -i "ceph|s3|bucket" . --glob '!manifests/**'`
  - ok · 291 lines, 51.3 KB
- **read** `/tmp/celld/crates/celld/bucket.rs`
  - ok · 51 lines, 1.9 KB

celld probes the bucket with a conditional write on startup…
````

- **User and agent messages** as written, with timestamps and the model.
- **Tool calls** as what went in (command, path, code) and how much came back,
  and whether it failed. **Not** their output: it mostly repeats files that
  live elsewhere, and the same `SKILL.md` or `flake.nix` read in hundreds of
  sessions would otherwise match every search.
- **Compaction summaries** where they happened.
- **Slash commands and loaded skills** as you typed them (`/compact`,
  `/skill:capture …`). A command the agent didn't answer, like one that only
  changes a setting, is left out.
- **Not thinking.** It can be turned on (see [Templates](#templates)).

Each turn's `authors` are you and the models that answered, so you can filter
by model in ferret. Its `meta` says where it comes from, for agents reading
through MCP or the API:

| Field | |
|---|---|
| `harness`, `host` | which agent, on which machine |
| `cwd` | the session's working directory |
| `file` | the session file |
| `session`, `session_name`, `session_started` | the session's id, name and start |
| `models` | the models in the session |
| `parent_session` | the session this one was forked from |
| `branch` | the entry where this branch leaves the session |

### Branches

Sessions are trees: you go back and try again, a reply is retried, a session
is forked. Every branch becomes its own thread; the branch that went on last
keeps the session's id, the others get `<session>#<entry>`. Turns that
branches share are copied into each, byte for byte, so ferret's embedding
cache embeds them only once. A search may show such a turn once per branch.

## Installation

You need a ferret instance and an API key that may write documents
(`ingest:write`). Documents go to the source `<host>/agents/<harness>`, so a
key limited to `<host>` covers everything one machine sends.

### home-manager

```nix
inputs.ferret-ingest-agents.url = "github:jkuball/ferret-ingest-agents";
inputs.ferret-ingest-agents.inputs.nixpkgs.follows = "nixpkgs";
```

```nix
{
  imports = [ inputs.ferret-ingest-agents.homeModules.default ];

  services.ferret-ingest-agents = {
    enable = true;
    url = "https://ferret.example.com";
    keyFile = "/run/secrets/ferret-ingest-agents"; # a path string, kept out of the store
    harnesses.pi.enable = true;
    harnesses.claude-code.enable = true;
    # Sessions started below these directories stay on this machine.
    exclude = [ "~/work/**" ];
  };
}
```

Harnesses are off unless enabled; one that isn't installed is skipped.

It runs every 15 minutes (`interval`): as a launchd agent on macOS (logs in
`~/Library/Logs/ferret-ingest-agents.log`), as a systemd user timer on Linux.

### NixOS and nix-darwin

`nixosModules.default` and `darwinModules.default` take the same options plus
`user`, the account whose sessions are indexed and who runs the ingester:

```nix
services.ferret-ingest-agents = {
  enable = true;
  user = "alice";
  # …as above
};
```

### Without the modules

With Nix but none of the above, install the package and write the config
file (below) to `~/.config/ferret-ingest-agents/config.toml`:

```sh
nix profile install github:jkuball/ferret-ingest-agents
ferret-ingest-agents run --dry-run   # what would be sent
ferret-ingest-agents run
```

Without Nix, `cargo install --git https://github.com/jkuball/ferret-ingest-agents`.

To keep ferret up to date, run it every few minutes, e.g. with cron
(`crontab -e`) on macOS and Linux alike:

```
*/15 * * * * ~/.nix-profile/bin/ferret-ingest-agents run >> /tmp/ferret-ingest-agents.log 2>&1
```

## Configuration

The Nix modules write this file for you.

```toml
url = "https://ferret.example.com"
# A file with the API key. Or leave this out and set FERRET_KEY.
key_file = "~/.config/ferret-ingest-agents/key"

# Names this machine in sources. The short hostname by default.
# host = "laptop"

# You, an author of every turn next to the models. $USER by default.
# user = "alice"

# Your own template for turns instead of the built-in one (see Templates).
# turn_template = "~/.config/ferret-ingest-agents/turn.md.j2"

# What earlier runs sent, so the next one only sends what changed.
# state_file = "~/.local/state/ferret-ingest-agents/state.json"

# How many session files are read and sent at once.
# concurrency = 4

# Which sessions are indexed, as globs on their working directory: included,
# and not excluded. `dir/**` covers dir and everything below it.
include = ["**"]
exclude = ["~/work/**"]

# One table per harness turns it on. Both settings are optional.
[harnesses.pi]
# sessions_dir = "~/.pi/agent/sessions"   # where its sessions are
# source = "laptop/agents/pi"             # the ferret source; <host>/agents/pi by default

[harnesses.claude-code]
# sessions_dir = "~/.claude/projects"
# source = "laptop/agents/claude-code"
```

## Usage

```sh
ferret-ingest-agents run                  # send new and changed sessions
ferret-ingest-agents run --dry-run        # count what would be sent
ferret-ingest-agents run --file <session> # just these session files
ferret-ingest-agents render <session>     # print the documents, as JSON lines
```

Runs are incremental. A state file remembers what each session file produced,
so unchanged files are skipped and only documents whose content changed are
sent. Turns and branches that disappear are deleted from ferret, as are
sessions you exclude later. A deleted session file stays in ferret, as an
archive. Changing the template or options renders everything again, still
sending only what changed.

## Templates

A turn's Markdown comes from a [minijinja](https://github.com/mitsuhiko/minijinja)
template; [`templates/turn.md.j2`](templates/turn.md.j2) is the built-in one
and documents everything it receives. Copy it and point `turn_template` at your
version to change what is shown and how much: tool calls, thinking, labels,
truncation. Titles, ids, threads, authors and `meta` stay in the code, since
they decide identity and what ferret can cache.

Filters besides minijinja's own (like `tojson`): `time`, `bytes`, `code`,
`fence`, `quote`, `one_line(n)` and `home`.

A changed template means new text, and new text is embedded again – choose
once rather than often.

## Contributing

`nix develop` has the Rust toolchain and the formatter (`nix fmt -- .`).
`nix flake check` builds, runs the tests and Clippy. Tests use synthetic
sessions only; please keep real session data out of the repository.

The harnesses' session formats are their own and change; a reader takes only
the few fields it needs and skips everything else. Adding a harness means a
reader in `src/harnesses/` that fills the shared model in `src/model.rs`.

## License

MIT
