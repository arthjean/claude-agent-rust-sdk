//! Which option fields a case makes observable.
//!
//! A case sets a field; the field is observable in that case when its
//! effect reaches what the harness compares: the recording of `fake-claude`
//! or the journal of a driver. The measure is direct. Each case is played
//! twice per side as is, which must give the same recording and journal
//! (otherwise the case is not deterministic and the measure means
//! nothing); then once per side for each field it sets, with that field
//! removed. The field is observable when, on either side, the run without
//! it differs from the run with it, or no longer runs at all: a driver
//! that ignored the field would then fail the comparison. A field whose
//! removal changes nothing on both sides is only set: no case can tell
//! whether an SDK honors it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use super::case::Case;
use super::compare::{self, Difference, Normalizer, show};
use super::journal;
use super::runner::{self, Driver, Outcome};

/// What one run of one side produced.
struct Run {
    recording: compare::Recording,
    journal: journal::Journal,
    work: Vec<String>,
}

/// The fields each case makes observable, and the cases the measure could
/// not use.
#[derive(Debug, Default)]
pub struct Observed {
    /// For each field, the cases in which it is observable.
    pub fields: BTreeMap<String, BTreeSet<String>>,
    /// Cases that do not run, or do not give the same result twice.
    pub problems: Vec<String>,
}

struct Probe<'a> {
    drivers: &'a [Driver; 2],
    cli: &'a Path,
    directory: &'a Path,
    timeout: Duration,
}

impl Probe<'_> {
    fn run(&self, driver: &Driver, case: &Case, label: &str) -> Result<Run, String> {
        let directory = self
            .directory
            .join(&case.name)
            .join(label)
            .join(driver.side);
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        match runner::run(driver, case, self.cli, &directory, self.timeout) {
            Outcome::Ran {
                recording,
                journal,
                work,
            } => Ok(Run {
                recording,
                journal,
                work,
            }),
            Outcome::Unsupported(item) => Err(format!("unsupported {item}")),
            Outcome::Timeout => Err("timeout".to_owned()),
            Outcome::Failed(reason) => Err(reason),
        }
    }

    /// The first difference between two runs of the same side, if any.
    fn first_difference(&self, left: &Run, right: &Run, case: &Case) -> Option<Difference> {
        let cli = vec![self.cli.to_string_lossy().into_owned()];
        let left_normalizer = Normalizer::new(&left.work, &cli);
        let right_normalizer = Normalizer::new(&right.work, &cli);
        compare::compare(
            (&left.recording, &left_normalizer),
            (&right.recording, &right_normalizer),
            &case.unordered,
        )
        .into_iter()
        .chain(journal::compare(
            (&left.journal, &left_normalizer),
            (&right.journal, &right_normalizer),
        ))
        .next()
    }

    /// The runs of a case as is, one per side, after checking that a
    /// second run of each side gives the same result.
    fn baseline(&self, case: &Case) -> Result<Vec<Run>, String> {
        let mut runs = Vec::new();
        for driver in self.drivers {
            let first = self.run(driver, case, "as-is").map_err(|reason| {
                format!(
                    "case {} does not run ({}: {reason})",
                    case.name, driver.side
                )
            })?;
            let second = self.run(driver, case, "as-is-again").map_err(|reason| {
                format!(
                    "case {} does not run ({}: {reason})",
                    case.name, driver.side
                )
            })?;
            if let Some(difference) = self.first_difference(&first, &second, case) {
                return Err(format!(
                    "case {} is not deterministic ({}: {} is {} then {})",
                    case.name,
                    driver.side,
                    difference.location,
                    show(difference.python.as_ref()),
                    show(difference.rust.as_ref())
                ));
            }
            runs.push(first);
        }
        Ok(runs)
    }

    /// Whether removing `field` from `case` changes what either side
    /// produces.
    fn observable(&self, case: &Case, field: &str, baseline: &[Run]) -> bool {
        let Some(probe) = case.without_option(field) else {
            return false;
        };
        self.drivers.iter().zip(baseline).any(|(driver, base)| {
            match self.run(driver, &probe, &format!("without-{field}")) {
                Ok(run) => self.first_difference(base, &run, case).is_some(),
                Err(_) => true,
            }
        })
    }
}

/// Measures, for every case, which of the fields it sets are observable.
/// Runs go under `directory`.
pub fn measure(
    cases: &[Case],
    drivers: &[Driver; 2],
    cli: &Path,
    directory: &Path,
    timeout: Duration,
) -> Observed {
    let probe = Probe {
        drivers,
        cli,
        directory,
        timeout,
    };
    let mut observed = Observed::default();
    for case in cases {
        let baseline = match probe.baseline(case) {
            Ok(baseline) => baseline,
            Err(problem) => {
                observed.problems.push(problem);
                continue;
            }
        };
        for field in case.option_names() {
            if probe.observable(case, &field, &baseline) {
                observed
                    .fields
                    .entry(field)
                    .or_default()
                    .insert(case.name.clone());
            }
        }
    }
    observed
}
