//! Locating the CLI executable.
//!
//! [`find_cli`] searches, in order: the managed install directory of this
//! crate ([`managed_install_dir`]), the directories of `PATH`, then the
//! locations the official installers use. The `cli_path` option, when set,
//! bypasses the search.
//!
//! On Windows only a native `claude.exe` is accepted, and only absolute
//! `PATH` entries are searched, so the working directory never provides the
//! executable. Batch scripts (`.bat`, `.cmd`) are refused: Windows runs
//! them through `cmd.exe`, which re-parses the whole command line and can
//! execute commands injected through argument values.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

const MANAGED_DIR_NAME: &str = "claude-agent-rust-sdk";
const MANAGED_CLI_SUBDIR: &str = "cli";
const UNIX_CLI_NAME: &str = "claude";
const WINDOWS_CLI_NAME: &str = "claude.exe";

const NOT_FOUND_UNIX: &str = "Claude CLI not found. Install with:\n  \
    npm install -g @anthropic-ai/claude-code\n\
    \nIf already installed locally, try:\n  \
    export PATH=\"$HOME/node_modules/.bin:$PATH\"\n\
    \nOr provide the path via ClaudeAgentOptions:\n  \
    ClaudeAgentOptions::builder().cli_path(\"/path/to/claude\")";

const NOT_FOUND_WINDOWS: &str = "Claude CLI not found. Install the native claude.exe with \
    (PowerShell):\n  \
    irm https://claude.ai/install.ps1 | iex\n\
    \nOr provide the path to a claude.exe via ClaudeAgentOptions:\n  \
    ClaudeAgentOptions::builder().cli_path(\"C:\\\\path\\\\to\\\\claude.exe\")\n\
    \n(npm install -g @anthropic-ai/claude-code produces a claude.cmd shim, which this SDK \
    refuses to run on Windows.)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Unix,
}

impl Platform {
    pub(crate) const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }

    const fn cli_name(self) -> &'static str {
        match self {
            Self::Windows => WINDOWS_CLI_NAME,
            Self::Unix => UNIX_CLI_NAME,
        }
    }
}

/// Directory where this crate keeps a CLI it installed itself.
///
/// The CLI found there takes precedence over `PATH`. The directory is
/// `%LOCALAPPDATA%\claude-agent-rust-sdk\cli` on Windows,
/// `~/Library/Application Support/claude-agent-rust-sdk/cli` on macOS, and
/// `$XDG_DATA_HOME/claude-agent-rust-sdk/cli` (default
/// `~/.local/share/claude-agent-rust-sdk/cli`) elsewhere. Returns `None` when
/// the base directory cannot be determined.
#[must_use]
pub fn managed_install_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        absolute_from_env("LOCALAPPDATA")?
    } else if cfg!(target_os = "macos") {
        std::env::home_dir()?
            .join("Library")
            .join("Application Support")
    } else {
        match absolute_from_env("XDG_DATA_HOME") {
            Some(data_home) => data_home,
            None => std::env::home_dir()?.join(".local").join("share"),
        }
    };
    Some(base.join(MANAGED_DIR_NAME).join(MANAGED_CLI_SUBDIR))
}

fn absolute_from_env(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os(name)?);
    path.is_absolute().then_some(path)
}

/// Finds the CLI executable without spawning anything.
///
/// # Errors
///
/// Returns [`Error::CliNotFound`], with install instructions, when no
/// location holds the executable.
pub fn find_cli() -> Result<PathBuf> {
    CliLocator::from_environment().find()
}

#[derive(Debug, Clone)]
pub(crate) struct CliLocator {
    pub(crate) platform: Platform,
    pub(crate) search_path: Option<OsString>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) managed_dir: Option<PathBuf>,
}

impl CliLocator {
    pub(crate) fn from_environment() -> Self {
        Self {
            platform: Platform::current(),
            search_path: std::env::var_os("PATH"),
            home: std::env::home_dir(),
            managed_dir: managed_install_dir(),
        }
    }

    pub(crate) fn find(&self) -> Result<PathBuf> {
        let name = self.platform.cli_name();
        if let Some(managed) = &self.managed_dir {
            let candidate = managed.join(name);
            if candidate.is_file() {
                tracing::info!(path = %candidate.display(), "using the managed Claude CLI");
                return Ok(candidate);
            }
        }
        if let Some(found) = self.search_path_entries(name) {
            return Ok(found);
        }
        self.known_locations()
            .into_iter()
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| self.not_found())
    }

    fn search_path_entries(&self, name: &str) -> Option<PathBuf> {
        let search_path = self.search_path.as_ref()?;
        std::env::split_paths(search_path).find_map(|directory| match self.platform {
            Platform::Windows => {
                let candidate = directory.join(name);
                (directory.is_absolute() && candidate.is_file()).then_some(candidate)
            }
            Platform::Unix => {
                let candidate = if directory.as_os_str().is_empty() {
                    PathBuf::from(name)
                } else {
                    directory.join(name)
                };
                is_executable_file(&candidate).then_some(candidate)
            }
        })
    }

    fn known_locations(&self) -> Vec<PathBuf> {
        let home = self.home.as_deref();
        let under_home = |relative: &[&str]| {
            home.map(|home| {
                relative
                    .iter()
                    .fold(home.to_path_buf(), |path, part| path.join(part))
            })
        };
        let locations = match self.platform {
            Platform::Windows => vec![under_home(&[".local", "bin", WINDOWS_CLI_NAME])],
            Platform::Unix => vec![
                under_home(&[".npm-global", "bin", UNIX_CLI_NAME]),
                Some(PathBuf::from("/usr/local/bin/claude")),
                under_home(&[".local", "bin", UNIX_CLI_NAME]),
                under_home(&["node_modules", ".bin", UNIX_CLI_NAME]),
                under_home(&[".yarn", "bin", UNIX_CLI_NAME]),
                under_home(&[".claude", "local", UNIX_CLI_NAME]),
            ],
        };
        locations.into_iter().flatten().collect()
    }

    fn not_found(&self) -> Error {
        let message = match self.platform {
            Platform::Windows => NOT_FOUND_WINDOWS,
            Platform::Unix => NOT_FOUND_UNIX,
        };
        Error::cli_not_found(Some(message), None)
    }
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn is_windows_batch_cli(cli_path: &str) -> bool {
    cli_path
        .replace('\\', "/")
        .split('/')
        .flat_map(|component| component.split(':'))
        .any(|segment| {
            let name = segment.trim_end_matches(['.', ' ']).to_lowercase();
            name.ends_with(".bat") || name.ends_with(".cmd")
        })
}

pub(crate) fn reject_windows_batch_cli(cli_path: &Path, platform: Platform) -> Result<()> {
    if platform != Platform::Windows || !is_windows_batch_cli(&cli_path.to_string_lossy()) {
        return Ok(());
    }
    Err(Error::cli_connection(format!(
        "Refusing to execute batch script {:?}: Windows runs .bat/.cmd files via cmd.exe, which \
         can execute commands injected through CLI arguments, and no reliable escaping for \
         cmd.exe exists. Use a native claude executable instead: install the CLI natively (irm \
         https://claude.ai/install.ps1 | iex), or point the cli_path option at a claude.exe.",
        cli_path.display().to_string()
    )))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use super::{CliLocator, Platform, is_windows_batch_cli, reject_windows_batch_cli};
    use crate::error::Error;

    fn add(directory: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(directory).unwrap();
        let path = directory.join(name);
        std::fs::write(&path, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    fn locator(platform: Platform, root: &Path, entries: &[PathBuf]) -> CliLocator {
        CliLocator {
            platform,
            search_path: Some(std::env::join_paths(entries).unwrap()),
            home: Some(root.join("home")),
            managed_dir: None,
        }
    }

    fn not_found_message(result: crate::error::Result<PathBuf>) -> String {
        match result {
            Err(Error::CliNotFound { message }) => message,
            other => panic!("expected CliNotFound, got {other:?}"),
        }
    }

    #[test]
    fn windows_finds_native_exe_on_path() {
        let root = tempfile::tempdir().unwrap();
        let exe = add(&root.path().join("bin"), "claude.exe");
        let found = locator(
            Platform::Windows,
            root.path(),
            &[root.path().join("missing"), root.path().join("bin")],
        )
        .find()
        .unwrap();
        assert_eq!(found, exe);
    }

    #[test]
    fn windows_never_searches_relative_path_entries() {
        let root = tempfile::tempdir().unwrap();
        let exe = add(&root.path().join("bin"), "claude.exe");
        let mut locator = locator(Platform::Windows, root.path(), &[]);
        locator.search_path = Some(
            std::env::join_paths([
                PathBuf::new(),
                PathBuf::from("."),
                PathBuf::from("tools"),
                root.path().join("bin"),
            ])
            .unwrap(),
        );
        assert_eq!(locator.find().unwrap(), exe);

        locator.search_path =
            Some(std::env::join_paths([PathBuf::new(), PathBuf::from(".")]).unwrap());
        not_found_message(locator.find());
    }

    #[test]
    fn windows_shim_and_wrapper_do_not_shadow_native_exe() {
        let root = tempfile::tempdir().unwrap();
        add(&root.path().join("npm"), "claude.cmd");
        add(&root.path().join("npm"), "claude.exe.cmd");
        add(&root.path().join("wsl"), "claude");
        let exe = add(&root.path().join("bin"), "claude.exe");
        let found = locator(
            Platform::Windows,
            root.path(),
            &[
                root.path().join("npm"),
                root.path().join("wsl"),
                root.path().join("bin"),
            ],
        )
        .find()
        .unwrap();
        assert_eq!(found, exe);
    }

    #[test]
    fn windows_shim_only_machine_gets_the_native_install_message() {
        let root = tempfile::tempdir().unwrap();
        add(&root.path().join("npm"), "claude.cmd");
        let message = not_found_message(
            locator(Platform::Windows, root.path(), &[root.path().join("npm")]).find(),
        );
        assert!(message.contains("claude.cmd shim"));
        assert!(message.contains("install.ps1"));
        assert!(message.contains("claude.exe"));
        assert!(message.find("install.ps1").unwrap() < message.find("npm").unwrap());
        assert!(message.contains("refuses"));
    }

    #[test]
    fn windows_path_hit_does_not_need_a_home_directory() {
        let root = tempfile::tempdir().unwrap();
        let exe = add(&root.path().join("bin"), "claude.exe");
        let mut locator = locator(Platform::Windows, root.path(), &[root.path().join("bin")]);
        locator.home = None;
        assert_eq!(locator.find().unwrap(), exe);
    }

    #[test]
    fn windows_native_installer_location_is_the_fallback() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("home").join(".local").join("bin");
        let exe = add(&bin, "claude.exe");
        add(&bin, "claude");
        let found = locator(
            Platform::Windows,
            root.path(),
            &[root.path().join("missing")],
        )
        .find()
        .unwrap();
        assert_eq!(found, exe);
    }

    #[test]
    fn unix_finds_the_cli_on_path_before_known_locations() {
        let root = tempfile::tempdir().unwrap();
        let on_path = add(&root.path().join("bin"), "claude");
        add(
            &root.path().join("home").join(".local").join("bin"),
            "claude",
        );
        let found = locator(Platform::Unix, root.path(), &[root.path().join("bin")])
            .find()
            .unwrap();
        assert_eq!(found, on_path);
    }

    #[test]
    fn unix_falls_back_to_known_locations_in_upstream_order() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let yarn = add(&home.join(".yarn").join("bin"), "claude");
        add(&home.join(".claude").join("local"), "claude");
        let locator = locator(Platform::Unix, root.path(), &[root.path().join("missing")]);
        if Path::new("/usr/local/bin/claude").is_file() {
            return;
        }
        assert_eq!(locator.find().unwrap(), yarn);

        let npm_global = add(&home.join(".npm-global").join("bin"), "claude");
        assert_eq!(locator.find().unwrap(), npm_global);
    }

    #[test]
    fn unix_not_found_message_gives_the_install_instruction() {
        let root = tempfile::tempdir().unwrap();
        let mut locator = locator(Platform::Unix, root.path(), &[]);
        locator.search_path = Some(OsString::new());
        if Path::new("/usr/local/bin/claude").is_file() {
            return;
        }
        let message = not_found_message(locator.find());
        assert!(message.starts_with(
            "Claude CLI not found. Install with:\n  npm install -g @anthropic-ai/claude-code\n"
        ));
        assert!(!message.contains("install.ps1"));
    }

    #[test]
    fn managed_install_directory_takes_precedence_over_path() {
        let root = tempfile::tempdir().unwrap();
        for platform in [Platform::Windows, Platform::Unix] {
            let name = platform.cli_name();
            add(&root.path().join("bin"), name);
            let managed = add(&root.path().join("managed"), name);
            let mut locator = locator(platform, root.path(), &[root.path().join("bin")]);
            locator.managed_dir = Some(root.path().join("managed"));
            assert_eq!(locator.find().unwrap(), managed);
        }
    }

    #[test]
    fn batch_scripts_are_recognized_under_every_spelling() {
        for path in [
            "C:\\tools\\claude.bat",
            "C:\\tools\\claude.cmd.",
            "C:\\tools\\claude.CMD ",
            "C:\\tools\\claude.cmd:stream",
            "C:\\tools\\.cmd",
            "C:claude.cmd",
            "C:/tools/claude.cmd",
            "\\\\server\\share\\claude.cmd",
            "C:\\tools\\claude.cmd\\.",
            "C:\\tools\\claude.cmd\\x\\..",
            "C:\\tools\\claude.cmd\\x\\.. ",
            "C:\\tools\\claude.cmd\\x\\.. .",
            "C:\\tools\\claude.cmd\\\\.",
            "C:/tools/claude.cmd//x/..",
            "C:\\tools\\claude.cmd\\",
            "C:\\tools\\claude.cmd\\...",
            "C:\\tools\\claude.cmd\\....",
            "C:\\tools\\claude:evil.cmd",
            "C:\\tools\\claude.exe:evil.cmd",
            ":claude.cmd",
            "C:\\tools\\claude.cmd\\...\\..",
            "C:\\tools\\claude.cmd\\. .\\..",
            "C:\\tools\\claude.cmd\\ \\..",
            "C:\\tools\\claude.cmd\\.. \\..",
        ] {
            assert!(is_windows_batch_cli(path), "{path:?}");
            let refusal = reject_windows_batch_cli(Path::new(path), Platform::Windows);
            match refusal {
                Err(Error::CliConnection { message }) => {
                    assert!(message.contains("batch script"), "{path:?}");
                }
                other => panic!("expected a refusal for {path:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn native_executables_are_not_batch_scripts() {
        for path in [
            "C:\\Users\\u\\.local\\bin\\claude.EXE",
            "/usr/local/bin/claude",
            "C:\\tools\\cmd\\claude.exe",
            "C:\\tools\\batch\\claude.exe",
        ] {
            assert!(!is_windows_batch_cli(path), "{path:?}");
        }
    }

    #[test]
    fn batch_guard_is_a_no_op_off_windows() {
        assert!(reject_windows_batch_cli(Path::new("/odd/claude.cmd"), Platform::Unix).is_ok());
    }
}
