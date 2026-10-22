//! The Markdown of every document comes from two minijinja templates, the
//! built-in ones unless the config names others.

use std::path::Path;

use anyhow::{Context, Result};
use minijinja::syntax::SyntaxConfig;
use minijinja::value::Serde;
use minijinja::{Environment, UndefinedBehavior};
use serde::Serialize;

use crate::config;

const TURN: &str = include_str!("../templates/turn.md.j2");

pub struct Templates {
    env: Environment<'static>,
    /// The template source, to tell when documents need rendering anew.
    pub sources: String,
}

impl Templates {
    /// The built-in template, or the file given instead.
    pub fn load(turn: Option<&Path>) -> Result<Templates> {
        let read = |path: Option<&Path>, builtin: &str| -> Result<String> {
            match path {
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("reading the template {}", path.display())),
                None => Ok(builtin.to_owned()),
            }
        };

        let mut env = Environment::new();
        env.set_syntax(
            SyntaxConfig::builder()
                .trim_blocks(true)
                .lstrip_blocks(true)
                .build()?,
        );
        // Printing a misspelled variable fails the render instead of vanishing;
        // testing a missing field, as in `args.command or args.path`, works.
        env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
        env.add_filter("quote", quote);
        env.add_filter("fence", fence);
        env.add_filter("one_line", |text: String, limit: usize| {
            crate::render::one_line(&text, limit)
        });
        env.add_filter("home", |path: String| config::display(Path::new(&path)));
        env.add_filter("time", time);
        env.add_filter("bytes", bytes);
        env.add_filter("code", code);
        let turn = read(turn, TURN)?;
        let sources = turn.clone();
        env.add_template_owned("turn", turn)
            .context("the turn template")?;
        Ok(Templates { env, sources })
    }

    pub fn turn(&self, context: &impl Serialize) -> Result<String> {
        self.render("turn", context)
    }

    fn render(&self, name: &str, context: &impl Serialize) -> Result<String> {
        let text = self
            .env
            .get_template(name)?
            .render(Serde(context))
            .with_context(|| format!("rendering the {name} template"))?;
        Ok(tidy(&text))
    }
}

/// Every line prefixed for a Markdown blockquote, after an optional header line.
fn quote(text: String, header: Option<String>) -> String {
    header
        .iter()
        .map(String::as_str)
        .chain(text.lines())
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An RFC 3339 timestamp as `2026-10-22 05:43 UTC`, or as given when it
/// isn't one in UTC. Always UTC, so a document reads the same on every machine.
fn time(at: String) -> String {
    let b = at.as_bytes();
    let utc = at.ends_with('Z') || at.ends_with("+00:00");
    if utc && b.len() >= 16 && b[10] == b'T' && b[13] == b':' {
        format!("{} {} UTC", &at[..10], &at[11..16])
    } else {
        at
    }
}

/// A size for people: `512 B`, `3.4 KB`, `1.2 MB`.
fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} KB", n as f64 / 1e3),
        _ => format!("{:.1} MB", n as f64 / 1e6),
    }
}

/// Inline code that holds any text: delimited by more backticks than any run
/// inside, padded where the text starts or ends with one.
fn code(text: String) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let ticks = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{ticks}{pad}{text}{pad}{ticks}")
}

/// A fenced code block whose fence is longer than any backtick run inside.
fn fence(text: String) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}\n{}\n{fence}", text.trim_end())
}

/// Templates can be loose with blank lines: runs of them collapse to one, and
/// the text is trimmed. Code blocks are left alone.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    let mut fence: Option<String> = None;
    for line in text.trim().lines() {
        let marker: String = line
            .trim_start()
            .chars()
            .take_while(|&c| c == '`')
            .collect();
        if marker.len() >= 3 {
            match &fence {
                None => fence = Some(marker),
                Some(open) if marker.len() >= open.len() && line.trim() == marker => fence = None,
                Some(_) => {}
            }
        }
        if fence.is_none() && line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_prefixes_every_line_and_keeps_blank_ones_inside() {
        assert_eq!(
            quote("a\n\nb".to_owned(), Some("[!QUOTE] User".to_owned())),
            "> [!QUOTE] User\n> a\n>\n> b"
        );
    }

    #[test]
    fn inline_code_outlasts_the_backticks_inside() {
        assert_eq!(code("ls".to_owned()), "`ls`");
        assert_eq!(code("a `b` c".to_owned()), "``a `b` c``");
        assert_eq!(code("`x`".to_owned()), "`` `x` ``");
    }

    #[test]
    fn a_fence_outlasts_the_backticks_inside() {
        assert_eq!(fence("a".to_owned()), "```\na\n```");
        assert_eq!(fence("x ```` y".to_owned()), "`````\nx ```` y\n`````");
    }

    #[test]
    fn tidy_collapses_blank_runs_but_not_inside_code() {
        assert_eq!(tidy("\n\na\n\n\n\nb\n\n"), "a\n\nb");
        assert_eq!(tidy("```\nx\n\n\ny\n```"), "```\nx\n\n\ny\n```");
    }

    #[test]
    fn a_misspelled_variable_fails_the_render() {
        let dir = std::env::temp_dir().join(format!("fia-template-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("turn.md.j2");
        std::fs::write(&path, "{{ partz }}").unwrap();
        let templates = Templates::load(Some(&path)).unwrap();
        assert!(templates.turn(&serde_json::json!({"parts": []})).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
