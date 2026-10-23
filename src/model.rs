//! The harness-neutral shape of a session that the readers produce and the
//! rendering consumes: content entries linked into a tree.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::harnesses::Harness;

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    /// What the user sent. A command (a slash command, a loaded skill) is
    /// written as the user would type it, e.g. `/compact` or `/skill:name`.
    User {
        text: String,
        command: bool,
    },
    Assistant {
        model: Option<String>,
        /// In the order the model wrote them.
        blocks: Vec<Block>,
    },
    Summary(String),
    Extension {
        kind: String,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    Thinking(String),
    ToolCall(ToolCall),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: Option<String>,
    pub name: String,
    pub arguments: Value,
    /// What came back, measured; the output itself isn't kept.
    pub result: Option<ToolResult>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolResult {
    pub bytes: usize,
    pub lines: usize,
    pub error: bool,
}

impl ToolResult {
    /// Measures a result's content: a string or text blocks.
    pub fn of(content: &Value, error: bool) -> ToolResult {
        let text = text_blocks(content).join("\n");
        ToolResult {
            bytes: text.len(),
            lines: if text.is_empty() {
                0
            } else {
                text.lines().count()
            },
            error,
        }
    }
}

/// Gives every tool call the result recorded for its id.
pub fn attach_results(entries: &mut [Entry], results: &HashMap<String, ToolResult>) {
    for entry in entries {
        if let Content::Assistant { blocks, .. } = &mut entry.content {
            for block in blocks {
                if let Block::ToolCall(call) = block {
                    call.result = call.id.as_ref().and_then(|id| results.get(id)).cloned();
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    /// The nearest ancestor that is content, skipping the entries in between.
    pub parent: Option<String>,
    pub timestamp: Option<String>,
    pub content: Content,
}

#[derive(Debug)]
pub struct Session {
    pub harness: Harness,
    pub id: String,
    pub cwd: String,
    pub started: Option<String>,
    pub name: Option<String>,
    pub parent_session: Option<String>,
    pub models: Vec<String>,
    /// Content entries in file order.
    pub entries: Vec<Entry>,
    /// Whether the last line was cut off, as when the harness is still writing it.
    pub torn: bool,
}

/// One line of a session file as far as the tree is concerned. Lines that
/// aren't content still link their children to their parent.
pub struct Node<'a> {
    pub id: &'a str,
    pub parent: Option<&'a str>,
    pub timestamp: Option<&'a str>,
    pub content: Option<Content>,
}

/// Keeps the content nodes, each linked to its nearest content ancestor.
pub fn link(mut nodes: Vec<Node>) -> Vec<Entry> {
    // Claude Code sometimes logs an entry again under a later parent. The
    // first occurrence is where it happened; the later parent can descend from
    // it, which would make the entry its own ancestor.
    let mut seen = HashSet::new();
    nodes.retain(|n| seen.insert(n.id));
    let parents: HashMap<&str, Option<&str>> = nodes.iter().map(|n| (n.id, n.parent)).collect();
    let content: HashSet<&str> = nodes
        .iter()
        .filter(|n| n.content.is_some())
        .map(|n| n.id)
        .collect();
    let mut entries = Vec::new();
    for node in &nodes {
        if let Some(c) = &node.content {
            entries.push(Entry {
                id: node.id.to_owned(),
                parent: nearest_content(node.parent, &parents, &content),
                timestamp: node.timestamp.map(str::to_owned),
                content: c.clone(),
            });
        }
    }
    entries
}

fn nearest_content<'a>(
    mut parent: Option<&'a str>,
    parents: &HashMap<&'a str, Option<&'a str>>,
    content: &HashSet<&'a str>,
) -> Option<String> {
    // Bounded by the number of nodes, so a broken cycle can't hang.
    for _ in 0..=parents.len() {
        match parent {
            Some(p) if content.contains(p) => return Some(p.to_owned()),
            Some(p) => parent = parents.get(p).copied().flatten(),
            None => return None,
        }
    }
    None
}

pub fn push_unique(list: &mut Vec<String>, value: Option<&str>) {
    if let Some(v) = value.filter(|v| !v.is_empty())
        && !list.iter().any(|m| m == v)
    {
        list.push(v.to_owned());
    }
}

pub fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

/// The non-empty text of a message's content: a string or `text` blocks.
pub fn text_blocks(content: &Value) -> Vec<String> {
    let texts = match content {
        Value::String(s) => vec![s.trim().to_owned()],
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| str_of(b, "type") == Some("text"))
            .filter_map(|b| str_of(b, "text"))
            .map(|t| t.trim().to_owned())
            .collect(),
        _ => Vec::new(),
    };
    texts.into_iter().filter(|t| !t.is_empty()).collect()
}

pub fn user(text: String) -> Option<Content> {
    (!text.is_empty()).then_some(Content::User {
        text,
        command: false,
    })
}

/// An assistant message's blocks; `tool` names the harness's tool call type.
pub fn assistant_blocks(content: &Value, tool: &str, arguments: &str) -> Vec<Block> {
    let Some(blocks) = content.as_array() else {
        return text_blocks(content).into_iter().map(Block::Text).collect();
    };
    let trimmed = |b: &Value, key: &str| {
        str_of(b, key)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    blocks
        .iter()
        .filter_map(|b| match str_of(b, "type")? {
            "text" => trimmed(b, "text").map(Block::Text),
            "thinking" => trimmed(b, "thinking").map(Block::Thinking),
            kind if kind == tool => Some(Block::ToolCall(ToolCall {
                id: str_of(b, "id").map(str::to_owned),
                name: str_of(b, "name").unwrap_or("tool").to_owned(),
                arguments: b.get(arguments).cloned().unwrap_or(Value::Null),
                result: None,
            })),
            _ => None,
        })
        .collect()
}
