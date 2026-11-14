//! Framing of the newline-delimited JSON the CLI writes to its output.

use serde_json::Value;

use crate::error::{Error, Result};

const LINE_FEED: u8 = b'\n';
const SKIPPED_LINE_PREVIEW_CHARS: usize = 200;

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
    serde_json::from_str::<Value>(text)
        .map(Some)
        .map_err(|source| Error::json_decode(text, source))
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
    use super::{LineFramer, parse_stdout_line};
    use crate::error::Error;

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
        assert!(matches!(error, Error::JsonDecode { .. }));
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
