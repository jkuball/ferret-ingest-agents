//! Pi (pi-coding-agent) sessions: `<dir>/<project>/<timestamp>_<id>.jsonl`, a
//! `session` header followed by entries that form a tree through `parentId`.
//!
//! Content are user and assistant messages, compaction and branch summaries,
//! and extension messages Pi displays. Tool results, model changes and
//! extension state only link the tree. Pi copies a parent session's entries,
//! with their ids, into a forked session's file, so every file stands alone.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

use super::Harness;
use crate::jsonl;
use crate::model::{
    Content, Node, Session, ToolResult, assistant_blocks, attach_results, link, push_unique,
    str_of, text_blocks, user,
};

pub fn discover(dir: &Path) -> Result<Vec<PathBuf>> {
    super::files_below(dir, 2)
}

pub fn parse(text: &str, path: &Path) -> Result<Session> {
    let (lines, torn) = jsonl::parse(text, path)?;
    let Some(header) = lines.iter().find(|e| str_of(e, "type") == Some("session")) else {
        bail!("{}: no session header", path.display());
    };

    let mut name = None;
    let mut models = Vec::new();
    let mut nodes = Vec::new();
    let mut results = std::collections::HashMap::new();
    for e in &lines {
        match str_of(e, "type") {
            Some("session_info") => {
                if let Some(n) = str_of(e, "name").filter(|n| !n.trim().is_empty()) {
                    name = Some(n.trim().to_owned());
                }
            }
            Some("model_change") => push_unique(&mut models, str_of(e, "modelId")),
            _ => {}
        }
        if let Some(message) = e.get("message") {
            push_unique(&mut models, str_of(message, "model"));
            if str_of(message, "role") == Some("toolResult")
                && let Some(id) = str_of(message, "toolCallId")
            {
                let error = message.get("isError").and_then(Value::as_bool) == Some(true);
                let content = message.get("content").unwrap_or(&Value::Null);
                results.insert(id.to_owned(), ToolResult::of(content, error));
            }
        }
        if let Some(id) = str_of(e, "id") {
            nodes.push(Node {
                id,
                parent: str_of(e, "parentId"),
                timestamp: str_of(e, "timestamp"),
                content: content_of(e),
            });
        }
    }

    let mut entries = link(nodes);
    attach_results(&mut entries, &results);

    Ok(Session {
        harness: Harness::Pi,
        id: str_of(header, "id")
            .map(str::to_owned)
            .unwrap_or_else(|| id_from_path(path)),
        cwd: str_of(header, "cwd").unwrap_or_default().to_owned(),
        started: str_of(header, "timestamp").map(str::to_owned),
        name,
        parent_session: str_of(header, "parentSession").map(|p| id_from_path(Path::new(p))),
        models,
        entries,
        torn,
    })
}

/// Pi names its files `<timestamp>_<session id>.jsonl`.
fn id_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    stem.rsplit_once('_').map_or(stem, |(_, id)| id).to_owned()
}

/// A loaded skill arrives as `<skill name="…" location="…">…</skill>`, the
/// skill's whole file, followed by what the user wrote with it. It becomes
/// `/skill:<name>` and that text, as typed.
fn skill_command(text: &str) -> Option<String> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let (name, _) = rest.split_once('"')?;
    let (_, after) = rest.split_once("</skill>")?;
    let after = after.trim();
    Some(if after.is_empty() {
        format!("/skill:{name}")
    } else {
        format!("/skill:{name} {after}")
    })
}

fn content_of(e: &Value) -> Option<Content> {
    match str_of(e, "type")? {
        "message" => {
            let message = e.get("message")?;
            let content = message.get("content").unwrap_or(&Value::Null);
            match str_of(message, "role")? {
                "user" => {
                    let text = text_blocks(content).join("\n\n");
                    match skill_command(&text) {
                        Some(command) => Some(Content::User {
                            text: command,
                            command: true,
                        }),
                        None => user(text),
                    }
                }
                "assistant" => Some(Content::Assistant {
                    model: str_of(message, "model").map(str::to_owned),
                    blocks: assistant_blocks(content, "toolCall", "arguments"),
                }),
                _ => None,
            }
        }
        "compaction" | "branch_summary" => {
            let summary = str_of(e, "summary")?.trim();
            (!summary.is_empty()).then(|| Content::Summary(summary.to_owned()))
        }
        "custom_message" if e.get("display").and_then(Value::as_bool) == Some(true) => {
            let text = text_blocks(e.get("content").unwrap_or(&Value::Null)).join("\n\n");
            (!text.is_empty()).then(|| Content::Extension {
                kind: str_of(e, "customType").unwrap_or("extension").to_owned(),
                text,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_skill_becomes_its_command() {
        let text = "<skill name=\"capture\" location=\"/s/SKILL.md\">\nLong skill body\n</skill>\n\nthe note";
        assert_eq!(
            skill_command(text).as_deref(),
            Some("/skill:capture the note")
        );
        assert_eq!(skill_command("plain text"), None);
    }

    #[test]
    fn a_file_without_a_header_is_no_session() {
        assert!(
            parse(
                "{\"type\":\"message\",\"id\":\"a\"}\n",
                Path::new("x_s1.jsonl")
            )
            .is_err()
        );
    }

    #[test]
    fn the_parent_session_is_named_by_its_file() {
        let header = r#"{"type":"session","id":"s2","cwd":"/p","parentSession":"/x/2026-01-01T00-00-00-000Z_s1.jsonl"}"#;
        let session = parse(&format!("{header}\n"), Path::new("x_s2.jsonl")).unwrap();
        assert_eq!(session.parent_session.as_deref(), Some("s1"));
    }
}
