//! Session metadata read from the first and last 64 KiB of a transcript,
//! without parsing every line.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::Value;

use super::SdkSessionInfo;
use super::timestamp::parse_iso_millis;

/// Size of the head and of the tail read from a transcript.
pub(crate) const LITE_READ_BUF_SIZE: usize = 65_536;

const FIRST_PROMPT_MAX_CHARS: usize = 200;

/// The head, tail, modification time, and size of a transcript.
pub(crate) struct LiteSessionFile {
    pub(crate) mtime: i64,
    pub(crate) size: u64,
    pub(crate) head: String,
    pub(crate) tail: String,
}

impl LiteSessionFile {
    /// The same slices for a transcript held in memory.
    pub(crate) fn from_bytes(bytes: &[u8], mtime: i64) -> Self {
        let head =
            String::from_utf8_lossy(&bytes[..bytes.len().min(LITE_READ_BUF_SIZE)]).into_owned();
        let tail = if bytes.len() > LITE_READ_BUF_SIZE {
            String::from_utf8_lossy(&bytes[bytes.len() - LITE_READ_BUF_SIZE..]).into_owned()
        } else {
            head.clone()
        };
        Self {
            mtime,
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            head,
            tail,
        }
    }
}

/// Reads the head and tail of a transcript. `None` on any error or for an
/// empty file.
pub(crate) fn read_session_lite(path: &Path) -> Option<LiteSessionFile> {
    let mut file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let size = metadata.len();
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    let head_bytes = read_up_to(&mut file, LITE_READ_BUF_SIZE)?;
    if head_bytes.is_empty() {
        return None;
    }
    let head = String::from_utf8_lossy(&head_bytes).into_owned();
    let buffer = u64::try_from(LITE_READ_BUF_SIZE).unwrap_or(u64::MAX);
    let tail = if size <= buffer {
        head.clone()
    } else {
        file.seek(SeekFrom::Start(size - buffer)).ok()?;
        String::from_utf8_lossy(&read_up_to(&mut file, LITE_READ_BUF_SIZE)?).into_owned()
    };
    Some(LiteSessionFile {
        mtime,
        size,
        head,
        tail,
    })
}

fn read_up_to(file: &mut File, limit: usize) -> Option<Vec<u8>> {
    let mut buffer = Vec::with_capacity(limit);
    file.take(u64::try_from(limit).ok()?)
        .read_to_end(&mut buffer)
        .ok()?;
    Some(buffer)
}

fn unescape_json_string(raw: &str) -> String {
    if !raw.contains('\\') {
        return raw.to_owned();
    }
    match crate::transport::framing::parse_json(&format!("\"{raw}\"")) {
        Ok(Value::String(text)) => text,
        _ => raw.to_owned(),
    }
}

/// Finds the end of the string value starting at `start`: the index of the
/// closing quote, skipping escaped characters.
fn string_value_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index),
            _ => index += 1,
        }
    }
    None
}

fn field_patterns(key: &str) -> [String; 2] {
    [format!("\"{key}\":\""), format!("\"{key}\": \"")]
}

/// The first `"key":"value"` string value of the text, found without
/// parsing, so it works on a truncated line.
pub(crate) fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    for pattern in field_patterns(key) {
        let Some(index) = text.find(&pattern) else {
            continue;
        };
        let start = index + pattern.len();
        if let Some(end) = string_value_end(text.as_bytes(), start) {
            return Some(unescape_json_string(&text[start..end]));
        }
    }
    None
}

/// Like [`extract_json_string_field`], for the last occurrence.
pub(crate) fn extract_last_json_string_field(text: &str, key: &str) -> Option<String> {
    let mut last = None;
    for pattern in field_patterns(key) {
        let mut from = 0;
        while let Some(found) = text[from..].find(&pattern) {
            let start = from + found + pattern.len();
            match string_value_end(text.as_bytes(), start) {
                Some(end) => {
                    last = Some(unescape_json_string(&text[start..end]));
                    from = end + 1;
                }
                None => break,
            }
        }
    }
    last
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn command_name(text: &str) -> Option<&str> {
    const OPEN: &str = "<command-name>";
    const CLOSE: &str = "</command-name>";
    let start = text.find(OPEN)? + OPEN.len();
    let end = text[start..].find(CLOSE)?;
    Some(&text[start..start + end])
}

/// Prompts written by the CLI or the IDE rather than typed by the user.
fn is_generated_prompt(text: &str) -> bool {
    const PREFIXES: [&str; 4] = [
        "<local-command-stdout>",
        "<session-start-hook>",
        "<tick>",
        "<goal>",
    ];
    if PREFIXES.iter().any(|prefix| text.starts_with(prefix)) {
        return true;
    }
    if let Some(rest) = text.strip_prefix("[Request interrupted by user") {
        return rest.contains(']');
    }
    let wraps = |open: &str, close: &str| {
        text.len() >= open.len() + close.len() && text.starts_with(open) && text.ends_with(close)
    };
    wraps("<ide_opened_file>", "</ide_opened_file>") || wraps("<ide_selection>", "</ide_selection>")
}

fn line_has(line: &str, compact: &str, spaced: &str) -> bool {
    line.contains(compact) || line.contains(spaced)
}

/// The first meaningful user prompt of a transcript head, truncated to 200
/// characters. Tool results, meta and compact-summary entries, slash
/// commands, and generated prompts are skipped; the first slash command
/// name is the fallback.
pub(crate) fn extract_first_prompt_from_head(head: &str) -> String {
    let mut command_fallback = String::new();
    for line in head.split('\n') {
        if !line_has(line, "\"type\":\"user\"", "\"type\": \"user\"")
            || line.contains("\"tool_result\"")
            || line_has(line, "\"isMeta\":true", "\"isMeta\": true")
            || line_has(
                line,
                "\"isCompactSummary\":true",
                "\"isCompactSummary\": true",
            )
        {
            continue;
        }
        let Ok(Value::Object(entry)) = crate::transport::framing::parse_json(line) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) != Some("user") {
            continue;
        }
        if !matches!(entry.get("message"), Some(Value::Object(_))) {
            continue;
        }
        for raw in entry_texts(&entry) {
            match classify_prompt(raw) {
                PromptText::Prompt(prompt) => return prompt,
                PromptText::Command(name) => {
                    if command_fallback.is_empty() {
                        command_fallback = name;
                    }
                }
                PromptText::Skipped => {}
            }
        }
    }
    command_fallback
}

/// What one text block of a user message is worth as a first prompt.
pub(crate) enum PromptText {
    /// Blank or generated text.
    Skipped,
    /// A slash command, by name: the fallback when no prompt follows.
    Command(String),
    /// A prompt, flattened to one line and truncated to 200 characters.
    Prompt(String),
}

/// Strips what Python's `str.strip()` strips: Unicode white space, and the
/// separators U+001C to U+001F, which Python counts as space and Rust
/// does not.
pub(crate) fn python_strip(text: &str) -> &str {
    text.trim_matches(|character: char| {
        character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
    })
}

pub(crate) fn classify_prompt(raw: &str) -> PromptText {
    let flattened = raw.replace('\n', " ");
    let text = python_strip(&flattened);
    if text.is_empty() {
        return PromptText::Skipped;
    }
    if let Some(name) = command_name(text) {
        return PromptText::Command(name.to_owned());
    }
    if is_generated_prompt(text) {
        return PromptText::Skipped;
    }
    if text.chars().count() > FIRST_PROMPT_MAX_CHARS {
        let cut: String = text.chars().take(FIRST_PROMPT_MAX_CHARS).collect();
        return PromptText::Prompt(format!("{}\u{2026}", cut.trim_end()));
    }
    PromptText::Prompt(text.to_owned())
}

/// The text blocks of the message of a user entry.
pub(crate) fn entry_texts(entry: &serde_json::Map<String, Value>) -> Vec<&str> {
    let Some(Value::Object(message)) = entry.get("message") else {
        return Vec::new();
    };
    match message.get("content") {
        Some(Value::String(text)) => vec![text.as_str()],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect(),
        _ => Vec::new(),
    }
}

/// Builds the metadata of a session from its head and tail.
///
/// `None` for a sidechain transcript and for a transcript with no title,
/// summary, or prompt.
pub(crate) fn parse_session_info_from_lite(
    session_id: &str,
    lite: &LiteSessionFile,
    project_path: Option<&str>,
) -> Option<SdkSessionInfo> {
    let LiteSessionFile {
        head,
        tail,
        mtime,
        size,
    } = lite;
    let first_line = head.split('\n').next().unwrap_or_default();
    if line_has(first_line, "\"isSidechain\":true", "\"isSidechain\": true") {
        return None;
    }
    let custom_title = non_empty(extract_last_json_string_field(tail, "customTitle"))
        .or_else(|| non_empty(extract_last_json_string_field(head, "customTitle")))
        .or_else(|| non_empty(extract_last_json_string_field(tail, "aiTitle")))
        .or_else(|| non_empty(extract_last_json_string_field(head, "aiTitle")));
    let first_prompt = non_empty(Some(extract_first_prompt_from_head(head)));
    let summary = custom_title
        .clone()
        .or_else(|| non_empty(extract_last_json_string_field(tail, "lastPrompt")))
        .or_else(|| non_empty(extract_last_json_string_field(tail, "summary")))
        .or_else(|| first_prompt.clone())?;
    let git_branch = non_empty(extract_last_json_string_field(tail, "gitBranch"))
        .or_else(|| non_empty(extract_json_string_field(head, "gitBranch")));
    let cwd = non_empty(extract_json_string_field(head, "cwd")).or_else(|| {
        project_path
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
    });
    let tag = tail
        .split('\n')
        .rev()
        .find(|line| line.starts_with("{\"type\":\"tag\""))
        .and_then(|line| non_empty(extract_last_json_string_field(line, "tag")));
    let created_at =
        extract_json_string_field(head, "timestamp").and_then(|stamp| parse_iso_millis(&stamp));
    Some(SdkSessionInfo {
        session_id: session_id.to_owned(),
        summary,
        last_modified: *mtime,
        file_size: Some(*size),
        custom_title,
        first_prompt,
        git_branch,
        cwd,
        tag,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_compact_spaced_and_escaped_fields() {
        assert_eq!(
            extract_json_string_field(r#"{"a":"b","c":"d"}"#, "c").as_deref(),
            Some("d")
        );
        assert_eq!(
            extract_json_string_field(r#"{"a": "b"}"#, "a").as_deref(),
            Some("b")
        );
        assert_eq!(
            extract_json_string_field(r#"{"a":"say \"hi\"\n"}"#, "a").as_deref(),
            Some("say \"hi\"\n")
        );
        assert_eq!(
            extract_json_string_field(r#"{"a":"unterminated"#, "a"),
            None
        );
        assert_eq!(
            extract_last_json_string_field("{\"a\":\"one\"}\n{\"a\":\"two\"}", "a").as_deref(),
            Some("two")
        );
    }

    fn user_line(content: &Value) -> String {
        serde_json::json!({"type": "user", "message": {"content": content}}).to_string()
    }

    #[test]
    fn first_prompt_skips_meta_tool_results_and_generated_prompts() {
        let head = [
            r#"{"type":"user","isMeta":true,"message":{"content":"meta"}}"#.to_owned(),
            user_line(&serde_json::json!([{"type": "tool_result", "content": "x"}])),
            user_line(&Value::from("<local-command-stdout>out")),
            user_line(&Value::from("[Request interrupted by user for tool use]")),
            user_line(&Value::from("<ide_opened_file>a.rs</ide_opened_file>")),
            user_line(&serde_json::json!([{"type": "image"}, {"type": "text", "text": "  real\nprompt "}])),
        ]
        .join("\n");
        assert_eq!(extract_first_prompt_from_head(&head), "real prompt");
    }

    #[test]
    fn first_prompt_truncates_and_falls_back_to_the_command_name() {
        let long = "x".repeat(250);
        assert_eq!(
            extract_first_prompt_from_head(&user_line(&Value::from(long))),
            format!("{}\u{2026}", "x".repeat(200))
        );
        let command = user_line(&Value::from("<command-name>/init</command-name>"));
        assert_eq!(extract_first_prompt_from_head(&command), "/init");
        assert_eq!(extract_first_prompt_from_head(""), "");
        assert_eq!(
            extract_first_prompt_from_head("{\"type\":\"assistant\"}\n"),
            ""
        );
    }
}
