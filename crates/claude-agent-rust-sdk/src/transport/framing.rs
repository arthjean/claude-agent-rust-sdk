//! Framing of the newline-delimited JSON the CLI writes to its output, and
//! the one JSON line reader that the session readers share with it, so a
//! transcript line the live session read is read again the same way.

//!
//! Two lines that `serde_json` refuses by default are still read, because
//! the CLI can write them and upstream `json.loads` reads them:
//!
//! - an escape of a lone UTF-16 surrogate (`"\ud83d"`), which JavaScript's
//!   `JSON.stringify` writes for a string cut in the middle of a surrogate
//!   pair. A Rust string cannot hold a lone surrogate, so each one becomes
//!   U+FFFD, where Python keeps the surrogate itself;
//! - nesting deeper than the 128 levels `serde_json` allows, up to
//!   [`MAX_NESTING_DEPTH`]. Python reads deeper, until its interpreter
//!   runs out of recursion; Rust stops at a depth whose parse, clone, and
//!   drop fit in the 2 MiB stack of a tokio worker thread in a debug build.

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

const LINE_FEED: u8 = b'\n';
const SKIPPED_LINE_PREVIEW_CHARS: usize = 200;

/// Deepest nesting of arrays and objects a line may have.
pub(crate) const MAX_NESTING_DEPTH: usize = 512;
const REPLACEMENT_ESCAPE: &str = "\\uFFFD";

#[derive(Debug, Default)]
pub(crate) struct LineFramer {
    pending: Vec<u8>,
}

impl LineFramer {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        let Some(last_break) = chunk.iter().rposition(|byte| *byte == LINE_FEED) else {
            self.pending.extend_from_slice(chunk);
            return Vec::new();
        };
        let (complete, tail) = chunk.split_at(last_break);
        let mut buffered = std::mem::take(&mut self.pending);
        buffered.extend_from_slice(complete);
        self.pending.extend_from_slice(&tail[1..]);
        buffered
            .split(|byte| *byte == LINE_FEED)
            .map(<[u8]>::to_vec)
            .collect()
    }

    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

pub(crate) fn parse_stdout_line(line: &[u8]) -> Result<Option<Value>> {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if !text.starts_with('{') {
        let preview: String = text.chars().take(SKIPPED_LINE_PREVIEW_CHARS).collect();
        tracing::debug!(line = %preview, "skipping non-JSON line from CLI stdout");
        return Ok(None);
    }
    parse_json(text)
        .map(Some)
        .map_err(|source| Error::json_decode(text, source))
}

/// Reads one JSON text as the CLI output is read: a lone surrogate escape
/// and a nesting up to [`MAX_NESTING_DEPTH`] levels are accepted, and any
/// other refusal keeps the error of `serde_json`.
pub(crate) fn parse_json(text: &str) -> serde_json::Result<Value> {
    serde_json::from_str::<Value>(text).or_else(|source| recover(text).ok_or(source))
}

/// Reads a line `serde_json` refused with its defaults, when the cause is
/// a lone surrogate escape or a nesting between 128 and
/// [`MAX_NESTING_DEPTH`] levels.
fn recover(text: &str) -> Option<Value> {
    let replaced = replace_lone_surrogates(text);
    let text = replaced.as_deref().unwrap_or(text);
    if nesting_depth(text) > MAX_NESTING_DEPTH {
        return None;
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    let value = Value::deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    Some(value)
}

/// The deepest nesting of arrays and objects, brackets inside strings left
/// out.
fn nesting_depth(text: &str) -> usize {
    let (mut depth, mut deepest) = (0_usize, 0_usize);
    let (mut in_string, mut escaped) = (false, false);
    for byte in text.bytes() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// Replaces each `\uXXXX` escape of a lone surrogate with `\uFFFD`, and
/// keeps escaped pairs and every other escape. Returns `None` when the text
/// holds no lone surrogate.
fn replace_lone_surrogates(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let unit_at = |index: usize| -> Option<u16> {
        let digits = bytes.get(index..index + 6)?;
        if digits[0] != b'\\' || digits[1] != b'u' {
            return None;
        }
        u16::from_str_radix(std::str::from_utf8(&digits[2..]).ok()?, 16).ok()
    };
    let is_high = |unit: u16| (0xD800..=0xDBFF).contains(&unit);
    let is_low = |unit: u16| (0xDC00..=0xDFFF).contains(&unit);
    let mut output = String::with_capacity(text.len());
    let (mut copied, mut index, mut replaced) = (0, 0, false);
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        match unit_at(index) {
            Some(unit) if is_high(unit) && unit_at(index + 6).is_some_and(is_low) => index += 12,
            Some(unit) if is_high(unit) || is_low(unit) => {
                output.push_str(&text[copied..index]);
                output.push_str(REPLACEMENT_ESCAPE);
                index += 6;
                copied = index;
                replaced = true;
            }
            Some(_) => index += 6,
            None => index += 2,
        }
    }
    replaced.then(|| {
        output.push_str(&text[copied..]);
        output
    })
}

pub(crate) fn oversize_error(length: usize, limit: usize) -> Error {
    let source = <serde_json::Error as serde::de::Error>::custom(format!(
        "Buffer size {length} exceeds limit {limit}"
    ));
    Error::json_decode(
        format!("JSON message exceeded maximum buffer size of {limit} bytes"),
        source,
    )
}

#[cfg(test)]
mod tests {
    use super::{LineFramer, MAX_NESTING_DEPTH, parse_stdout_line, replace_lone_surrogates};
    use crate::error::{Error, ErrorKind};

    fn lines(framer: &mut LineFramer, chunk: &str) -> Vec<String> {
        framer
            .push(chunk.as_bytes())
            .into_iter()
            .map(|line| String::from_utf8(line).unwrap())
            .collect()
    }

    #[test]
    fn splits_on_line_feed_only_and_keeps_the_tail() {
        let mut framer = LineFramer::default();
        assert!(lines(&mut framer, "{\"a\":").is_empty());
        assert_eq!(framer.pending_len(), 5);
        assert_eq!(
            lines(&mut framer, " 1}\n{\"b\": 2}\r\n\n{\"c\""),
            ["{\"a\": 1}", "{\"b\": 2}\r", ""]
        );
        assert_eq!(framer.pending_len(), 4);
        assert_eq!(framer.flush(), b"{\"c\"");
        assert_eq!(framer.pending_len(), 0);
    }

    #[test]
    fn whitespace_at_a_chunk_boundary_is_preserved() {
        let mut framer = LineFramer::default();
        assert!(lines(&mut framer, "{\"text\": \"hello ").is_empty());
        assert_eq!(
            lines(&mut framer, " world\"}\n"),
            ["{\"text\": \"hello  world\"}"]
        );
    }

    #[test]
    fn a_multibyte_character_split_across_chunks_is_reassembled() {
        let text = "{\"text\": \"\u{65e5}\u{672c}\"}\n".as_bytes();
        let mut framer = LineFramer::default();
        assert!(framer.push(&text[..12]).is_empty());
        let complete = framer.push(&text[12..]);
        let value = parse_stdout_line(&complete[0]).unwrap().unwrap();
        assert_eq!(value["text"], "\u{65e5}\u{672c}");
    }

    #[test]
    fn a_lone_surrogate_escape_becomes_a_replacement_character() {
        let value = parse_stdout_line(br#"{"t": "cut \ud83d", "u": "\udc00!"}"#)
            .unwrap()
            .unwrap();
        assert_eq!(value["t"], "cut \u{fffd}");
        assert_eq!(value["u"], "\u{fffd}!");
        let pair = parse_stdout_line(br#"{"t": "\ud83d\ude00 \ud83d"}"#)
            .unwrap()
            .unwrap();
        assert_eq!(pair["t"], "\u{1f600} \u{fffd}");
    }

    #[test]
    fn an_escaped_backslash_before_u_is_not_a_surrogate_escape() {
        assert_eq!(replace_lone_surrogates(r#"{"t": "\\ud83d"}"#), None);
        assert_eq!(
            replace_lone_surrogates(r#"{"t": "\\\ud83d"}"#).as_deref(),
            Some(r#"{"t": "\\\uFFFD"}"#)
        );
        assert_eq!(replace_lone_surrogates(r#"{"t": "\u00e9 \n"}"#), None);
    }

    #[test]
    fn nesting_up_to_the_limit_is_read_and_deeper_fails() {
        let nested = |depth: usize| {
            format!(
                "{{\"a\": {}{}}}",
                "[".repeat(depth - 1),
                "]".repeat(depth - 1)
            )
        };
        let value = parse_stdout_line(nested(MAX_NESTING_DEPTH).as_bytes())
            .unwrap()
            .unwrap();
        assert!(value["a"].is_array());
        assert_eq!(
            parse_stdout_line(nested(MAX_NESTING_DEPTH + 1).as_bytes())
                .as_ref()
                .err()
                .map(Error::kind),
            Some(ErrorKind::JsonDecode)
        );
        let in_string = format!("{{\"a\": \"{}\"}}", "[".repeat(MAX_NESTING_DEPTH * 2));
        assert!(parse_stdout_line(in_string.as_bytes()).unwrap().is_some());
    }

    #[test]
    fn a_line_that_is_still_invalid_keeps_the_original_error() {
        let error = parse_stdout_line(br#"{"t": "\ud83d", }"#).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::JsonDecode);
        assert!(error.line().is_some_and(|line| line.contains("\\ud83d")));
    }

    #[test]
    fn blank_and_non_json_lines_carry_no_message() {
        assert!(parse_stdout_line(b"").unwrap().is_none());
        assert!(parse_stdout_line(b"  \r").unwrap().is_none());
        assert!(
            parse_stdout_line(b"[SandboxDebug] seccomp filter")
                .unwrap()
                .is_none()
        );
        assert!(parse_stdout_line(b"Loading...").unwrap().is_none());
    }

    #[test]
    fn a_line_that_looks_like_json_but_does_not_parse_is_an_error() {
        let error = parse_stdout_line(b"{\"type\": \"assistant\", ").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::JsonDecode);
        assert!(
            error
                .to_string()
                .starts_with("Failed to decode JSON: {\"type\"")
        );
    }

    #[test]
    fn carriage_return_line_endings_are_tolerated() {
        let value = parse_stdout_line(b"{\"type\": \"system\"}\r")
            .unwrap()
            .unwrap();
        assert_eq!(value["type"], "system");
    }
}
