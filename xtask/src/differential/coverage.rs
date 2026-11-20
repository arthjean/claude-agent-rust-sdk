//! `cargo xtask parity-diff --coverage`: which option fields the cases set,
//! and in which cases their effect is observable.
//!
//! The fields are read from the `ClaudeAgentOptions` class of the upstream
//! `types.py`, so a field added upstream appears at the next submodule
//! bump. Every field must be set by at least one case, or be listed in the
//! `[excluded]` table of `parity/cases/coverage.toml` with the reason no
//! case can set it. The command fails on a field neither set nor excluded,
//! on an exclusion of a field a case now sets (stale), on an exclusion of a
//! name that is not a field, and on a case that sets a name that is not a
//! field.
//!
//! A field a case sets is not covered by that alone: its effect must reach
//! the recording or the journal, which `observe` measures by playing the
//! case without the field. A field set by cases but observable in none
//! fails, unless the `[unobservable]` table lists it with the reason no
//! case can observe it; such an entry is stale once a case observes the
//! field. The parity page counts the fields set; only this command runs
//! the cases to tell them apart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Deserialize;

use super::case;
use super::observe::Observed;
use crate::parity::python;

const OPTIONS_CLASS: &str = "ClaudeAgentOptions";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exclusions {
    #[serde(default)]
    excluded: BTreeMap<String, String>,
    #[serde(default)]
    unobservable: BTreeMap<String, String>,
}

/// The tables of `coverage.toml`.
#[derive(Debug, Default)]
pub struct Listed {
    /// Fields no case can set, with the reason.
    pub excluded: BTreeMap<String, String>,
    /// Fields set by cases whose effect no case can observe, with the
    /// reason.
    pub unobservable: BTreeMap<String, String>,
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
    /// Number of fields observable in some case, when observability was
    /// measured.
    pub observable: usize,
    /// Number of fields some case sets that `coverage.toml` lists as
    /// unobservable.
    pub unobservable: usize,
    /// Number of fields excluded with a reason.
    pub excluded: usize,
}

fn names(cases: &BTreeSet<&str>) -> String {
    cases.iter().copied().collect::<Vec<_>>().join(", ")
}

/// The line and the problem of a field some case sets. `observed` holds
/// the cases in which the field is observable, when that was measured.
fn set_field(
    field: &str,
    setters: &BTreeSet<&str>,
    observed: Option<&BTreeSet<String>>,
    unobservable: Option<&String>,
    report: &mut Report,
) {
    let Some(observed) = observed else {
        report.lines.push(format!("{field:<28} {}", names(setters)));
        return;
    };
    let seen: BTreeSet<&str> = observed.iter().map(String::as_str).collect();
    let without_effect: BTreeSet<&str> = setters.difference(&seen).copied().collect();
    match (seen.is_empty(), unobservable) {
        (false, listed) => {
            report.observable += 1;
            let mut line = format!("{field:<28} observable in {}", names(&seen));
            if !without_effect.is_empty() {
                line.push_str(&format!(
                    "; set without effect in {}",
                    names(&without_effect)
                ));
            }
            report.lines.push(line);
            if listed.is_some() {
                report.problems.push(format!(
                    "stale entry: coverage.toml lists {field} as unobservable, which case {} observes",
                    names(&seen)
                ));
            }
        }
        (true, Some(reason)) => report.lines.push(format!(
            "{field:<28} set without effect in {}: {reason}",
            names(setters)
        )),
        (true, None) => {
            report.lines.push(format!(
                "{field:<28} SET WITHOUT EFFECT in {}",
                names(setters)
            ));
            report.problems.push(format!(
                "{field} is set by {} but its effect reaches neither the recording nor the \
                 journal: make a case observe it, or list it under [unobservable] in coverage.toml",
                names(setters)
            ));
        }
    }
}

/// Checks which fields the cases set against the tables of
/// `coverage.toml`. With `observed`, the cases in which each field is
/// observable, a field set but observable in no case is a problem unless
/// it is listed as unobservable.
pub fn check(
    fields: &BTreeSet<String>,
    cases: &[(String, Vec<String>)],
    listed: &Listed,
    observed: Option<&BTreeMap<String, BTreeSet<String>>>,
) -> Report {
    let mut setters: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut report = Report::default();
    for (name, options) in cases {
        for option in options {
            if fields.contains(option) {
                setters
                    .entry(option.as_str())
                    .or_default()
                    .insert(name.as_str());
            } else {
                report.problems.push(format!(
                    "case {name} sets {option}, which is not a field of {OPTIONS_CLASS}"
                ));
            }
        }
    }
    for (table, entries) in [
        ("excludes", &listed.excluded),
        ("lists as unobservable", &listed.unobservable),
    ] {
        for (field, reason) in entries {
            if !fields.contains(field) {
                report.problems.push(format!(
                    "coverage.toml {table} {field}, which is not a field of {OPTIONS_CLASS}"
                ));
            } else if reason.trim().is_empty() {
                report
                    .problems
                    .push(format!("coverage.toml {table} {field} without a reason"));
            }
        }
    }
    let empty = BTreeSet::new();
    for field in fields {
        let observed_cases = observed.map(|observed| observed.get(field).unwrap_or(&empty));
        let unobservable = listed.unobservable.get(field);
        match (setters.get(field.as_str()), listed.excluded.get(field)) {
            (Some(cases), None) => {
                report.covered += 1;
                set_field(field, cases, observed_cases, unobservable, &mut report);
            }
            (None, Some(reason)) => {
                report.excluded += 1;
                report.lines.push(format!("{field:<28} excluded: {reason}"));
            }
            (Some(cases), Some(_)) => {
                report.covered += 1;
                set_field(field, cases, observed_cases, unobservable, &mut report);
                report.problems.push(format!(
                    "stale exclusion: coverage.toml excludes {field}, which case {} sets",
                    names(cases)
                ));
            }
            (None, None) => {
                report.lines.push(format!("{field:<28} NOT COVERED"));
                report.problems.push(format!(
                    "{field} is set by no case and not excluded in coverage.toml"
                ));
            }
        }
        if unobservable.is_some() && setters.contains_key(field.as_str()) {
            report.unobservable += 1;
        }
        if unobservable.is_some() && !setters.contains_key(field.as_str()) {
            report.problems.push(format!(
                "coverage.toml lists {field} as unobservable, but no case sets it: exclude it instead"
            ));
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
    /// The check of the cases against the tables of `coverage.toml`.
    pub report: Report,
}

/// Measures which fields the cases of `parity` set, without running them.
pub fn measure(upstream: &Path, parity: &Path) -> Result<Measure, String> {
    measure_with(upstream, parity, None)
}

fn measure_with(
    upstream: &Path,
    parity: &Path,
    observed: Option<&BTreeMap<String, BTreeSet<String>>>,
) -> Result<Measure, String> {
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
    let listed = Listed {
        excluded: exclusions.excluded,
        unobservable: exclusions.unobservable,
    };
    let options: Vec<(String, Vec<String>)> = cases
        .iter()
        .map(|case| (case.name.clone(), case.option_names()))
        .collect();
    let report = check(&fields, &options, &listed, observed);
    Ok(Measure {
        fields,
        cases: cases.len(),
        report,
    })
}

/// Runs the coverage check over the cases of `parity`, with `observed`,
/// the fields each case makes observable.
pub fn run(upstream: &Path, parity: &Path, observed: &Observed) -> Result<bool, String> {
    let Measure {
        fields,
        cases,
        report,
    } = measure_with(upstream, parity, Some(&observed.fields))?;
    for line in &report.lines {
        println!("{line}");
    }
    for problem in report.problems.iter().chain(&observed.problems) {
        println!("error: {problem}");
    }
    println!(
        "{} fields: {} set by a case, {} of them observable, {} excluded; {} cases",
        fields.len(),
        report.covered,
        report.observable,
        report.excluded,
        cases
    );
    Ok(report.problems.is_empty() && observed.problems.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::{Listed, check};

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

    fn table(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, reason)| ((*name).to_owned(), (*reason).to_owned()))
            .collect()
    }

    fn exclusions(entries: &[(&str, &str)]) -> Listed {
        Listed {
            excluded: table(entries),
            unobservable: BTreeMap::new(),
        }
    }

    fn observed(entries: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
        entries
            .iter()
            .map(|(field, cases)| {
                (
                    (*field).to_owned(),
                    cases.iter().map(|case| (*case).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn every_field_is_covered_or_excluded() {
        let report = check(
            &fields(&["model", "stderr", "tools"]),
            &cases(&[("a", &["model"]), ("b", &["model", "tools"])]),
            &exclusions(&[("stderr", "a callback, never sent as data")]),
            None,
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
            None,
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
            None,
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

    #[test]
    fn a_field_set_without_effect_fails_unless_listed_as_unobservable() {
        let listed = Listed {
            excluded: BTreeMap::new(),
            unobservable: table(&[("max_buffer_size", "no message reaches the limit")]),
        };
        let report = check(
            &fields(&["model", "max_buffer_size", "stderr"]),
            &cases(&[
                ("a", &["model", "stderr", "max_buffer_size"]),
                ("b", &["model"]),
            ]),
            &listed,
            Some(&observed(&[("model", &["a"])])),
        );
        assert_eq!(
            report.lines,
            [
                "max_buffer_size              set without effect in a: no message reaches the limit",
                "model                        observable in a; set without effect in b",
                "stderr                       SET WITHOUT EFFECT in a",
            ]
        );
        assert_eq!(
            report.problems,
            [
                "stderr is set by a but its effect reaches neither the recording nor the journal: \
                 make a case observe it, or list it under [unobservable] in coverage.toml"
            ]
        );
        assert_eq!(
            (report.covered, report.observable, report.unobservable),
            (3, 1, 1)
        );
    }

    #[test]
    fn stale_and_misplaced_unobservable_entries_fail() {
        let listed = Listed {
            excluded: BTreeMap::new(),
            unobservable: table(&[("model", "never"), ("tools", "unset"), ("gone", "x")]),
        };
        let report = check(
            &fields(&["model", "tools"]),
            &cases(&[("a", &["model"])]),
            &listed,
            Some(&observed(&[("model", &["a"])])),
        );
        assert_eq!(
            report.problems,
            [
                "coverage.toml lists as unobservable gone, which is not a field of ClaudeAgentOptions",
                "stale entry: coverage.toml lists model as unobservable, which case a observes",
                "tools is set by no case and not excluded in coverage.toml",
                "coverage.toml lists tools as unobservable, but no case sets it: exclude it instead",
            ]
        );
    }
}
