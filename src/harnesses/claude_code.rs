//! Claude Code transcripts: `<dir>/<project>/<session id>.jsonl`, with
//! subagents in `<dir>/<project>/<session id>/subagents/<agent>.jsonl`.
//! Entries form a tree through `uuid` and `parentUuid`.
//!
//! The format is internal to Claude Code and grows new entry types often, so
//! only a few fields are read and everything unknown is skipped. Content are
//! the user's own messages, assistant messages and compaction summaries. Tool
//! results arrive as user messages and are left out, as are messages Claude
//! Code injects (meta messages, command output, task notifications).

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::Harness;
use crate::jsonl;
use crate::model::{
    Content, Node, Session, ToolResult, assistant_blocks, attach_results, link, push_unique,
    str_of, text_blocks, user,
};

/// User messages starting with these are written by Claude Code, not the user.
const INJECTED: &[&str] = &[
    "<local-command-caveat>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<task-notification>",
    "<system-reminder>",
    "<bash-stdout>",
    "<bash-stderr>",
];

pub fn discover(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = super::files_below(dir, 2)?;
    for project in super::dirs(dir)? {
        for session in super::dirs(&project)? {
            files.extend(super::files_below(&session.join("subagents"), 1).unwrap_or_default());
        }
    }
    files.sort();
    Ok(files)
}

pub fn parse(text: &str, path: &Path) -> Result<Session> {
    let (lines, torn) = jsonl::parse(text, path)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    let subagent = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|d| d == "subagents");

    let mut session_id = None;
    let mut agent_id = None;
    let mut cwd = None;
    let mut started = None;
    let mut ai_title = None;
    let mut custom_title = None;
    let mut models = Vec::new();
    let mut nodes = Vec::new();
    let mut results = std::collections::HashMap::new();

    for e in &lines {
        session_id = session_id.or(str_of(e, "sessionId"));
        agent_id = agent_id.or(str_of(e, "agentId"));
        cwd = cwd.or(str_of(e, "cwd"));
        if started.is_none() {
            started = str_of(e, "timestamp");
        }
        match str_of(e, "type") {
            Some("ai-title") => ai_title = str_of(e, "aiTitle").or(ai_title),
            Some("custom-title") => custom_title = str_of(e, "customTitle").or(custom_title),
            _ => {}
        }
        if let Some(message) = e.get("message") {
            push_unique(
                &mut models,
                str_of(message, "model").filter(|m| !m.starts_with('<')),
            );
            let blocks = message.get("content").and_then(Value::as_array);
            for block in blocks.into_iter().flatten() {
                if str_of(block, "type") == Some("tool_result")
                    && let Some(id) = str_of(block, "tool_use_id")
                {
                    let error = block.get("is_error").and_then(Value::as_bool) == Some(true);
                    let content = block.get("content").unwrap_or(&Value::Null);
                    results.insert(id.to_owned(), ToolResult::of(content, error));
                }
            }
        }
        if let Some(id) = str_of(e, "uuid") {
            nodes.push(Node {
                id,
                // A compaction boundary starts a new chain and names the
                // entry it continues in logicalParentUuid.
                parent: str_of(e, "parentUuid").or(str_of(e, "logicalParentUuid")),
                timestamp: str_of(e, "timestamp"),
                content: content_of(e),
            });
        }
    }

    let session_id = session_id
        .map(str::to_owned)
        .unwrap_or_else(|| stem.clone());
    let (id, parent_session) = if subagent {
        let agent = agent_id.map(str::to_owned).unwrap_or(stem);
        (format!("{session_id}/{agent}"), Some(session_id))
    } else {
        (session_id, None)
    };

    let mut entries = link(nodes);
    attach_results(&mut entries, &results);

    Ok(Session {
        harness: Harness::ClaudeCode,
        id,
        cwd: cwd.unwrap_or_default().to_owned(),
        started: started.map(str::to_owned),
        name: custom_title
            .or(ai_title)
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty()),
        parent_session,
        models,
        entries,
        torn,
    })
}

/// A slash command arrives as `<command-name>/x</command-name>` with its
/// message and arguments in sibling tags. It becomes `/x args`, as typed.
fn slash_command(text: &str) -> Option<String> {
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{name}>"))? + start;
        Some(text[start..end].trim())
    };
    if !text.starts_with("<command-") {
        return None;
    }
    let name = tag("command-name").filter(|n| !n.is_empty())?;
    Some(match tag("command-args").filter(|a| !a.is_empty()) {
        Some(args) => format!("{name} {args}"),
        None => name.to_owned(),
    })
}

fn content_of(e: &Value) -> Option<Content> {
    let message = e.get("message")?;
    let content = message.get("content").unwrap_or(&Value::Null);
    match str_of(e, "type")? {
        "user" if e.get("isCompactSummary").and_then(Value::as_bool) == Some(true) => {
            let text = text_blocks(content).join("\n\n");
            (!text.is_empty()).then_some(Content::Summary(text))
        }
        "user" if e.get("isMeta").and_then(Value::as_bool) != Some(true) => {
            let text = text_blocks(content).join("\n\n");
            if INJECTED.iter().any(|tag| text.starts_with(tag)) {
                return None;
            }
            match slash_command(&text) {
                Some(command) => Some(Content::User {
                    text: command,
                    command: true,
                }),
                None => user(text),
            }
        }
        "assistant" => Some(Content::Assistant {
            model: str_of(message, "model")
                .filter(|m| !m.starts_with('<'))
                .map(str::to_owned),
            blocks: assistant_blocks(content, "tool_use", "input"),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(values: &[Value]) -> String {
        values.iter().map(|v| format!("{v}\n")).collect()
    }

    #[test]
    fn injected_and_tool_result_messages_are_not_content() {
        let text = lines(&[
            json!({"type": "user", "uuid": "1", "parentUuid": null, "sessionId": "s", "cwd": "/p",
                   "message": {"role": "user", "content": "Real question"}}),
            json!({"type": "assistant", "uuid": "2", "parentUuid": "1",
                   "message": {"role": "assistant", "model": "m", "content": [
                       {"type": "text", "text": "Answer"},
                       {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}}]}}),
            json!({"type": "user", "uuid": "3", "parentUuid": "2",
                   "message": {"role": "user", "content": [{"type": "tool_result", "content": "out"}]}}),
            json!({"type": "user", "uuid": "4", "parentUuid": "3", "isMeta": true,
                   "message": {"role": "user", "content": "meta"}}),
            json!({"type": "user", "uuid": "5", "parentUuid": "4",
                   "message": {"role": "user", "content": "<local-command-stdout>x</local-command-stdout>"}}),
            json!({"type": "user", "uuid": "6", "parentUuid": "5",
                   "message": {"role": "user", "content": "Follow-up"}}),
            json!({"type": "ai-title", "aiTitle": "Generated", "sessionId": "s"}),
            json!({"type": "custom-title", "customTitle": "Mine", "sessionId": "s"}),
        ]);
        let session = parse(&text, Path::new("/x/proj/s.jsonl")).unwrap();
        assert_eq!(session.id, "s");
        assert_eq!(session.name.as_deref(), Some("Mine"));
        assert_eq!(session.models, ["m"]);
        let ids: Vec<_> = session.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "6"]);
        assert_eq!(session.entries[2].parent.as_deref(), Some("2"));
    }

    #[test]
    fn a_slash_command_becomes_what_was_typed() {
        let text = "<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>";
        assert_eq!(slash_command(text).as_deref(), Some("/model opus"));
        assert_eq!(
            slash_command("<command-name>/voice</command-name><command-args></command-args>")
                .as_deref(),
            Some("/voice")
        );
        assert_eq!(slash_command("just text"), None);
    }

    #[test]
    fn a_compaction_boundary_continues_its_logical_parent() {
        let text = lines(&[
            json!({"type": "user", "uuid": "1", "parentUuid": null, "message": {"content": "Before"}}),
            json!({"type": "system", "subtype": "compact_boundary", "uuid": "2", "parentUuid": null, "logicalParentUuid": "1"}),
            json!({"type": "user", "uuid": "3", "parentUuid": "2", "isCompactSummary": true, "message": {"content": "Summary"}}),
            json!({"type": "user", "uuid": "4", "parentUuid": "3", "message": {"content": "After"}}),
        ]);
        let session = parse(&text, Path::new("/x/proj/s.jsonl")).unwrap();
        assert_eq!(session.entries[1].parent.as_deref(), Some("1"));
        assert!(matches!(session.entries[1].content, Content::Summary(_)));
    }

    #[test]
    fn a_relogged_entry_keeps_its_first_parent() {
        let text = lines(&[
            json!({"type": "user", "uuid": "1", "parentUuid": null, "message": {"content": "Before"}}),
            json!({"type": "user", "uuid": "2", "parentUuid": "1", "message": {"content": "/compact"}}),
            json!({"type": "system", "uuid": "3", "parentUuid": "2"}),
            json!({"type": "system", "uuid": "4", "parentUuid": "3"}),
            json!({"type": "user", "uuid": "2", "parentUuid": "4", "message": {"content": "/compact"}}),
            json!({"type": "system", "uuid": "5", "parentUuid": "2"}),
            json!({"type": "user", "uuid": "6", "parentUuid": "5", "message": {"content": "After"}}),
        ]);
        let session = parse(&text, Path::new("/x/proj/s.jsonl")).unwrap();
        let links: Vec<_> = session
            .entries
            .iter()
            .map(|e| (e.id.as_str(), e.parent.as_deref()))
            .collect();
        assert_eq!(links, [("1", None), ("2", Some("1")), ("6", Some("2"))]);
    }

    #[test]
    fn a_subagent_is_its_own_session_under_its_parent() {
        let text = lines(&[
            json!({"type": "user", "uuid": "1", "parentUuid": null, "sessionId": "s",
                                  "agentId": "a1", "isSidechain": true, "message": {"content": "Task"}}),
        ]);
        let session = parse(&text, Path::new("/x/proj/s/subagents/agent-a1.jsonl")).unwrap();
        assert_eq!(session.id, "s/a1");
        assert_eq!(session.parent_session.as_deref(), Some("s"));
    }
}
