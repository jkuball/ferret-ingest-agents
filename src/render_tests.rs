//! Rendering synthetic sessions; no real session data is checked in.

use std::path::Path;

use serde_json::{Value, json};

use crate::harnesses::Harness;
use crate::model::Session;
use crate::render::{Document, Options, documents};
use crate::templates::Templates;

fn session(lines: &[Value]) -> Session {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    Harness::Pi
        .parse(&text, Path::new("2026-01-01T00-00-00-000Z_s1.jsonl"))
        .unwrap()
}

fn header() -> Value {
    json!({"type": "session", "version": 3, "id": "s1", "timestamp": "2026-01-01T00:00:00Z", "cwd": "/home/me/project"})
}

fn user(id: &str, parent: Option<&str>, text: &str) -> Value {
    json!({"type": "message", "id": id, "parentId": parent, "timestamp": format!("2026-01-01T00:{id}Z"),
           "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
}

fn assistant(id: &str, parent: &str, text: &str) -> Value {
    json!({"type": "message", "id": id, "parentId": parent, "timestamp": format!("2026-01-01T00:{id}Z"),
           "message": {"role": "assistant", "model": "m1", "content": [
               {"type": "thinking", "thinking": "private reasoning"},
               {"type": "text", "text": text},
               {"type": "toolCall", "id": format!("call-{id}"), "name": "bash", "arguments": {"command": "ls\n  -la"}}]}})
}

fn tool_result(id: &str, parent: &str) -> Value {
    json!({"type": "message", "id": id, "parentId": parent,
           "message": {"role": "toolResult", "toolCallId": format!("call-{parent}"), "isError": true,
                       "content": [{"type": "text", "text": "raw tool output"}]}})
}

fn render(lines: &[Value]) -> Vec<Document> {
    let templates = Templates::load(None).unwrap();
    documents(
        &session(lines),
        "pi/test",
        "~/x.jsonl",
        &Options::default(),
        &templates,
    )
    .unwrap()
}

fn turns(docs: &[Document]) -> Vec<&Document> {
    docs.iter().collect()
}

#[test]
fn a_linear_session_is_one_thread_of_turns() {
    let docs = render(&[
        header(),
        user("01", None, "First question"),
        assistant("02", "01", "First answer"),
        tool_result("03", "02"),
        json!({"type": "custom_message", "id": "04", "parentId": "03", "customType": "relay", "display": false, "content": "noise"}),
        user("05", Some("04"), "Second question"),
    ]);

    assert_eq!(docs.len(), 2);
    assert!(docs.iter().all(|d| d.thread.as_deref() == Some("s1")));

    let turns = turns(&docs);
    assert_eq!(turns[0].external_id, "turn:s1:01");
    assert!(turns[0].text.contains("First answer"));
    assert!(
        turns[0]
            .text
            .starts_with("**2026-01-01 00:01 UTC · User**\n\nFirst question")
    );
    assert!(
        turns[0]
            .text
            .contains("\n\n---\n**2026-01-01 00:02 UTC · Agent (m1)**\n\n")
    );
    assert!(
        turns[0]
            .text
            .contains("First answer\n\n- **bash**\n  - -> `ls -la`\n  - <- failed · 1 line, 15 B")
    );
    assert!(!turns[0].text.contains("raw tool output"));
    assert!(!turns[0].text.contains("private reasoning"));
    assert!(!turns[0].text.contains("noise"));
    // The hidden entries between them don't break the chain.
    assert_eq!(turns[1].meta["parent_entry"], "02");
}

#[test]
fn a_session_name_is_meta_and_not_a_title() {
    let docs = render(&[
        header(),
        user("01", None, "Question"),
        assistant("02", "01", "A"),
        json!({"type": "session_info", "id": "03", "parentId": "02", "name": "Named"}),
    ]);
    assert_eq!(docs[0].title, "Question");
    assert_eq!(docs[0].meta["session_name"], "Named");
}

#[test]
fn a_summary_before_the_first_message_opens_the_first_turn() {
    let docs = render(&[
        header(),
        json!({"type": "compaction", "id": "00", "parentId": null, "timestamp": "2026-01-01T00:00:30Z", "summary": "Earlier work"}),
        user("01", Some("00"), "Q"),
        assistant("02", "01", "A"),
    ]);
    assert_eq!(docs.len(), 1);
    assert!(
        docs[0]
            .text
            .starts_with("**2026-01-01 00:00 UTC · Summary**\n\nEarlier work")
    );
}

#[test]
fn each_branch_is_a_thread_and_shared_turns_are_copied_identically() {
    let docs = render(&[
        header(),
        user("01", None, "Shared"),
        assistant("02", "01", "Answer"),
        user("03", Some("02"), "Left"),
        user("04", Some("02"), "Right"),
    ]);
    let mut threads: Vec<_> = docs.iter().map(|d| d.thread.clone().unwrap()).collect();
    threads.dedup();
    // The branch written last is the main one.
    assert_eq!(threads, ["s1", "s1#03"]);

    let shared: Vec<_> = turns(&docs)
        .into_iter()
        .filter(|d| d.meta["entry"] == "01")
        .collect();
    assert_eq!(shared.len(), 2);
    assert_ne!(shared[0].external_id, shared[1].external_id);
    // Equal title and text let ferret's embedding cache serve the copy.
    assert_eq!(shared[0].title, shared[1].title);
    assert_eq!(shared[0].text, shared[1].text);
}

#[test]
fn a_retried_reply_without_new_user_messages_is_no_branch() {
    let docs = render(&[
        header(),
        user("01", None, "Question"),
        assistant("02", "01", "Failed attempt"),
        assistant("03", "01", "Retry"),
        user("04", Some("03"), "Thanks"),
    ]);
    assert!(docs.iter().all(|d| d.thread.as_deref() == Some("s1")));
    assert_eq!(turns(&docs).len(), 2);
}

#[test]
fn a_tool_call_shows_how_much_came_back_but_not_what() {
    let docs = render(&[
        header(),
        user("01", None, "Q"),
        assistant("02", "01", "A"),
        tool_result("03", "02"),
    ]);
    let turn = turns(&docs)[0];
    assert!(
        turn.text
            .contains("- **bash**\n  - -> `ls -la`\n  - <- failed · 1 line, 15 B")
    );
    assert!(!turn.text.contains("raw tool output"));
    assert!(!turn.text.contains("private reasoning"));
}

#[test]
fn the_main_branch_is_where_the_session_went_on_not_the_abandoned_one() {
    let docs = render(&[
        header(),
        user("01", None, "Start"),
        assistant("02", "01", "Answer"),
        user("03", Some("02"), "Abandoned"),
        user("04", Some("02"), "Went on"),
        assistant("05", "04", "More"),
        user("06", Some("05"), "Later"),
    ]);
    let main: Vec<_> = turns(&docs)
        .into_iter()
        .filter(|d| d.thread.as_deref() == Some("s1"))
        .map(|d| d.title.as_str())
        .collect();
    assert_eq!(main, ["Start", "Went on", "Later"]);
}

#[test]
fn the_user_and_the_answering_models_are_the_authors() {
    let docs = render(&[header(), user("01", None, "Q"), assistant("02", "01", "A")]);
    assert_eq!(turns(&docs)[0].authors, ["user", "m1"]);
}

#[test]
fn where_a_turn_comes_from_is_meta_not_text() {
    let docs = render(&[header(), user("01", None, "Q"), assistant("02", "01", "A")]);
    let turn = turns(&docs)[0];
    assert!(!turn.text.contains("/home/me/project"));
    assert!(!turn.text.contains("x.jsonl"));
    assert_eq!(turn.meta["harness"], "pi");
    assert_eq!(turn.meta["file"], "~/x.jsonl");
    assert_eq!(turn.meta["cwd"], "/home/me/project");
}

#[test]
fn a_command_without_an_answer_is_no_turn() {
    let skill = |id: &str, parent: &str, text: &str| {
        let mut entry = user(id, Some(parent), text);
        entry["message"]["content"][0]["text"] = json!(text);
        entry
    };
    let docs = render(&[
        header(),
        user("01", None, "Q"),
        assistant("02", "01", "A"),
        skill(
            "03",
            "02",
            "<skill name=\"capture\" location=\"/s\">body</skill>\n\nnote this",
        ),
        assistant("04", "03", "Noted"),
        skill(
            "05",
            "04",
            "<skill name=\"quiet\" location=\"/s\">body</skill>",
        ),
    ]);
    let titles: Vec<_> = turns(&docs).iter().map(|d| d.title.clone()).collect();
    assert_eq!(titles, ["Q", "/skill:capture note this"]);
}

#[test]
fn a_tool_call_says_whether_it_worked() {
    let ok = json!({"type": "message", "id": "03", "parentId": "02",
                    "message": {"role": "toolResult", "toolCallId": "call-02", "isError": false,
                                "content": [{"type": "text", "text": "a\nb"}]}});
    let docs = render(&[
        header(),
        user("01", None, "Q"),
        assistant("02", "01", "A"),
        ok,
    ]);
    assert!(turns(&docs)[0].text.contains("  - <- ok · 2 lines, 3 B"));

    let docs = render(&[header(), user("01", None, "Q"), assistant("02", "01", "A")]);
    assert!(turns(&docs)[0].text.contains("  - <- no result"));
}
