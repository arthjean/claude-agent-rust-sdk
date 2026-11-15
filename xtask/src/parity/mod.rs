//! The parity commands: `parity-check`, `parity-report`, `parity-tests`,
//! `migration-table`, and `upstream-diff`.

mod diff;
mod ledger;
mod python;
mod rust;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ledger::{Counts, Ledger, State};

const TABLE_BEGIN: &str = "<!-- parity:table:begin -->";
const TABLE_END: &str = "<!-- parity:table:end -->";

/// Where the commands read and write.
#[derive(Debug, Clone)]
pub struct Locations {
    /// Workspace root.
    pub root: PathBuf,
    /// Checkout of the upstream Python SDK.
    pub upstream: PathBuf,
    /// Directory holding `surface.toml` and `tests.toml`.
    pub parity: PathBuf,
    /// The migration guide whose table is generated from the surface ledger.
    pub migration: PathBuf,
}

impl Locations {
    /// The locations of this workspace.
    pub fn of_workspace() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self {
            upstream: root.join("upstream").join("claude-agent-sdk-python"),
            parity: root.join("parity"),
            migration: root.join("docs").join("migration.md"),
            root,
        }
    }
}

struct Arguments {
    locations: Locations,
    deny_pending: bool,
    write: bool,
    positional: Vec<String>,
}

fn parse(arguments: &[String]) -> Result<Arguments, String> {
    let mut parsed = Arguments {
        locations: Locations::of_workspace(),
        deny_pending: false,
        write: true,
        positional: Vec::new(),
    };
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let mut path = |name: &str| {
            rest.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} needs a path"))
        };
        match argument.as_str() {
            "--upstream" => parsed.locations.upstream = path("--upstream")?,
            "--parity" => parsed.locations.parity = path("--parity")?,
            "--migration" => parsed.locations.migration = path("--migration")?,
            "--deny-pending" => parsed.deny_pending = true,
            "--check" => parsed.write = false,
            option if option.starts_with("--") => return Err(format!("unknown option {option}")),
            _ => parsed.positional.push(argument.clone()),
        }
    }
    Ok(parsed)
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map(|text| text.replace("\r\n", "\n"))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn upstream_exports(upstream: &Path) -> Result<Vec<String>, String> {
    let init = upstream
        .join("src")
        .join("claude_agent_sdk")
        .join("__init__.py");
    if !init.is_file() {
        return Err(format!(
            "{} is missing; run `git submodule update --init`",
            init.display()
        ));
    }
    let exports = python::exports(&read(&init)?);
    if exports.is_empty() {
        return Err(format!("no `__all__` found in {}", init.display()));
    }
    Ok(exports)
}

fn test_modules(directory: &Path, found: &mut Vec<PathBuf>) -> Result<(), String> {
    let listing_error = |error| format!("cannot list {}: {error}", directory.display());
    for entry in std::fs::read_dir(directory).map_err(listing_error)? {
        let path = entry.map_err(listing_error)?.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if path.is_dir() {
            test_modules(&path, found)?;
        } else if name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
        {
            found.push(path);
        }
    }
    Ok(())
}

fn upstream_tests(upstream: &Path) -> Result<Vec<String>, String> {
    let mut modules = Vec::new();
    test_modules(&upstream.join("tests"), &mut modules)?;
    modules.sort();
    let mut tests = Vec::new();
    for module in modules {
        let name = module
            .strip_prefix(upstream)
            .unwrap_or(&module)
            .to_string_lossy()
            .replace('\\', "/");
        for test in python::tests(&read(&module)?) {
            tests.push(format!("{name}::{test}"));
        }
    }
    Ok(tests)
}

struct Loaded {
    surface: Ledger,
    tests: Ledger,
    exports: Vec<String>,
    upstream_tests: Vec<String>,
}

fn load(locations: &Locations) -> Result<Loaded, String> {
    Ok(Loaded {
        surface: ledger::load_surface(&locations.parity.join("surface.toml"))?,
        tests: ledger::load_tests(&locations.parity.join("tests.toml"))?,
        exports: upstream_exports(&locations.upstream)?,
        upstream_tests: upstream_tests(&locations.upstream)?,
    })
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn migration_table(surface: &Ledger) -> String {
    let mut table = String::from("| Python | Rust | Notes |\n|---|---|---|\n");
    for (name, entry) in &surface.entries {
        let note = entry.note.as_deref().unwrap_or_default();
        let (rust, notes) = match entry.state() {
            Some(State::Implemented) => {
                let paths: Vec<String> = entry
                    .rust
                    .iter()
                    .flat_map(|paths| paths.as_slice())
                    .map(|path| format!("`{path}`"))
                    .collect();
                (paths.join(", "), note.to_owned())
            }
            Some(State::Excluded) => {
                let reason = entry
                    .excluded
                    .as_ref()
                    .and_then(|key| surface.exclusions.get(key))
                    .map(String::as_str)
                    .unwrap_or_default();
                let notes: Vec<&str> = [reason, note]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect();
                ("not ported".to_owned(), notes.join(" "))
            }
            Some(State::Pending) | None => (
                "pending".to_owned(),
                entry.pending.clone().unwrap_or_default(),
            ),
        };
        let _ = writeln!(table, "| `{name}` | {} | {} |", cell(&rust), cell(&notes));
    }
    table
}

fn with_table(document: &str, table: &str, file: &Path) -> Result<String, String> {
    let (Some(begin), Some(end)) = (document.find(TABLE_BEGIN), document.find(TABLE_END)) else {
        return Err(format!(
            "{} must contain `{TABLE_BEGIN}` and `{TABLE_END}`",
            file.display()
        ));
    };
    if end < begin {
        return Err(format!(
            "{}: the table markers are out of order",
            file.display()
        ));
    }
    Ok(format!(
        "{}{TABLE_BEGIN}\n{table}{}",
        &document[..begin],
        &document[end..]
    ))
}

fn migration_problem(locations: &Locations, surface: &Ledger) -> Result<Option<String>, String> {
    let document = read(&locations.migration)?;
    let expected = with_table(&document, &migration_table(surface), &locations.migration)?;
    Ok((document != expected).then(|| {
        format!(
            "{} is out of date with parity/surface.toml; run `cargo xtask migration-table`",
            locations.migration.display()
        )
    }))
}

fn missing_paths(
    ledger: &Ledger,
    exists: impl FnOnce(&[&str]) -> Result<Vec<bool>, String>,
) -> Result<Vec<String>, String> {
    let pairs = ledger.rust_paths();
    let distinct: Vec<&str> = pairs
        .iter()
        .map(|pair| pair.1)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let found = exists(&distinct)?;
    let missing: BTreeSet<&str> = distinct
        .iter()
        .zip(found)
        .filter(|(_, found)| !found)
        .map(|(path, _)| *path)
        .collect();
    Ok(pairs
        .iter()
        .filter(|pair| missing.contains(pair.1))
        .map(|(item, path)| {
            format!(
                "{}: `{item}` names Rust path `{path}`, which does not exist",
                ledger.file
            )
        })
        .collect())
}

fn check(arguments: &Arguments) -> Result<Vec<String>, String> {
    let locations = &arguments.locations;
    let loaded = load(locations)?;

    let mut problems = loaded.surface.structural_problems(&loaded.exports);
    problems.extend(loaded.tests.structural_problems(&loaded.upstream_tests));
    if !problems.is_empty() {
        return Ok(problems);
    }

    eprintln!("xtask: resolving the Rust paths of parity/surface.toml");
    problems.extend(missing_paths(&loaded.surface, |paths| {
        let unresolved = rust::unresolved_paths(&locations.root, paths)?;
        Ok((0..paths.len())
            .map(|index| !unresolved.contains(&index))
            .collect())
    })?);
    eprintln!("xtask: listing the Rust tests named by parity/tests.toml");
    problems.extend(missing_paths(&loaded.tests, |paths| {
        let tests = rust::tests(&locations.root)?;
        Ok(paths
            .iter()
            .map(|path| {
                let (path, platform) = ledger::split_platform(path);
                let compiled = platform.and_then(ledger::compiled_here).unwrap_or(true);
                !compiled || tests.contains(path)
            })
            .collect())
    })?);
    if !problems.is_empty() {
        return Ok(problems);
    }

    problems.extend(migration_problem(locations, &loaded.surface)?);

    for ledger in [&loaded.surface, &loaded.tests] {
        let pending = ledger.pending();
        if pending.is_empty() {
            continue;
        }
        eprintln!("{}: {} pending entries", ledger.file, pending.len());
        if arguments.deny_pending {
            for (item, note) in pending {
                problems.push(format!("{}: `{item}` is pending: {note}", ledger.file));
            }
        }
    }
    if problems.is_empty() {
        eprintln!(
            "xtask: parity-check passed: {} exports and {} tests are listed, every Rust path exists",
            loaded.exports.len(),
            loaded.upstream_tests.len()
        );
    }
    Ok(problems)
}

fn percentage(count: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = count as f64 / total as f64;
    ratio * 100.0
}

fn render_report(ledger: &Ledger, upstream: &[String]) -> String {
    let Counts {
        implemented,
        excluded,
        pending,
        by_reason,
    } = ledger.counts();
    let unlisted = upstream
        .iter()
        .filter(|item| !ledger.entries.contains_key(*item))
        .count();
    let pending = pending + unlisted;
    let total = implemented + excluded + pending;
    let mut report = format!("{}: {total} upstream {}s\n", ledger.file, ledger.kind);
    for (label, count) in [
        ("implemented", implemented),
        ("excluded", excluded),
        ("pending", pending),
    ] {
        let _ = writeln!(
            report,
            "  {label:<12}{count:>6}  {:>5.1}%",
            percentage(count, total)
        );
        if label == "excluded" {
            for (reason, count) in &by_reason {
                let _ = writeln!(report, "    {reason:<32}{count:>6}");
            }
        }
    }
    if unlisted > 0 {
        let _ = writeln!(
            report,
            "  ({unlisted} of the pending are not listed at all)"
        );
    }
    report
}

fn outcome(command: &str, result: Result<Vec<String>, String>) -> ExitCode {
    let problems = match result {
        Ok(problems) if problems.is_empty() => return ExitCode::SUCCESS,
        Ok(problems) => problems,
        Err(error) => vec![error],
    };
    for problem in &problems {
        eprintln!("{problem}");
    }
    eprintln!("xtask: {command} failed with {} problem(s)", problems.len());
    ExitCode::FAILURE
}

/// `cargo xtask parity-check`.
pub fn run_check(arguments: &[String]) -> ExitCode {
    outcome(
        "parity-check",
        parse(arguments).and_then(|arguments| check(&arguments)),
    )
}

/// `cargo xtask parity-report`.
pub fn run_report(arguments: &[String]) -> ExitCode {
    let result = parse(arguments).and_then(|arguments| {
        let loaded = load(&arguments.locations)?;
        print!("{}", render_report(&loaded.surface, &loaded.exports));
        println!();
        print!("{}", render_report(&loaded.tests, &loaded.upstream_tests));
        Ok(Vec::new())
    });
    outcome("parity-report", result)
}

/// `cargo xtask migration-table`: rewrites the table of the migration
/// guide from the surface ledger, or with `--check` only compares.
pub fn run_migration(arguments: &[String]) -> ExitCode {
    let result = parse(arguments).and_then(|arguments| {
        let locations = &arguments.locations;
        let surface = ledger::load_surface(&locations.parity.join("surface.toml"))?;
        if !arguments.write {
            return Ok(migration_problem(locations, &surface)?
                .into_iter()
                .collect());
        }
        let document = read(&locations.migration)?;
        let updated = with_table(&document, &migration_table(&surface), &locations.migration)?;
        std::fs::write(&locations.migration, updated)
            .map_err(|error| format!("cannot write {}: {error}", locations.migration.display()))?;
        eprintln!("xtask: wrote {}", locations.migration.display());
        Ok(Vec::new())
    });
    outcome("migration-table", result)
}

/// `cargo xtask parity-tests`: prints every Rust test path `tests.toml`
/// may name.
pub fn run_tests(arguments: &[String]) -> ExitCode {
    let result = parse(arguments).and_then(|arguments| {
        for test in rust::tests(&arguments.locations.root)? {
            println!("{test}");
        }
        Ok(Vec::new())
    });
    outcome("parity-tests", result)
}

/// `cargo xtask upstream-diff <tag>`.
pub fn run_diff(arguments: &[String]) -> ExitCode {
    let result = parse(arguments).and_then(|arguments| {
        let [revision] = arguments.positional.as_slice() else {
            return Err("usage: cargo xtask upstream-diff <tag>".to_owned());
        };
        print!("{}", diff::run(&arguments.locations.upstream, revision)?);
        Ok(Vec::new())
    });
    outcome("upstream-diff", result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use ledger::Entry;

    fn entry(rust: Option<&str>, excluded: Option<&str>, pending: Option<&str>) -> Entry {
        let table = [("rust", rust), ("excluded", excluded), ("pending", pending)]
            .iter()
            .filter_map(|(key, value)| Some(format!("{key} = \"{}\"", (*value)?)))
            .collect::<Vec<_>>()
            .join("\n");
        toml::from_str(&table).unwrap()
    }

    fn sample() -> Ledger {
        let mut exclusions = IndexMap::new();
        exclusions.insert("python-only".to_owned(), "Python typing helper.".to_owned());
        let mut entries = IndexMap::new();
        entries.insert(
            "query".to_owned(),
            entry(Some("claude_agent_kit::query"), None, None),
        );
        entries.insert(
            "Shadowed".to_owned(),
            entry(None, Some("python-only"), None),
        );
        entries.insert("later".to_owned(), entry(None, None, Some("next bump")));
        Ledger {
            file: "parity/surface.toml".to_owned(),
            kind: "export",
            exclusions,
            entries,
        }
    }

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn report_prints_the_three_percentages() {
        let report = render_report(
            &sample(),
            &names(&["query", "Shadowed", "later", "unlisted"]),
        );
        assert!(report.starts_with("parity/surface.toml: 4 upstream exports\n"));
        assert!(report.contains("implemented      1   25.0%"));
        assert!(report.contains("excluded         1   25.0%"));
        assert!(report.contains("pending          2   50.0%"));
        assert!(report.contains("python-only"));
        assert!(report.contains("(1 of the pending are not listed at all)"));
    }

    #[test]
    fn migration_table_has_one_row_per_export() {
        let table = migration_table(&sample());
        assert_eq!(table.lines().count(), 5);
        assert!(table.contains("| `query` | `claude_agent_kit::query` |  |"));
        assert!(table.contains("| `Shadowed` | not ported | Python typing helper. |"));
        assert!(table.contains("| `later` | pending | next bump |"));
    }

    #[test]
    fn the_table_replaces_only_the_marked_region() {
        let document = format!("intro\n{TABLE_BEGIN}\nold\n{TABLE_END}\noutro\n");
        let updated = with_table(&document, "new\n", Path::new("guide.md")).unwrap();
        assert_eq!(
            updated,
            format!("intro\n{TABLE_BEGIN}\nnew\n{TABLE_END}\noutro\n")
        );
        assert!(with_table("no markers", "new\n", Path::new("guide.md")).is_err());
    }

    #[test]
    fn missing_paths_names_the_item_and_the_path() {
        let problems = missing_paths(&sample(), |paths| Ok(vec![false; paths.len()])).unwrap();
        assert_eq!(
            problems,
            [
                "parity/surface.toml: `query` names Rust path `claude_agent_kit::query`, which does not exist"
            ]
        );
    }
}
