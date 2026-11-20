//! Running one driver on one case.
//!
//! Each run gets its own directory: the resolved case, the scenario, the
//! record file, the journal of the driver (`PARITY_JOURNAL`), the driver's
//! output, and `work`, the working directory of the driver, which holds its
//! home and temporary directories. The driver
//! starts with an environment built from scratch, so nothing of the
//! caller's environment (a `CLAUDE_*` variable of an enclosing session, an
//! API key) reaches the recording: only what the operating system needs,
//! the variables of the side, and the case's `process_env`.
//!
//! A driver that does not finish within the time limit is killed with its
//! children: its process group on Unix, its process tree on Windows, and
//! every `fake-claude` process named in the record file, since an SDK may
//! start the CLI in a process group of its own.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use super::case::Case;
use super::compare::Recording;
use super::journal::Journal;

/// Environment variable naming the journal a driver writes.
pub const JOURNAL_VARIABLE: &str = "PARITY_JOURNAL";

/// Exit status a driver uses for an unsupported option, step, or value.
pub const EXIT_UNSUPPORTED: i32 = 3;

const SYSTEM_VARIABLES: [&str; 8] = [
    "PATH",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "LANG",
    "LC_ALL",
];

/// How to start one side's driver.
#[derive(Debug, Clone)]
pub struct Driver {
    /// `python` or `rust`.
    pub side: &'static str,
    /// Program to run.
    pub program: PathBuf,
    /// Arguments before the case path.
    pub arguments: Vec<OsString>,
    /// Variables the side needs, such as `PYTHONPATH`.
    pub env: Vec<(String, OsString)>,
}

/// What a run produced.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The driver ran the case; the recording, the journal, and the
    /// spellings of its working directory.
    Ran {
        /// What `fake-claude` received.
        recording: Recording,
        /// What the application observed.
        journal: Journal,
        /// The working directory, as created and canonical.
        work: Vec<String>,
    },
    /// The driver does not support the case.
    Unsupported(String),
    /// The driver did not finish in time.
    Timeout,
    /// The driver failed.
    Failed(String),
}

fn last_line(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn prepare(directory: &Path, case: &Case, work: &Path) -> Result<(), String> {
    let create = |path: &Path| {
        std::fs::create_dir_all(path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))
    };
    create(work)?;
    create(&work.join(".home"))?;
    create(&work.join(".tmp"))?;
    let work = work.to_string_lossy();
    let write = |name: &str, text: String| {
        let path = directory.join(name);
        std::fs::write(&path, text)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    };
    write("case.json", case.resolved_text(&work))?;
    write("scenario.json", case.resolved_scenario(&work).to_string())
}

fn command(driver: &Driver, directory: &Path, work: &Path, case: &Case, cli: &Path) -> Command {
    let mut command = Command::new(&driver.program);
    command
        .args(&driver.arguments)
        .arg(directory.join("case.json"))
        .arg("--cli")
        .arg(cli)
        .current_dir(work)
        .env_clear();
    for name in SYSTEM_VARIABLES {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let home = work.join(".home");
    let temporary = work.join(".tmp");
    command
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("TMPDIR", &temporary)
        .env("TEMP", &temporary)
        .env("TMP", &temporary)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("FAKE_CLAUDE_SCENARIO", directory.join("scenario.json"))
        .env("FAKE_CLAUDE_RECORD", directory.join("record.jsonl"))
        .env(JOURNAL_VARIABLE, directory.join("journal.jsonl"));
    for (name, value) in &driver.env {
        command.env(name, value);
    }
    for (name, value) in &case.process_env {
        command.env(name, value);
    }
    command.stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
}

fn kill(pid: u32, group: bool) {
    #[cfg(unix)]
    let mut killer = {
        let mut killer = Command::new("kill");
        let target = if group {
            format!("-{pid}")
        } else {
            pid.to_string()
        };
        killer.args(["-KILL", "--", &target]);
        killer
    };
    #[cfg(windows)]
    let mut killer = {
        let _ = group;
        let mut killer = Command::new("taskkill");
        killer.args(["/F", "/T", "/PID", &pid.to_string()]);
        killer
    };
    let _ = killer
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn wait(child: &mut std::process::Child, limit: Duration) -> Result<Option<ExitStatus>, String> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Runs one driver on one case in `directory`, which must be empty.
pub fn run(driver: &Driver, case: &Case, cli: &Path, directory: &Path, limit: Duration) -> Outcome {
    let work = directory.join("work");
    if let Err(error) = prepare(directory, case, &work) {
        return Outcome::Failed(error);
    }
    let output = |name: &str| std::fs::File::create(directory.join(name));
    let (Ok(stdout), Ok(stderr)) = (output("stdout.txt"), output("stderr.txt")) else {
        return Outcome::Failed(format!(
            "cannot create the output files in {}",
            directory.display()
        ));
    };
    let mut command = command(driver, directory, &work, case, cli);
    command.stdout(stdout).stderr(stderr);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Outcome::Failed(format!(
                "cannot start {}: {error}",
                driver.program.display()
            ));
        }
    };
    let record = directory.join("record.jsonl");
    let status = match wait(&mut child, limit) {
        Ok(Some(status)) => status,
        Ok(None) => {
            kill(child.id(), true);
            let text = std::fs::read_to_string(&record).unwrap_or_default();
            for pid in Recording::pids(&text) {
                kill(pid, false);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Outcome::Timeout;
        }
        Err(error) => return Outcome::Failed(error),
    };
    let stderr = directory.join("stderr.txt");
    match status.code() {
        Some(0) => {}
        Some(EXIT_UNSUPPORTED) => return Outcome::Unsupported(last_line(&stderr)),
        code => {
            let code = code.map_or_else(|| "a signal".to_owned(), |code| format!("status {code}"));
            return Outcome::Failed(format!("exited with {code}: {}", last_line(&stderr)));
        }
    }
    let text = match std::fs::read_to_string(&record) {
        Ok(text) => text,
        Err(error) => {
            return Outcome::Failed(format!("no recording at {}: {error}", record.display()));
        }
    };
    let journal_path = directory.join("journal.jsonl");
    let journal = match std::fs::read_to_string(&journal_path)
        .map_err(|error| format!("no journal at {}: {error}", journal_path.display()))
        .and_then(|text| Journal::parse(&text))
    {
        Ok(journal) => journal,
        Err(error) => return Outcome::Failed(error),
    };
    match Recording::parse(&text) {
        Ok(recording) if recording.processes.is_empty() => {
            Outcome::Failed("the driver exited 0, but fake-claude was never started".to_owned())
        }
        Ok(recording) => {
            let mut spellings = vec![work.to_string_lossy().into_owned()];
            if let Ok(canonical) = work.canonicalize() {
                let canonical = canonical.to_string_lossy().into_owned();
                spellings.push(
                    canonical
                        .strip_prefix(r"\\?\")
                        .map_or(canonical.clone(), str::to_owned),
                );
            }
            Outcome::Ran {
                recording,
                journal,
                work: spellings,
            }
        }
        Err(error) => Outcome::Failed(error),
    }
}
