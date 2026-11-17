//! `cargo xtask parity-diff --coverage`: which option fields the cases set.
//!
//! The fields are read from the `ClaudeAgentOptions` class of the upstream
//! `types.py`, so a field added upstream appears at the next submodule
//! bump. Every field must be set by at least one case, or be listed in the
//! `[excluded]` table of `parity/cases/coverage.toml` with the reason no
//! case can set it. The command fails on a field neither set nor excluded,
//! on an exclusion of a field a case now sets (stale), on an exclusion of a
//! name that is not a field, and on a case that sets a name that is not a
//! field.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use super::case;
use crate::parity::python;

const OPTIONS_CLASS: &str = "ClaudeAgentOptions";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusions {
    #[serde(default)]
    excluded: BTreeMap<String, String>,
}

/// The fields of `ClaudeAgentOptions` in the upstream `types.py`.
pub fn upstream_fields(upstream: &Path) -> Result<BTreeSet<String>, String> {
    let types = upstream
        .join("src")
        .join("claude_agent_sdk")
        .join("types.py");
    let source = std::fs::read_to_string(&types)
        .map_err(|error| format!("cannot read {}: {error}", types.display()))?;
    let fields = python::class_fields(&source, OPTIONS_CLASS);
    if fields.is_empty() {
        return Err(format!(
            "no field of {OPTIONS_CLASS} found in {}",
            types.display()
        ));
    }
    Ok(fields)
}

/// The report of one coverage check.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// One line per field.
    pub lines: Vec<String>,
    /// Problems that fail the check.
    pub problems: Vec<String>,
    /// Number of fields some case sets.
    pub covered: usize,
    /// Number of fields excluded with a reason.
    pub excluded: usize,
}

/// Checks which fields the cases set against the exclusions.
pub fn check(
    fields: &BTreeSet<String>,
    cases: &[(String, Vec<String>)],
    exclusions: &BTreeMap<String, String>,
) -> Report {
    let mut setters: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut report = Report::default();
    for (name, options) in cases {
        for option in options {
            if fields.contains(option) {
                setters
                    .entry(option.as_str())
                    .or_default()
                    .push(name.as_str());
            } else {
                report.problems.push(format!(
                    "case {name} sets {option}, which is not a field of {OPTIONS_CLASS}"
                ));
            }
        }
    }
    for (field, reason) in exclusions {
        if !fields.contains(field) {
            report.problems.push(format!(
                "coverage.toml excludes {field}, which is not a field of {OPTIONS_CLASS}"
            ));
        } else if reason.trim().is_empty() {
            report
                .problems
                .push(format!("coverage.toml excludes {field} without a reason"));
        }
    }
    for field in fields {
        match (setters.get(field.as_str()), exclusions.get(field)) {
            (Some(cases), None) => {
                report.covered += 1;
                report
                    .lines
                    .push(format!("{field:<28} {}", cases.join(", ")));
            }
            (None, Some(reason)) => {
                report.excluded += 1;
                report.lines.push(format!("{field:<28} excluded: {reason}"));
            }
            (Some(cases), Some(_)) => {
                report.covered += 1;
                report
                    .lines
                    .push(format!("{field:<28} {}", cases.join(", ")));
                report.problems.push(format!(
                    "stale exclusion: coverage.toml excludes {field}, which case {} sets",
                    cases.join(", ")
                ));
            }
            (None, None) => {
                report.lines.push(format!("{field:<28} NOT COVERED"));
                report.problems.push(format!(
                    "{field} is set by no case and not excluded in coverage.toml"
                ));
            }
        }
    }
    report
}

/// The coverage of the cases of `parity`, measured against the upstream
/// checkout.
#[derive(Debug)]
pub struct Measure {
    /// The fields of `ClaudeAgentOptions` upstream.
    pub fields: BTreeSet<String>,
    /// Number of cases.
    pub cases: usize,
    /// The check of the cases against the exclusions.
    pub report: Report,
}

/// Measures which fields the cases of `parity` set.
pub fn measure(upstream: &Path, parity: &Path) -> Result<Measure, String> {
    let fields = upstream_fields(upstream)?;
    let cases = case::load_all(
        &parity.join("cases"),
        &parity.join("scenarios"),
        &parity.join("case.schema.json"),
    )?;
    let exclusions_path = parity.join("cases").join("coverage.toml");
    let text = std::fs::read_to_string(&exclusions_path)
        .map_err(|error| format!("cannot read {}: {error}", exclusions_path.display()))?;
    let exclusions: Exclusions =
        toml::from_str(&text).map_err(|error| format!("{}: {error}", exclusions_path.display()))?;
    let options: Vec<(String, Vec<String>)> = cases
        .iter()
        .map(|case| (case.name.clone(), case.option_names()))
        .collect();
    let report = check(&fields, &options, &exclusions.excluded);
    Ok(Measure {
        fields,
        cases: cases.len(),
        report,
    })
}

/// Runs the coverage check over the cases of `parity`.
pub fn run(upstream: &Path, parity: &Path) -> Result<bool, String> {
    let Measure {
        fields,
        cases,
        report,
    } = measure(upstream, parity)?;
    for line in &report.lines {
        println!("{line}");
    }
    for problem in &report.problems {
        println!("error: {problem}");
    }
    println!(
        "{} fields: {} covered, {} excluded; {} cases",
        fields.len(),
        report.covered,
        report.excluded,
        cases
    );
    Ok(report.problems.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::check;

    fn fields(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn cases(entries: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        entries
            .iter()
            .map(|(name, options)| {
                (
                    (*name).to_owned(),
                    options.iter().map(|option| (*option).to_owned()).collect(),
                )
            })
            .collect()
    }

    fn exclusions(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, reason)| ((*name).to_owned(), (*reason).to_owned()))
            .collect()
    }

    #[test]
    fn every_field_is_covered_or_excluded() {
        let report = check(
            &fields(&["model", "stderr", "tools"]),
            &cases(&[("a", &["model"]), ("b", &["model", "tools"])]),
            &exclusions(&[("stderr", "a callback, never sent as data")]),
        );
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!((report.covered, report.excluded), (2, 1));
        assert!(report.lines[0].starts_with("model") && report.lines[0].ends_with("a, b"));
        assert!(report.lines[1].contains("excluded: a callback"));
    }

    #[test]
    fn a_new_upstream_field_fails_naming_it() {
        let report = check(
            &fields(&["model", "new_field"]),
            &cases(&[("a", &["model"])]),
            &exclusions(&[]),
        );
        assert_eq!(
            report.problems,
            ["new_field is set by no case and not excluded in coverage.toml"]
        );
    }

    #[test]
    fn stale_unknown_and_unreasoned_exclusions_fail() {
        let report = check(
            &fields(&["model", "tools"]),
            &cases(&[("a", &["model", "tool"]), ("b", &["tools"])]),
            &exclusions(&[("model", "never"), ("gone", "removed"), ("tools", " ")]),
        );
        assert_eq!(
            report.problems,
            [
                "case a sets tool, which is not a field of ClaudeAgentOptions",
                "coverage.toml excludes gone, which is not a field of ClaudeAgentOptions",
                "coverage.toml excludes tools without a reason",
                "stale exclusion: coverage.toml excludes model, which case a sets",
                "stale exclusion: coverage.toml excludes tools, which case b sets",
            ]
        );
    }
}
