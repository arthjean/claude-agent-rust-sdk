//! Where the CLI keeps transcripts: the config directory, the projects
//! directory, and the name of the directory of each project.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use unicode_normalization::UnicodeNormalization;

const CONFIG_DIR_VARIABLE: &str = "CLAUDE_CONFIG_DIR";

/// Longest sanitized directory name kept whole. Most filesystems limit a
/// path component to 255 bytes; the rest is room for the hash suffix.
pub(crate) const MAX_SANITIZED_LENGTH: usize = 200;

const WORKTREE_LIST_LIMIT: Duration = Duration::from_secs(5);
const WORKTREE_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// The units the CLI counts when it names a project directory.
///
/// The CLI runs on JavaScript, whose strings are sequences of UTF-16 code
/// units: a character outside the Basic Multilingual Plane counts twice.
/// Both the length limit and the hash go through this one function.
fn code_units(text: &str) -> impl Iterator<Item = u16> + '_ {
    text.encode_utf16()
}

/// 32-bit hash of the CLI, in base 36: `h = h * 31 + unit` on signed 32
/// bits, then the absolute value.
pub(crate) fn simple_hash(text: &str) -> String {
    let hash = code_units(text).fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    to_base36(u64::from(hash.unsigned_abs()))
}

fn to_base36(mut value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_owned();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(char::from(DIGITS[usize::try_from(value % 36).unwrap_or(0)]));
        value /= 36;
    }
    digits.iter().rev().collect()
}

/// Makes a path safe as a directory name: every unit outside
/// `[a-zA-Z0-9]` becomes `-`, and a name over 200 units is cut and suffixed
/// with `-` and the hash of the unsanitized path.
pub(crate) fn sanitize_path(name: &str) -> String {
    let sanitized: String = code_units(name)
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() => char::from(byte),
            _ => '-',
        })
        .collect();
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return sanitized;
    }
    format!(
        "{}-{}",
        &sanitized[..MAX_SANITIZED_LENGTH],
        simple_hash(name)
    )
}

/// The config directory of the CLI: `CLAUDE_CONFIG_DIR` when set and not
/// empty, otherwise `~/.claude`, normalized to NFC.
pub(crate) fn config_home_dir() -> PathBuf {
    match std::env::var(CONFIG_DIR_VARIABLE) {
        Ok(directory) if !directory.is_empty() => PathBuf::from(nfc(&directory)),
        _ => {
            let home = std::env::home_dir().unwrap_or_default().join(".claude");
            PathBuf::from(nfc(&home.to_string_lossy()))
        }
    }
}

/// The projects directory. `env` is consulted before the environment of
/// this process, so a `CLAUDE_CONFIG_DIR` passed to the child resolves the
/// directory the child writes to.
pub(crate) fn projects_dir(env: Option<&IndexMap<String, String>>) -> PathBuf {
    if let Some(directory) = env
        .and_then(|env| env.get(CONFIG_DIR_VARIABLE))
        .filter(|directory| !directory.is_empty())
    {
        return PathBuf::from(nfc(directory)).join("projects");
    }
    config_home_dir().join("projects")
}

/// Resolves a directory to its canonical form: the real path, normalized
/// to NFC.
///
/// Like Python's `os.path.realpath`, a path that does not exist yet keeps
/// its missing tail but has its longest existing ancestor resolved, so
/// symbolic links (`/var` on macOS) and Windows short names (`RUNNER~1`)
/// above it give the same key as once it exists. A path with no existing
/// ancestor is made absolute instead.
pub(crate) fn canonicalize_path(directory: &Path) -> String {
    let absolute = std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf());
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    let resolved = loop {
        if let Ok(real) = std::fs::canonicalize(existing) {
            let mut resolved = strip_verbatim_prefix(real);
            resolved.extend(missing.iter().rev());
            break resolved;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                existing = parent;
            }
            _ => break absolute.clone(),
        }
    };
    nfc(&resolved.to_string_lossy())
}

/// Windows canonical paths carry the `\\?\` verbatim prefix, which the CLI
/// never sees in its working directory.
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// Finds the project directory of a canonical path under `projects`.
///
/// For a name over 200 units the hash suffix may not match the one the CLI
/// computed, so the lookup falls back to the first directory whose name is
/// the 200-unit prefix followed by `-`.
pub(crate) fn find_project_dir_in(projects: &Path, project_path: &str) -> Option<PathBuf> {
    let sanitized = sanitize_path(project_path);
    let exact = projects.join(&sanitized);
    if exact.is_dir() {
        return Some(exact);
    }
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return None;
    }
    let prefix = format!("{}-", &sanitized[..MAX_SANITIZED_LENGTH]);
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
        })
}

/// [`find_project_dir_in`] under the projects directory of the
/// environment.
pub(crate) fn find_project_dir(project_path: &str) -> Option<PathBuf> {
    find_project_dir_in(&projects_dir(None), project_path)
}

/// The directories directly under `directory`, in the order of the
/// filesystem. An unreadable directory has none.
pub(crate) fn subdirectories(directory: &Path) -> Option<Vec<PathBuf>> {
    let entries = std::fs::read_dir(directory).ok()?;
    Some(
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect(),
    )
}

/// The worktree paths of the git repository holding `cwd`, normalized to
/// NFC. Empty when git is missing, fails, or takes more than 5 s.
pub(crate) fn worktree_paths(cwd: &str) -> Vec<String> {
    let Some(output) = git_worktree_list(cwd) else {
        return Vec::new();
    };
    output
        .split('\n')
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(nfc)
        .collect()
}

fn git_worktree_list(cwd: &str) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(["worktree", "list", "--porcelain"])
        .current_dir(cwd);
    run_with_limit(command, WORKTREE_LIST_LIMIT)
}

/// Runs a command and returns its output when it succeeds within `limit`.
/// A command still running at the limit is killed.
pub(crate) fn run_with_limit(mut command: Command, limit: Duration) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_console_window(&mut command);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut buffer).map(|_| buffer)
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(WORKTREE_POLL_INTERVAL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let output = reader.join().ok()?.ok()?;
    if !status.success() || output.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&output).into_owned())
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}

/// The [`SessionKey::project_key`](crate::SessionKey) of a directory, by
/// default the current working directory.
///
/// It is the name the CLI gives the project directory of that path: the
/// real path, normalized to NFC, with every character outside
/// `[a-zA-Z0-9]` replaced by `-`, and for a name over 200 UTF-16 code
/// units, cut and suffixed with a hash. Keys therefore match between local
/// transcripts and mirrored ones.
#[must_use]
pub fn project_key_for_directory(directory: Option<&Path>) -> String {
    sanitize_path(&canonicalize_path(directory.unwrap_or(Path::new("."))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_the_javascript_reference() {
        assert_eq!(simple_hash(""), "0");
        assert_eq!(simple_hash("a"), "2p");
        assert_eq!(simple_hash("hello"), "1n1e4y");
        assert_eq!(
            simple_hash("/Users/me/project"),
            simple_hash("/Users/me/project")
        );
    }

    #[test]
    fn hash_takes_the_absolute_value_of_the_minimum() {
        assert_eq!(to_base36(u64::from(i32::MIN.unsigned_abs())), "zik0zk");
    }

    #[test]
    fn sanitize_replaces_every_unit_outside_ascii_alphanumerics() {
        assert_eq!(
            sanitize_path("/Users/me/my project"),
            "-Users-me-my-project"
        );
        assert_eq!(sanitize_path("/tmp/é"), "-tmp--");
        assert_eq!(sanitize_path("/tmp/😀"), "-tmp---");
    }

    #[test]
    fn long_names_are_cut_at_200_units_and_hashed_over_the_original() {
        let long = format!("/{}", "x".repeat(250));
        let sanitized = sanitize_path(&long);
        assert_eq!(
            sanitized,
            format!("-{}-{}", "x".repeat(199), simple_hash(&long))
        );
        let exactly = "y".repeat(200);
        assert_eq!(sanitize_path(&exactly), exactly);
    }

    #[test]
    fn units_outside_the_basic_plane_count_twice_toward_the_limit() {
        let name = format!("{}😀", "a".repeat(199));
        let sanitized = sanitize_path(&name);
        assert!(sanitized.starts_with(&format!("{}-", "a".repeat(199))));
        assert_eq!(sanitized.len(), 201 + simple_hash(&name).len());
    }

    #[test]
    fn names_match_the_directories_created_by_cli_2_1_325() {
        let base = r"C:\Users\Arthur\AppData\Local\Temp\.tmpqNYwoX";
        assert_eq!(
            sanitize_path(&format!(r"{base}\ascii-case")),
            "C--Users-Arthur-AppData-Local-Temp--tmpqNYwoX-ascii-case"
        );
        assert_eq!(
            sanitize_path(&format!("{base}\\emoji-\u{1F600}-case")),
            "C--Users-Arthur-AppData-Local-Temp--tmpqNYwoX-emoji----case"
        );
        let long = format!(r"{base}\long{}", r"\nested-directory-segment".repeat(8));
        assert_eq!(long.encode_utf16().count(), 250);
        assert_eq!(
            sanitize_path(&long),
            format!(
                "C--Users-Arthur-AppData-Local-Temp--tmpqNYwoX-long{}-u9687a",
                "-nested-directory-segment".repeat(6)
            )
        );
    }

    #[test]
    fn paths_are_normalized_to_nfc_before_sanitizing() {
        let decomposed = std::env::temp_dir().join("cafe\u{301}-missing-dir");
        let canonical = canonicalize_path(&decomposed);
        assert!(canonical.ends_with("caf\u{e9}-missing-dir"), "{canonical}");
        assert!(project_key_for_directory(Some(&decomposed)).ends_with("caf--missing-dir"));
    }

    #[test]
    fn a_missing_path_resolves_its_existing_ancestor() {
        let root = tempfile::tempdir().unwrap();
        let real = canonicalize_path(root.path());
        let missing = root.path().join("not").join("there");
        assert_eq!(
            PathBuf::from(canonicalize_path(&missing)),
            PathBuf::from(&real).join("not").join("there")
        );
        #[cfg(unix)]
        {
            let target = root.path().join("target");
            std::fs::create_dir(&target).unwrap();
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert_eq!(
                PathBuf::from(canonicalize_path(&link.join("missing"))),
                PathBuf::from(&real).join("target").join("missing")
            );
        }
    }

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\dev\x")),
            PathBuf::from(r"C:\dev\x")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\server\share")
        );
    }

    #[test]
    fn prefix_fallback_finds_a_directory_with_another_hash() {
        let root = tempfile::tempdir().unwrap();
        let long = format!("/{}", "z".repeat(240));
        let prefix = &sanitize_path(&long)[..MAX_SANITIZED_LENGTH];
        let other = root.path().join(format!("{prefix}-otherhash"));
        std::fs::create_dir(&other).unwrap();
        assert_eq!(find_project_dir_in(root.path(), &long), Some(other));
        assert_eq!(find_project_dir_in(root.path(), "/short"), None);
    }

    #[test]
    fn a_command_past_its_limit_is_killed_and_gives_nothing() {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("ping");
            command.args(["-n", "30", "127.0.0.1"]);
            command
        } else {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        command.current_dir(".");
        let started = Instant::now();
        assert_eq!(run_with_limit(command, Duration::from_millis(200)), None);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_missing_program_gives_nothing() {
        let command = Command::new("definitely-not-a-git-binary-4f1c");
        assert_eq!(run_with_limit(command, Duration::from_secs(1)), None);
        assert!(worktree_paths(&std::env::temp_dir().to_string_lossy()).len() <= 1);
    }

    #[test]
    fn env_override_wins_for_the_projects_directory() {
        let mut env = IndexMap::new();
        env.insert(CONFIG_DIR_VARIABLE.to_owned(), "/custom/dir".to_owned());
        assert_eq!(
            projects_dir(Some(&env)),
            PathBuf::from("/custom/dir").join("projects")
        );
    }
}
