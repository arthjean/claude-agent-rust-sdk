//! Readers for the parts of the upstream Python sources the parity tools
//! compare: exported names, tests, CLI flags, option fields, and control
//! subtypes.
//!
//! They work on a line lexer that drops comments and string contents, so a
//! `def test_` inside a string literal is never taken for a test.

use std::collections::BTreeSet;

/// One source line with comments removed and string literals replaced by
/// `""`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Number of leading spaces.
    pub indent: usize,
    /// Whether the line begins inside a string opened on an earlier line.
    pub continuation: bool,
    /// Code of the line, with each string literal replaced by `""`.
    pub code: String,
    /// Contents of the string literals that open and close on the line.
    pub strings: Vec<String>,
}

/// Splits Python source into lexed lines. Lines lying entirely inside a
/// multi-line string are dropped.
pub fn lex(source: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut open: Option<Vec<char>> = None;
    for raw in source.lines() {
        let characters: Vec<char> = raw.chars().collect();
        let continuation = open.is_some();
        let mut code = String::new();
        let mut strings = Vec::new();
        let mut index = 0;
        if let Some(delimiter) = open.take() {
            match find(&characters, 0, &delimiter) {
                Some(end) => index = end + delimiter.len(),
                None => {
                    open = Some(delimiter);
                    continue;
                }
            }
        }
        while index < characters.len() {
            let character = characters[index];
            if character == '#' {
                break;
            }
            if character != '"' && character != '\'' {
                code.push(character);
                index += 1;
                continue;
            }
            code.push_str("\"\"");
            let triple = characters.get(index + 1) == Some(&character)
                && characters.get(index + 2) == Some(&character);
            let delimiter = vec![character; if triple { 3 } else { 1 }];
            let start = index + delimiter.len();
            match find(&characters, start, &delimiter) {
                Some(end) => {
                    strings.push(characters[start..end].iter().collect());
                    index = end + delimiter.len();
                }
                None => {
                    if triple {
                        open = Some(delimiter);
                    }
                    break;
                }
            }
        }
        lines.push(Line {
            indent: raw.len() - raw.trim_start_matches(' ').len(),
            continuation,
            code,
            strings,
        });
    }
    lines
}

fn find(characters: &[char], from: usize, delimiter: &[char]) -> Option<usize> {
    let mut index = from;
    while index + delimiter.len() <= characters.len() {
        if characters[index] == '\\' {
            index += 2;
            continue;
        }
        if characters[index..index + delimiter.len()] == *delimiter {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn identifier(text: &str) -> &str {
    let end = text
        .find(|character: char| !(character.is_alphanumeric() || character == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

fn class_name(code: &str) -> Option<&str> {
    let name = identifier(code.strip_prefix("class ")?.trim_start());
    (!name.is_empty()).then_some(name)
}

fn function_name(code: &str) -> Option<&str> {
    let rest = code.strip_prefix("async ").unwrap_or(code).trim_start();
    let name = identifier(rest.strip_prefix("def ")?.trim_start());
    (!name.is_empty()).then_some(name)
}

fn structural(line: &Line) -> Option<&str> {
    let code = line.code.trim();
    let skipped = line.continuation || code.is_empty() || code.starts_with([')', ']', '}']);
    (!skipped).then_some(code)
}

/// Names listed in the `__all__` of a module, in source order.
pub fn exports(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in lex(source) {
        let code = line.code.trim();
        if !inside {
            if line.indent != 0 || !code.starts_with("__all__") || !code.contains('[') {
                continue;
            }
            inside = true;
        }
        names.extend(line.strings);
        if code.contains(']') {
            break;
        }
    }
    names
}

/// Tests pytest collects from a module with its default rules: functions
/// named `test_*` at module level or inside classes named `Test*`. Each is
/// returned as `Class::test_name`, in source order, without duplicates.
pub fn tests(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut scopes: Vec<(usize, Option<String>)> = Vec::new();
    for line in lex(source) {
        let Some(code) = structural(&line) else {
            continue;
        };
        while scopes.last().is_some_and(|scope| scope.0 >= line.indent) {
            scopes.pop();
        }
        if let Some(name) = class_name(code) {
            let collected = name.starts_with("Test");
            scopes.push((line.indent, collected.then(|| name.to_owned())));
        } else if let Some(name) = function_name(code) {
            if name.starts_with("test_") && scopes.iter().all(|scope| scope.1.is_some()) {
                let mut path: Vec<&str> = scopes
                    .iter()
                    .filter_map(|scope| scope.1.as_deref())
                    .collect();
                path.push(name);
                let test = path.join("::");
                if seen.insert(test.clone()) {
                    found.push(test);
                }
            }
            scopes.push((line.indent, None));
        }
    }
    found
}

/// Long options (`--name`) that appear as string literals in a module.
pub fn cli_flags(source: &str) -> BTreeSet<String> {
    lex(source)
        .into_iter()
        .flat_map(|line| line.strings)
        .filter(|text| {
            text.strip_prefix("--").is_some_and(|name| {
                name.starts_with(|first: char| first.is_ascii_lowercase())
                    && name.chars().all(|character| {
                        character.is_ascii_lowercase()
                            || character.is_ascii_digit()
                            || character == '-'
                    })
            })
        })
        .collect()
}

/// Annotated fields declared directly in the body of a class.
pub fn class_fields(source: &str, class: &str) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    let mut body: Option<usize> = None;
    for line in lex(source) {
        let Some(code) = structural(&line) else {
            continue;
        };
        match body {
            None => {
                if class_name(code) == Some(class) {
                    body = Some(line.indent);
                }
            }
            Some(indent) if line.indent <= indent => break,
            Some(indent) => {
                let name = identifier(code);
                let annotated = code[name.len()..].starts_with(':');
                if line.indent == indent + 4 && annotated && !name.is_empty() {
                    fields.insert(name.to_owned());
                }
            }
        }
    }
    fields
}

/// Control subtypes: the `subtype: Literal[...]` values of the request
/// classes of `types.py`, and the `"subtype": "..."` values `query.py`
/// sends.
pub fn control_subtypes(types: &str, query: &str) -> BTreeSet<String> {
    let mut subtypes = BTreeSet::new();
    let mut request_class: Option<usize> = None;
    for line in lex(types) {
        let Some(code) = structural(&line) else {
            continue;
        };
        if request_class.is_some_and(|indent| line.indent <= indent) {
            request_class = None;
        }
        if let Some(name) = class_name(code) {
            if name.starts_with("SDK") && name.ends_with("Request") {
                request_class = Some(line.indent);
            }
        } else if request_class.is_some() && code.starts_with("subtype: Literal[") {
            subtypes.extend(line.strings);
        }
    }
    for line in lex(query) {
        if !line.code.contains("\"\": \"\"") {
            continue;
        }
        for pair in line.strings.windows(2) {
            if pair[0] == "subtype" {
                subtypes.insert(pair[1].clone());
            }
        }
    }
    subtypes
}

/// Version headings (`## x.y.z`) of a changelog, in source order.
pub fn changelog_versions(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(|heading| heading.trim().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_drops_comments_and_string_contents() {
        let lines = lex("x = \"a # b\"  # trailing\ny = 'it\\'s'\n");
        assert_eq!(lines[0].code.trim_end(), "x = \"\"");
        assert_eq!(lines[0].strings, ["a # b"]);
        assert_eq!(lines[1].strings, ["it\\'s"]);
    }

    #[test]
    fn lexer_skips_lines_inside_a_multi_line_string() {
        let source = "text = \"\"\"\ndef test_inside():\n    pass\n\"\"\"\nafter = 1\n";
        let lines = lex(source);
        let codes: Vec<&str> = lines.iter().map(|line| line.code.trim()).collect();
        assert_eq!(codes, ["text = \"\"", "", "after = 1"]);
        assert!(lines[1].continuation);
    }

    #[test]
    fn exports_reads_the_all_list() {
        let source =
            "__all__ = [\n    # Main\n    \"query\",\n    'Transport',\n]\nother = [\"no\"]\n";
        assert_eq!(exports(source), ["query", "Transport"]);
    }

    #[test]
    fn tests_follow_the_pytest_collection_rules() {
        let source = [
            "def test_top():",
            "    def test_nested_helper():",
            "        pass",
            "",
            "async def test_async(",
            "    fixture,",
            ") -> None:",
            "    pass",
            "",
            "def helper():",
            "    pass",
            "",
            "class TestGroup:",
            "    \"\"\"def test_in_docstring(): pass\"\"\"",
            "",
            "    def test_method(self):",
            "        pass",
            "",
            "    class TestInner:",
            "        async def test_deep(self):",
            "            pass",
            "",
            "    def not_a_test(self):",
            "        pass",
            "",
            "class Helpers:",
            "    def test_not_collected(self):",
            "        pass",
            "",
            "SCRIPT = '''",
            "def test_in_string():",
            "    pass",
            "'''",
            "",
            "def test_top():",
            "    pass",
        ]
        .join("\n");
        assert_eq!(
            tests(&source),
            [
                "test_top",
                "test_async",
                "TestGroup::test_method",
                "TestGroup::TestInner::test_deep",
            ]
        );
    }

    #[test]
    fn cli_flags_keeps_only_long_option_literals() {
        let source = "cmd = [\"--output-format\", \"stream-json\", '--verbose', \"--\", f\"--{flag}\", \"--Bad\"]\n";
        let flags: Vec<String> = cli_flags(source).into_iter().collect();
        assert_eq!(flags, ["--output-format", "--verbose"]);
    }

    #[test]
    fn class_fields_stops_at_the_end_of_the_class() {
        let source = [
            "@dataclass",
            "class Options:",
            "    \"\"\"Docs.",
            "",
            "    not_a_field: here",
            "    \"\"\"",
            "",
            "    model: str | None = None",
            "    env: dict[str, str] = field(",
            "        default_factory=dict,",
            "    )",
            "",
            "    def method(self, value: int) -> None:",
            "        local: int = 1",
            "",
            "class Other:",
            "    extra: int = 0",
        ]
        .join("\n");
        let fields: Vec<String> = class_fields(&source, "Options").into_iter().collect();
        assert_eq!(fields, ["env", "model"]);
    }

    #[test]
    fn control_subtypes_merges_declared_and_sent_values() {
        let types = [
            "class SDKControlInterruptRequest(TypedDict):",
            "    subtype: Literal[\"interrupt\"]",
            "",
            "class ControlResponse(TypedDict):",
            "    subtype: Literal[\"success\"]",
        ]
        .join("\n");
        let query = "request = {\n    \"subtype\": \"set_model\",\n    \"model\": model,\n}\n";
        let subtypes: Vec<String> = control_subtypes(&types, query).into_iter().collect();
        assert_eq!(subtypes, ["interrupt", "set_model"]);
    }
}
