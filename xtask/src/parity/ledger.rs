//! The two parity ledgers, `parity/surface.toml` and `parity/tests.toml`,
//! and the comparison of a ledger with what upstream holds.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use indexmap::IndexMap;
use serde::Deserialize;

/// One or several Rust paths.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Paths {
    /// A single path.
    One(String),
    /// Several paths that together cover the upstream item.
    Many(Vec<String>),
}

impl Paths {
    /// The paths, in ledger order.
    pub fn as_slice(&self) -> &[String] {
        match self {
            Self::One(path) => std::slice::from_ref(path),
            Self::Many(paths) => paths,
        }
    }
}

/// Splits a Rust test path into the path and the platform it is compiled
/// on, written as a ` @platform` suffix: `windows`, `unix`, `linux`, or
/// `macos`.
pub fn split_platform(path: &str) -> (&str, Option<&str>) {
    match path.rsplit_once(" @") {
        Some((path, platform)) => (path, Some(platform)),
        None => (path, None),
    }
}

/// Whether tests of `platform` are compiled on this host, or `None` for a
/// platform name the ledger does not know.
pub fn compiled_here(platform: &str) -> Option<bool> {
    match platform {
        "windows" => Some(cfg!(windows)),
        "unix" => Some(cfg!(unix)),
        "linux" => Some(cfg!(target_os = "linux")),
        "macos" => Some(cfg!(target_os = "macos")),
        _ => None,
    }
}

/// Disposition of one upstream item. Exactly one of `rust`, `adapted`,
/// `excluded`, and `pending` is set.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Rust paths that implement or test the upstream item.
    pub rust: Option<Paths>,
    /// Rust paths of a different kind of item that takes the place of the
    /// upstream one (an enum variant for a class, a method for a warning
    /// class). An adapted entry must carry a `note` that says how.
    pub adapted: Option<Paths>,
    /// Key of the `[exclusions]` table giving why the item is not ported.
    pub excluded: Option<String>,
    /// Why the item has no disposition yet.
    pub pending: Option<String>,
    /// Free text shown next to the item in reports.
    pub note: Option<String>,
}

/// State of a ledger entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The entry names Rust paths of the same kind of item.
    Implemented,
    /// The entry names Rust paths of a different kind of item.
    Adapted,
    /// The entry names an exclusion reason.
    Excluded,
    /// The entry is waiting for a disposition.
    Pending,
}

impl Entry {
    /// State of the entry, or `None` when it does not set exactly one of
    /// `rust`, `adapted`, `excluded`, and `pending`.
    pub fn state(&self) -> Option<State> {
        match (&self.rust, &self.adapted, &self.excluded, &self.pending) {
            (Some(paths), None, None, None) if !paths.as_slice().is_empty() => {
                Some(State::Implemented)
            }
            (None, Some(paths), None, None) if !paths.as_slice().is_empty() => Some(State::Adapted),
            (None, None, Some(_), None) => Some(State::Excluded),
            (None, None, None, Some(_)) => Some(State::Pending),
            _ => None,
        }
    }

    /// The Rust paths of an implemented or adapted entry, in ledger order.
    pub fn paths(&self) -> impl Iterator<Item = &String> {
        self.rust
            .iter()
            .chain(self.adapted.iter())
            .flat_map(Paths::as_slice)
    }
}

/// Fails on the first adapted entry without a `note`, naming it.
fn require_adapted_notes(ledger: Ledger) -> Result<Ledger, String> {
    let unexplained = ledger.entries.iter().find(|(_, entry)| {
        entry.adapted.is_some()
            && entry
                .note
                .as_deref()
                .is_none_or(|note| note.trim().is_empty())
    });
    match unexplained {
        Some((item, _)) => Err(format!(
            "{}: `{item}` is adapted but has no `note` saying how the Rust item differs",
            ledger.file
        )),
        None => Ok(ledger),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceFile {
    #[serde(default)]
    exclusions: IndexMap<String, String>,
    names: IndexMap<String, Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestsFile {
    #[serde(default)]
    exclusions: IndexMap<String, String>,
    tests: IndexMap<String, IndexMap<String, Entry>>,
}

/// A ledger: exclusion reasons and one entry per upstream item.
#[derive(Debug)]
pub struct Ledger {
    /// Path of the file, for messages.
    pub file: String,
    /// What one entry describes: `export` or `test`.
    pub kind: &'static str,
    /// Exclusion reasons by key.
    pub exclusions: IndexMap<String, String>,
    /// Entries by upstream identifier.
    pub entries: IndexMap<String, Entry>,
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn label(path: &Path) -> String {
    let name = path.file_name().unwrap_or(path.as_os_str());
    format!("parity/{}", name.to_string_lossy())
}

/// Loads `surface.toml`.
pub fn load_surface(path: &Path) -> Result<Ledger, String> {
    let file: SurfaceFile =
        toml::from_str(&read(path)?).map_err(|error| format!("{}: {error}", path.display()))?;
    require_adapted_notes(Ledger {
        file: label(path),
        kind: "export",
        exclusions: file.exclusions,
        entries: file.names,
    })
}

/// Loads `tests.toml`. An entry is identified as `file::test`.
pub fn load_tests(path: &Path) -> Result<Ledger, String> {
    let file: TestsFile =
        toml::from_str(&read(path)?).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut entries = IndexMap::new();
    for (module, tests) in file.tests {
        for (test, entry) in tests {
            entries.insert(format!("{module}::{test}"), entry);
        }
    }
    require_adapted_notes(Ledger {
        file: label(path),
        kind: "test",
        exclusions: file.exclusions,
        entries,
    })
}

/// Counts of a ledger by state.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    /// Entries naming Rust paths of the same kind of item.
    pub implemented: usize,
    /// Entries naming Rust paths of a different kind of item.
    pub adapted: usize,
    /// Entries naming an exclusion reason.
    pub excluded: usize,
    /// Entries waiting for a disposition.
    pub pending: usize,
    /// Excluded entries by reason key.
    pub by_reason: BTreeMap<String, usize>,
}

impl Ledger {
    /// Problems that do not depend on Rust code: upstream items missing
    /// from the ledger, entries upstream no longer has, malformed entries,
    /// and exclusion keys that are unknown or unused.
    pub fn structural_problems(&self, upstream: &[String]) -> Vec<String> {
        let mut problems = Vec::new();
        let known: BTreeSet<&str> = upstream.iter().map(String::as_str).collect();
        for item in upstream {
            if !self.entries.contains_key(item) {
                problems.push(format!(
                    "{}: upstream {} `{item}` is not listed",
                    self.file, self.kind
                ));
            }
        }
        let mut used = BTreeSet::new();
        for (item, entry) in &self.entries {
            if !known.contains(item.as_str()) {
                problems.push(format!(
                    "{}: `{item}` is listed but upstream has no such {}",
                    self.file, self.kind
                ));
            }
            if entry.state().is_none() {
                problems.push(format!(
                    "{}: `{item}` must set exactly one of `rust`, `adapted`, `excluded`, `pending`",
                    self.file
                ));
            }
            for path in entry.paths() {
                if let (_, Some(platform)) = split_platform(path)
                    && compiled_here(platform).is_none()
                {
                    problems.push(format!(
                        "{}: `{item}` names the unknown platform `@{platform}`",
                        self.file
                    ));
                }
            }
            if let Some(reason) = &entry.excluded {
                used.insert(reason.as_str());
                if !self.exclusions.contains_key(reason) {
                    problems.push(format!(
                        "{}: `{item}` is excluded for `{reason}`, which `[exclusions]` does not define",
                        self.file
                    ));
                }
            }
        }
        for reason in self.exclusions.keys() {
            if !used.contains(reason.as_str()) {
                problems.push(format!(
                    "{}: exclusion `{reason}` is defined but no entry uses it",
                    self.file
                ));
            }
        }
        problems
    }

    /// Every `(upstream item, Rust path)` pair of the ledger.
    pub fn rust_paths(&self) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .flat_map(|(item, entry)| {
                entry
                    .paths()
                    .map(move |path| (item.as_str(), path.as_str()))
            })
            .collect()
    }

    /// Entries waiting for a disposition, with their note.
    pub fn pending(&self) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .filter_map(|(item, entry)| Some((item.as_str(), entry.pending.as_deref()?)))
            .collect()
    }

    /// Counts by state.
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for entry in self.entries.values() {
            match entry.state() {
                Some(State::Implemented) => counts.implemented += 1,
                Some(State::Adapted) => counts.adapted += 1,
                Some(State::Excluded) => {
                    counts.excluded += 1;
                    if let Some(reason) = &entry.excluded {
                        *counts.by_reason.entry(reason.clone()).or_default() += 1;
                    }
                }
                Some(State::Pending) | None => counts.pending += 1,
            }
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(text: &str) -> Ledger {
        let file: SurfaceFile = toml::from_str(text).unwrap();
        Ledger {
            file: "parity/surface.toml".to_owned(),
            kind: "export",
            exclusions: file.exclusions,
            entries: file.names,
        }
    }

    fn upstream(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    const LEDGER: &str = r#"
[exclusions]
python-only = "No counterpart in Rust."

[names]
query = { rust = "claude_agent_kit::query" }
Message = { rust = ["claude_agent_kit::Message", "claude_agent_kit::parse_message"] }
CanUseToolShadowedWarning = { excluded = "python-only" }
tool = { pending = "waiting" }
PermissionResultAllow = { adapted = "claude_agent_kit::PermissionResult::Allow", note = "Variant of `PermissionResult`." }
"#;

    #[test]
    fn a_complete_ledger_has_no_structural_problem() {
        let ledger = surface(LEDGER);
        let names = upstream(&[
            "query",
            "Message",
            "CanUseToolShadowedWarning",
            "tool",
            "PermissionResultAllow",
        ]);
        assert_eq!(ledger.structural_problems(&names), Vec::<String>::new());
        let counts = ledger.counts();
        assert_eq!(
            (
                counts.implemented,
                counts.adapted,
                counts.excluded,
                counts.pending
            ),
            (2, 1, 1, 1)
        );
        assert_eq!(counts.by_reason["python-only"], 1);
        assert_eq!(ledger.rust_paths().len(), 4);
        assert!(ledger.rust_paths().contains(&(
            "PermissionResultAllow",
            "claude_agent_kit::PermissionResult::Allow"
        )));
        assert_eq!(ledger.pending(), [("tool", "waiting")]);
    }

    #[test]
    fn an_upstream_export_missing_from_the_ledger_is_named() {
        let ledger = surface(LEDGER);
        let names = upstream(&[
            "query",
            "Message",
            "CanUseToolShadowedWarning",
            "tool",
            "PermissionResultAllow",
            "BrandNewExport",
        ]);
        let problems = ledger.structural_problems(&names);
        assert_eq!(
            problems,
            ["parity/surface.toml: upstream export `BrandNewExport` is not listed"]
        );
    }

    #[test]
    fn stale_malformed_and_unknown_reasons_are_reported() {
        let ledger = surface(
            r#"
[exclusions]
unused = "Nothing uses this."

[names]
gone = { rust = "claude_agent_kit::gone" }
both = { rust = "claude_agent_kit::both", excluded = "unused" }
twice = { rust = "claude_agent_kit::Twice", adapted = "claude_agent_kit::Twice", note = "x" }
empty = {}
unknown = { excluded = "missing-reason" }
"#,
        );
        let problems = ledger
            .structural_problems(&upstream(&["both", "twice", "empty", "unknown"]))
            .join("\n");
        assert!(problems.contains("`gone` is listed but upstream has no such export"));
        assert!(
            problems.contains(
                "`both` must set exactly one of `rust`, `adapted`, `excluded`, `pending`"
            )
        );
        assert!(problems.contains("`twice` must set exactly one of"));
        assert!(problems.contains("`empty` must set exactly one of"));
        assert!(problems.contains("`missing-reason`, which `[exclusions]` does not define"));
        assert!(!problems.contains("exclusion `unused` is defined but no entry uses it"));
    }

    #[test]
    fn an_unused_exclusion_is_reported() {
        let ledger = surface(
            "[exclusions]\nunused = \"Nothing.\"\n\n[names]\nquery = { rust = \"claude_agent_kit::query\" }\n",
        );
        let problems = ledger.structural_problems(&upstream(&["query"]));
        assert_eq!(
            problems,
            ["parity/surface.toml: exclusion `unused` is defined but no entry uses it"]
        );
    }

    #[test]
    fn platform_suffixes_are_split_and_checked() {
        assert_eq!(
            split_platform("tests/a.rs::name @windows"),
            ("tests/a.rs::name", Some("windows"))
        );
        assert_eq!(
            split_platform("tests/a.rs::name"),
            ("tests/a.rs::name", None)
        );
        assert_eq!(compiled_here("windows"), Some(cfg!(windows)));
        assert_eq!(compiled_here("unix"), Some(cfg!(unix)));
        assert_eq!(compiled_here("plan9"), None);

        let ledger = surface("[names]\nquery = { rust = \"claude_agent_kit::query @plan9\" }\n");
        let problems = ledger.structural_problems(&upstream(&["query"]));
        assert_eq!(
            problems,
            ["parity/surface.toml: `query` names the unknown platform `@plan9`"]
        );
    }

    #[test]
    fn an_adapted_entry_without_a_note_fails_loading_naming_it() {
        let directory = std::env::temp_dir().join(format!("xtask-adapted-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let surface_path = directory.join("surface.toml");
        std::fs::write(
            &surface_path,
            "[names]\nquery = { rust = \"claude_agent_kit::query\" }\nWarning = { adapted = \"claude_agent_kit::Options::warning\" }\n",
        )
        .unwrap();
        let tests_path = directory.join("tests.toml");
        std::fs::write(
            &tests_path,
            "[tests.\"tests/test_a.py\"]\ntest_one = { adapted = \"tests/a.rs::one\", note = \" \" }\n",
        )
        .unwrap();

        let surface = load_surface(&surface_path).unwrap_err();
        let tests = load_tests(&tests_path).unwrap_err();
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(
            surface,
            "parity/surface.toml: `Warning` is adapted but has no `note` saying how the Rust item differs"
        );
        assert_eq!(
            tests,
            "parity/tests.toml: `tests/test_a.py::test_one` is adapted but has no `note` saying how the Rust item differs"
        );
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let parsed: Result<SurfaceFile, _> =
            toml::from_str("[names]\nquery = { rust = \"x\", rusty = \"y\" }\n");
        assert!(parsed.is_err());
    }
}
