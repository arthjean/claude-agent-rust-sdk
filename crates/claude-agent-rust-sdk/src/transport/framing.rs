//! Framing of the newline-delimited JSON the CLI writes to its output, and
//! the one JSON line reader that the session readers share with it, so a
//! transcript line the live session read is read again the same way.

//!
//! A line is decoded with each invalid UTF-8 sequence replaced by U+FFFD,
//! where upstream decodes strictly and a `UnicodeDecodeError` ends the
//! session.
//!
//! Two lines that `serde_json` refuses by default are still read, because
//! the CLI can write them and upstream `json.loads` reads them:
//!
//! - an escape of a lone UTF-16 surrogate (`"\ud83d"`), which JavaScript's
//!   `JSON.stringify` writes for a string cut in the middle of a surrogate
//!   pair. A Rust string cannot hold a lone surrogate, so each one becomes
//!   U+FFFD, where Python keeps the surrogate itself;
//! - nesting deeper than the 128 levels `serde_json` allows. Up to
//!   [`MAX_NESTING_DEPTH`] levels the line is read whole. Deeper, each
//!   array or object that opens at that depth is kept as the JSON text it
//!   was written as, a string, so the value never nests
//!   deeper than the parse, clone, and drop of a 2 MiB tokio worker stack
//!   allow in a debug build. The rest of the line (its type, its uuid, the
//!   parent it names) reads as written, so neither the session nor the
//!   history of a transcript loses the message. Python reads the whole
//!   line until its interpreter runs out of recursion.

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
    let text = crate::sessions::python_strip(&text);
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
/// and any nesting are accepted (the arrays and objects deeper than
/// [`MAX_NESTING_DEPTH`] kept as their JSON text), and any other refusal
/// keeps the error of `serde_json`.
pub(crate) fn parse_json(text: &str) -> serde_json::Result<Value> {
    serde_json::from_str::<Value>(text).or_else(|source| recover(text).ok_or(source))
}

/// Reads a line `serde_json` refused with its defaults, when the cause is
/// a lone surrogate escape or a nesting deeper than 128 levels.
fn recover(text: &str) -> Option<Value> {
    let replaced = replace_lone_surrogates(text);
    let text = replaced.as_deref().unwrap_or(text);
    if nesting_depth(text) > MAX_NESTING_DEPTH {
        return bounded(text);
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    let value = Value::deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    Some(value)
}

/// Reads `text`, whose nesting exceeds [`MAX_NESTING_DEPTH`], in one
/// pass: each array or object that opens at that depth is replaced by the
/// JSON string of its text, which leaves a value no deeper than the limit
/// for a single parse. A text whose brackets do not pair or whose strings
/// do not close is refused before that parse, which would otherwise recurse through every
/// level left open.
fn bounded(text: &str) -> Option<Value> {
    let mut flattened = String::with_capacity(text.len());
    let (mut open, mut copied, mut opened_at) = (Vec::new(), 0_usize, None);
    let (mut in_string, mut escaped) = (false, false);
    for (index, byte) in text.bytes().enumerate() {
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
                open.push(if byte == b'[' { b']' } else { b'}' });
                if open.len() == MAX_NESTING_DEPTH {
                    opened_at = Some(index);
                }
            }
            b']' | b'}' => {
                if open.len() == MAX_NESTING_DEPTH
                    && let Some(start) = opened_at.take()
                {
                    flattened.push_str(&text[copied..start]);
                    flattened.push_str(&serde_json::to_string(&text[start..=index]).ok()?);
                    copied = index + 1;
                }
                if open.pop() != Some(byte) {
                    return None;
                }
            }
            _ => {}
        }
    }
    if !open.is_empty() || in_string || opened_at.is_some() {
        return None;
    }
    flattened.push_str(&text[copied..]);
    let mut deserializer = serde_json::Deserializer::from_str(&flattened);
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
    use crate::error::ErrorKind;

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
    fn nesting_up_to_the_limit_is_read_whole_and_deeper_keeps_the_rest_of_the_line() {
        let nested = |depth: usize| {
            format!(
                "{{\"type\": \"user\", \"a\": {}{}, \"uuid\": \"u2\"}}",
                "[".repeat(depth - 1),
                "]".repeat(depth - 1)
            )
        };
        let value = parse_stdout_line(nested(MAX_NESTING_DEPTH).as_bytes())
            .unwrap()
            .unwrap();
        assert!(value["a"].is_array());

        let deeper = nested(MAX_NESTING_DEPTH + 88);
        let value = parse_stdout_line(deeper.as_bytes()).unwrap().unwrap();
        assert_eq!(value["type"], "user");
        assert_eq!(value["uuid"], "u2");
        let mut level = &value["a"];
        for _ in 2..MAX_NESTING_DEPTH {
            level = &level[0];
        }
        let kept = level
            .as_str()
            .expect("the container at the limit is kept as its text");
        let inner = 88 + 1;
        assert_eq!(kept, format!("{}{}", "[".repeat(inner), "]".repeat(inner)));

        let in_string = format!("{{\"a\": \"{}\"}}", "[".repeat(MAX_NESTING_DEPTH * 2));
        assert!(parse_stdout_line(in_string.as_bytes()).unwrap().is_some());
    }

    #[test]
    fn a_deep_line_whose_brackets_do_not_pair_or_close_is_refused_on_a_small_stack() {
        let unclosed = format!("{{\"a\": {}", "[".repeat(200_000));
        let unclosed_string = format!("{{\"a\": {}\"x", "[".repeat(600));
        let overclosed = format!("{{\"a\": {}{}}}", "[".repeat(600), "]".repeat(601));
        let mismatched = format!("{{\"a\": {}}}{}}}", "[".repeat(600), "]".repeat(599));
        let refused = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                [unclosed, unclosed_string, overclosed, mismatched]
                    .iter()
                    .map(|line| {
                        parse_stdout_line(line.as_bytes())
                            .err()
                            .map(|error| error.kind())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(refused, [Some(ErrorKind::JsonDecode); 4]);
    }

    #[test]
    fn a_line_nested_fifty_thousand_levels_is_read_on_a_small_stack_and_dropped() {
        let line = format!(
            "{{\"type\": \"user\", \"a\": {}{}}}",
            "{\"b\": [".repeat(25_000),
            "]}".repeat(25_000)
        );
        let read = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let value = parse_stdout_line(line.as_bytes()).unwrap().unwrap();
                let copy = value.clone();
                drop(value);
                copy["type"].as_str().map(str::to_owned)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(read.as_deref(), Some("user"));
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
    fn a_line_led_by_a_record_separator_is_read_as_python_strip_reads_it() {
        let parsed = parse_stdout_line(b"\x1e{\"type\":\"x\"}\n").unwrap();
        assert_eq!(parsed, Some(serde_json::json!({"type": "x"})));
    }

    #[test]
    fn carriage_return_line_endings_are_tolerated() {
        let value = parse_stdout_line(b"{\"type\": \"system\"}\r")
            .unwrap()
            .unwrap();
        assert_eq!(value["type"], "system");
    }
}
