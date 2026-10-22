//! Reading the JSON lines of a session file that its agent may still be writing.

use std::path::Path;

use anyhow::{Result, bail};
use serde_json::Value;

/// The parsed lines and whether the last one was cut off. A half-written last
/// line is left for the next run; a broken line anywhere else is an error.
pub fn parse(text: &str, path: &Path) -> Result<(Vec<Value>, bool)> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut values = Vec::with_capacity(lines.len());
    let mut torn = false;
    for (number, line) in lines.iter().enumerate() {
        let parsed = serde_json::from_str::<Value>(line).or_else(|error| {
            serde_json::from_str(&replace_lone_surrogates(line)).map_err(|_| error)
        });
        match parsed {
            Ok(value) => values.push(value),
            Err(_) if number + 1 == lines.len() && !text.ends_with('\n') => torn = true,
            Err(error) => bail!("{}:{}: {error}", path.display(), number + 1),
        }
    }
    Ok((values, torn))
}

/// Replaces `\uXXXX` escapes of unpaired UTF-16 surrogates, which JSON
/// writers produce when they cut an emoji in half, with U+FFFD.
pub fn replace_lone_surrogates(line: &str) -> String {
    let bytes = line.as_bytes();
    let escape_at = |i: usize| -> Option<u16> {
        let hex = bytes.get(i..i + 6)?;
        if &hex[..2] != b"\\u" {
            return None;
        }
        u16::from_str_radix(std::str::from_utf8(&hex[2..]).ok()?, 16).ok()
    };
    let high = |u: u16| (0xD800..0xDC00).contains(&u);
    let low = |u: u16| (0xDC00..0xE000).contains(&u);

    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut backslashes = 0;
    while i < bytes.len() {
        // Only an unescaped backslash starts an escape.
        if bytes[i] == b'\\'
            && backslashes % 2 == 0
            && let Some(unit) = escape_at(i)
        {
            if high(unit) && escape_at(i + 6).is_some_and(low) {
                out.push_str(&line[i..i + 12]);
                i += 12;
                backslashes = 0;
                continue;
            }
            if high(unit) || low(unit) {
                out.push_str("\\ufffd");
                i += 6;
                backslashes = 0;
                continue;
            }
        }
        backslashes = if bytes[i] == b'\\' {
            backslashes + 1
        } else {
            0
        };
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lone_surrogate_becomes_a_replacement_character() {
        let fixed = replace_lone_surrogates(r#"{"t":"a\ud83d b"}"#);
        let value: Value = serde_json::from_str(&fixed).unwrap();
        assert_eq!(value["t"], "a\u{fffd} b");
    }

    #[test]
    fn surrogate_pairs_and_escaped_backslashes_stay() {
        let pair = r#"{"t":"😀"}"#;
        assert_eq!(replace_lone_surrogates(pair), pair);
        let literal = r#"{"t":"\\ud83d"}"#;
        assert_eq!(replace_lone_surrogates(literal), literal);
    }

    #[test]
    fn a_torn_last_line_is_skipped_and_reported() {
        let text = "{\"a\":1}\n{\"b\":";
        let (values, torn) = parse(text, Path::new("x.jsonl")).unwrap();
        assert!(torn);
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn a_broken_line_before_the_end_fails() {
        assert!(parse("{}\nnot json\n{}\n", Path::new("x.jsonl")).is_err());
    }
}
