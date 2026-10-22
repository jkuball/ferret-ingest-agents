//! The harnesses whose sessions can be read, each into the same model.

mod claude_code;
mod pi;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Harness {
    Pi,
    ClaudeCode,
}

impl Harness {
    pub const ALL: [Harness; 2] = [Harness::Pi, Harness::ClaudeCode];

    /// The name in the config and in ferret sources.
    pub fn name(self) -> &'static str {
        match self {
            Harness::Pi => "pi",
            Harness::ClaudeCode => "claude-code",
        }
    }

    /// How the harness is called in documents.
    pub fn label(self) -> &'static str {
        match self {
            Harness::Pi => "Pi",
            Harness::ClaudeCode => "Claude Code",
        }
    }

    pub fn from_name(name: &str) -> Option<Harness> {
        Harness::ALL.into_iter().find(|a| a.name() == name)
    }

    pub fn default_dir(self) -> &'static str {
        match self {
            Harness::Pi => "~/.pi/agent/sessions",
            Harness::ClaudeCode => "~/.claude/projects",
        }
    }

    pub fn discover(self, dir: &Path) -> Result<Vec<PathBuf>> {
        match self {
            Harness::Pi => pi::discover(dir),
            Harness::ClaudeCode => claude_code::discover(dir),
        }
    }

    pub fn read(self, path: &Path) -> Result<Session> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        self.parse(&text, path)
    }

    pub fn parse(self, text: &str, path: &Path) -> Result<Session> {
        match self {
            Harness::Pi => pi::parse(text, path),
            Harness::ClaudeCode => claude_code::parse(text, path),
        }
    }
}

fn dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            found.push(path);
        }
    }
    Ok(found)
}

/// The `.jsonl` files exactly `depth` levels below `dir` (1: directly in it).
fn files_below(dir: &Path, depth: usize) -> Result<Vec<PathBuf>> {
    let mut levels = vec![dir.to_path_buf()];
    for _ in 1..depth {
        levels = levels
            .iter()
            .map(|d| dirs(d))
            .collect::<Result<Vec<_>>>()?
            .concat();
    }
    let mut files = Vec::new();
    for level in levels {
        for entry in
            std::fs::read_dir(&level).with_context(|| format!("reading {}", level.display()))?
        {
            let path = entry?.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}
