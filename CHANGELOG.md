# Changelog

## [0.1.0] - 2026-11-12

### 🐛 Fix a bug

- Let Presswerk fetch release assets
- Restrict publication to the main release chain
- Restore the release workflow
- Reject stale release candidates
- Accept static PIE release binaries
- Keep the first of duplicated Claude Code entries

### 🔧 Add or update configuration files

- Keep exact candidate publication semantics
- Reset the unreleased version baseline
- home-manager: link the config to where the CLI looks for it

### 🔒️ Fix security or privacy issues

- Use the built-in token for release builds
- Limit release workflow tokens

### ♻️ Refactor code

- Group Rust build bindings by concern
- Keep each session in one thread, linked by parent turns

### 💄 Add or update the UI and style files

- Show a tool call as one item with its result below

### 📝 Add or update documentation

- README: explain the config, meta and installing without modules

### ✨ Introduce new features

- Add Presswerk releases

### 🎉 Begin a project

- Index Pi and Claude Code sessions into ferret

