//! Turning a session into ferret documents.
//!
//! A session is one ferret thread with one document per turn: a user message
//! and every entry below it up to the next user message. Sessions are trees
//! (you go back and try again, a reply is retried), and so is the thread: a
//! turn's `parent` is the turn it went on from. Two turns with the same parent
//! are a fork. Ids never depend on which branch went on later, so continuing
//! a session anywhere, any time, keeps every existing document as it is.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Value, json};

use anyhow::Result;

use crate::model::{Content, Entry, Session, push_unique};
use crate::templates::Templates;
use crate::views::{self, TurnView};

/// ferret's request body limit is well above this; it only guards outliers.
const MAX_TEXT_BYTES: usize = 1_000_000;
const TITLE_CHARS: usize = 120;

#[derive(Debug, Clone)]
pub struct Options {
    /// An author of every turn, next to the models that answered.
    pub user: String,
    pub host: String,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            user: "user".to_owned(),
            host: "host".to_owned(),
        }
    }
}

/// One document as ferret's `PUT /api/documents` takes it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Document {
    pub source: String,
    pub external_id: String,
    pub content_type: &'static str,
    pub title: String,
    pub text: String,
    pub url: Option<String>,
    pub thread: Option<String>,
    /// The `external_id` of the turn this one went on from.
    pub parent: Option<String>,
    pub authors: Vec<String>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub meta: Value,
}

struct Turn<'a> {
    user: &'a Entry,
    /// Entries before the session's first user message, only on the first turn.
    before: Vec<&'a Entry>,
    rest: Vec<&'a Entry>,
}

impl Turn<'_> {
    fn answered(&self) -> bool {
        self.rest
            .iter()
            .any(|e| matches!(e.content, Content::Assistant { .. }))
    }

    /// The user, then each model that answered, in order.
    fn authors(&self, user: &str) -> Vec<String> {
        let mut authors = vec![user.to_owned()];
        for entry in &self.rest {
            if let Content::Assistant {
                model: Some(model), ..
            } = &entry.content
            {
                push_unique(&mut authors, Some(model));
            }
        }
        authors
    }
}

pub fn documents(
    session: &Session,
    source: &str,
    file: &str,
    options: &Options,
    templates: &Templates,
) -> Result<Vec<Document>> {
    let entries = &session.entries;
    let index: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    let parent_of = |i: usize| -> Option<usize> {
        entries[i]
            .parent
            .as_deref()
            .and_then(|p| index.get(p).copied())
    };
    let is_user = |i: usize| matches!(entries[i].content, Content::User { .. });

    // The turn an entry belongs to: its nearest user message, itself
    // included. Bounded by the number of entries, so a cycle can't hang.
    let owner = |mut i: usize| -> Option<usize> {
        for _ in 0..=entries.len() {
            if is_user(i) {
                return Some(i);
            }
            i = parent_of(i)?;
        }
        None
    };
    let owners: Vec<Option<usize>> = (0..entries.len()).map(owner).collect();

    let mut turns: HashMap<usize, Turn> = HashMap::new();
    let mut order = Vec::new();
    let mut preamble = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        match owners[i] {
            Some(u) if u == i => {
                order.push(i);
                turns.insert(
                    i,
                    Turn {
                        user: entry,
                        before: Vec::new(),
                        rest: Vec::new(),
                    },
                );
            }
            Some(u) => {
                if let Some(turn) = turns.get_mut(&u) {
                    turn.rest.push(entry);
                }
            }
            // What comes before any user message, like the summary a
            // continued session starts with, goes with the first turn.
            None => preamble.push(entry),
        }
    }
    // A command nobody answered, like one that only changes a setting, says
    // nothing worth finding.
    let kept = |u: &usize| turns[u].answered() || !is_command(turns[u].user);
    order.retain(kept);
    if let Some(first) = order.first() {
        turns.get_mut(first).unwrap().before = preamble;
    }

    // The turn a turn went on from, past any that were dropped.
    let parent_turn = |u: usize| -> Option<usize> {
        let mut at = u;
        for _ in 0..=entries.len() {
            at = owners[parent_of(at)?]?;
            if order.contains(&at) {
                return Some(at);
            }
        }
        None
    };
    let external_id = |u: usize| format!("turn:{}:{}", session.id, entries[u].id);

    let base_meta = json!({
        "harness": session.harness.name(),
        "host": options.host,
        "session": session.id,
        "session_name": session.name,
        "parent_session": session.parent_session,
        "session_started": session.started,
        "models": session.models,
        "cwd": session.cwd,
        "file": file,
    });

    let mut docs = Vec::new();
    for &u in &order {
        let turn = &turns[&u];
        let parts = turn
            .before
            .iter()
            .copied()
            .chain(std::iter::once(turn.user))
            .chain(turn.rest.iter().copied());
        let text = templates.turn(&TurnView {
            harness: session.harness.into(),
            host: &options.host,
            cwd: &session.cwd,
            parts: views::parts(parts),
        })?;
        let mut meta = base_meta.clone();
        meta["entry"] = json!(turn.user.id);
        docs.push(Document {
            source: source.to_owned(),
            external_id: external_id(u),
            content_type: "markdown",
            title: one_line(user_text(turn.user), TITLE_CHARS),
            text: cap(text),
            url: None,
            thread: Some(session.id.clone()),
            parent: parent_turn(u).map(external_id),
            authors: turn.authors(&options.user),
            created_at: turn.user.timestamp.clone(),
            modified_at: turn
                .rest
                .iter()
                .rev()
                .find_map(|e| e.timestamp.clone())
                .or_else(|| turn.user.timestamp.clone()),
            meta,
        });
    }
    Ok(docs)
}

fn user_text(entry: &Entry) -> &str {
    match &entry.content {
        Content::User { text, .. } => text,
        _ => "",
    }
}

fn is_command(entry: &Entry) -> bool {
    matches!(entry.content, Content::User { command: true, .. })
}

pub fn one_line(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn cap(text: String) -> String {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n…(truncated)", &text[..end])
}
