//! What the templates see. Documented in the templates themselves.

use serde::Serialize;

use crate::harnesses::Harness;
use crate::model::{Block, Content, Entry, ToolResult};

#[derive(Serialize)]
pub struct HarnessView {
    pub name: &'static str,
    pub label: &'static str,
}

impl From<Harness> for HarnessView {
    fn from(harness: Harness) -> Self {
        HarnessView {
            name: harness.name(),
            label: harness.label(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Part {
    User {
        text: String,
        command: bool,
    },
    Assistant {
        text: String,
    },
    Thinking {
        text: String,
    },
    Tools {
        calls: Vec<CallView>,
    },
    /// A compaction or branch summary the harness wrote.
    Summary {
        text: String,
    },
    Extension {
        #[serde(rename = "type")]
        kind: String,
        text: String,
    },
}

#[derive(Serialize)]
pub struct CallView {
    pub name: String,
    /// As the model wrote them; templates pick and cut what they show.
    pub arguments: serde_json::Value,
    pub result: Option<ToolResult>,
}

#[derive(Serialize)]
pub struct TurnView<'a> {
    pub harness: HarnessView,
    pub host: &'a str,
    pub cwd: &'a str,
    pub parts: Vec<PartView>,
}

/// A part with who wrote it and when.
#[derive(Serialize)]
pub struct PartView {
    #[serde(flatten)]
    pub part: Part,
    /// `user`, `agent`, or the extension's type.
    pub speaker: String,
    /// Whether the speaker changed with this part, so a template can open a
    /// new message here.
    pub first: bool,
    /// The timestamp of the entry the part comes from, as recorded.
    pub at: Option<String>,
    /// The model of an agent part, when known.
    pub model: Option<String>,
}

/// The parts of a stretch of entries in order. Tool calls in a row form one
/// part, across messages, until text comes between them.
pub fn parts<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Vec<PartView> {
    let mut parts: Vec<PartView> = Vec::new();
    for entry in entries {
        let at = entry.timestamp.as_ref();
        match &entry.content {
            Content::User { text, command } => push(
                &mut parts,
                at,
                Part::User {
                    text: text.clone(),
                    command: *command,
                },
                "user",
                None,
            ),
            Content::Summary(text) => push(
                &mut parts,
                at,
                Part::Summary { text: text.clone() },
                "summary",
                None,
            ),
            Content::Extension { kind, text } => push(
                &mut parts,
                at,
                Part::Extension {
                    kind: kind.clone(),
                    text: text.clone(),
                },
                kind,
                None,
            ),
            Content::Assistant { blocks, model } => {
                for block in blocks {
                    match block {
                        Block::Text(text) => push(
                            &mut parts,
                            at,
                            Part::Assistant { text: text.clone() },
                            "agent",
                            model.as_ref(),
                        ),
                        Block::Thinking(text) => push(
                            &mut parts,
                            at,
                            Part::Thinking { text: text.clone() },
                            "agent",
                            model.as_ref(),
                        ),
                        Block::ToolCall(call) => {
                            let view = CallView {
                                name: call.name.clone(),
                                arguments: call.arguments.clone(),
                                result: call.result.clone(),
                            };
                            // Thinking between calls doesn't end a run of them.
                            let run = parts
                                .iter_mut()
                                .rev()
                                .find(|p| !matches!(p.part, Part::Thinking { .. }));
                            if let Some(PartView {
                                part: Part::Tools { calls },
                                ..
                            }) = run
                            {
                                calls.push(view);
                            } else {
                                push(
                                    &mut parts,
                                    at,
                                    Part::Tools { calls: vec![view] },
                                    "agent",
                                    model.as_ref(),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    parts
}

fn push(
    parts: &mut Vec<PartView>,
    at: Option<&String>,
    part: Part,
    speaker: &str,
    model: Option<&String>,
) {
    let first = parts.last().is_none_or(|p| p.speaker != speaker);
    parts.push(PartView {
        part,
        speaker: speaker.to_owned(),
        first,
        at: at.cloned(),
        model: model.cloned(),
    });
}
