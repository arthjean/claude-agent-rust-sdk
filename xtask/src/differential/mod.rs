//! `cargo xtask parity-diff`: the differential parity harness.
//!
//! Every case of `parity/cases` is played by two drivers, one per SDK,
//! against `fake-claude`, which records what it received. The two
//! recordings are compared after normalization (`compare`), and every
//! difference must be excused by an entry of `parity/differences.toml`
//! (`ledger`). The run fails on a difference no entry covers, on an entry
//! that covers nothing, on a case a driver does not support, on a driver
//! that fails or times out, and when Python or a dependency of the Python
//! driver is missing; it never reports a match for a side that did not
//! run. `parity/README.md` documents the format and the rules.
//!
//! Options: `--python <path>` (default: `PARITY_PYTHON`, then `python3`,
//! then `python`), `--timeout <seconds>` (default 30), `--repeat <n>` (run
//! the corpus `n` times and require identical results), `--coverage`
//! (report which option fields the cases set, see `coverage`),
//! `--upstream <path>`, `--parity <path>`, and case names to run only those
//! cases (the stale-entry check is then skipped).

pub(crate) mod case;
mod compare;
pub(crate) mod coverage;
pub(crate) mod ledger;
mod requirements;
mod runner;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::Value;

use case::Case;
use compare::{Difference, Normalizer, show};
use ledger::Ledger;
use runner::{Driver, Outcome};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MINIMUM_PYTHON: (u32, u32) = (3, 10);
const PYTHON_IMPORTS: &str = "import anyio, sniffio, mcp, jsonschema, claude_agent_sdk";

struct Settings {
    root: PathBuf,
    upstream: PathBuf,
    parity: PathBuf,
    python: Option<PathBuf>,
    timeout: Duration,
    repeat: usize,
    coverage: bool,
    check_requirements: bool,
    cases: Vec<String>,
}

fn parse(arguments: &[String]) -> Result<Settings, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let mut settings = Settings {
        upstream: root.join("upstream").join("claude-agent-sdk-python"),
        parity: root.join("parity"),
        root,
        python: None,
        timeout: DEFAULT_TIMEOUT,
        repeat: 1,
        coverage: false,
        check_requirements: false,
        cases: Vec::new(),
    };
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match argument.as_str() {
            "--upstream" => settings.upstream = PathBuf::from(value("--upstream")?),
            "--parity" => settings.parity = PathBuf::from(value("--parity")?),
            "--python" => settings.python = Some(PathBuf::from(value("--python")?)),
            "--timeout" => {
                let seconds: u64 = value("--timeout")?
                    .parse()
                    .map_err(|_| "--timeout needs a number of seconds".to_owned())?;
                settings.timeout = Duration::from_secs(seconds);
            }
            "--repeat" => {
                settings.repeat = value("--repeat")?
                    .parse()
                    .ok()
                    .filter(|count| *count > 0)
                    .ok_or_else(|| "--repeat needs a positive number".to_owned())?;
            }
            "--coverage" => settings.coverage = true,
            "--check-requirements" => settings.check_requirements = true,
            option if option.starts_with("--") => return Err(format!("unknown option {option}")),
            name => settings.cases.push(name.to_owned()),
        }
    }
    Ok(settings)
}

fn check_submodule(upstream: &Path) -> Result<(), String> {
    if upstream
        .join("src")
        .join("claude_agent_sdk")
        .join("__init__.py")
        .is_file()
    {
        Ok(())
    } else {
        Err(format!(
            "upstream submodule is empty: run git submodule update --init ({} has no src/claude_agent_sdk)",
            upstream.display()
        ))
    }
}

fn python_version(python: &Path) -> Option<(u32, u32)> {
    let output = Command::new(python)
        .args(["-c", "import sys; print('%d.%d' % sys.version_info[:2])"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn find_python(settings: &Settings) -> Result<PathBuf, String> {
    let candidates: Vec<PathBuf> = match (&settings.python, std::env::var_os("PARITY_PYTHON")) {
        (Some(python), _) => vec![python.clone()],
        (None, Some(python)) => vec![PathBuf::from(python)],
        (None, None) => vec![PathBuf::from("python3"), PathBuf::from("python")],
    };
    for candidate in &candidates {
        if python_version(candidate).is_some_and(|version| version >= MINIMUM_PYTHON) {
            return Ok(candidate.clone());
        }
    }
    let tried: Vec<String> = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    Err(format!(
        "python {}.{} or later not found (tried {})",
        MINIMUM_PYTHON.0,
        MINIMUM_PYTHON.1,
        tried.join(", ")
    ))
}

fn python_driver(settings: &Settings, python: PathBuf) -> Driver {
    Driver {
        side: "python",
        program: python,
        arguments: vec![
            settings
                .parity
                .join("drivers")
                .join("python_driver.py")
                .into_os_string(),
        ],
        env: vec![(
            "PYTHONPATH".to_owned(),
            settings.upstream.join("src").into_os_string(),
        )],
    }
}

fn check_python_imports(driver: &Driver) -> Result<(), String> {
    let mut command = Command::new(&driver.program);
    command.args(["-c", PYTHON_IMPORTS]).env_clear();
    for name in ["PATH", "SYSTEMROOT", "HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    for (name, value) in &driver.env {
        command.env(name, value);
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {}: {error}", driver.program.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no error output");
    Err(format!(
        "the Python driver cannot import its dependencies with {}: {reason}; install the packages of \
         parity/requirements.txt",
        driver.program.display()
    ))
}

fn build(root: &Path) -> Result<(PathBuf, PathBuf), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .current_dir(root)
        .args([
            "build",
            "--package",
            "fake-claude",
            "--package",
            "parity-driver",
            "--message-format=json",
        ])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !output.status.success() {
        return Err("building fake-claude and parity-driver failed".to_owned());
    }
    let executable = |target: &str| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|message| {
                message["reason"] == "compiler-artifact" && message["target"]["name"] == target
            })
            .find_map(|message| message["executable"].as_str().map(PathBuf::from))
            .ok_or_else(|| format!("cargo did not report the {target} executable"))
    };
    Ok((executable("fake-claude")?, executable("parity-driver")?))
}

enum Verdict {
    Match { known: usize },
    Differ(Vec<Difference>),
    Unsupported { side: &'static str, item: String },
    Timeout { side: &'static str },
    Failed { side: &'static str, reason: String },
}

impl Verdict {
    fn line(&self, name: &str) -> String {
        let mut line = String::new();
        match self {
            Self::Match { known: 0 } => {
                let _ = write!(line, "match       {name}");
            }
            Self::Match { known } => {
                let _ = write!(line, "match       {name} ({known} known differences)");
            }
            Self::Differ(differences) => {
                let _ = write!(line, "differ      {name}");
                for difference in differences {
                    let _ = write!(
                        line,
                        "\n    {}: python={} rust={}",
                        difference.location,
                        show(difference.python.as_ref()),
                        show(difference.rust.as_ref())
                    );
                }
            }
            Self::Unsupported { side, item } => {
                let _ = write!(line, "unsupported {name} ({side}: {item})");
            }
            Self::Timeout { side } => {
                let _ = write!(line, "timeout     {name} ({side} did not finish)");
            }
            Self::Failed { side, reason } => {
                let _ = write!(line, "failed      {name} ({side}: {reason})");
            }
        }
        line
    }

    fn passed(&self) -> bool {
        matches!(self, Self::Match { .. })
    }
}

struct Corpus<'a> {
    cases: &'a [Case],
    drivers: [Driver; 2],
    cli: PathBuf,
    ledger: &'a Ledger,
    timeout: Duration,
}

fn side_outcome(
    verdict_side: &'static str,
    outcome: Outcome,
) -> Result<(compare::Recording, Vec<String>), Verdict> {
    match outcome {
        Outcome::Ran { recording, work } => Ok((recording, work)),
        Outcome::Unsupported(item) => Err(Verdict::Unsupported {
            side: verdict_side,
            item,
        }),
        Outcome::Timeout => Err(Verdict::Timeout { side: verdict_side }),
        Outcome::Failed(reason) => Err(Verdict::Failed {
            side: verdict_side,
            reason,
        }),
    }
}

impl Corpus<'_> {
    fn run_side(
        &self,
        driver: &Driver,
        case: &Case,
        directory: &Path,
    ) -> Result<(compare::Recording, Vec<String>), Verdict> {
        let side_directory = directory.join(&case.name).join(driver.side);
        if let Err(error) = std::fs::create_dir_all(&side_directory) {
            return Err(Verdict::Failed {
                side: driver.side,
                reason: format!("cannot create {}: {error}", side_directory.display()),
            });
        }
        side_outcome(
            driver.side,
            runner::run(driver, case, &self.cli, &side_directory, self.timeout),
        )
    }

    fn run_case(&self, case: &Case, directory: &Path, used: &mut BTreeSet<String>) -> Verdict {
        let [python_driver, rust_driver] = &self.drivers;
        let (python, python_work) = match self.run_side(python_driver, case, directory) {
            Ok(side) => side,
            Err(verdict) => return verdict,
        };
        let (rust, rust_work) = match self.run_side(rust_driver, case, directory) {
            Ok(side) => side,
            Err(verdict) => return verdict,
        };
        let cli = vec![self.cli.to_string_lossy().into_owned()];
        let differences = compare::compare(
            (&python, &Normalizer::new(&python_work, &cli)),
            (&rust, &Normalizer::new(&rust_work, &cli)),
            &case.unordered,
        );
        let mut unexplained = Vec::new();
        let mut known = 0;
        for difference in differences {
            let matching: Vec<&ledger::Entry> = self
                .ledger
                .entries
                .iter()
                .filter(|entry| entry.matches(&case.name, &difference))
                .collect();
            if matching.is_empty() {
                unexplained.push(difference);
            } else {
                known += 1;
                used.extend(matching.iter().map(|entry| entry.id.clone()));
            }
        }
        if unexplained.is_empty() {
            Verdict::Match { known }
        } else {
            Verdict::Differ(unexplained)
        }
    }

    fn run(&self, directory: &Path) -> (Vec<(String, Verdict)>, BTreeSet<String>) {
        let mut used = BTreeSet::new();
        let verdicts = self
            .cases
            .iter()
            .map(|case| (case.name.clone(), self.run_case(case, directory, &mut used)))
            .collect();
        (verdicts, used)
    }
}

fn prepare_directory(root: &Path) -> Result<PathBuf, String> {
    let directory = root.join("target").join("parity-diff");
    if directory.exists() {
        std::fs::remove_dir_all(&directory)
            .map_err(|error| format!("cannot clear {}: {error}", directory.display()))?;
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    Ok(directory)
}

fn run_corpus(settings: &Settings) -> Result<bool, String> {
    let ledger = ledger::load(
        &settings.parity.join("differences.toml"),
        &settings.upstream,
        &settings.root,
    )?;
    let all = case::load_all(
        &settings.parity.join("cases"),
        &settings.parity.join("scenarios"),
        &settings.parity.join("case.schema.json"),
    )?;
    for name in &settings.cases {
        if !all.iter().any(|case| &case.name == name) {
            return Err(format!(
                "no case named {name} in {}",
                settings.parity.join("cases").display()
            ));
        }
    }
    let cases: Vec<Case> = all
        .into_iter()
        .filter(|case| settings.cases.is_empty() || settings.cases.contains(&case.name))
        .collect();
    if cases.is_empty() {
        return Err(format!(
            "no case in {}",
            settings.parity.join("cases").display()
        ));
    }

    let python = python_driver(settings, find_python(settings)?);
    check_python_imports(&python)?;
    let (cli, rust_program) = build(&settings.root)?;
    let rust = Driver {
        side: "rust",
        program: rust_program,
        arguments: Vec::new(),
        env: Vec::new(),
    };
    let corpus = Corpus {
        cases: &cases,
        drivers: [python, rust],
        cli,
        ledger: &ledger,
        timeout: settings.timeout,
    };

    let directory = prepare_directory(&settings.root)?;
    let mut passed = true;
    let mut first: Option<Vec<String>> = None;
    for round in 1..=settings.repeat {
        let round_directory = directory.join(format!("run-{round}"));
        let (verdicts, used) = corpus.run(&round_directory);
        let lines: Vec<String> = verdicts
            .iter()
            .map(|(name, verdict)| verdict.line(name))
            .collect();
        if round == 1 || verdicts.iter().any(|(_, verdict)| !verdict.passed()) {
            for line in &lines {
                println!("{line}");
            }
        }
        passed &= verdicts.iter().all(|(_, verdict)| verdict.passed());
        if settings.cases.is_empty() {
            for entry in ledger
                .entries
                .iter()
                .filter(|entry| !used.contains(&entry.id))
            {
                println!(
                    "stale       wire.{}: {} matched no difference ({}: {})",
                    entry.id, entry.location, entry.category, entry.reason
                );
                passed = false;
            }
        }
        match &first {
            None => first = Some(lines),
            Some(expected) if *expected != lines => {
                println!("run {round} gave results that differ from run 1");
                passed = false;
            }
            Some(_) => {}
        }
    }
    let total = cases.len();
    println!(
        "{total} cases, {} runs each; recordings in {}",
        settings.repeat,
        directory.display()
    );
    Ok(passed)
}

/// Runs `cargo xtask parity-diff`.
pub fn run(arguments: &[String]) -> ExitCode {
    let settings = match parse(arguments) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("parity-diff: {error}");
            return ExitCode::from(2);
        }
    };
    let result = check_submodule(&settings.upstream).and_then(|()| {
        if settings.check_requirements {
            requirements::run(
                &settings.upstream,
                &settings.parity.join("requirements.txt"),
            )
        } else if settings.coverage {
            coverage::run(&settings.upstream, &settings.parity)
        } else {
            run_corpus(&settings)
        }
    });
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("parity-diff: {error}");
            ExitCode::FAILURE
        }
    }
}
