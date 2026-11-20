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
//! - `category`: one of [`CATEGORIES`].
//! - `reason`: why the difference is accepted.
//! - `ref`: for a `python-defect`, the upstream line at fault as
//!   `path:line` or `path:first-last`, relative to the upstream checkout at
//!   the pinned commit.
//!
//! An entry matches a difference when the case, the location, and both
//! values agree. An entry that matches nothing in a full run is stale.
//!
//! The `[observed]` table has the same shape and excuses a difference in
//! what the application observed, found by comparing the journals of the
//! drivers: its `location` starts with `journal `, and `python` and `rust`
//! are the normalized values of the journal.
//!
//! A value that moves with every release is written as a token instead of
//! a literal, so no release has to edit the ledger: [`CRATE_VERSION`]
//! stands for the workspace version of the root `Cargo.toml`, and
//! [`UPSTREAM_VERSION`] for the version of the upstream checkout's
//! `pyproject.toml`. Tokens are replaced in the whole file before it is
//! parsed, and the comparison and the parity page both see the replaced
//! text.
//!
//! The `[behavior]` table lists the divergences the harness cannot see,
//! because they are not on the wire (how inbound messages are parsed, how
//! the child process is placed):
//!
//! ```toml
//! [behavior.unknown-messages-kept]
//! summary = "An unknown message type"
//! python = "Skipped: the parser returns None."
//! rust = "Kept as Message::Unknown with its raw JSON."
//! ref = "src/claude_agent_sdk/_internal/message_parser.py:397-401"
//! category = "rust-more-robust"
//! reason = "A newer CLI must not make an older program lose data."
//! test = "crates/claude-agent-rust-sdk/tests/message_parser.rs::unknown_message_is_kept"
//! ```
//!
//! `ref` is required on every behavior entry, and `test` names a Rust test
//! as `cargo xtask parity-tests` prints it, with the platform suffix of
//! `parity/tests.toml` when the test is compiled on one platform only.
//! `parity-check` fails when the test does not exist.
//!
//! The `[unclassified]` table names the divergences of the parity audit
//! (`docs/research/python-sdk-parity.md`) that are not yet reviewed into
//! one of the two tables, with `source` (the audit section) and `summary`.
//! `docs/parity.md` prints them with their count.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::compare::{Difference, same};

/// The categories of a difference, in the order `docs/parity.md` shows
/// them.
pub const CATEGORIES: [&str; 5] = [
    "deliberate",
    "rust-follows-cli",
    "rust-more-robust",
    "rust-stricter",
    "python-defect",
];
const LOCATION_KINDS: [&str; 6] = ["processes", "cwd", "env ", "arg ", "frame ", "frames"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLedger {
    #[serde(default)]
    wire: BTreeMap<String, RawEntry>,
    #[serde(default)]
    observed: BTreeMap<String, RawEntry>,
    #[serde(default)]
    behavior: BTreeMap<String, RawBehavior>,
    #[serde(default)]
    unclassified: BTreeMap<String, RawUnclassified>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBehavior {
    summary: String,
    python: String,
    rust: String,
    #[serde(rename = "ref")]
    reference: String,
    anchor: String,
    category: String,
    reason: String,
    test: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUnclassified {
    source: String,
    summary: String,
}

/// One divergence the differential harness cannot see.
#[derive(Debug, Clone)]
pub struct Behavior {
    /// Key of the entry in the `[behavior]` table.
    pub id: String,
    /// What differs, in a few words.
    pub summary: String,
    /// What the Python SDK does.
    pub python: String,
    /// What claude-agent-rust-sdk does.
    pub rust: String,
    /// The upstream lines that show the Python behavior, `path:line` or
    /// `path:first-last`.
    pub reference: String,
    /// Category of the difference.
    pub category: String,
    /// Why the difference is accepted.
    pub reason: String,
    /// The Rust test that shows the Rust behavior.
    pub test: String,
}

/// A divergence of the audit not yet reviewed.
#[derive(Debug, Clone)]
pub struct Unclassified {
    /// Where the audit names it.
    pub source: String,
    /// What differs.
    pub summary: String,
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
    anchor: Option<String>,
}

/// One reviewed difference.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The table of the entry: `wire` or `observed`.
    pub table: &'static str,
    /// Key of the entry in its table.
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
    /// The upstream line at fault, for a `python-defect`.
    pub reference: Option<String>,
}

impl Entry {
    /// The entry as `table.id`, unique across tables.
    pub fn key(&self) -> String {
        format!("{}.{}", self.table, self.id)
    }

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

/// The entries of the ledger.
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    /// The `[wire]` entries, then the `[observed]` entries, each in key
    /// order.
    pub entries: Vec<Entry>,
    /// The `[behavior]` entries, in key order.
    pub behavior: Vec<Behavior>,
    /// The `[unclassified]` entries, in key order.
    pub unclassified: Vec<Unclassified>,
}

fn json_value(key: &str, side: &str, text: Option<&String>) -> Result<Option<Value>, String> {
    text.map(|text| {
        serde_json::from_str(text)
            .map_err(|error| format!("{key}: `{side}` is not JSON text ({error}): {text}"))
    })
    .transpose()
}

/// Whether `location` is a location of the table: one the recording
/// comparison writes for `wire`, one the journal comparison writes for
/// `observed`.
fn is_location(table: &str, location: &str) -> bool {
    if table == "observed" {
        return location.starts_with("journal ");
    }
    LOCATION_KINDS.iter().any(|kind| {
        let location = location
            .split_once("] ")
            .filter(|(process, _)| process.starts_with("process["))
            .map_or(location, |(_, rest)| rest);
        location == kind.trim_end() || (kind.ends_with(' ') && location.starts_with(kind))
    })
}

fn difference_entry(
    table: &'static str,
    id: String,
    entry: RawEntry,
    upstream: &Path,
) -> Result<Entry, String> {
    let key = format!("{table}.{id}");
    check_category(&key, &entry.category)?;
    if entry.reason.trim().is_empty() {
        return Err(format!("{key}: `reason` is empty"));
    }
    if entry.case.trim().is_empty() {
        return Err(format!("{key}: `case` is empty"));
    }
    if !is_location(table, &entry.location) {
        return Err(format!("{key}: `{}` is not a location", entry.location));
    }
    let python = json_value(&key, "python", entry.python.as_ref())?;
    let rust = json_value(&key, "rust", entry.rust.as_ref())?;
    if python.is_none() && rust.is_none() {
        return Err(format!(
            "{key}: at least one of `python` and `rust` must be set"
        ));
    }
    if entry.category == "python-defect" && entry.reference.is_none() {
        return Err(format!(
            "{key}: a python-defect entry must cite the upstream line at fault as `ref = \"path:line\"`"
        ));
    }
    if let Some(reference) = entry.reference.as_deref() {
        let anchor = entry.anchor.as_deref().unwrap_or_default();
        check_reference(&key, reference, anchor, upstream)?;
    }
    Ok(Entry {
        table,
        id,
        case: entry.case,
        location: entry.location,
        python,
        rust,
        category: entry.category,
        reason: entry.reason,
        reference: entry.reference,
    })
}

/// Checks that `reference`, written `path:line` or `path:first-last`,
/// names lines that exist in a file of the upstream checkout, and that
/// those lines hold `anchor` verbatim, so a ref that drifts onto unrelated
/// code when upstream moves fails. `key` is the entry, as `table.id`, for
/// messages.
fn check_reference(
    key: &str,
    reference: &str,
    anchor: &str,
    upstream: &Path,
) -> Result<(), String> {
    let invalid =
        || format!("{key}: `ref` must be `path:line` or `path:first-last`, got `{reference}`");
    let (path, lines) = reference.rsplit_once(':').ok_or_else(invalid)?;
    let (first, last) = lines.split_once('-').unwrap_or((lines, lines));
    let first: usize = first.parse().map_err(|_| invalid())?;
    let last: usize = last.parse().map_err(|_| invalid())?;
    if first > last {
        return Err(invalid());
    }
    let inside = Path::new(path)
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)));
    if path.is_empty() || !inside {
        return Err(format!(
            "{key}: `ref` must name a file inside the upstream checkout, got `{path}`"
        ));
    }
    let file = upstream.join(path);
    let text = std::fs::read_to_string(&file).map_err(|error| {
        format!(
            "{key}: `ref` names {}, which cannot be read: {error}",
            file.display()
        )
    })?;
    let count = text.lines().count();
    if first == 0 || last > count {
        return Err(format!(
            "{key}: `ref` names line {} of {path}, which has {count} lines",
            if first == 0 { first } else { last }
        ));
    }
    if anchor.trim().is_empty() {
        return Err(format!(
            "{key}: a `ref` needs an `anchor`, text the cited lines hold verbatim"
        ));
    }
    let cited: Vec<&str> = text
        .lines()
        .skip(first - 1)
        .take(last - first + 1)
        .collect();
    if !cited.join("\n").contains(anchor) {
        return Err(format!(
            "{key}: `ref` {reference} no longer holds its `anchor` `{anchor}`; find the lines \
             that do the cited thing now and update both"
        ));
    }
    Ok(())
}

fn check_category(key: &str, category: &str) -> Result<(), String> {
    if CATEGORIES.contains(&category) {
        Ok(())
    } else {
        Err(format!(
            "{key}: category `{category}` is not one of {}",
            CATEGORIES.join(", ")
        ))
    }
}

fn behavior_entry(id: String, entry: RawBehavior, upstream: &Path) -> Result<Behavior, String> {
    let key = format!("behavior.{id}");
    check_category(&key, &entry.category)?;
    for (field, value) in [
        ("summary", &entry.summary),
        ("python", &entry.python),
        ("rust", &entry.rust),
        ("reason", &entry.reason),
        ("test", &entry.test),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{key}: `{field}` is empty"));
        }
    }
    check_reference(&key, &entry.reference, &entry.anchor, upstream)?;
    Ok(Behavior {
        id,
        summary: entry.summary,
        python: entry.python,
        rust: entry.rust,
        reference: entry.reference,
        category: entry.category,
        reason: entry.reason,
        test: entry.test,
    })
}

/// Parses the ledger. `upstream` is the checkout `ref` paths resolve in.
pub fn parse(text: &str, upstream: &Path) -> Result<Ledger, String> {
    let raw: RawLedger = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    for (table, raw_entries) in [("wire", raw.wire), ("observed", raw.observed)] {
        for (id, entry) in raw_entries {
            entries.push(difference_entry(table, id, entry, upstream)?);
        }
    }
    let behavior = raw
        .behavior
        .into_iter()
        .map(|(id, entry)| behavior_entry(id, entry, upstream))
        .collect::<Result<_, _>>()?;
    let mut unclassified = Vec::new();
    for (id, entry) in raw.unclassified {
        if entry.source.trim().is_empty() || entry.summary.trim().is_empty() {
            return Err(format!(
                "unclassified.{id}: `source` and `summary` must both be set"
            ));
        }
        unclassified.push(Unclassified {
            source: entry.source,
            summary: entry.summary,
        });
    }
    Ok(Ledger {
        entries,
        behavior,
        unclassified,
    })
}

/// The token for the workspace version.
pub const CRATE_VERSION: &str = "{crate_version}";
/// The token for the version of the upstream package.
pub const UPSTREAM_VERSION: &str = "{upstream_version}";

/// The `[workspace.package]` version of the manifest at `root`.
pub fn crate_version(root: &Path) -> Result<String, String> {
    let path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest: toml::Table = text
        .parse()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: no [workspace.package] version", path.display()))
}

/// The `version = "..."` of a `pyproject.toml`.
pub fn pyproject_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("version")?.trim_start();
        let value = value.strip_prefix('=')?.trim();
        Some(value.trim_matches('"').to_owned())
    })
}

/// Replaces the version tokens of `text`, reading each version only when
/// its token appears.
pub fn resolve_tokens(text: &str, upstream: &Path, root: &Path) -> Result<String, String> {
    let mut text = text.to_owned();
    if text.contains(CRATE_VERSION) {
        text = text.replace(CRATE_VERSION, &crate_version(root)?);
    }
    if text.contains(UPSTREAM_VERSION) {
        let manifest = upstream.join("pyproject.toml");
        let version = std::fs::read_to_string(&manifest)
            .ok()
            .as_deref()
            .and_then(pyproject_version)
            .ok_or_else(|| format!("no `version = ...` line in {}", manifest.display()))?;
        text = text.replace(UPSTREAM_VERSION, &version);
    }
    Ok(text)
}

/// Reads and parses the ledger file, with its version tokens replaced.
/// `root` is the workspace whose manifest gives the crate version.
pub fn load(path: &Path, upstream: &Path, root: &Path) -> Result<Ledger, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let text = resolve_tokens(&text, upstream, root)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    parse(&text, upstream).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::parse;
    use crate::differential::compare::Difference;

    #[test]
    fn version_tokens_are_replaced_from_the_manifests_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nversion = \"7.8.9\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("upstream")).unwrap();
        std::fs::write(
            root.join("upstream/pyproject.toml"),
            "[project]\nname = \"x\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("differences.toml"),
            "[wire.version]\ncase = \"*\"\nlocation = \"env V\"\npython = '\"{upstream_version}\"'\nrust = '\"{crate_version}\"'\ncategory = \"deliberate\"\nreason = \"Own versions.\"\n",
        )
        .unwrap();
        let ledger =
            super::load(&root.join("differences.toml"), &root.join("upstream"), root).unwrap();
        assert_eq!(ledger.entries[0].python, Some(json!("1.2.3")));
        assert_eq!(ledger.entries[0].rust, Some(json!("7.8.9")));

        let untouched =
            super::resolve_tokens("no token", Path::new("/missing"), Path::new("/missing"));
        assert_eq!(untouched.unwrap(), "no token");
        let missing = super::resolve_tokens("{crate_version}", root, Path::new("/missing"));
        assert!(missing.unwrap_err().contains("Cargo.toml"));
    }

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
                    "location = \"env A\"\npython = '1'\ncategory = \"python-defect\"\nref = \"src/a.py:4\"\nanchor = \"one\"",
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
                entry(
                    "location = \"env A\"\npython = '1'\ncategory = \"python-defect\"\nref = \"src/a.py:1\"",
                ),
                "needs an `anchor`",
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
            "location = \"process[1] arg --x\"\nrust = '1'\ncategory = \"python-defect\"\nref = \"src/a.py:3\"\nanchor = \"three\"",
        );
        assert!(parse(&valid, upstream.path()).is_ok());
    }

    const BEHAVIOR: &str = r#"
[behavior.kept]
summary = "An unknown message"
python = "Skipped."
rust = "Kept raw."
ref = "src/a.py:2-3"
anchor = "two"
category = "rust-more-robust"
reason = "No data is lost."
test = "tests/a.rs::kept"

[unclassified.later]
source = "4.4 item 10"
summary = "Budget spelling."
"#;

    #[test]
    fn behavior_and_unclassified_entries_are_read_with_their_fields() {
        let upstream = upstream();
        let ledger = parse(BEHAVIOR, upstream.path()).unwrap();
        assert!(ledger.entries.is_empty());
        let [kept] = ledger.behavior.as_slice() else {
            panic!("{:?}", ledger.behavior)
        };
        assert_eq!(
            (
                kept.id.as_str(),
                kept.reference.as_str(),
                kept.category.as_str(),
                kept.test.as_str()
            ),
            (
                "kept",
                "src/a.py:2-3",
                "rust-more-robust",
                "tests/a.rs::kept"
            )
        );
        let [later] = ledger.unclassified.as_slice() else {
            panic!("{:?}", ledger.unclassified)
        };
        assert_eq!(
            (later.source.as_str(), later.summary.as_str()),
            ("4.4 item 10", "Budget spelling.")
        );
    }

    #[test]
    fn a_behavior_entry_needs_a_reference_inside_upstream_a_category_and_every_field() {
        let upstream = upstream();
        for (from, to, expected) in [
            ("ref = \"src/a.py:2-3\"\n", "", "missing field `ref`"),
            ("src/a.py:2-3", "src/a.py:2-4", "which has 3 lines"),
            ("src/a.py:2-3", "src/a.py:3-2", "`path:first-last`"),
            ("src/a.py:2-3", "src/a.py:0", "line 0"),
            ("src/a.py:2-3", "src/a.py", "`path:first-last`"),
            ("rust-more-robust", "nicer", "category `nicer`"),
            (
                "anchor = \"two\"",
                "anchor = \"four\"",
                "no longer holds its `anchor`",
            ),
            ("anchor = \"two\"\n", "", "missing field `anchor`"),
            (
                "test = \"tests/a.rs::kept\"",
                "test = \" \"",
                "`test` is empty",
            ),
            (
                "summary = \"Budget spelling.\"",
                "summary = \"\"",
                "must both be set",
            ),
        ] {
            let text = BEHAVIOR.replacen(from, to, 1);
            assert_ne!(text, BEHAVIOR, "{from}");
            let error = parse(&text, upstream.path()).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn an_observed_entry_takes_a_journal_location_and_keys_stay_apart_from_wire() {
        let upstream = upstream();
        let entry = |table: &str, location: &str| {
            format!(
                "[{table}.same]\ncase = \"*\"\nlocation = \"{location}\"\nrust = '1'\n\
                 category = \"deliberate\"\nreason = \"r\"\n"
            )
        };
        let both = format!(
            "{}\n{}",
            entry("wire", "env A"),
            entry("observed", "journal message Unknown")
        );
        let ledger = parse(&both, upstream.path()).unwrap();
        let keys: Vec<String> = ledger.entries.iter().map(super::Entry::key).collect();
        assert_eq!(keys, ["wire.same", "observed.same"]);
        let error = parse(&entry("observed", "env A"), upstream.path()).unwrap_err();
        assert!(
            error.contains("observed.same: `env A` is not a location"),
            "{error}"
        );
        let error = parse(&entry("wire", "journal call[0]"), upstream.path()).unwrap_err();
        assert!(
            error.contains("wire.same: `journal call[0]` is not a location"),
            "{error}"
        );
    }
}
