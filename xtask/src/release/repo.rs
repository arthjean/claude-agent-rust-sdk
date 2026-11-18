//! The workspace the release commands act on: git queries, the files they
//! may write, and a set of writes that is undone as a whole on failure.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::version::Version;

/// The branch releases are made from.
pub const MAIN: &str = "main";
/// The submodule holding the upstream Python SDK.
pub const UPSTREAM: &str = "upstream/claude-agent-sdk-python";
/// The library source holding [`CLI_CONSTANT`] and [`SDK_CONSTANT`].
pub const LIBRARY_ROOT: &str = "crates/claude-agent-rust-sdk/src/lib.rs";
/// The constant naming the tested CLI version.
pub const CLI_CONSTANT: &str = "pub const CLI_VERSION: &str = \"";
/// The constant naming the upstream SDK version.
pub const SDK_CONSTANT: &str = "pub const UPSTREAM_SDK_VERSION: &str = \"";

/// The files `release prepare` may change, and so the only files the
/// release commit of `release tag` may hold.
pub const RELEASE_FILES: [&str; 8] = [
    "Cargo.toml",
    "Cargo.lock",
    "crates/claude-agent-rust-sdk/Cargo.toml",
    "README.md",
    "crates/claude-agent-rust-sdk-macros/README.md",
    "CHANGELOG.md",
    "docs/migration.md",
    "docs/parity.md",
];

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// The workspace root of this checkout of `xtask`.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// A workspace at a root directory.
#[derive(Debug, Clone)]
pub struct Repo {
    /// The root directory.
    pub root: PathBuf,
}

impl Repo {
    /// The output of `git <arguments>` in `directory`, or the error git
    /// printed.
    pub fn git_in(directory: &Path, arguments: &[&str]) -> Result<String, String> {
        let output = Command::new("git")
            .current_dir(directory)
            .args(arguments)
            .output()
            .map_err(|error| format!("cannot run git: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "git {} failed: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// The output of `git <arguments>` at the root.
    pub fn git(&self, arguments: &[&str]) -> Result<String, String> {
        Self::git_in(&self.root, arguments)
    }

    /// Runs `cargo <arguments>` at the root with inherited output.
    pub fn cargo(&self, arguments: &[&str]) -> Result<(), String> {
        eprintln!("xtask: cargo {}", arguments.join(" "));
        let status = Command::new(cargo())
            .current_dir(&self.root)
            .args(arguments)
            .status()
            .map_err(|error| format!("cargo could not start: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "cargo {} failed with {status}",
                arguments.join(" ")
            ))
        }
    }

    /// The path of `relative` under the root.
    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The text of `relative`, with Windows line ends made Unix ones.
    pub fn read(&self, relative: &str) -> Result<String, String> {
        let path = self.path(relative);
        std::fs::read_to_string(&path)
            .map(|text| text.replace("\r\n", "\n"))
            .map_err(|error| format!("cannot read {}: {error}", path.display()))
    }

    /// The files git reports as changed, staged, or untracked.
    pub fn changed_files(&self) -> Result<Vec<String>, String> {
        let status = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        let mut files = Vec::new();
        let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
        while let Some(entry) = entries.next() {
            let Some(path) = entry.get(3..) else {
                continue;
            };
            files.push(path.to_owned());
            if entry.starts_with('R') || entry.starts_with('C') {
                entries.next();
            }
        }
        Ok(files)
    }

    /// Fails, listing the files, when the working tree has any change.
    pub fn require_clean(&self) -> Result<(), String> {
        let changed = self.changed_files()?;
        if changed.is_empty() {
            return Ok(());
        }
        Err(format!(
            "the working tree has uncommitted changes; commit or stash them first:\n  {}",
            changed.join("\n  ")
        ))
    }

    /// Fails unless the checked-out branch is [`MAIN`].
    pub fn require_main(&self) -> Result<(), String> {
        let branch = self
            .git(&["symbolic-ref", "--quiet", "--short", "HEAD"])
            .map(|branch| branch.trim().to_owned())
            .unwrap_or_default();
        if branch == MAIN {
            Ok(())
        } else if branch.is_empty() {
            Err(format!("HEAD is detached; check out `{MAIN}` first"))
        } else {
            Err(format!(
                "the branch is `{branch}`; releases are made from `{MAIN}`"
            ))
        }
    }

    /// Whether the tag `name` exists.
    pub fn tag_exists(&self, name: &str) -> bool {
        self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{name}"),
        ])
        .is_ok()
    }

    /// The versions of the `v<version>` tags, newest first; with `merged`,
    /// only the tags reachable from `HEAD`.
    pub fn release_tags(&self, merged: bool) -> Result<Vec<Version>, String> {
        let mut arguments = vec!["tag", "--list", "v*"];
        if merged {
            arguments.extend(["--merged", "HEAD"]);
        }
        let mut versions: Vec<Version> = self
            .git(&arguments)?
            .lines()
            .filter_map(|tag| Version::parse(tag.trim().strip_prefix('v')?).ok())
            .collect();
        versions.sort();
        versions.reverse();
        Ok(versions)
    }

    /// The full messages of the commits after the tag `since` up to
    /// `HEAD`, oldest first, or of every commit when `since` is `None`.
    pub fn commit_messages(&self, since: Option<&str>) -> Result<Vec<String>, String> {
        let range = since.map_or_else(|| "HEAD".to_owned(), |tag| format!("{tag}..HEAD"));
        let log = self.git(&["log", "--reverse", "--format=%B%x00", &range])?;
        Ok(log
            .split('\0')
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// The `[workspace.package]` version.
    pub fn workspace_version(&self) -> Result<Version, String> {
        Version::parse(&crate::differential::ledger::crate_version(&self.root)?)
    }

    /// The value of a string constant of the library root, given by the
    /// text before its value.
    pub fn library_constant(&self, constant: &str) -> Result<String, String> {
        self.read(LIBRARY_ROOT)?
            .lines()
            .find_map(|line| line.strip_prefix(constant)?.split('"').next())
            .map(str::to_owned)
            .ok_or_else(|| format!("{LIBRARY_ROOT} has no line `{constant}...\"`"))
    }
}

/// Today's date in UTC, `YYYY-MM-DD`.
pub fn today() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    civil_date(seconds / 86_400)
}

/// The proleptic Gregorian date `days` after 1970-01-01 (the algorithm of
/// Howard Hinnant's `civil_from_days`).
fn civil_date(days: u64) -> String {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// One copy of a pinned value in a file: `template` with the placeholders
/// `{version}` (the crate version), `{minor}` (its `major.minor`), `{sdk}`
/// (the upstream SDK version), `{cli}` (the CLI version), and `{commit}`
/// (the upstream commit).
#[derive(Debug, Clone, Copy)]
pub struct Pinned {
    /// The file, relative to the root.
    pub file: &'static str,
    /// The text around the value.
    pub template: &'static str,
}

/// The values the placeholders of a [`Pinned`] stand for.
#[derive(Debug, Clone, Default)]
pub struct Values {
    /// `{version}`.
    pub version: String,
    /// `{minor}`.
    pub minor: String,
    /// `{sdk}`.
    pub sdk: String,
    /// `{cli}`.
    pub cli: String,
    /// `{commit}`.
    pub commit: String,
}

impl Values {
    fn fill(&self, template: &str) -> String {
        template
            .replace("{version}", &self.version)
            .replace("{minor}", &self.minor)
            .replace("{sdk}", &self.sdk)
            .replace("{cli}", &self.cli)
            .replace("{commit}", &self.commit)
    }
}

/// Rewrites every copy of `copies` from the `old` values to the `new`
/// ones, in memory: the new content of each file, in the order the files
/// first appear. Fails naming the file and the text when a copy is
/// missing, so a moved copy is noticed instead of skipped.
pub fn rewrite_copies(
    repo: &Repo,
    copies: &[Pinned],
    old: &Values,
    new: &Values,
) -> Result<Vec<(String, String)>, String> {
    let mut files: Vec<(String, String)> = Vec::new();
    for copy in copies {
        let position = files.iter().position(|(file, _)| file == copy.file);
        let text = match position {
            Some(index) => files[index].1.clone(),
            None => repo.read(copy.file)?,
        };
        let before = old.fill(copy.template);
        if !text.contains(&before) {
            return Err(format!(
                "{}: no copy of `{}`; update the copy list of xtask/src/release if it moved",
                copy.file,
                before.trim()
            ));
        }
        let text = text.replace(&before, &new.fill(copy.template));
        match position {
            Some(index) => files[index].1 = text,
            None => files.push((copy.file.to_owned(), text)),
        }
    }
    Ok(files)
}

/// Writes made by one command: until [`Transaction::commit`], dropping it
/// puts every recorded file back as it was, so a failure leaves no file
/// partly rewritten.
pub struct Transaction {
    root: PathBuf,
    originals: Vec<(PathBuf, Option<Vec<u8>>)>,
    committed: bool,
}

impl Transaction {
    /// A transaction on the files under `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            originals: Vec::new(),
            committed: false,
        }
    }

    /// Records the current content of `relative`, before something else
    /// (cargo, a regeneration) rewrites it.
    pub fn record(&mut self, relative: &str) {
        let path = self.root.join(relative);
        if self.originals.iter().any(|(known, _)| *known == path) {
            return;
        }
        let content = std::fs::read(&path).ok();
        self.originals.push((path, content));
    }

    /// Writes `content` to `relative` through a temporary file renamed
    /// over it.
    pub fn write(&mut self, relative: &str, content: &str) -> Result<(), String> {
        self.record(relative);
        let path = self.root.join(relative);
        let temporary = path.with_extension("xtask-release.tmp");
        std::fs::write(&temporary, content)
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, &path).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("cannot replace {}: {error}", path.display())
        })
    }

    /// Keeps the writes.
    pub fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for (path, content) in self.originals.iter().rev() {
            let restored = match content {
                Some(content) => std::fs::write(path, content),
                None => std::fs::remove_file(path),
            };
            if let Err(error) = restored {
                eprintln!("xtask: cannot restore {}: {error}", path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_days() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(11_016), "2000-02-29");
        assert_eq!(civil_date(20_775), "2026-11-18");
        assert_eq!(today().len(), 10);
    }

    #[test]
    fn a_dropped_transaction_restores_every_file_and_a_committed_one_keeps_them() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("kept.txt"), "before").unwrap();
        {
            let mut transaction = Transaction::new(root);
            transaction.write("kept.txt", "after").unwrap();
            transaction.write("new.txt", "created").unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join("kept.txt")).unwrap(),
                "after"
            );
        }
        assert_eq!(
            std::fs::read_to_string(root.join("kept.txt")).unwrap(),
            "before"
        );
        assert!(!root.join("new.txt").exists());

        let mut transaction = Transaction::new(root);
        transaction.write("kept.txt", "after").unwrap();
        transaction.commit();
        assert_eq!(
            std::fs::read_to_string(root.join("kept.txt")).unwrap(),
            "after"
        );
    }

    #[test]
    fn copies_are_rewritten_in_memory_and_a_missing_copy_names_its_file() {
        let directory = tempfile::tempdir().unwrap();
        let repo = Repo {
            root: directory.path().to_path_buf(),
        };
        std::fs::write(repo.path("a.txt"), "cli 1.0 and sdk 2.0, cli 1.0\n").unwrap();
        let copies = [
            Pinned {
                file: "a.txt",
                template: "cli {cli}",
            },
            Pinned {
                file: "a.txt",
                template: "sdk {sdk},",
            },
        ];
        let old = Values {
            cli: "1.0".to_owned(),
            sdk: "2.0".to_owned(),
            ..Values::default()
        };
        let new = Values {
            cli: "1.1".to_owned(),
            sdk: "2.1".to_owned(),
            ..Values::default()
        };
        let files = rewrite_copies(&repo, &copies, &old, &new).unwrap();
        assert_eq!(
            files,
            [(
                "a.txt".to_owned(),
                "cli 1.1 and sdk 2.1, cli 1.1\n".to_owned()
            )]
        );
        assert_eq!(
            std::fs::read_to_string(repo.path("a.txt")).unwrap(),
            "cli 1.0 and sdk 2.0, cli 1.0\n"
        );
        let error = rewrite_copies(&repo, &copies, &new, &old).unwrap_err();
        assert!(error.starts_with("a.txt: no copy of `cli 1.1`"), "{error}");
    }
}
