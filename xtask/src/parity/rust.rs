//! What exists on the Rust side, asked of the toolchain instead of guessed
//! from source text: the tests the workspace builds, and whether a path
//! resolves against the compiled library.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

const LIBRARY_PACKAGE: &str = "claude-agent-kit";
const PROBE_PACKAGE: &str = "parity-probe";

fn tool(variable: &str, fallback: &str) -> OsString {
    std::env::var_os(variable).unwrap_or_else(|| OsString::from(fallback))
}

fn cargo_messages(
    directory: &Path,
    arguments: &[&OsStr],
    stderr: Stdio,
) -> Result<(bool, Vec<Value>), String> {
    let output = Command::new(tool("CARGO", "cargo"))
        .current_dir(directory)
        .args(arguments)
        .arg("--message-format=json")
        .stderr(stderr)
        .output()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    let messages = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    Ok((output.status.success(), messages))
}

fn sysroot() -> Result<PathBuf, String> {
    let output = Command::new(tool("RUSTC", "rustc"))
        .args(["--print", "sysroot"])
        .output()
        .map_err(|error| format!("cannot run rustc: {error}"))?;
    if !output.status.success() {
        return Err("`rustc --print sysroot` failed".to_owned());
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn library_search_variable() -> &'static str {
    if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    }
}

fn library_search_path(executable: &Path) -> Result<OsString, String> {
    let sysroot = sysroot()?;
    let mut directories = vec![sysroot.join(if cfg!(windows) { "bin" } else { "lib" })];
    if let Some(directory) = executable.parent() {
        directories.push(directory.to_path_buf());
    }
    if let Some(existing) = std::env::var_os(library_search_variable()) {
        directories.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(directories).map_err(|error| format!("invalid library path: {error}"))
}

fn relative(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every test the workspace builds with all features, as
/// `<target source file>::<test name>`, the source file being relative to
/// the workspace root.
pub fn tests(root: &Path) -> Result<BTreeSet<String>, String> {
    let arguments = ["test", "--workspace", "--all-features", "--no-run"].map(OsStr::new);
    let (built, messages) = cargo_messages(root, &arguments, Stdio::inherit())?;
    if !built {
        return Err("`cargo test --workspace --all-features --no-run` failed".to_owned());
    }
    let mut tests = BTreeSet::new();
    for artifact in messages {
        let (Some(executable), Some(source)) = (
            artifact["executable"].as_str(),
            artifact["target"]["src_path"].as_str(),
        ) else {
            continue;
        };
        if artifact["reason"] != "compiler-artifact" || artifact["profile"]["test"] != true {
            continue;
        }
        let executable = Path::new(executable);
        let output = Command::new(executable)
            .args(["--list", "--format", "terse"])
            .env(library_search_variable(), library_search_path(executable)?)
            .output()
            .map_err(|error| format!("cannot run {}: {error}", executable.display()))?;
        if !output.status.success() {
            return Err(format!(
                "{} --list failed: {}",
                executable.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let target = relative(root, source);
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(name) = line.strip_suffix(": test") {
                tests.insert(format!("{target}::{name}"));
            }
        }
    }
    Ok(tests)
}

fn metadata(root: &Path) -> Result<Value, String> {
    let output = Command::new(tool("CARGO", "cargo"))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cannot run cargo: {error}"))?;
    if !output.status.success() {
        return Err("`cargo metadata` failed".to_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cannot read the output of `cargo metadata`: {error}"))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// A throwaway package that depends on the library.
///
/// Cargo keys its build cache on the name of a package, not on where the
/// package lives, and trusts file times to decide what is up to date. Two
/// probes with one name would share a cache entry, and a probe whose source
/// was written before another finished building would be taken for already
/// checked. Each process therefore names its probe after its own id, and
/// drops the cache entry when it is done.
struct Probe {
    directory: PathBuf,
    target: PathBuf,
    package: String,
}

impl Drop for Probe {
    fn drop(&mut self) {
        let _ = Command::new(tool("CARGO", "cargo"))
            .current_dir(&self.directory)
            .args([
                "clean",
                "--quiet",
                "--package",
                &self.package,
                "--target-dir",
            ])
            .arg(&self.target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Probe {
    fn create(root: &Path) -> Result<Self, String> {
        let metadata = metadata(root)?;
        let target = metadata["target_directory"]
            .as_str()
            .map(PathBuf::from)
            .ok_or("`cargo metadata` reports no target directory")?;
        let library = metadata["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|package| package["name"] == LIBRARY_PACKAGE)
            .ok_or_else(|| format!("the workspace has no package {LIBRARY_PACKAGE}"))?;
        let source = library["manifest_path"]
            .as_str()
            .and_then(|manifest| Path::new(manifest).parent())
            .ok_or_else(|| format!("{LIBRARY_PACKAGE} has no manifest path"))?
            .to_string_lossy()
            .replace('\\', "/");
        let features: Vec<String> = library["features"]
            .as_object()
            .into_iter()
            .flat_map(|features| features.keys())
            .map(|feature| format!("{feature:?}"))
            .collect();

        let process = std::process::id();
        let package = format!("{PROBE_PACKAGE}-{process}");
        let directory = target.join("xtask-parity").join(process.to_string());
        std::fs::create_dir_all(directory.join("src"))
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let manifest = [
            "[package]".to_owned(),
            format!("name = {package:?}"),
            "version = \"0.0.0\"".to_owned(),
            "edition = \"2024\"".to_owned(),
            "publish = false".to_owned(),
            String::new(),
            "[workspace]".to_owned(),
            String::new(),
            "[dependencies]".to_owned(),
            format!(
                "{LIBRARY_PACKAGE} = {{ path = {source:?}, features = [{}] }}",
                features.join(", ")
            ),
            String::new(),
        ];
        write(&directory.join("Cargo.toml"), &manifest.join("\n"))?;
        std::fs::copy(root.join("Cargo.lock"), directory.join("Cargo.lock"))
            .map_err(|error| format!("cannot copy Cargo.lock: {error}"))?;
        Ok(Self {
            directory,
            target,
            package,
        })
    }

    fn failing_lines(&self, lines: &[String]) -> Result<BTreeSet<usize>, String> {
        let mut text = String::from("#![allow(unused)]\n");
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        write(&self.directory.join("src").join("lib.rs"), &text)?;

        let arguments = [
            OsStr::new("check"),
            OsStr::new("--quiet"),
            OsStr::new("--target-dir"),
            self.target.as_os_str(),
        ];
        let (built, messages) = cargo_messages(&self.directory, &arguments, Stdio::null())?;
        let library = self.package.replace('-', "_");
        let mut failing = BTreeSet::new();
        let mut unplaced = Vec::new();
        for message in messages.iter().filter(|message| {
            message["reason"] == "compiler-message"
                && message["target"]["name"] == library.as_str()
                && message["message"]["level"] == "error"
        }) {
            let line = message["message"]["spans"]
                .as_array()
                .and_then(|spans| spans.iter().find_map(|span| span["line_start"].as_u64()))
                .and_then(|line| usize::try_from(line).ok());
            match line {
                Some(line) if line >= 2 && line - 2 < lines.len() => {
                    failing.insert(line - 2);
                }
                _ => {
                    if let Some(text) = message["message"]["message"].as_str() {
                        unplaced.push(text.to_owned());
                    }
                }
            }
        }
        if !built && failing.is_empty() {
            return Err(format!(
                "cargo could not check the path probe in {}: {}",
                self.directory.display(),
                unplaced.join("; ")
            ));
        }
        Ok(failing)
    }
}

/// Indexes of the paths that do not resolve against the library built with
/// all features. A path resolves when it can be imported with `use` (module,
/// type, trait, function, constant, macro, enum variant) or used as a value
/// (associated function or constant).
///
/// The paths are compiled by cargo in a throwaway package of the target
/// directory that depends on the library, so the compiler decides.
pub fn unresolved_paths(root: &Path, paths: &[&str]) -> Result<Vec<usize>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let probe = Probe::create(root)?;
    let imports: Vec<String> = paths
        .iter()
        .enumerate()
        .map(|(index, path)| format!("use {path} as P{index};"))
        .collect();
    let not_imported: Vec<usize> = probe.failing_lines(&imports)?.into_iter().collect();
    if not_imported.is_empty() {
        return Ok(Vec::new());
    }

    let values: Vec<String> = not_imported
        .iter()
        .map(|index| format!("fn p{index}() {{ let _ = {}; }}", paths[*index]))
        .collect();
    Ok(probe
        .failing_lines(&values)?
        .into_iter()
        .map(|position| not_imported[position])
        .collect())
}
