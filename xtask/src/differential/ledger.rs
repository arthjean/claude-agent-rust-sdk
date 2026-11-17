//! The ledger of reviewed differences, `parity/differences.toml`.
//!
//! Each entry of the `[wire]` table excuses one difference the
//! differential harness finds:
//!
//! ```toml
//! [wire.entrypoint-label]
//! case = "*"
//! location = "env CLAUDE_CODE_ENTRYPOINT"
//! python = '"sdk-py"'
//! rust = '"sdk-rs"'
//! category = "deliberate"
//! reason = "Each SDK names itself to the CLI."
//! ```
//!
//! - `case`: the name of a case, or `*` for every case.
//! - `location`: one location, as `compare` writes it.
//! - `python`, `rust`: the value on each side, as JSON text; a missing key
//!   means that side sent nothing there.
//! - `category`: `deliberate`, `rust-follows-cli`, or `python-defect`.
//! - `reason`: why the difference is accepted.
//! - `ref`: for a `python-defect`, the upstream line at fault as
//!   `path:line`, relative to the upstream checkout at the pinned commit.
//!
//! An entry matches a difference when the case, the location, and both
//! values agree. An entry that matches nothing in a full run is stale.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::compare::{Difference, same};

const CATEGORIES: [&str; 3] = ["deliberate", "rust-follows-cli", "python-defect"];
const LOCATION_KINDS: [&str; 6] = ["processes", "cwd", "env ", "arg ", "frame ", "frames"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLedger {
    #[serde(default)]
    wire: BTreeMap<String, RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    case: String,
    location: String,
    python: Option<String>,
    rust: Option<String>,
    category: String,
    reason: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
}

/// One reviewed difference.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Key of the entry in the `[wire]` table.
    pub id: String,
    /// Case name, or `*`.
    pub case: String,
    /// The one location the entry covers.
    pub location: String,
    /// Value on the Python side, `None` when absent.
    pub python: Option<Value>,
    /// Value on the Rust side, `None` when absent.
    pub rust: Option<Value>,
    /// Category of the difference.
    pub category: String,
    /// Why the difference is accepted.
    pub reason: String,
}

impl Entry {
    /// Whether the entry excuses this difference of this case.
    pub fn matches(&self, case: &str, difference: &Difference) -> bool {
        let value_matches =
            |expected: &Option<Value>, actual: &Option<Value>| match (expected, actual) {
                (None, None) => true,
                (Some(expected), Some(actual)) => same(expected, actual),
                _ => false,
            };
        (self.case == "*" || self.case == case)
            && self.location == difference.location
            && value_matches(&self.python, &difference.python)
            && value_matches(&self.rust, &difference.rust)
    }
}

/// The `[wire]` entries of the ledger.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    /// Entries, in key order.
    pub entries: Vec<Entry>,
}

fn json_value(id: &str, side: &str, text: Option<&String>) -> Result<Option<Value>, String> {
    text.map(|text| {
        serde_json::from_str(text)
            .map_err(|error| format!("wire.{id}: `{side}` is not JSON text ({error}): {text}"))
    })
    .transpose()
}

fn check_reference(id: &str, reference: Option<&str>, upstream: &Path) -> Result<(), String> {
    let Some(reference) = reference else {
        return Err(format!(
            "wire.{id}: a python-defect entry must cite the upstream line at fault as `ref = \"path:line\"`"
        ));
    };
    let invalid = || format!("wire.{id}: `ref` must be `path:line`, got `{reference}`");
    let (path, line) = reference.rsplit_once(':').ok_or_else(invalid)?;
    let line: usize = line.parse().map_err(|_| invalid())?;
    let inside = Path::new(path)
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)));
    if path.is_empty() || !inside {
        return Err(format!(
            "wire.{id}: `ref` must name a file inside the upstream checkout, got `{path}`"
        ));
    }
    let file = upstream.join(path);
    let text = std::fs::read_to_string(&file).map_err(|error| {
        format!(
            "wire.{id}: `ref` names {}, which cannot be read: {error}",
            file.display()
        )
    })?;
    if line == 0 || line > text.lines().count() {
        return Err(format!(
            "wire.{id}: `ref` names line {line} of {path}, which has {} lines",
            text.lines().count()
        ));
    }
    Ok(())
}

/// Parses the ledger. `upstream` is the checkout `ref` paths resolve in.
pub fn parse(text: &str, upstream: &Path) -> Result<Ledger, String> {
    let raw: RawLedger = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    for (id, entry) in raw.wire {
        if !CATEGORIES.contains(&entry.category.as_str()) {
            return Err(format!(
                "wire.{id}: category `{}` is not one of {}",
                entry.category,
                CATEGORIES.join(", ")
            ));
        }
        if entry.reason.trim().is_empty() {
            return Err(format!("wire.{id}: `reason` is empty"));
        }
        if entry.case.trim().is_empty() {
            return Err(format!("wire.{id}: `case` is empty"));
        }
        let location_kind = LOCATION_KINDS.iter().any(|kind| {
            let location = entry
                .location
                .split_once("] ")
                .filter(|(process, _)| process.starts_with("process["))
                .map_or(entry.location.as_str(), |(_, rest)| rest);
            location == kind.trim_end() || (kind.ends_with(' ') && location.starts_with(kind))
        });
        if !location_kind {
            return Err(format!("wire.{id}: `{}` is not a location", entry.location));
        }
        let python = json_value(&id, "python", entry.python.as_ref())?;
        let rust = json_value(&id, "rust", entry.rust.as_ref())?;
        if python.is_none() && rust.is_none() {
            return Err(format!(
                "wire.{id}: at least one of `python` and `rust` must be set"
            ));
        }
        if entry.category == "python-defect" {
            check_reference(&id, entry.reference.as_deref(), upstream)?;
        }
        entries.push(Entry {
            id,
            case: entry.case,
            location: entry.location,
            python,
            rust,
            category: entry.category,
            reason: entry.reason,
        });
    }
    Ok(Ledger { entries })
}

/// Reads and parses the ledger file.
pub fn load(path: &Path, upstream: &Path) -> Result<Ledger, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse(&text, upstream).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse;
    use crate::differential::compare::Difference;

    fn upstream() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("src")).unwrap();
        std::fs::write(directory.path().join("src/a.py"), "one\ntwo\nthree\n").unwrap();
        directory
    }

    #[test]
    fn an_entry_matches_its_case_location_and_both_values() {
        let upstream = upstream();
        let ledger = parse(
            r#"
            [wire.label]
            case = "*"
            location = "env CLAUDE_CODE_ENTRYPOINT"
            python = '"sdk-py"'
            rust = '"sdk-rs"'
            category = "deliberate"
            reason = "Each SDK names itself."

            [wire.only-python]
            case = "default"
            location = "frame control_request:initialize /request/hooks"
            python = 'null'
            category = "rust-follows-cli"
            reason = "The CLI reads a missing key as null."
            "#,
            upstream.path(),
        )
        .unwrap();
        assert_eq!(ledger.entries.len(), 2);
        let label = Difference {
            location: "env CLAUDE_CODE_ENTRYPOINT".to_owned(),
            python: Some(json!("sdk-py")),
            rust: Some(json!("sdk-rs")),
        };
        let label_entry = ledger
            .entries
            .iter()
            .find(|entry| entry.id == "label")
            .unwrap();
        assert!(label_entry.matches("any", &label));
        let other_value = Difference {
            rust: Some(json!("sdk-ts")),
            ..label.clone()
        };
        assert!(!label_entry.matches("any", &other_value));

        let hooks = Difference {
            location: "frame control_request:initialize /request/hooks".to_owned(),
            python: Some(json!(null)),
            rust: None,
        };
        let hooks_entry = ledger
            .entries
            .iter()
            .find(|entry| entry.id == "only-python")
            .unwrap();
        assert!(hooks_entry.matches("default", &hooks));
        assert!(!hooks_entry.matches("other", &hooks));
        let both_null = Difference {
            rust: Some(json!(null)),
            ..hooks
        };
        assert!(!hooks_entry.matches("default", &both_null));
    }

    #[test]
    fn malformed_entries_are_rejected_with_their_key() {
        let upstream = upstream();
        let entry = |body: &str| format!("[wire.e]\ncase = \"*\"\nreason = \"r\"\n{body}\n");
        let cases = [
            (
                entry("location = \"env A\"\npython = '1'\ncategory = \"nice\""),
                "category `nice`",
            ),
            (
                entry("location = \"somewhere\"\npython = '1'\ncategory = \"deliberate\""),
                "not a location",
            ),
            (
                entry("location = \"env A\"\npython = 'not json'\ncategory = \"deliberate\""),
                "not JSON text",
            ),
            (
                entry("location = \"env A\"\ncategory = \"deliberate\""),
                "at least one of",
            ),
            (
                entry("location = \"env A\"\npython = '1'\ncategory = \"python-defect\""),
                "must cite the upstream line",
            ),
            (
                entry(
                    "location = \"env A\"\npython = '1'\ncategory = \"python-defect\"\nref = \"src/a.py:4\"",
                ),
                "which has 3 lines",
            ),
            (
                entry(
                    "location = \"env A\"\npython = '1'\ncategory = \"python-defect\"\nref = \"src/missing.py:1\"",
                ),
                "cannot be read",
            ),
            (
                entry(
                    "location = \"env A\"\npython = '1'\ncategory = \"python-defect\"\nref = \"../outside.py:1\"",
                ),
                "inside the upstream checkout",
            ),
            (
                entry(
                    "location = \"env A\"\npython = '1'\ncategory = \"python-defect\"\nref = \"/etc/hosts:1\"",
                ),
                "inside the upstream checkout",
            ),
            (
                entry("location = \"env A\"\npython = '1'\ncategory = \"deliberate\"\nextra = 1"),
                "unknown field",
            ),
        ];
        for (text, expected) in cases {
            let error = parse(&text, upstream.path()).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
            assert!(
                error.contains("wire.e") || expected == "unknown field",
                "{error}"
            );
        }
        let valid = entry(
            "location = \"process[1] arg --x\"\nrust = '1'\ncategory = \"python-defect\"\nref = \"src/a.py:3\"",
        );
        assert!(parse(&valid, upstream.path()).is_ok());
    }
}
