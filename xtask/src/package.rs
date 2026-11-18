//! `cargo xtask package-check`: packages the two published crates the way
//! a release does, lets cargo build each archive, and checks what the
//! archives hold.
//!
//! Both crates are packaged in one cargo call, so the library is verified
//! against the macros archive of the same run instead of a version from the
//! registry. `--allow-dirty` is never passed: uncommitted changes in a
//! packaged file fail the command with cargo's own message.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// The compressed size an archive may not exceed: 1 MiB.
const SIZE_LIMIT: u64 = 1024 * 1024;

/// The manifest fields the registry page and the license checks of users
/// depend on.
const REQUIRED_FIELDS: [&str; 4] = ["repository", "license", "description", "readme"];

/// The files cargo writes into every archive.
const GENERATED: [&str; 4] = [
    ".cargo_vcs_info.json",
    "Cargo.lock",
    "Cargo.toml",
    "Cargo.toml.orig",
];

/// What one published crate may and must package.
pub(crate) struct Spec {
    /// The package name.
    pub(crate) name: &'static str,
    /// The files at the root of the archive that must be there. No other
    /// file is allowed at the root, apart from the ones cargo generates.
    pub(crate) required: &'static [&'static str],
    /// The directories whose files are allowed, each ending with `/`.
    pub(crate) directories: &'static [&'static str],
    /// The directory of the crate, relative to the workspace root.
    pub(crate) path: &'static str,
    /// The directories of the crate that stay out of the archive, each
    /// ending with `/`.
    pub(crate) unpackaged: &'static [&'static str],
}

/// The published crates, in dependency order.
pub(crate) const SPECS: [Spec; 2] = [
    Spec {
        name: "claude-agent-rust-sdk-macros",
        required: &["README.md", "LICENSE-MIT", "LICENSE-APACHE"],
        directories: &["src/"],
        path: "crates/claude-agent-rust-sdk-macros",
        unpackaged: &[],
    },
    Spec {
        name: "claude-agent-rust-sdk",
        required: &[
            "README.md",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "THIRD_PARTY_NOTICES",
        ],
        directories: &["src/", "examples/"],
        path: "crates/claude-agent-rust-sdk",
        unpackaged: &["tests/"],
    },
];

/// One packaged archive, as read back from disk.
pub(crate) struct Archive {
    /// The paths of the archive, relative to its top-level directory.
    pub(crate) files: Vec<String>,
    /// The `Cargo.toml` cargo wrote into the archive.
    pub(crate) manifest: String,
    /// The size of the `.crate` file in bytes.
    pub(crate) compressed_bytes: u64,
}

/// Every rule `archive` breaks, each as one message naming its cause. An
/// empty list means the archive may be published.
pub(crate) fn violations(spec: &Spec, archive: &Archive) -> Vec<String> {
    let name = spec.name;
    let mut found = Vec::new();
    for file in &archive.files {
        let generated = GENERATED.contains(&file.as_str());
        let required = spec.required.contains(&file.as_str());
        let in_directory = spec
            .directories
            .iter()
            .any(|directory| file.starts_with(directory));
        if !(generated || required || in_directory) {
            found.push(format!(
                "{name}: the archive holds `{file}`, which is outside the allowed set"
            ));
        } else if file.ends_with(".profraw") {
            found.push(format!(
                "{name}: the archive holds the coverage profile `{file}`"
            ));
        }
    }
    for file in spec.required {
        if !archive.files.iter().any(|packaged| packaged == file) {
            found.push(format!("{name}: the archive misses `{file}`"));
        }
    }
    match archive.manifest.parse::<toml::Table>() {
        Ok(manifest) => {
            let package = manifest.get("package").and_then(toml::Value::as_table);
            for field in REQUIRED_FIELDS {
                let value = package
                    .and_then(|package| package.get(field))
                    .and_then(toml::Value::as_str);
                if value.is_none_or(str::is_empty) {
                    found.push(format!("{name}: the packaged manifest misses `{field}`"));
                }
            }
        }
        Err(error) => found.push(format!(
            "{name}: the packaged manifest is not valid TOML: {error}"
        )),
    }
    if archive.compressed_bytes > SIZE_LIMIT {
        found.push(format!(
            "{name}: the archive is {} bytes compressed, over the limit of {SIZE_LIMIT}",
            archive.compressed_bytes
        ));
    }
    found
}

/// The files of a crate directory that belong to none of its known places,
/// each as one message. `paths` are relative to the crate directory and
/// hold every file git tracks or would track there, so a stray file is
/// reported whether or not it is committed, and whether or not the `include`
/// list of the manifest would package it.
pub(crate) fn stray_files(spec: &Spec, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| {
            let known_file =
                path.as_str() == "Cargo.toml" || spec.required.contains(&path.as_str());
            let known_directory = spec
                .directories
                .iter()
                .chain(spec.unpackaged)
                .any(|directory| path.starts_with(directory));
            !(known_file || known_directory)
        })
        .map(|path| {
            format!(
                "{}: `{}/{path}` is not a file this crate is expected to hold",
                spec.name, spec.path
            )
        })
        .collect()
}

/// The files git tracks or would track under `directory`, relative to it.
fn files_known_to_git(root: &Path, directory: &str) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .current_dir(root.join(directory))
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()
        .map_err(|error| format!("git could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed in {directory}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect())
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// The version shared by the workspace members.
fn workspace_version(root: &Path) -> Result<String, String> {
    let path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let manifest: toml::Table = text
        .parse()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: no [workspace.package] version", path.display()))
}

/// The directory cargo builds into, as cargo itself reports it.
fn target_directory(root: &Path) -> Result<PathBuf, String> {
    let output = Command::new(cargo())
        .current_dir(root)
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
        .output()
        .map_err(|error| format!("cargo metadata could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata printed invalid JSON: {error}"))?;
    metadata
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata gave no target_directory".to_owned())
}

/// Runs `tar` on `archive` and returns what it printed.
fn tar(arguments: &[&str], archive: &Path) -> Result<String, String> {
    let output = Command::new("tar")
        .args(arguments)
        .arg(archive)
        .output()
        .map_err(|error| format!("tar could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "tar failed on {}: {}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| {
        format!(
            "tar printed invalid UTF-8 for {}: {error}",
            archive.display()
        )
    })
}

/// Reads back the archive cargo wrote for `name` at `version`.
fn read_archive(directory: &Path, name: &str, version: &str) -> Result<Archive, String> {
    let top = format!("{name}-{version}");
    let path = directory.join(format!("{top}.crate"));
    let compressed_bytes = std::fs::metadata(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?
        .len();
    let prefix = format!("{top}/");
    let listing = tar(&["-tzf"], &path)?;
    let mut files = Vec::new();
    for line in listing.lines().filter(|line| !line.ends_with('/')) {
        let file = line.strip_prefix(&prefix).ok_or_else(|| {
            format!(
                "{}: `{line}` is outside the directory `{top}`",
                path.display()
            )
        })?;
        files.push(file.to_owned());
    }
    let member = format!("{top}/Cargo.toml");
    let output = Command::new("tar")
        .arg("-xzOf")
        .arg(&path)
        .arg(&member)
        .output()
        .map_err(|error| format!("tar could not start: {error}"))?;
    if !output.status.success() {
        return Err(format!("{}: cannot extract `{member}`", path.display()));
    }
    Ok(Archive {
        files,
        manifest: String::from_utf8_lossy(&output.stdout).into_owned(),
        compressed_bytes,
    })
}

fn check(root: &Path) -> Result<Vec<String>, String> {
    let version = workspace_version(root)?;
    let mut stray = Vec::new();
    for spec in &SPECS {
        stray.extend(stray_files(spec, &files_known_to_git(root, spec.path)?));
    }
    if !stray.is_empty() {
        return Ok(stray);
    }
    let mut command = Command::new(cargo());
    command.current_dir(root).args(["package", "--locked"]);
    for spec in &SPECS {
        command.args(["--package", spec.name]);
    }
    eprintln!(
        "xtask: cargo package --locked{}",
        SPECS
            .iter()
            .map(|spec| format!(" --package {}", spec.name))
            .collect::<String>()
    );
    let status = command
        .status()
        .map_err(|error| format!("cargo package could not start: {error}"))?;
    if !status.success() {
        return Err(format!("cargo package failed with {status}"));
    }
    let directory = target_directory(root)?.join("package");
    let mut found = Vec::new();
    for spec in &SPECS {
        let archive = read_archive(&directory, spec.name, &version)?;
        eprintln!(
            "xtask: {} {version}: {} files, {} bytes compressed",
            spec.name,
            archive.files.len(),
            archive.compressed_bytes
        );
        found.extend(violations(spec, &archive));
    }
    Ok(found)
}

/// `cargo xtask package-check`.
pub fn run(arguments: &[String]) -> ExitCode {
    if let Some(unknown) = arguments.first() {
        eprintln!("xtask: package-check takes no argument, got `{unknown}`");
        return ExitCode::from(2);
    }
    match check(&workspace_root()) {
        Ok(found) if found.is_empty() => {
            eprintln!("xtask: both crates package, verify, and hold the expected files");
            ExitCode::SUCCESS
        }
        Ok(found) => {
            for violation in found {
                eprintln!("xtask: {violation}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[package]\nname = \"claude-agent-rust-sdk\"\n\
        description = \"A crate\"\nlicense = \"MIT OR Apache-2.0\"\n\
        repository = \"https://example.com/repository\"\nreadme = \"README.md\"\n";

    fn library() -> &'static Spec {
        &SPECS[1]
    }

    fn archive(files: &[&str]) -> Archive {
        Archive {
            files: files.iter().map(|file| (*file).to_owned()).collect(),
            manifest: MANIFEST.to_owned(),
            compressed_bytes: 350 * 1024,
        }
    }

    const EXPECTED: [&str; 11] = [
        ".cargo_vcs_info.json",
        "Cargo.lock",
        "Cargo.toml",
        "Cargo.toml.orig",
        "README.md",
        "LICENSE-MIT",
        "LICENSE-APACHE",
        "THIRD_PARTY_NOTICES",
        "src/lib.rs",
        "src/process/sys.rs",
        "examples/quick_start.rs",
    ];

    #[test]
    fn an_archive_with_exactly_the_expected_files_passes() {
        assert_eq!(violations(library(), &archive(&EXPECTED)), [""; 0]);
    }

    #[test]
    fn a_stray_file_is_reported_by_path() {
        let mut files = EXPECTED.to_vec();
        files.extend(["tests/query.rs", "notes.txt", "src/default_1.profraw"]);

        assert_eq!(
            violations(library(), &archive(&files)),
            [
                "claude-agent-rust-sdk: the archive holds `tests/query.rs`, which is outside \
                 the allowed set",
                "claude-agent-rust-sdk: the archive holds `notes.txt`, which is outside the \
                 allowed set",
                "claude-agent-rust-sdk: the archive holds the coverage profile \
                 `src/default_1.profraw`",
            ]
        );
    }

    #[test]
    fn a_file_allowed_for_the_library_only_is_a_stray_file_of_the_macros_crate() {
        let files = [
            "Cargo.toml",
            "README.md",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "THIRD_PARTY_NOTICES",
            "src/lib.rs",
            "examples/quick_start.rs",
        ];

        assert_eq!(
            violations(&SPECS[0], &archive(&files)),
            [
                "claude-agent-rust-sdk-macros: the archive holds `THIRD_PARTY_NOTICES`, which \
                 is outside the allowed set",
                "claude-agent-rust-sdk-macros: the archive holds `examples/quick_start.rs`, \
                 which is outside the allowed set",
            ]
        );
    }

    #[test]
    fn a_file_new_to_a_crate_directory_is_reported_before_packaging() {
        let paths: Vec<String> = [
            "Cargo.toml",
            "LICENSE-MIT",
            "THIRD_PARTY_NOTICES",
            "src/lib.rs",
            "examples/quick_start.rs",
            "tests/query.rs",
            "notes.txt",
            "benches/spawn.rs",
        ]
        .iter()
        .map(|path| (*path).to_owned())
        .collect();

        assert_eq!(
            stray_files(library(), &paths),
            [
                "claude-agent-rust-sdk: `crates/claude-agent-rust-sdk/notes.txt` is not a file \
                 this crate is expected to hold",
                "claude-agent-rust-sdk: `crates/claude-agent-rust-sdk/benches/spawn.rs` is not \
                 a file this crate is expected to hold",
            ]
        );
        let found = stray_files(&SPECS[0], &paths);
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn a_missing_license_or_notice_is_reported_by_name() {
        let files: Vec<&str> = EXPECTED
            .iter()
            .copied()
            .filter(|file| !["LICENSE-APACHE", "THIRD_PARTY_NOTICES"].contains(file))
            .collect();

        assert_eq!(
            violations(library(), &archive(&files)),
            [
                "claude-agent-rust-sdk: the archive misses `LICENSE-APACHE`",
                "claude-agent-rust-sdk: the archive misses `THIRD_PARTY_NOTICES`",
            ]
        );
    }

    #[test]
    fn an_oversized_archive_is_reported_with_its_size() {
        let mut oversized = archive(&EXPECTED);
        oversized.compressed_bytes = SIZE_LIMIT;
        assert_eq!(violations(library(), &oversized), [""; 0]);

        oversized.compressed_bytes = SIZE_LIMIT + 1;
        assert_eq!(
            violations(library(), &oversized),
            [
                "claude-agent-rust-sdk: the archive is 1048577 bytes compressed, over the limit \
              of 1048576"
            ]
        );
    }

    #[test]
    fn a_missing_or_empty_manifest_field_is_reported_by_name() {
        let mut incomplete = archive(&EXPECTED);
        incomplete.manifest = "[package]\nname = \"claude-agent-rust-sdk\"\n\
            license = \"MIT OR Apache-2.0\"\ndescription = \"\"\n"
            .to_owned();

        assert_eq!(
            violations(library(), &incomplete),
            [
                "claude-agent-rust-sdk: the packaged manifest misses `repository`",
                "claude-agent-rust-sdk: the packaged manifest misses `description`",
                "claude-agent-rust-sdk: the packaged manifest misses `readme`",
            ]
        );

        incomplete.manifest = "[package".to_owned();
        let found = violations(library(), &incomplete);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("is not valid TOML"), "{found:?}");
    }
}
