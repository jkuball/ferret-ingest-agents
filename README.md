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

- **bash**
  - -> `rg -n -i "ceph|s3|bucket" . --glob '!manifests/**'`
  - <- ok · 291 lines, 51.3 KB
- **read**
  - -> `/tmp/celld/crates/celld/bucket.rs`
  - <- ok · 51 lines, 1.9 KB

celld probes the bucket with a conditional write on startup…
````

- **User and agent messages** as written, with timestamps and the model.
- **Tool calls** as what went in (command, path, code) and how much came back,
  and whether it failed. **Not** their output: it mostly repeats files that
  live elsewhere, and the same `SKILL.md` or `flake.nix` read in hundreds of
  sessions would otherwise match every search.
- **Compaction summaries** where they happened.
- **Thinking** is passed to the template but left out by default.
- Slash commands and loaded skills as typed (`/compact`, `/skill:capture …`);
  ones nobody answered are dropped.

Where a turn comes from is in its `meta`, for agents reading through MCP or
the API: `harness`, `host`, `cwd`, `file`, `session`, `session_name`,
`session_started`, `models`, `parent_session`, `branch`. `authors` are you and
the models that answered, so ferret can filter by model.

### Branches

Sessions are trees: you go back and try again, a reply is retried, a session
is forked. Every branch becomes its own thread; the branch that went on last
keeps the session's id, the others get `<session>#<entry>`. Turns that
branches share are copied into each, byte for byte, so ferret's embedding
cache embeds them once. A search may show such a turn once per branch.

## Installation

You need a ferret instance and an API key with `ingest:write` (Settings → API
keys). Scope it to the machine's source, e.g. `laptop`: sources are
`<host>/agents/<harness>`, so one key covers all agents of one machine.

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
    exclude = [ "~/work/**" ];
  };
}
```

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

The modules are also published as `modules.{homeManager,nixos,darwin}.default`
for [flake-parts](https://flake.parts/options/flake-parts-modules.html).

### By hand

```sh
nix run github:jkuball/ferret-ingest-agents -- run --dry-run
```

or `cargo build --release`, with a config file (below) at
`$XDG_CONFIG_HOME/ferret-ingest-agents/config.toml`.

## Configuration

The modules write this file for you.

```toml
url = "https://ferret.example.com"
key_file = "~/.config/ferret-ingest-agents/key"   # or FERRET_KEY in the environment

# host = "laptop"            # names the machine in sources; the short hostname by default
# user = "alice"             # you, in every turn's authors; $USER by default
# turn_template = "~/.config/ferret-ingest-agents/turn.md.j2"
# state_file = "~/.local/state/ferret-ingest-agents/state.json"
# concurrency = 4

# Globs on a session's working directory. `dir/**` covers dir and everything below.
include = ["**"]
exclude = ["~/work/**"]

# Each table turns a harness on.
[harnesses.pi]
# sessions_dir = "~/.pi/agent/sessions"
# source = "laptop/agents/pi"

[harnesses.claude-code]
# sessions_dir = "~/.claude/projects"
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
