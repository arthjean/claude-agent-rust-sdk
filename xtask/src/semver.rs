//! The API compatibility check: `cargo xtask semver-check`, run by the
//! `semver` job of `.github/workflows/ci.yml`, and the refusal of
//! `cargo xtask release prepare patch` after a break.
//!
//! Both run `cargo-semver-checks`, which compares the public API of the
//! library with its newest non-yanked, non-pre-release version on crates.io
//! and decides from the workspace version whether a difference is allowed:
//! in `0.y.z`, a `y` change may break, a `z` change or no change may not.
//! The tool reads the rustdoc JSON of the compiler that builds the two
//! versions, whose format changes with Rust releases, so the tool and the
//! toolchain are pinned together here and in the `semver` job.
//!
//! The check fails closed: a run that did not end with the tool's
//! comparison summary is an error, never a pass.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};

/// The pinned `cargo-semver-checks` release, installed with
/// `cargo install --locked`. The `semver` job repeats it.
pub const TOOL_VERSION: &str = "0.51.0";

/// The Rust release whose rustdoc JSON format [`TOOL_VERSION`] reads. The
/// `semver` job installs the same one.
pub const TOOLCHAIN: &str = "1.100.0";

/// The one crate checked. The macros crate is left out: its only public
/// item is the `tool` attribute, which the library re-exports under the
/// `macros` feature and requires at the exact same version, and rustdoc
/// describes a procedural macro by its name only, so checking it would see
/// nothing the library check does not.
pub const PACKAGE: &str = "claude-agent-rust-sdk";

/// The features left out of the check: `default` is only a list of other
/// features, and `testing` exposes test doubles outside the compatibility
/// promise.
const LEFT_OUT: [&str; 2] = ["default", "testing"];

/// The environment variable that names another program to run in place of
/// `cargo-semver-checks`, so the tests can stand one in.
const PROGRAM_VARIABLE: &str = "XTASK_SEMVER_CHECKS";

/// What the tool concluded once it compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The workspace version allows every difference found.
    Compatible,
    /// The workspace version does not allow these differences, one line
    /// per broken item, as `lint: item`.
    Breaking(Vec<String>),
}

/// The features of the library the check enables: every feature of its
/// manifest except [`LEFT_OUT`].
pub fn features(root: &Path) -> Result<Vec<String>, String> {
    let path = root.join("crates").join(PACKAGE).join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest: toml::Table = text
        .parse()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let mut features: Vec<String> = manifest
        .get("features")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default();
    features.retain(|feature| !LEFT_OUT.contains(&feature.as_str()));
    features.sort();
    Ok(features)
}

fn program() -> std::ffi::OsString {
    std::env::var_os(PROGRAM_VARIABLE).unwrap_or_else(|| "cargo-semver-checks".into())
}

fn install_hint() -> String {
    format!(
        "install it with `cargo install cargo-semver-checks@{TOOL_VERSION} --locked` and its \
         toolchain with `rustup toolchain install {TOOLCHAIN}`"
    )
}

/// Fails unless the installed tool is the pinned release.
fn require_pinned_tool() -> Result<(), String> {
    let output = Command::new(program())
        .args(["semver-checks", "--version"])
        .output()
        .map_err(|error| {
            format!(
                "cannot run cargo-semver-checks ({error}): {}",
                install_hint()
            )
        })?;
    let text = String::from_utf8_lossy(&output.stdout);
    let installed = text
        .trim()
        .strip_prefix("cargo-semver-checks ")
        .unwrap_or("");
    if !output.status.success() || installed != TOOL_VERSION {
        return Err(format!(
            "cargo-semver-checks {TOOL_VERSION} is pinned, `{}` is installed: {}",
            text.trim(),
            install_hint()
        ));
    }
    Ok(())
}

/// The arguments of one comparison, after the program name.
fn arguments(root: &Path, features: &[String]) -> Vec<String> {
    let mut arguments = vec![
        "semver-checks".to_owned(),
        "check-release".to_owned(),
        "--color".to_owned(),
        "never".to_owned(),
        "--manifest-path".to_owned(),
        root.join("Cargo.toml").to_string_lossy().into_owned(),
        "--package".to_owned(),
        PACKAGE.to_owned(),
        "--only-explicit-features".to_owned(),
    ];
    if !features.is_empty() {
        arguments.push("--features".to_owned());
        arguments.push(features.join(","));
    }
    arguments
}

/// Compares the library of the workspace at `root` with its newest release
/// on crates.io, with `features` enabled. Returns the verdict and what the
/// tool printed.
pub fn compare(root: &Path, features: &[String]) -> Result<(Verdict, String), String> {
    require_pinned_tool()?;
    let output = Command::new(program())
        .args(arguments(root, features))
        .current_dir(root)
        .env("RUSTUP_TOOLCHAIN", TOOLCHAIN)
        .env_remove("CARGO")
        .output()
        .map_err(|error| {
            format!(
                "cannot run cargo-semver-checks ({error}): {}",
                install_hint()
            )
        })?;
    let report = String::from_utf8_lossy(&output.stdout);
    let progress = String::from_utf8_lossy(&output.stderr);
    let text = format!("{report}{progress}");
    match classify(output.status.code(), &report, &progress) {
        Ok(verdict) => Ok((verdict, text)),
        Err(error) => Err(format!("{}\n{error}", text.trim_end())),
    }
}

/// The verdict of a run that exited with `code` after printing `report` on
/// its standard output and `progress` on its standard error, or why it
/// compared nothing. The failure blocks are in the report; the summary line
/// is in either stream.
fn classify(code: Option<i32>, report: &str, progress: &str) -> Result<Verdict, String> {
    let summary = report
        .lines()
        .chain(progress.lines())
        .find_map(|line| line.trim().strip_prefix("Summary "));
    match (code, summary) {
        (Some(0), Some("no semver update required")) => Ok(Verdict::Compatible),
        (Some(100), Some(summary)) if summary.starts_with("semver requires new ") => {
            Ok(Verdict::Breaking(broken_items(report)))
        }
        _ => Err(not_compared(code, &format!("{report}{progress}"))),
    }
}

/// One `lint: item` line per item listed under `Failed in:` in a failure
/// block of the tool's report, where each item is indented by two spaces.
fn broken_items(output: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut lint = None;
    let mut listing = false;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("--- failure ") {
            lint = rest.split(':').next().map(str::to_owned);
            listing = false;
        } else if line.trim() == "Failed in:" {
            listing = true;
        } else if listing
            && let Some(item) = line.strip_prefix("  ")
            && !item.starts_with(' ')
            && !item.is_empty()
            && let Some(lint) = &lint
        {
            items.push(format!("{lint}: {item}"));
        } else {
            listing = false;
        }
    }
    if items.is_empty() {
        items.push("the tool reported a break without listing its items".to_owned());
    }
    items
}

/// What cargo and the tool print when the registry cannot be read: cargo's
/// own index update, and the lookups of `src/rustdoc_gen.rs` in the tool.
/// A bare `index.crates.io` is not among them, since it is also the
/// directory of every dependency's source, which any compile error names.
const REGISTRY_FAILURES: [&str; 5] = [
    "unable to update registry",
    "failed to retrieve index of crate versions from registry",
    "failed to retrieve crate data from registry",
    "failed to open crates.io sparse index",
    "failed to read index metadata",
];

/// The error of a run that compared nothing, naming what to change.
fn not_compared(code: Option<i32>, output: &str) -> String {
    let exit = code.map_or_else(|| "a signal".to_owned(), |code| format!("exit {code}"));
    let pins = format!(
        "The pins are TOOL_VERSION ({TOOL_VERSION}) and TOOLCHAIN ({TOOLCHAIN}) in \
         xtask/src/semver.rs, repeated in the `semver` job of .github/workflows/ci.yml."
    );
    let cause = if output.contains("unsupported rustdoc format") {
        format!(
            "cargo-semver-checks {TOOL_VERSION} cannot read the rustdoc JSON of Rust {TOOLCHAIN}: \
             update the pins together, to a release of the tool that reads the format of \
             the toolchain."
        )
    } else if REGISTRY_FAILURES
        .iter()
        .any(|failure| output.contains(failure))
    {
        "crates.io could not be read: no pin is at fault; run it again once the registry \
         answers."
            .to_owned()
    } else if output.contains("toolchain") && output.contains("is not installed") {
        format!("The toolchain is missing: {}.", install_hint())
    } else if code == Some(0) {
        "The tool exited without its comparison summary.".to_owned()
    } else {
        "Read the output above; when a new tool or Rust release caused it, update the pins \
         together."
            .to_owned()
    };
    format!("cargo-semver-checks did not compare the API ({exit}).\n{cause}\n{pins}")
}

/// The text naming the broken items, one per line.
pub fn listing(items: &[String]) -> String {
    let mut text = String::new();
    for item in items {
        let _ = write!(text, "\n  {item}");
    }
    text
}

/// `cargo xtask semver-check [--features a,b] [--root PATH]`.
pub fn run(arguments: &[String]) -> ExitCode {
    let result = (|| {
        let mut root = crate::release::workspace_root();
        let mut features = None;
        let mut rest = arguments.iter();
        while let Some(argument) = rest.next() {
            match argument.as_str() {
                "--root" => {
                    root = rest
                        .next()
                        .map(Into::into)
                        .ok_or_else(|| "--root needs a path".to_owned())?;
                }
                "--features" => {
                    let list = rest
                        .next()
                        .ok_or_else(|| "--features needs a list".to_owned())?;
                    features = Some(
                        list.split(',')
                            .map(|feature| feature.trim().to_owned())
                            .filter(|feature| !feature.is_empty())
                            .collect::<Vec<_>>(),
                    );
                }
                extra => {
                    return Err(format!(
                        "unexpected argument `{extra}`\nusage: cargo xtask semver-check \
                         [--features a,b]"
                    ));
                }
            }
        }
        let features = match features {
            Some(features) => features,
            None => self::features(&root)?,
        };
        let (verdict, output) = compare(&root, &features)?;
        print!("{output}");
        match verdict {
            Verdict::Compatible => {
                println!("semver-check: {PACKAGE} keeps the API its version promises");
                Ok(())
            }
            Verdict::Breaking(items) => Err(format!(
                "{PACKAGE} breaks the API of its newest release, which its workspace version \
                 does not allow; release it with `cargo xtask release prepare minor`:{}",
                listing(&items)
            )),
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: semver-check: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BREAK: &str = "     Checked [   0.073s] 202 checks: 201 pass, 1 fail, 0 warn, 58 skip

--- failure pub_module_level_const_missing: pub module-level const is missing ---

Description:
A public const is missing or renamed
        ref: https://doc.rust-lang.org/cargo/reference/semver.html#item-remove

Failed in:
  VERSION in file /registry/claude-agent-rust-sdk-0.1.0/src/lib.rs:220

--- failure function_missing: pub fn removed or renamed ---

Failed in:
  function claude_agent_rust_sdk::query, previously in file /registry/src/lib.rs:12
  function claude_agent_rust_sdk::other, previously in file /registry/src/lib.rs:14

     Summary semver requires new major version: 2 major and 0 minor checks failed
    Finished [   2.822s] claude-agent-rust-sdk
";

    #[test]
    fn a_comparison_summary_gives_the_verdict_and_the_broken_items() {
        assert_eq!(
            classify(
                Some(0),
                "",
                "     Checked [   0.066s] 202 checks: 202 pass, 58 skip\n     Summary no semver update required\n"
            ),
            Ok(Verdict::Compatible)
        );
        assert_eq!(
            classify(Some(100), BREAK, "    Checking claude-agent-rust-sdk v0.1.0 -> v0.1.0\n     Summary semver requires new major version\n"),
            Ok(Verdict::Breaking(vec![
                "pub_module_level_const_missing: VERSION in file /registry/claude-agent-rust-sdk-0.1.0/src/lib.rs:220".to_owned(),
                "function_missing: function claude_agent_rust_sdk::query, previously in file /registry/src/lib.rs:12".to_owned(),
                "function_missing: function claude_agent_rust_sdk::other, previously in file /registry/src/lib.rs:14".to_owned(),
            ]))
        );
    }

    #[test]
    fn a_run_without_its_summary_never_passes_and_names_the_pin_to_update() {
        let format = classify(
            Some(1),
            "",
            "error: unsupported rustdoc format v61 for file: /t/doc/x.json\n(supported formats are v56, v57)\n",
        )
        .unwrap_err();
        assert!(
            format.contains("cannot read the rustdoc JSON of Rust"),
            "{format}"
        );
        assert!(
            format.contains("TOOL_VERSION (0.51.0) and TOOLCHAIN (1.100.0)"),
            "{format}"
        );
        assert!(
            format.contains("the `semver` job of .github/workflows/ci.yml"),
            "{format}"
        );

        let registry = classify(
            Some(101),
            "",
            "Caused by:\n  unable to update registry `crates-io`\n\nCaused by:\n  [7] Could not connect to server (Failed to connect to index.crates.io:443)\n",
        )
        .unwrap_err();
        assert!(
            registry.contains("crates.io could not be read: no pin is at fault"),
            "{registry}"
        );
        let lookup = classify(
            Some(1),
            "",
            "error: failed to retrieve index of crate versions from registry\n",
        )
        .unwrap_err();
        assert!(lookup.contains("crates.io could not be read"), "{lookup}");
        let compile = classify(
            Some(1),
            "",
            "error[E0308]: mismatched types\n  --> /home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.0/src/lib.rs:1:1\n",
        )
        .unwrap_err();
        assert!(
            !compile.contains("crates.io could not be read"),
            "a compile error in a dependency is not a registry failure: {compile}"
        );
        assert!(compile.contains("update the pins together"), "{compile}");

        let silent = classify(Some(0), "", "    Finished\n").unwrap_err();
        assert!(
            silent.contains("without its comparison summary"),
            "{silent}"
        );
        let mismatched = classify(
            Some(0),
            "",
            "     Summary semver requires new major version\n",
        );
        assert!(
            mismatched.is_err(),
            "a break reported with exit 0 is not a pass"
        );
        let unlisted = classify(Some(100), "", "     Summary no semver update required\n");
        assert!(unlisted.is_err());
        let killed = classify(None, "", "").unwrap_err();
        assert!(killed.contains("(a signal)"), "{killed}");
    }

    #[test]
    fn the_comparison_names_the_library_and_only_the_features_given() {
        let root = Path::new("/w");
        let with = arguments(root, &["macros".to_owned(), "otel".to_owned()]);
        let joined = with.join(" ");
        assert!(
            joined.starts_with("semver-checks check-release --color never --manifest-path "),
            "{joined}"
        );
        assert!(
            joined.ends_with(
                "--package claude-agent-rust-sdk --only-explicit-features --features macros,otel"
            ),
            "{joined}"
        );
        let without = arguments(root, &[]);
        assert_eq!(
            without.last().map(String::as_str),
            Some("--only-explicit-features")
        );
    }

    #[test]
    fn the_checked_features_are_those_of_the_manifest_but_default_and_testing() {
        let directory = tempfile::tempdir().unwrap();
        let crate_directory = directory.path().join("crates").join(PACKAGE);
        std::fs::create_dir_all(&crate_directory).unwrap();
        std::fs::write(
            crate_directory.join("Cargo.toml"),
            "[package]\nname = \"x\"\n\n[features]\ndefault = [\"b\"]\nb = []\ntesting = []\na = []\n",
        )
        .unwrap();
        assert_eq!(features(directory.path()).unwrap(), ["a", "b"]);
    }
}
