//! `parity-page`: the generated region of `docs/parity.md`.
//!
//! The region between [`PAGE_BEGIN`] and [`PAGE_END`] is rendered from
//! facts read elsewhere: the upstream version and the pinned commit, the
//! counts of both ledgers, the coverage of the differential cases, and the
//! entries of `parity/differences.toml`. Everything outside the region is
//! written by hand, and must not claim more than the counts show: a
//! hand-written "100%", "full parity", "at parity with", or "every known
//! difference" fails the check, in this page, the README, and the migration
//! guide.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use super::ledger::{Counts, Ledger, State};
use crate::differential::ledger::{self as differences, CATEGORIES};

/// Start of the generated region.
pub const PAGE_BEGIN: &str = "<!-- parity:page:begin -->";
/// End of the generated region.
pub const PAGE_END: &str = "<!-- parity:page:end -->";

/// Claims the hand-written text may not make, lowercase.
const UNSUPPORTED_CLAIMS: [&str; 5] = [
    "100%",
    "100 %",
    "full parity",
    "at parity with",
    "every known difference",
];

/// The upstream checkout the page describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// Version of the upstream package, from its `pyproject.toml`.
    pub version: String,
    /// Full hash of the checked-out commit.
    pub commit: String,
    /// Committer date of that commit, `YYYY-MM-DD`.
    pub date: String,
}

/// Counts of one ledger, including upstream items it does not list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shares {
    /// Entries naming Rust paths of the same kind of item.
    pub implemented: usize,
    /// Entries naming Rust paths of a different kind of item.
    pub adapted: usize,
    /// Entries naming an exclusion reason.
    pub excluded: usize,
    /// Entries waiting for a disposition, and upstream items not listed.
    pub pending: usize,
}

impl Shares {
    /// The shares of `ledger` against the items upstream holds.
    pub fn of(ledger: &Ledger, upstream: &[String]) -> Self {
        let Counts {
            implemented,
            adapted,
            excluded,
            pending,
            ..
        } = ledger.counts();
        let unlisted = upstream
            .iter()
            .filter(|item| !ledger.entries.contains_key(*item))
            .count();
        Self {
            implemented,
            adapted,
            excluded,
            pending: pending + unlisted,
        }
    }

    fn total(&self) -> usize {
        self.implemented + self.adapted + self.excluded + self.pending
    }

    fn cell(&self, count: usize) -> String {
        let total = self.total();
        if total == 0 {
            return count.to_string();
        }
        #[allow(
            clippy::cast_precision_loss,
            reason = "ledger counts are far below 2^52, where f64 is exact"
        )]
        let share = count as f64 * 100.0 / total as f64;
        format!("{count} ({share:.1}%)")
    }
}

/// Everything the generated region shows.
#[derive(Debug, Clone)]
pub struct Facts {
    /// The upstream checkout.
    pub upstream: Upstream,
    /// Shares of `parity/surface.toml`.
    pub surface: Shares,
    /// Shares of `parity/tests.toml`.
    pub tests: Shares,
    /// Upstream exports not ported, with the reason.
    pub excluded_exports: Vec<(String, String)>,
    /// Upstream tests not ported, by reason key: the reason and the count.
    pub excluded_tests: Vec<(String, String, usize)>,
    /// Number of differential cases.
    pub cases: usize,
    /// Number of fields of `ClaudeAgentOptions` upstream.
    pub fields: usize,
    /// Fields some case sets.
    pub covered_fields: usize,
    /// Fields excluded from the cases with a reason.
    pub excluded_fields: usize,
    /// Fields some case sets whose effect no case observes, listed with a
    /// reason.
    pub unobservable_fields: usize,
    /// The ledger of differences.
    pub differences: differences::Ledger,
}

/// Reads the version, commit, and commit date of the upstream checkout.
pub fn upstream(checkout: &Path) -> Result<Upstream, String> {
    let manifest = checkout.join("pyproject.toml");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
    let version = text
        .lines()
        .find_map(|line| {
            let value = line.trim().strip_prefix("version")?.trim_start();
            let value = value.strip_prefix('=')?.trim();
            Some(value.trim_matches('"').to_owned())
        })
        .ok_or_else(|| format!("no `version = ...` line in {}", manifest.display()))?;
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["show", "--no-patch", "--format=%H %cs", "HEAD"])
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (commit, date) = text
        .trim()
        .split_once(' ')
        .filter(|_| output.status.success())
        .ok_or_else(|| {
            format!(
                "cannot read the commit of {}: {}",
                checkout.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })?;
    Ok(Upstream {
        version,
        commit: commit.to_owned(),
        date: date.to_owned(),
    })
}

/// The exclusions of a ledger, by reason key, with the reason and count.
pub fn exclusions_by_reason(ledger: &Ledger) -> Vec<(String, String, usize)> {
    let counts = ledger.counts().by_reason;
    ledger
        .exclusions
        .iter()
        .map(|(key, reason)| {
            (
                key.clone(),
                reason.clone(),
                counts.get(key).copied().unwrap_or_default(),
            )
        })
        .collect()
}

/// The excluded entries of a ledger with their reason.
pub fn excluded_items(ledger: &Ledger) -> Vec<(String, String)> {
    ledger
        .entries
        .iter()
        .filter(|(_, entry)| entry.state() == Some(State::Excluded))
        .map(|(item, entry)| {
            let reason = entry
                .excluded
                .as_ref()
                .and_then(|key| ledger.exclusions.get(key))
                .cloned()
                .unwrap_or_default();
            (item.clone(), reason)
        })
        .collect()
}

/// The form of a verb that agrees with a count.
fn verb(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn code(text: &str) -> String {
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

fn title(category: &str) -> (&'static str, &'static str) {
    match category {
        "deliberate" => (
            "Deliberate choices",
            "Rust chose another form on purpose; the CLI accepts both.",
        ),
        "rust-follows-cli" => (
            "Rust follows the CLI",
            "Rust does what the CLI does where Python does not.",
        ),
        "rust-more-robust" => (
            "Rust handles what stops Python",
            "Rust keeps going, or keeps the data, where Python fails or drops it.",
        ),
        "rust-stricter" => (
            "Rust refuses what Python accepts",
            "Rust returns an error for input Python lets through.",
        ),
        "python-defect" => (
            "Python defects the port does not copy",
            "Python gets this wrong; each entry cites the upstream lines at fault.",
        ),
        _ => ("Other", ""),
    }
}

fn wire_value(value: Option<&serde_json::Value>) -> String {
    value.map_or_else(|| "nothing".to_owned(), |value| code(&value.to_string()))
}

fn render_differences(page: &mut String, ledger: &differences::Ledger) {
    let observed = ledger
        .entries
        .iter()
        .filter(|entry| entry.table == "observed")
        .count();
    let wire = ledger.entries.len() - observed;
    let behavior = ledger.behavior.len();
    let _ = writeln!(
        page,
        "## Deliberately different\n\n{} differences are reviewed and accepted: {wire} on the \
         wire and {observed} in what the application observes, found by `cargo xtask \
         parity-diff`, and {behavior} in behavior, each shown by a Rust test. They are listed \
         in `parity/differences.toml`.\n",
        wire + observed + behavior
    );
    for category in CATEGORIES {
        let (heading, meaning) = title(category);
        let wire: Vec<_> = ledger
            .entries
            .iter()
            .filter(|entry| entry.category == category)
            .collect();
        let behavior: Vec<_> = ledger
            .behavior
            .iter()
            .filter(|entry| entry.category == category)
            .collect();
        let _ = writeln!(
            page,
            "### {heading} ({})\n\n{meaning}\n",
            wire.len() + behavior.len()
        );
        if wire.is_empty() && behavior.is_empty() {
            page.push_str("None.\n\n");
            continue;
        }
        page.push_str(
            "| Difference | Python | Rust | Reason | Shown by |\n|---|---|---|---|---|\n",
        );
        for entry in wire {
            let shown_by = if entry.case == "*" {
                "every differential case".to_owned()
            } else {
                format!("differential case {}", code(&entry.case))
            };
            let mut python = wire_value(entry.python.as_ref());
            if let Some(reference) = &entry.reference {
                let _ = write!(python, " (upstream {})", code(reference));
            }
            let place = if entry.table == "observed" {
                "observed by the application"
            } else {
                "on the wire"
            };
            let _ = writeln!(
                page,
                "| {} {place} | {} | {} | {} | {} |",
                cell(&code(&entry.location)),
                cell(&python),
                cell(&wire_value(entry.rust.as_ref())),
                cell(&entry.reason),
                cell(&shown_by)
            );
        }
        for entry in behavior {
            let _ = writeln!(
                page,
                "| {} | {} (upstream {}) | {} | {} | {} |",
                cell(&entry.summary),
                cell(&entry.python),
                cell(&code(&entry.reference)),
                cell(&entry.rust),
                cell(&entry.reason),
                cell(&code(&entry.test))
            );
        }
        page.push('\n');
    }
}

fn render_not_ported(page: &mut String, facts: &Facts) {
    page.push_str(
        "## Not ported\n\nUpstream items with no Rust counterpart, each with the reason of its \
         ledger. These are absences, not differences.\n\n",
    );
    if facts.excluded_exports.is_empty() {
        page.push_str("Every upstream export has a Rust counterpart.\n\n");
    } else {
        page.push_str("| Upstream export | Reason |\n|---|---|\n");
        for (name, reason) in &facts.excluded_exports {
            let _ = writeln!(page, "| {} | {} |", cell(&code(name)), cell(reason));
        }
        page.push('\n');
    }
    let _ = writeln!(
        page,
        "{} upstream tests are not ported, by reason:\n",
        facts.tests.excluded
    );
    page.push_str("| Reason | Tests | Why |\n|---|---|---|\n");
    for (key, reason, count) in &facts.excluded_tests {
        let _ = writeln!(page, "| {} | {count} | {} |", code(key), cell(reason));
    }
    page.push('\n');
}

fn render_unclassified(page: &mut String, ledger: &differences::Ledger) {
    let _ = writeln!(
        page,
        "## Not yet classified ({})\n\nDivergences the parity audit \
         (`docs/research/python-sdk-parity.md`) found that are neither reviewed above nor \
         closed. Each is listed in the `[unclassified]` table of `parity/differences.toml` \
         until it is.\n",
        ledger.unclassified.len()
    );
    if ledger.unclassified.is_empty() {
        page.push_str("None.\n");
        return;
    }
    page.push_str("| Audit section | Divergence |\n|---|---|\n");
    for entry in &ledger.unclassified {
        let _ = writeln!(
            page,
            "| {} | {} |",
            cell(&entry.source),
            cell(&entry.summary)
        );
    }
}

/// Renders the generated region, without its markers.
pub fn render(facts: &Facts) -> String {
    let Upstream {
        version,
        commit,
        date,
    } = &facts.upstream;
    let mut page = format!(
        "## What was compared\n\n\
         `claude-agent-rust-sdk` against the Python Claude Agent SDK {version}, at upstream commit \
         `{commit}`, committed on {date}. The upstream source is the submodule \
         `upstream/claude-agent-sdk-python`.\n\n\
         ### The ledgers\n\n\
         Every name of the upstream `__all__` is listed in `parity/surface.toml`, and every \
         upstream test in `parity/tests.toml`. An implemented entry names a Rust item of the same \
         kind; an adapted entry names one of a different kind, such as an enum variant where \
         Python has a class (the rule is at the top of `parity/surface.toml`).\n\n\
         | State | Upstream exports | Upstream tests |\n|---|---|---|\n"
    );
    let (surface, tests) = (&facts.surface, &facts.tests);
    for (label, exports, upstream_tests) in [
        ("Implemented", surface.implemented, tests.implemented),
        ("Adapted", surface.adapted, tests.adapted),
        ("Not ported", surface.excluded, tests.excluded),
        ("Pending", surface.pending, tests.pending),
    ] {
        let _ = writeln!(
            page,
            "| {label} | {} | {} |",
            surface.cell(exports),
            tests.cell(upstream_tests)
        );
    }
    let _ = writeln!(
        page,
        "| Total | {} | {} |\n",
        facts.surface.total(),
        facts.tests.total()
    );
    let neither = facts
        .fields
        .saturating_sub(facts.covered_fields + facts.excluded_fields);
    let _ = writeln!(
        page,
        "### The differential harness\n\n\
         `cargo xtask parity-diff` plays {} {} through both SDKs against `fake-claude` and \
         compares what each sent the CLI (the command line, the environment, and every frame) \
         and what each handed the application (every message, the outcome of every call, and \
         the arguments of every callback, as each driver maps them to JSON, with keys whose \
         value is null left out). \
         The cases set {} of the {} fields of `ClaudeAgentOptions`; {} {} excluded with a \
         reason in `parity/cases/coverage.toml`, and {} {} neither. Of the fields set, {} \
         {} an effect no case can observe, listed with the reason in the same file; \
         `cargo xtask parity-diff --coverage` checks that each of the others changes the \
         recording or the journal of a case when it is removed.\n",
        facts.cases,
        verb(facts.cases, "case", "cases"),
        facts.covered_fields,
        facts.fields,
        facts.excluded_fields,
        verb(facts.excluded_fields, "is", "are"),
        neither,
        verb(neither, "is", "are"),
        facts.unobservable_fields,
        verb(facts.unobservable_fields, "has", "have")
    );
    render_differences(&mut page, &facts.differences);
    render_not_ported(&mut page, facts);
    render_unclassified(&mut page, &facts.differences);
    page
}

/// Replaces the generated region of `document` with `generated`.
pub fn with_page(document: &str, generated: &str, file: &Path) -> Result<String, String> {
    super::with_region(document, PAGE_BEGIN, PAGE_END, generated, file)
}

/// Claims of the hand-written text that the counts do not support.
pub fn unsupported_claims(document: &str, file: &Path) -> Vec<String> {
    let hand_written = match (document.find(PAGE_BEGIN), document.find(PAGE_END)) {
        (Some(begin), Some(end)) if begin < end => {
            format!("{}{}", &document[..begin], &document[end..])
        }
        _ => document.to_owned(),
    };
    let lowercase = hand_written.to_lowercase();
    UNSUPPORTED_CLAIMS
        .iter()
        .filter(|claim| lowercase.contains(*claim))
        .map(|claim| {
            format!(
                "{}: the hand-written text claims `{claim}`; state measured counts in the generated region instead",
                file.display()
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        let upstream = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(upstream.path().join("src")).unwrap();
        std::fs::write(upstream.path().join("src/a.py"), "one\ntwo\nthree\n").unwrap();
        let differences = differences::parse(
            r#"
[wire.label]
case = "*"
location = "env CLAUDE_CODE_ENTRYPOINT"
python = '"sdk-py"'
rust = '"sdk-rs"'
category = "deliberate"
reason = "Each SDK names itself."

[wire.defect]
case = "one"
location = "arg --x"
python = '"a|b"'
category = "python-defect"
reason = "Python writes a pipe."
ref = "src/a.py:2"
anchor = "two"

[observed.unknown]
case = "two"
location = "journal message Unknown"
rust = '{"message":{"$type":"Unknown"}}'
category = "rust-more-robust"
reason = "Kept raw."

[behavior.kept]
summary = "An unknown message"
python = "Skipped."
rust = "Kept raw."
ref = "src/a.py:2-3"
anchor = "two"
category = "rust-more-robust"
reason = "No data is lost."
test = "tests/a.rs::kept"

[unclassified.budget]
source = "4.4 item 10"
summary = "Budget spelling."
"#,
            upstream.path(),
        )
        .unwrap();
        Facts {
            upstream: Upstream {
                version: "0.2.182".to_owned(),
                commit: "4720d36".to_owned(),
                date: "2026-11-10".to_owned(),
            },
            surface: Shares {
                implemented: 3,
                adapted: 1,
                excluded: 0,
                pending: 0,
            },
            tests: Shares {
                implemented: 6,
                adapted: 0,
                excluded: 2,
                pending: 0,
            },
            excluded_exports: Vec::new(),
            excluded_tests: vec![(
                "python-runtime".to_owned(),
                "No tokio counterpart.".to_owned(),
                2,
            )],
            cases: 7,
            fields: 10,
            covered_fields: 8,
            excluded_fields: 1,
            unobservable_fields: 2,
            differences,
        }
    }

    #[test]
    fn the_region_states_the_version_counts_coverage_and_one_table_per_category() {
        let page = render(&facts());
        for expected in [
            "Python Claude Agent SDK 0.2.182, at upstream commit `4720d36`, committed on 2026-11-10",
            "| Implemented | 3 (75.0%) | 6 (75.0%) |",
            "| Adapted | 1 (25.0%) | 0 (0.0%) |",
            "| Not ported | 0 (0.0%) | 2 (25.0%) |",
            "| Total | 4 | 8 |",
            "plays 7 cases",
            "The cases set 8 of the 10 fields of `ClaudeAgentOptions`; 1 is excluded",
            "and 1 is neither. Of the fields set, 2 have an effect no case can observe",
            "4 differences are reviewed and accepted: 2 on the wire and 1 in what the application observes",
            "| `journal message Unknown` observed by the application | nothing | `{\"message\":{\"$type\":\"Unknown\"}}` | Kept raw. | differential case `two` |",
            "### Deliberate choices (1)",
            "| `env CLAUDE_CODE_ENTRYPOINT` on the wire | `\"sdk-py\"` | `\"sdk-rs\"` | Each SDK names itself. | every differential case |",
            "### Rust follows the CLI (0)\n\nRust does what the CLI does where Python does not.\n\nNone.",
            "| An unknown message | Skipped. (upstream `src/a.py:2-3`) | Kept raw. | No data is lost. | `tests/a.rs::kept` |",
            "| `arg --x` on the wire | `\"a\\|b\"` (upstream `src/a.py:2`) | nothing | Python writes a pipe. | differential case `one` |",
            "| `python-runtime` | 2 | No tokio counterpart. |",
            "Every upstream export has a Rust counterpart.",
            "## Not yet classified (1)",
            "| 4.4 item 10 | Budget spelling. |",
        ] {
            assert!(page.contains(expected), "no `{expected}` in:\n{page}");
        }
        let headings: Vec<&str> = page
            .lines()
            .filter(|line| line.starts_with("### ") && line.contains('('))
            .collect();
        assert_eq!(headings.len(), CATEGORIES.len(), "{headings:?}");
        for line in page.lines().filter(|line| line.starts_with('|')) {
            assert!(line.ends_with('|'), "{line}");
        }
    }

    #[test]
    fn a_hand_written_claim_of_full_parity_is_reported_and_a_generated_one_is_not() {
        let file = Path::new("docs/parity.md");
        let clean = format!("# Parity\n{PAGE_BEGIN}\n| Adapted | 0 (100.0%) |\n{PAGE_END}\nEnd.\n");
        assert!(unsupported_claims(&clean, file).is_empty());
        let boastful =
            format!("# Full Parity\n{PAGE_BEGIN}\n{PAGE_END}\nRust covers 100% of it.\n");
        let problems = unsupported_claims(&boastful, file);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("`100%`"), "{problems:?}");
        assert!(problems[1].contains("`full parity`"), "{problems:?}");
    }

    #[test]
    fn a_code_span_holding_a_backtick_uses_a_double_fence() {
        assert_eq!(code("a`b"), "`` a`b ``");
        assert_eq!(code("ab"), "`ab`");
    }
}
