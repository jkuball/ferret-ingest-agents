//! Turning a session into ferret documents.
//!
//! Every branch of a session is one ferret thread: a session document as its
//! overview, followed by one document per turn (a user message and everything
//! up to the next one). A branch is a distinct sequence of user messages; a
//! path whose user messages are a prefix of another path's, like a retried
//! reply, is not one. Turns that branches share are written once per branch.
//! Their text and title don't depend on the branch, so ferret's embedding
//! cache serves the copies without embedding them again.

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
    pub authors: Vec<String>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub meta: Value,
}

pub struct Branch {
    /// The entry where this branch leaves the main one; None for the main branch.
    pub fork: Option<String>,
    /// Indices into the session's entries, root first.
    pub path: Vec<usize>,
}

/// The session's branches, main branch first. The main branch leads to the
/// entry written last, which is where the agent resumes; every other branch
/// is named by its first entry off the main branch.
pub fn branches(session: &Session) -> Vec<Branch> {
    let index: HashMap<&str, usize> = session
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    let mut children: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, entry) in session.entries.iter().enumerate() {
        let parent = entry.parent.as_deref().and_then(|p| index.get(p).copied());
        children.entry(parent).or_default().push(i);
    }

    // Every root-to-leaf path; a leaf's index is when it was written.
    let mut paths: Vec<Vec<usize>> = Vec::new();
    let mut stack: Vec<(Option<usize>, Vec<usize>)> = vec![(None, Vec::new())];
    while let Some((node, path)) = stack.pop() {
        let kids = children.get(&node).map(Vec::as_slice).unwrap_or_default();
        if kids.is_empty() {
            if !path.is_empty() {
                paths.push(path);
            }
            continue;
        }
        for &kid in kids.iter().rev() {
            let mut next = path.clone();
            next.push(kid);
            stack.push((Some(kid), next));
        }
    }
    let leaf = |p: &Vec<usize>| *p.last().unwrap();

    // A branch is a distinct sequence of user messages: a path is dropped
    // when another one's user messages extend its own, or equal them and the
    // other was written later, like a reply that was retried.
    let users: Vec<Vec<usize>> = paths
        .iter()
        .map(|p| {
            p.iter()
                .copied()
                .filter(|&i| matches!(session.entries[i].content, Content::User { .. }))
                .collect()
        })
        .collect();
    let mut kept: Vec<Vec<usize>> = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let covered = users.iter().enumerate().any(|(j, other)| {
            j != i
                && other.starts_with(&users[i])
                && (other.len() > users[i].len() || leaf(&paths[j]) > leaf(path))
        });
        if !covered {
            kept.push(path.clone());
        }
    }

    let Some(main) = kept.iter().max_by_key(|p| leaf(p)).cloned() else {
        return Vec::new();
    };
    let on_main: std::collections::HashSet<usize> = main.iter().copied().collect();
    let mut branches = vec![Branch {
        fork: None,
        path: main.clone(),
    }];
    for path in kept.into_iter().filter(|p| *p != main) {
        let off = path.iter().find(|i| !on_main.contains(i)).copied();
        branches.push(Branch {
            fork: off.map(|i| session.entries[i].id.clone()),
            path,
        });
    }
    branches
}

pub fn documents(
    session: &Session,
    source: &str,
    file: &str,
    options: &Options,
    templates: &Templates,
) -> Result<Vec<Document>> {
    let mut docs = Vec::new();
    for branch in branches(session) {
        docs.extend(branch_documents(
            session, &branch, source, file, options, templates,
        )?);
    }
    Ok(docs)
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

fn branch_documents(
    session: &Session,
    branch: &Branch,
    source: &str,
    file: &str,
    options: &Options,
    templates: &Templates,
) -> Result<Vec<Document>> {
    let thread = match &branch.fork {
        None => session.id.clone(),
        Some(fork) => format!("{}#{fork}", session.id),
    };

    // What comes before the first user message, like a summary a forked
    // session starts with, goes with the first turn.
    let mut preamble = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();
    for &i in &branch.path {
        let entry = &session.entries[i];
        match (&entry.content, turns.last_mut()) {
            (Content::User { .. }, _) => turns.push(Turn {
                user: entry,
                before: std::mem::take(&mut preamble),
                rest: Vec::new(),
            }),
            (_, Some(turn)) => turn.rest.push(entry),
            (_, None) => preamble.push(entry),
        }
    }
    // A command nobody answered, like one that only changes a setting, says
    // nothing worth finding.
    turns.retain(|t| t.answered() || !is_command(t.user));

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
        "branch": branch.fork,
    });

    let mut docs = Vec::new();
    for turn in &turns {
        // Only what all copies of this turn share: no file, session or title,
        // so the copies in forks and branches stay byte for byte the same.
        let entries = turn
            .before
            .iter()
            .copied()
            .chain(std::iter::once(turn.user))
            .chain(turn.rest.iter().copied());
        let text = templates.turn(&TurnView {
            harness: session.harness.into(),
            host: &options.host,
            cwd: &session.cwd,
            parts: views::parts(entries),
        })?;
        let mut meta = base_meta.clone();
        meta["entry"] = json!(turn.user.id);
        meta["parent_entry"] = json!(turn.user.parent);
        docs.push(Document {
            source: source.to_owned(),
            external_id: format!("turn:{thread}:{}", turn.user.id),
            content_type: "markdown",
            title: one_line(user_text(turn.user), TITLE_CHARS),
            text: cap(text),
            url: None,
            thread: Some(thread.clone()),
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
