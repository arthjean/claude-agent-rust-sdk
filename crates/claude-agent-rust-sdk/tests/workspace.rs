#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use claude_agent_rust_sdk::{CLI_VERSION, MINIMUM_CLI_VERSION, UPSTREAM_SDK_VERSION};

const UPSTREAM_COMMIT: &str = "69846cdf82aef59add3b14a26b4f16d1ccee26ba";
const UPSTREAM_PATH: &str = "upstream/claude-agent-sdk-python";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The files git tracks, as paths relative to the repository root, or `None`
/// outside a git checkout.
fn tracked_files() -> Option<Vec<String>> {
    let root = workspace_root();
    if !root.join(".git").exists() {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .ok()?;
    assert!(output.status.success());
    let listing = String::from_utf8(output.stdout).unwrap();
    Some(
        listing
            .split('\0')
            .filter(|path| !path.is_empty() && root.join(path).is_file())
            .map(str::to_owned)
            .collect(),
    )
}

/// The tracked files that are text, each with its content.
fn tracked_texts() -> Option<Vec<(String, String)>> {
    let files = tracked_files()?
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(workspace_root().join(&path)).ok()?;
            Some((path, text))
        })
        .collect();
    Some(files)
}

/// Dated records and closed PRDs keep the names and the license of their day.
const DATED_RECORDS: [&str; 4] = ["docs/research/", "docs/validation/", "tasks/", "upstream/"];

/// The occurrences of the former crate name, in its three spellings, in
/// `files`: one `path:line` per hit, the path itself included.
fn former_name_hits(files: &[(String, String)]) -> Vec<String> {
    let former = ["claude-agent", "kit"].join("-");
    let spellings = [
        former.clone(),
        former.replace('-', "_"),
        former.replace('-', "_").to_uppercase(),
    ];
    let mut hits = Vec::new();
    for (path, text) in files {
        if DATED_RECORDS.iter().any(|record| path.starts_with(record)) {
            continue;
        }
        if spellings.iter().any(|spelling| path.contains(spelling)) {
            hits.push(path.clone());
        }
        for (number, line) in text.lines().enumerate() {
            if spellings.iter().any(|spelling| line.contains(spelling)) {
                hits.push(format!("{path}:{}", number + 1));
            }
        }
    }
    hits
}

#[test]
fn no_tracked_file_outside_dated_records_holds_the_former_crate_name() {
    let Some(files) = tracked_texts() else {
        return;
    };
    assert!(files.len() > 100, "{} files read", files.len());
    let hits = former_name_hits(&files);
    assert!(
        hits.is_empty(),
        "the former crate name remains in {hits:#?}"
    );
}

#[test]
fn the_former_name_check_reports_each_spelling_and_spares_dated_records() {
    let former = ["claude-agent", "kit"].join("-");
    let snake = former.replace('-', "_");
    let files = vec![
        (
            "README.md".to_owned(),
            format!("first\ncargo add {former}\n"),
        ),
        ("src/lib.rs".to_owned(), format!("use {snake}::query;\n")),
        (
            "ci.yml".to_owned(),
            format!("{}_E2E_AUTH: cli\n", snake.to_uppercase()),
        ),
        (
            format!("crates/{former}/Cargo.toml"),
            "[package]\n".to_owned(),
        ),
        ("docs/research/old.md".to_owned(), format!("{former}\n")),
        ("tasks/prd.md".to_owned(), format!("{snake}\n")),
        (
            "docs/guide.md".to_owned(),
            "claude-agent-rust-sdk\n".to_owned(),
        ),
    ];
    assert_eq!(
        former_name_hits(&files),
        [
            "README.md:2".to_owned(),
            "src/lib.rs:1".to_owned(),
            "ci.yml:1".to_owned(),
            format!("crates/{former}/Cargo.toml"),
        ]
    );
}

#[test]
fn the_entrypoint_label_and_the_run_time_names_follow_the_crate_name() {
    let environment = read("crates/claude-agent-rust-sdk/src/transport/environment.rs");
    assert!(environment.contains("const ENTRYPOINT: &str = \"sdk-rs\";"));
    let cli = read("crates/claude-agent-rust-sdk/src/cli.rs");
    assert!(cli.contains("const MANAGED_DIR_NAME: &str = \"claude-agent-rust-sdk\";"));
    let process = read("crates/claude-agent-rust-sdk/src/process/mod.rs");
    assert!(process.contains("const THREAD_NAME: &str = \"claude-agent-rust-sdk-spawn\";"));
    let manifest = read("crates/claude-agent-rust-sdk/Cargo.toml");
    assert!(
        !manifest.contains("[lib]"),
        "the library name follows the package name"
    );
}

#[test]
fn workspace_has_its_five_members() {
    let manifest = read("Cargo.toml");
    for member in [
        "crates/claude-agent-rust-sdk",
        "crates/claude-agent-rust-sdk-macros",
        "crates/fake-claude",
        "crates/parity-driver",
        "xtask",
    ] {
        assert!(manifest.contains(&format!("\"{member}\"")), "{member}");
        assert!(workspace_root().join(member).join("Cargo.toml").is_file());
    }

    let library = read("crates/claude-agent-rust-sdk/Cargo.toml");
    assert!(library.contains("name = \"claude-agent-rust-sdk\""));
    assert!(!library.contains("publish = false"));

    let macros = read("crates/claude-agent-rust-sdk-macros/Cargo.toml");
    assert!(macros.contains("name = \"claude-agent-rust-sdk-macros\""));
    assert!(macros.contains("proc-macro = true"));

    for (path, name) in [
        ("crates/fake-claude/Cargo.toml", "fake-claude"),
        ("crates/parity-driver/Cargo.toml", "parity-driver"),
        ("xtask/Cargo.toml", "xtask"),
    ] {
        let manifest = read(path);
        assert!(manifest.contains(&format!("name = \"{name}\"")));
        assert!(manifest.contains("publish = false"), "{name}");
        let main = Path::new(path).with_file_name("src").join("main.rs");
        assert!(workspace_root().join(main).is_file(), "{name} is a binary");
    }
}

#[test]
fn root_manifest_sets_toolchain_license_and_lint_policy() {
    let manifest = compact(&read("Cargo.toml"));
    for setting in [
        "rust-version = \"1.88\"",
        "edition = \"2024\"",
        "license = \"MIT OR Apache-2.0\"",
        "[workspace.lints.clippy]",
        "panic = \"deny\"",
        "unimplemented = \"deny\"",
        "dbg_macro = \"deny\"",
        "todo = \"warn\"",
        "unwrap_used = \"warn\"",
        "expect_used = \"warn\"",
    ] {
        assert!(manifest.contains(setting), "{setting}");
    }

    for member in [
        "crates/claude-agent-rust-sdk",
        "crates/claude-agent-rust-sdk-macros",
        "crates/fake-claude",
        "crates/parity-driver",
        "xtask",
    ] {
        let manifest = compact(&read(&format!("{member}/Cargo.toml")));
        assert!(manifest.contains("[lints] workspace = true"), "{member}");
        assert!(manifest.contains("license.workspace = true"), "{member}");
        assert!(manifest.contains("edition.workspace = true"), "{member}");
        assert!(
            manifest.contains("rust-version.workspace = true"),
            "{member}"
        );
    }
}

/// The version every member inherits from `[workspace.package]`.
fn workspace_version() -> String {
    let manifest = read("Cargo.toml");
    let package = manifest
        .split("[workspace.package]")
        .nth(1)
        .expect("the root manifest has no [workspace.package]");
    let version = package
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("the workspace has no version");
    version.to_owned()
}

/// The quoted strings of the array `key` in `manifest`.
fn manifest_list(manifest: &str, key: &str) -> Vec<String> {
    let list = manifest
        .split(&format!("\n{key} = ["))
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_else(|| panic!("the manifest has no `{key}` list"));
    list.split(',')
        .map(|entry| entry.trim().trim_matches('"').to_owned())
        .filter(|entry| !entry.is_empty())
        .collect()
}

const REPOSITORY: &str = "https://github.com/arthjean/claude-agent-rust-sdk";
const LIBRARY: &str = "crates/claude-agent-rust-sdk";
const MACROS: &str = "crates/claude-agent-rust-sdk-macros";

#[test]
fn published_manifests_carry_the_registry_metadata() {
    let root = compact(&read("Cargo.toml"));
    assert!(
        root.contains("repository = \"https://github.com/arthjean/claude-agent-rust-sdk\""),
        "repository"
    );
    assert!(
        !root.contains("homepage ="),
        "cargo warns that a homepage equal to the repository is redundant"
    );
    let categories = [
        "api-bindings",
        "asynchronous",
        "development-tools::procedural-macro-helpers",
    ];
    for (member, name, readme) in [
        (LIBRARY, "claude-agent-rust-sdk", "../../README.md"),
        (MACROS, "claude-agent-rust-sdk-macros", "README.md"),
    ] {
        let manifest = read(&format!("{member}/Cargo.toml"));
        for setting in [
            "\ndescription = \"".to_owned(),
            "\nlicense.workspace = true\n".to_owned(),
            "\nrepository.workspace = true\n".to_owned(),
            format!("\ndocumentation = \"https://docs.rs/{name}\"\n"),
            format!("\nreadme = \"{readme}\"\n"),
        ] {
            assert!(manifest.contains(&setting), "{member}: {setting}");
        }
        assert!(!manifest.contains("publish = false"), "{member}");
        let keywords = manifest_list(&manifest, "keywords");
        assert!((1..=5).contains(&keywords.len()), "{member}: {keywords:?}");
        let declared = manifest_list(&manifest, "categories");
        assert!((1..=5).contains(&declared.len()), "{member}: {declared:?}");
        for category in &declared {
            assert!(
                categories.contains(&category.as_str()),
                "{member}: {category}"
            );
        }
    }

    let readme = read(&format!("{MACROS}/README.md"));
    assert!(
        compact(&readme)
            .contains("It is used through the `macros` feature of `claude-agent-rust-sdk`")
    );
}

#[test]
fn the_library_requires_the_macros_crate_at_the_exact_workspace_version() {
    let manifest = read(&format!("{LIBRARY}/Cargo.toml"));
    let requirement = format!(
        "claude-agent-rust-sdk-macros = {{ path = \"../claude-agent-rust-sdk-macros\", \
         version = \"={}\", optional = true }}",
        workspace_version()
    );
    assert!(manifest.contains(&requirement), "{requirement}");
}

#[test]
fn published_crates_hold_copies_of_the_license_files_identical_to_the_root() {
    for (member, files) in [
        (
            LIBRARY,
            &["LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_NOTICES"][..],
        ),
        (MACROS, &["LICENSE-MIT", "LICENSE-APACHE"][..]),
    ] {
        for file in files {
            assert_eq!(
                read(&format!("{member}/{file}")),
                read(file),
                "{member}/{file} differs from the file at the repository root"
            );
        }
    }
}

#[test]
fn published_crates_declare_what_they_package_and_package_no_test() {
    assert_eq!(
        manifest_list(&read(&format!("{LIBRARY}/Cargo.toml")), "include"),
        [
            "/README.md",
            "/LICENSE-MIT",
            "/LICENSE-APACHE",
            "/THIRD_PARTY_NOTICES",
            "/src/**",
            "/examples/**",
        ]
    );
    assert_eq!(
        manifest_list(&read(&format!("{MACROS}/Cargo.toml")), "include"),
        ["/README.md", "/LICENSE-MIT", "/LICENSE-APACHE", "/src/**"]
    );
    for member in [LIBRARY, MACROS] {
        assert!(!read(&format!("{member}/Cargo.toml")).contains("\nexclude = "));
    }
    assert!(read(".gitignore").lines().any(|line| line == "*.profraw"));
}

/// The `#[path]` attributes of `text` whose target leaves the directory of
/// the file, each as its line number.
fn paths_leaving_the_directory(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start();
            code.starts_with("#[path") && code.contains("..")
        })
        .map(|(number, _)| number + 1)
        .collect()
}

#[test]
fn no_published_source_reaches_a_file_outside_its_package() {
    for member in [LIBRARY, MACROS] {
        let mut sources = Vec::new();
        rust_sources(&workspace_root().join(member).join("src"), &mut sources);
        assert!(!sources.is_empty());
        for source in sources {
            let text = std::fs::read_to_string(&source).unwrap();
            let leaving = paths_leaving_the_directory(&text);
            assert!(leaving.is_empty(), "{}:{leaving:?}", source.display());
        }
    }
    assert_eq!(
        paths_leaving_the_directory("mod a;\n#[path = \"../tests/common/capture.rs\"]\nmod b;\n"),
        [2]
    );
    assert!(
        workspace_root()
            .join(LIBRARY)
            .join("src/test_capture.rs")
            .is_file()
    );
    let common = read(&format!("{LIBRARY}/tests/common/mod.rs"));
    assert!(common.contains("#[path = \"../../src/test_capture.rs\"]\npub mod capture;"));
}

#[test]
fn docs_rs_builds_every_feature_and_shows_which_feature_gates_an_item() {
    let manifest = compact(&read(&format!("{LIBRARY}/Cargo.toml")));
    assert!(manifest.contains(
        "[package.metadata.docs.rs] all-features = true rustdoc-args = [\"--cfg\", \"docsrs\"]"
    ));
    let library = read(&format!("{LIBRARY}/src/lib.rs"));
    assert!(library.contains("#![cfg_attr(docsrs, feature(doc_cfg))]"));
    assert!(
        !library.contains("#![feature("),
        "an unconditional feature breaks stable"
    );
}

/// The members of the workspace that could be published: those whose
/// manifest lacks `publish = false`.
fn publishable(members: &[(String, String)]) -> Vec<&str> {
    members
        .iter()
        .filter(|(_, manifest)| {
            !manifest
                .lines()
                .any(|line| line.trim() == "publish = false")
        })
        .map(|(member, _)| member.as_str())
        .collect()
}

#[test]
fn only_the_library_and_its_macros_are_publishable() {
    let root = read("Cargo.toml");
    let members: Vec<(String, String)> = manifest_list(&root, "members")
        .into_iter()
        .map(|member| {
            let manifest = read(&format!("{member}/Cargo.toml"));
            (member, manifest)
        })
        .collect();
    assert_eq!(members.len(), 5);
    assert_eq!(publishable(&members), [LIBRARY, MACROS]);

    let opened = vec![
        (
            "xtask".to_owned(),
            "[package]\nname = \"xtask\"\n".to_owned(),
        ),
        (
            "crates/fake-claude".to_owned(),
            "[package]\npublish = false\n".to_owned(),
        ),
    ];
    assert_eq!(publishable(&opened), ["xtask"]);
}

#[test]
fn clippy_config_exempts_tests() {
    let config = compact(&read("clippy.toml"));
    for setting in [
        "allow-unwrap-in-tests = true",
        "allow-expect-in-tests = true",
        "allow-panic-in-tests = true",
    ] {
        assert!(config.contains(setting), "{setting}");
    }
}

#[test]
fn library_denies_missing_docs_and_unsafe_code() {
    let library = read("crates/claude-agent-rust-sdk/src/lib.rs");
    assert!(library.contains("#![deny(missing_docs)]"));
    assert!(library.contains("#![deny(unsafe_code)]"));
}

fn rust_sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn unsafe_code_is_confined_to_the_process_sys_module() {
    let source_root = workspace_root().join("crates/claude-agent-rust-sdk/src");
    let sys_module = source_root.join("process").join("sys.rs");
    let mut sources = Vec::new();
    rust_sources(&source_root, &mut sources);
    assert!(sources.contains(&sys_module));

    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        let allows_unsafe = text.contains("allow(unsafe_code)");
        let uses_unsafe = text.contains("unsafe {") || text.contains("unsafe fn");
        if source == sys_module {
            assert!(text.contains("#![allow(unsafe_code)]"));
            assert!(uses_unsafe);
        } else {
            assert!(!allows_unsafe, "{}", source.display());
            assert!(!uses_unsafe, "{}", source.display());
        }
    }
}

#[test]
fn library_sources_carry_no_implementation_comments() {
    let mut sources = Vec::new();
    rust_sources(
        &workspace_root().join("crates/claude-agent-rust-sdk/src"),
        &mut sources,
    );
    rust_sources(
        &workspace_root().join("crates/fake-claude/src"),
        &mut sources,
    );
    rust_sources(
        &workspace_root().join("crates/parity-driver/src"),
        &mut sources,
    );
    for source in sources {
        let text = std::fs::read_to_string(&source).unwrap();
        for (number, line) in text.lines().enumerate() {
            let code = line.trim_start();
            let is_comment =
                code.starts_with("//") && !code.starts_with("///") && !code.starts_with("//!");
            assert!(!is_comment, "{}:{}: {line}", source.display(), number + 1);
        }
    }
}

#[test]
fn readme_states_who_authenticates_the_affiliation_and_the_two_licenses() {
    let readme = compact(&read("README.md").replace("\n> ", " "));
    for statement in [
        "End users authenticate with their own credentials.",
        "This is not an Anthropic product.",
        "It is not affiliated with, endorsed by, or supported by Anthropic.",
        "`claude-agent-rust-sdk` is licensed under either of the [MIT license]({REPOSITORY}/blob/main/LICENSE-MIT) or the [Apache License, Version \
         2.0]({REPOSITORY}/blob/main/LICENSE-APACHE), at your option.",
        "Unless you explicitly state otherwise, any contribution intentionally submitted for \
         inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual \
         licensed as above, without any additional terms or conditions.",
        "The crate is derived from the Python Claude Agent SDK, which is MIT licensed. Its \
         notice is reproduced in \
         [`THIRD_PARTY_NOTICES`]({REPOSITORY}/blob/main/THIRD_PARTY_NOTICES).",
    ] {
        let statement = statement.replace("{REPOSITORY}", REPOSITORY);
        assert!(readme.contains(&statement), "{statement}");
    }
    assert!(readme.contains(&format!("tested against {CLI_VERSION}")));
    assert!(readme.contains(&format!(
        "It is a port of the Python Claude Agent SDK, version {UPSTREAM_SDK_VERSION}."
    )));

    let library = read("crates/claude-agent-rust-sdk/src/lib.rs");
    assert!(library.contains("#[doc = include_str!(\"../../../README.md\")]"));
    let manifest = read("crates/claude-agent-rust-sdk/Cargo.toml");
    assert!(manifest.contains("readme = \"../../README.md\""));
}

/// The `major.minor` requirement the README snippet gives for the library.
fn readme_requirement(readme: &str) -> Option<&str> {
    readme
        .lines()
        .find_map(|line| line.strip_prefix("claude-agent-rust-sdk = \""))
        .and_then(|rest| rest.split('"').next())
}

#[test]
fn readme_installs_from_the_registry_at_the_workspace_minor_version() {
    let readme = read("README.md");
    let installation = readme
        .split("\n## Installation\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the README has no Installation section");
    assert!(installation.contains("\ncargo add claude-agent-rust-sdk\n"));
    assert!(!installation.contains("git ="), "{installation}");
    assert!(!readme.contains("not published on crates.io"));

    let version = workspace_version();
    let mut parts = version.split('.');
    let minor = format!("{}.{}", parts.next().unwrap(), parts.next().unwrap());
    assert_eq!(readme_requirement(installation), Some(minor.as_str()));
    let macros = read(&format!("{MACROS}/README.md"));
    assert!(macros.contains(&format!("claude-agent-rust-sdk = {{ version = \"{minor}\"")));

    assert_eq!(
        readme_requirement("[dependencies]\nclaude-agent-rust-sdk = \"7.3\"\ntokio = \"1\"\n"),
        Some("7.3")
    );
    assert_ne!(
        readme_requirement("claude-agent-rust-sdk = \"7.3\"\n"),
        Some(minor.as_str())
    );
}

/// The link targets of `markdown` that are not absolute `https` URLs: a
/// registry page has no repository to resolve them against.
fn relative_link_targets(markdown: &str) -> Vec<&str> {
    markdown
        .split("](")
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
        .filter(|target| !target.starts_with("https://"))
        .collect()
}

#[test]
fn readme_opens_with_badges_and_links_only_absolute_urls() {
    let readme = read("README.md");
    let opening: Vec<&str> = readme.lines().take(5).collect();
    assert_eq!(
        opening,
        [
            "# claude-agent-rust-sdk",
            "",
            "[![crates.io](https://img.shields.io/crates/v/claude-agent-rust-sdk.svg)](https://crates.io/crates/claude-agent-rust-sdk)",
            "[![docs.rs](https://docs.rs/claude-agent-rust-sdk/badge.svg)](https://docs.rs/claude-agent-rust-sdk)",
            &format!(
                "[![CI]({REPOSITORY}/actions/workflows/ci.yml/badge.svg?branch=main)]({REPOSITORY}/actions/workflows/ci.yml)"
            ),
        ]
    );

    for file in ["README.md", "crates/claude-agent-rust-sdk-macros/README.md"] {
        let text = read(file);
        let relative = relative_link_targets(&text);
        assert!(relative.is_empty(), "{file} links {relative:?} relatively");
        let repository_links = text.matches("](https://github.com/").count();
        let own_links = text.matches(&format!("]({REPOSITORY}/")).count();
        assert_eq!(repository_links, own_links, "{file}");
    }
    for path in [
        "docs/migration.md",
        "docs/parity.md",
        "parity/README.md",
        "LICENSE-MIT",
    ] {
        assert!(
            readme.contains(&format!("]({REPOSITORY}/blob/main/{path})")),
            "{path}"
        );
        assert!(workspace_root().join(path).is_file(), "{path}");
    }

    assert_eq!(
        relative_link_targets(
            "[a](https://example.com/a) [guide](docs/guide.md) ![logo](./logo.png) [top](#top)"
        ),
        ["docs/guide.md", "./logo.png", "#top"]
    );
}

#[test]
fn readme_and_crate_docs_state_the_compatibility_promise() {
    let readme = read("README.md");
    let section = readme
        .split("\n## Versioning and compatibility\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the README has no Versioning and compatibility section");
    let section = compact(section);
    for statement in [
        "In `0.y.z`, a change of the minor version `y` may break your build.",
        "A patch release (a change of `z`) never does.",
        "Raising the minimum supported Rust version, 1.88 today, is a minor change.",
        "The `testing` feature is outside the compatibility promise",
        "Each release states the version of the Python SDK and of the CLI it matches.",
        &format!("[`CHANGELOG.md`]({REPOSITORY}/blob/main/CHANGELOG.md)"),
    ] {
        assert!(section.contains(statement), "{statement}");
    }
    assert!(compact(&read("Cargo.toml")).contains("rust-version = \"1.88\""));

    let library = compact(&read("crates/claude-agent-rust-sdk/src/lib.rs"));
    assert!(library.contains(
        "//! - `testing`: the `testing` module, with a scriptable transport for tests. //! It is \
         outside the compatibility promise"
    ));
}

/// Cargo unifies features across the whole program, so a library that
/// turns `serde_json/preserve_order` on in its dependency line changes
/// every `serde_json::Map` of its dependents with no way out. The library
/// turns it on only through its own `preserve-order` feature.
#[test]
fn serde_json_key_order_is_a_feature_of_the_library_its_dependents_can_turn_off() {
    let manifest = read("crates/claude-agent-rust-sdk/Cargo.toml");
    let dependency = manifest
        .lines()
        .find(|line| line.starts_with("serde_json"))
        .expect("the library depends on serde_json");
    assert!(!dependency.contains("preserve_order"), "{dependency}");
    assert!(
        manifest.contains("preserve-order = [\"serde_json/preserve_order\"]"),
        "{manifest}"
    );
}

#[test]
fn crate_root_documents_features_platforms_and_the_tested_cli_version() {
    let library = read("crates/claude-agent-rust-sdk/src/lib.rs");
    let manifest = read("crates/claude-agent-rust-sdk/Cargo.toml");
    let features = manifest
        .split("[features]")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("the manifest has no [features] table");
    let mut documented = 0;
    let mut defaults = Vec::new();
    for line in features.lines() {
        let Some((feature, enables)) = line.split_once(" = ") else {
            continue;
        };
        let feature = feature.trim();
        if feature == "default" {
            defaults = enables
                .trim_matches(|c| c == '[' || c == ']')
                .split(',')
                .map(|name| name.trim().trim_matches('"').to_owned())
                .filter(|name| !name.is_empty())
                .collect();
            continue;
        }
        assert!(
            library.contains(&format!("//! - `{feature}`: ")),
            "the crate root does not document the feature `{feature}`"
        );
        documented += 1;
    }
    assert!(documented >= 5, "{features}");
    assert_eq!(defaults, ["jsonschema", "preserve-order"], "{features}");
    for feature in &defaults {
        assert!(
            library.contains(&format!("//! - `{feature}`: on by default.")),
            "the crate root does not say that `{feature}` is on by default"
        );
    }
    for statement in [
        "//! # Platform support",
        "//! Linux, macOS, and Windows are supported",
        &format!("//! {CLI_VERSION} ([`CLI_VERSION`])"),
        &format!("//! {MINIMUM_CLI_VERSION} ([`MINIMUM_CLI_VERSION`])"),
    ] {
        assert!(library.contains(statement), "{statement}");
    }
}

#[test]
fn every_example_of_the_readme_exists() {
    let readme = read("README.md");
    let table = readme
        .split("## Examples")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the README has no Examples section");
    let named: Vec<&str> = table
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split('`').next())
        .collect();
    assert_eq!(named.len(), 15, "{named:?}");
    for example in named {
        let file = workspace_root()
            .join("crates/claude-agent-rust-sdk/examples")
            .join(format!("{example}.rs"));
        assert!(file.is_file(), "{} is missing", file.display());
    }
}

#[test]
fn the_project_is_dual_licensed_and_the_upstream_notice_is_preserved() {
    let mit = read("LICENSE-MIT");
    assert!(mit.starts_with("MIT License\n\nCopyright (c) 2026 Arthur Jean\n\nPermission is"));
    assert!(mit.contains("THE SOFTWARE IS PROVIDED \"AS IS\""));

    let apache = read("LICENSE-APACHE");
    assert!(
        apache.contains("Apache License\n                           Version 2.0, January 2004")
    );
    assert!(apache.contains("TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION"));
    assert!(apache.contains("END OF TERMS AND CONDITIONS"));
    assert!(apache.contains("Copyright [yyyy] [name of copyright owner]"));
    assert_eq!(apache.lines().count(), 202, "the Apache text is edited");
    assert!(!workspace_root().join("LICENSE").exists());

    let notices = read("THIRD_PARTY_NOTICES");
    assert!(compact(&notices).contains(
        "This project is licensed under either of the MIT license (see LICENSE-MIT) or the \
         Apache License, Version 2.0 (see LICENSE-APACHE), at your option."
    ));
    assert!(notices.contains("claude-agent-sdk-python"));
    assert!(notices.contains(&format!("{UPSTREAM_COMMIT} (v{UPSTREAM_SDK_VERSION})")));
    assert!(notices.contains("MIT License\n\nCopyright (c) 2025 Anthropic, PBC\n"));
    assert!(notices.contains(
        "The above copyright notice and this permission notice shall be included in all\n\
         copies or substantial portions of the Software."
    ));
    assert!(notices.contains("THE SOFTWARE IS PROVIDED \"AS IS\""));

    let upstream = workspace_root().join(UPSTREAM_PATH).join("LICENSE");
    if let Ok(license) = std::fs::read_to_string(upstream) {
        let license = license.replace("\r\n", "\n");
        assert!(
            notices.contains(license.trim()),
            "the upstream notice is not reproduced word for word"
        );
    }
}

/// The identifiers of the `allow` list of `deny.toml`.
fn allowed_licenses(policy: &str) -> Vec<String> {
    let list = policy
        .split("allow = [")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("deny.toml has no allow list");
    list.split(',')
        .map(|entry| entry.trim().trim_matches('"').to_owned())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// The allowed identifiers of a copyleft or source-available family, which a
/// permissively licensed crate cannot depend on.
fn restrictive_licenses(allowed: &[String]) -> Vec<&str> {
    allowed
        .iter()
        .map(String::as_str)
        .filter(|identifier| {
            ["GPL", "AGPL", "LGPL", "SSPL", "BUSL"]
                .iter()
                .any(|family| identifier.starts_with(family))
        })
        .collect()
}

#[test]
fn license_allow_list_is_enforced_in_ci() {
    let policy = read("deny.toml");
    assert!(policy.contains("[licenses]"));
    let allowed = allowed_licenses(&policy);
    assert!(allowed.iter().any(|identifier| identifier == "MIT"));
    assert!(allowed.iter().any(|identifier| identifier == "Apache-2.0"));
    let restrictive = restrictive_licenses(&allowed);
    assert!(restrictive.is_empty(), "deny.toml allows {restrictive:?}");

    let workflow = compact(&read(".github/workflows/ci.yml"));
    assert!(workflow.contains("EmbarkStudios/cargo-deny-action@"));
    assert!(workflow.contains("command: check licenses"));
}

#[test]
fn an_allow_list_with_a_copyleft_or_source_available_license_is_reported_by_identifier() {
    let families = ["GPL", "AGPL", "LGPL", "SSPL", "BUSL"];
    let identifiers = ["3.0-or-later", "3.0-only", "2.1-or-later", "1.0", "1.1"];
    let mut policy = String::from("[licenses]\nallow = [\n    \"MIT\",\n");
    let mut expected = Vec::new();
    for (family, suffix) in families.iter().zip(identifiers) {
        let identifier = [*family, suffix].join("-");
        policy.push_str(&format!("    \"{identifier}\",\n"));
        expected.push(identifier);
    }
    policy.push_str("    \"MPL-2.0\",\n]\n");

    let allowed = allowed_licenses(&policy);

    assert_eq!(allowed.len(), 7, "{allowed:?}");
    assert_eq!(restrictive_licenses(&allowed), expected);
}

/// The lines of `files` that name the former copyleft license, which no
/// file outside the dated records may state as the license of the project.
fn former_license_hits(files: &[(String, String)]) -> Vec<String> {
    let marks = [
        ["GPL", "3.0"].join("-"),
        ["General", "Public", "License"].join(" "),
    ];
    let mut hits = Vec::new();
    for (path, text) in files {
        if DATED_RECORDS.iter().any(|record| path.starts_with(record)) {
            continue;
        }
        for (number, line) in text.lines().enumerate() {
            if marks.iter().any(|mark| line.contains(mark)) {
                hits.push(format!("{path}:{}", number + 1));
            }
        }
    }
    hits
}

#[test]
fn no_tracked_file_outside_dated_records_states_the_former_license() {
    let Some(files) = tracked_texts() else {
        return;
    };
    let hits = former_license_hits(&files);
    assert!(hits.is_empty(), "the former license remains in {hits:#?}");

    let mark = ["GPL", "3.0", "or", "later"].join("-");
    let sample = vec![
        (
            "Cargo.toml".to_owned(),
            format!("[package]\nlicense = \"{mark}\"\n"),
        ),
        ("tasks/prd.md".to_owned(), format!("{mark}\n")),
    ];
    assert_eq!(former_license_hits(&sample), ["Cargo.toml:2"]);
}

#[test]
fn ci_runs_the_four_quality_gates_on_three_operating_systems() {
    let workflow = compact(&read(".github/workflows/ci.yml"));
    assert!(workflow.contains("os: [ubuntu-latest, macos-latest, windows-latest]"));
    assert!(workflow.contains("runs-on: ${{ matrix.os }}"));
    for gate in [
        "run: cargo fmt --all --check",
        "run: cargo clippy --workspace --all-targets --all-features --no-deps -- -D warnings",
        "run: cargo test --workspace --all-features",
        "run: cargo doc --workspace --all-features --no-deps",
        "RUSTDOCFLAGS: -D warnings",
    ] {
        assert!(workflow.contains(gate), "{gate}");
    }
    assert!(workflow.contains("permissions: contents: read"));
}

#[test]
fn ci_tests_the_crate_without_its_default_features() {
    let job = ci_job(&read(".github/workflows/ci.yml"), "gates");
    assert!(
        job.contains(
            "run: cargo test --package claude-agent-rust-sdk --no-default-features --features testing"
        ),
        "{job}"
    );
    let gates = compact(&read("xtask/src/gates.rs"));
    assert!(gates.contains(
        r#"&[ "test", "--package", "claude-agent-rust-sdk", "--no-default-features", "--features", "testing", ]"#
    ));
}

#[test]
fn the_examples_are_built_with_default_features_in_ci_and_in_the_gates() {
    let workflow = compact(&read(".github/workflows/ci.yml"));
    assert!(workflow.contains("run: cargo check --package claude-agent-rust-sdk --examples"));
    let gates = compact(&read("xtask/src/gates.rs"));
    assert!(gates.contains(r#"&["check", "--package", "claude-agent-rust-sdk", "--examples"]"#));
}

fn ci_job(workflow: &str, name: &str) -> String {
    let mut lines = workflow
        .lines()
        .skip_while(|line| *line != format!("  {name}:"))
        .skip(1);
    let body: Vec<&str> = lines
        .by_ref()
        .take_while(|line| line.is_empty() || line.starts_with("    "))
        .collect();
    assert!(!body.is_empty(), "the workflow has no job `{name}`");
    compact(&body.join("\n"))
}

#[test]
fn ci_packages_both_crates_on_every_pull_request_without_secrets() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(compact(&workflow).starts_with("name: CI on: pull_request: push: branches: - main"));
    let job = ci_job(&workflow, "package");
    for step in [
        "runs-on: ubuntu-latest",
        "permissions: contents: read",
        "persist-credentials: false",
        "toolchain: nightly",
        "toolchain: stable",
        "run: cargo xtask package-check",
        "run: cargo +nightly doc --package claude-agent-rust-sdk --all-features --no-deps env: \
         RUSTDOCFLAGS: --cfg docsrs -D warnings",
    ] {
        assert!(job.contains(step), "{step}: {job}");
    }
    let nightly = job.find("toolchain: nightly").unwrap();
    let stable = job.find("toolchain: stable").unwrap();
    assert!(nightly < stable, "stable must stay the default toolchain");
    for forbidden in ["if:", "secrets.", "write", "--allow-dirty", "matrix"] {
        assert!(!job.contains(forbidden), "{forbidden}: {job}");
    }

    let command = read("xtask/src/package.rs");
    assert!(command.contains(".args([\"package\", \"--locked\"])"));
    let flag = ["--allow", "dirty"].join("-");
    let passes_flag = command
        .lines()
        .any(|line| !line.trim_start().starts_with("//") && line.contains(&flag));
    assert!(!passes_flag, "package-check must never pass {flag}");
    let usage = read("xtask/src/main.rs");
    assert!(usage.contains("\n  package-check "));
    assert!(usage.contains("Some(\"package-check\") => package::run(rest),"));
    let readme = read("README.md");
    assert!(readme.contains("\ncargo xtask package-check "));
}

#[test]
fn ci_checks_the_parity_ledgers_on_every_pull_request() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(compact(&workflow).starts_with("name: CI on: pull_request: push:"));
    let job = ci_job(&workflow, "parity");
    assert!(job.contains("run: cargo xtask parity-check --deny-pending"));
    assert!(job.contains("submodules: true"));
    assert!(job.contains("os: [ubuntu-latest, macos-latest, windows-latest]"));
    assert!(!job.contains("if:"), "{job}");
}

#[test]
fn ci_runs_the_differential_harness_on_every_pull_request_without_secrets() {
    let workflow = read(".github/workflows/ci.yml");
    let job = ci_job(&workflow, "differential");
    for step in [
        "submodules: true",
        "persist-credentials: false",
        "permissions: contents: read",
        "uses: actions/setup-python@",
        "python-version: \"3.13\"",
        "run: cargo xtask parity-diff --check-requirements",
        "pip install --require-hashes --no-deps -r parity/requirements.txt",
        "run: cargo xtask parity-diff env: PARITY_PYTHON:",
        "run: cargo xtask parity-diff --coverage",
    ] {
        assert!(job.contains(step), "{step}: {job}");
    }
    assert!(!job.contains("if:"), "{job}");
    assert!(!job.contains("secrets."), "{job}");
    assert!(!job.contains("write"), "{job}");
    assert!(
        !job.contains("pip install claude-agent-sdk") && !job.contains("pip install -e"),
        "{job}"
    );
    let coverage = ci_job(&workflow, "coverage");
    assert!(
        coverage.contains("COVERAGE_IGNORE: (crates/fake-claude|crates/parity-driver|xtask)/"),
        "{coverage}"
    );
}

/// The features of the library's manifest, `default` left out because it
/// only lists other features.
fn library_features() -> Vec<String> {
    let manifest = read(&format!("{LIBRARY}/Cargo.toml"));
    let table = manifest
        .split("\n[features]\n")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("the manifest has no [features] table");
    let mut features: Vec<String> = table
        .lines()
        .filter_map(|line| line.split_once(" = "))
        .map(|(feature, _)| feature.trim().to_owned())
        .filter(|feature| feature != "default")
        .collect();
    features.sort();
    features
}

/// The value of the constant `name` of `xtask/src/semver.rs`.
fn semver_pin(name: &str) -> String {
    read("xtask/src/semver.rs")
        .split(&format!("pub const {name}: &str = \""))
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("xtask/src/semver.rs has no `{name}`"))
        .to_owned()
}

#[test]
fn ci_compares_the_public_api_with_the_newest_release_on_every_pull_request() {
    let workflow = read(".github/workflows/ci.yml");
    assert!(compact(&workflow).starts_with("name: CI on: pull_request: push:"));
    let job = ci_job(&workflow, "semver");
    let toolchain = semver_pin("TOOLCHAIN");
    let tool = semver_pin("TOOL_VERSION");
    for step in [
        "runs-on: ubuntu-latest".to_owned(),
        "permissions: contents: read".to_owned(),
        "persist-credentials: false".to_owned(),
        format!("uses: dtolnay/rust-toolchain@v1 with: toolchain: {toolchain}"),
        format!("run: cargo install cargo-semver-checks@{tool} --locked"),
        "The macros crate is left out:".to_owned(),
    ] {
        assert!(job.contains(&step), "{step}: {job}");
    }
    assert_eq!(
        job.matches("toolchain:").count(),
        1,
        "the job builds with the pinned toolchain only: {job}"
    );
    for forbidden in [
        "if:",
        "secrets.",
        "write",
        "continue-on-error",
        "--baseline",
    ] {
        assert!(!job.contains(forbidden), "{forbidden}: {job}");
    }

    let run = job
        .split("run: cargo xtask semver-check --features ")
        .nth(1)
        .expect("the job does not run `cargo xtask semver-check --features`");
    let mut checked: Vec<String> = run
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .split(',')
        .map(str::to_owned)
        .collect();
    checked.sort();
    let mut expected = library_features();
    expected.retain(|feature| feature != "testing");
    assert_eq!(
        checked, expected,
        "the `semver` job must name every feature of the library but `testing`"
    );

    let command = read("xtask/src/semver.rs");
    for statement in [
        "\"--only-explicit-features\"",
        "pub const PACKAGE: &str = \"claude-agent-rust-sdk\";",
        ".env(\"RUSTUP_TOOLCHAIN\", TOOLCHAIN)",
    ] {
        assert!(command.contains(statement), "{statement}");
    }
    let usage = read("xtask/src/main.rs");
    assert!(usage.contains("\n  semver-check "));
    assert!(usage.contains("Some(\"semver-check\") => semver::run(rest),"));
    assert!(read("README.md").contains("\ncargo xtask semver-check "));
    assert!(
        read("xtask/src/release/prepare.rs").contains("crate::semver::compare(&repo.root"),
        "release prepare patch consults the tool"
    );
}

#[test]
fn the_advisory_check_runs_on_a_schedule_with_read_access_and_never_on_forks() {
    let workflow = read(".github/workflows/advisories.yml");
    let compacted = compact(&workflow);
    assert!(compacted.starts_with("name: Advisories "), "{compacted}");
    assert!(
        compacted.contains(
            "on: schedule: - cron: \"17 5 * * 1\" workflow_dispatch: permissions: contents: read jobs:"
        ),
        "{compacted}"
    );
    for forbidden in [
        "pull_request",
        "push:",
        "workflow_run:",
        "secrets.",
        "write",
        "git commit",
        "git push",
        "gh issue",
        "continue-on-error",
        "--disable-fetch",
        "--offline",
    ] {
        assert!(!workflow.contains(forbidden), "{forbidden}");
    }
    assert_eq!(job_ids(&workflow), ["advisories"]);
    let job = ci_job(&workflow, "advisories");
    for step in [
        "if: ${{ !github.event.repository.fork }}",
        "persist-credentials: false",
        "uses: EmbarkStudios/cargo-deny-action@",
        "command: check advisories",
    ] {
        assert!(job.contains(step), "{step}: {job}");
    }
    assert!(!job.contains("permissions:"), "{job}");

    let policy = read("deny.toml");
    let advisories = policy
        .split("\n[advisories]\n")
        .nth(1)
        .expect("deny.toml has no [advisories] section");
    assert!(advisories.contains("yanked = \"deny\""), "{advisories}");
    for level in ["unmaintained", "unsound"] {
        assert!(
            !advisories.contains(&format!("{level} = ")),
            "{level} keeps its failing default"
        );
    }
    for entry in advisories.lines().filter(|line| line.contains("id = ")) {
        assert!(
            entry.contains("reason = \""),
            "an ignored advisory needs a reason: {entry}"
        );
    }
    let releasing = compact(&read("RELEASING.md"));
    assert!(
        releasing.contains("| `Advisories` fails, naming a `RUSTSEC-` advisory |"),
        "RELEASING.md says what to do when the run fails"
    );
    assert!(releasing.contains("| `Advisories` fails without naming an advisory |"));
}

#[test]
fn the_upstream_watch_runs_daily_and_only_its_issue_job_can_write() {
    let workflow = read(".github/workflows/upstream-watch.yml");
    let compacted = compact(&workflow);
    assert!(
        compacted.contains("on: schedule: - cron: \"41 6 * * *\" workflow_dispatch:"),
        "{compacted}"
    );
    for forbidden in [
        "pull_request",
        "push:",
        "workflow_run:",
        "secrets.",
        "git commit",
        "git push",
        "gh pr ",
        "pull-requests:",
        "contents: write",
    ] {
        assert!(!workflow.contains(forbidden), "{forbidden}");
    }
    let header = workflow.split("\njobs:").next().unwrap();
    assert!(
        compact(header).ends_with("permissions: contents: read"),
        "{header}"
    );
    assert_eq!(workflow.matches("issues: write").count(), 1);

    let detect = ci_job(&workflow, "detect");
    assert!(
        detect.contains("if: ${{ !github.event.repository.fork }}"),
        "{detect}"
    );
    assert!(detect.contains("submodules: true"), "{detect}");
    assert!(
        detect.contains("run: bash .github/scripts/upstream-watch.sh detect"),
        "{detect}"
    );
    assert!(!detect.contains("permissions:"), "{detect}");

    let report = ci_job(&workflow, "report");
    assert!(report.contains("needs: detect"), "{report}");
    assert!(
        report.contains("permissions: contents: read issues: write"),
        "{report}"
    );
    assert!(
        report.contains("bash .github/scripts/upstream-watch.sh report"),
        "{report}"
    );
    assert!(!report.contains("cargo"), "{report}");

    let scripts = ci_job(&read(".github/workflows/ci.yml"), "scripts");
    assert!(
        scripts.contains("run: bash .github/scripts/upstream-watch.test.sh"),
        "{scripts}"
    );
    let script = read(".github/scripts/upstream-watch.sh");
    assert!(
        script.contains("cargo xtask upstream-sync"),
        "the issue gives the sync command"
    );
    assert!(
        script.contains("Verdict: `%s`"),
        "the issue carries the verdict"
    );
    for file in ["README.md", "AGENTS.md"] {
        let text = read(file);
        assert!(!text.to_lowercase().contains("weekly"), "{file}");
        assert!(!text.contains("Once a week"), "{file}");
    }
    assert!(read("README.md").contains("Once a day, the `Upstream watch` workflow"));
}

/// The format problems of a changelog whose workspace version is
/// `version`, one message each.
fn changelog_problems(changelog: &str, version: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let titles: Vec<&str> = changelog
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .collect();
    if !changelog.starts_with("# Changelog\n\n## Unreleased\n") {
        problems.push("the file must open with `# Changelog` then `## Unreleased`".to_owned());
    }
    let parse = |text: &str| -> Option<Vec<u64>> {
        let (core, pre) = text
            .split_once('-')
            .map_or((text, None), |(a, b)| (a, Some(b)));
        let mut numbers: Vec<u64> = core
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        if numbers.len() != 3 {
            return None;
        }
        numbers.push(u64::from(pre.is_none()));
        if let Some(pre) = pre {
            numbers.extend(pre.split('.').filter_map(|part| part.parse::<u64>().ok()));
        }
        Some(numbers)
    };
    let mut previous: Option<(String, Vec<u64>)> = None;
    let mut newest = None;
    for title in titles.iter().skip(1) {
        let Some((released, date)) = title.split_once(" - ") else {
            problems.push(format!("`## {title}` is not `## <version> - <YYYY-MM-DD>`"));
            continue;
        };
        let date_ok = date.len() == 10
            && date.char_indices().all(|(index, character)| {
                if index == 4 || index == 7 {
                    character == '-'
                } else {
                    character.is_ascii_digit()
                }
            });
        let Some(order) = parse(released).filter(|_| date_ok) else {
            problems.push(format!("`## {title}` is not `## <version> - <YYYY-MM-DD>`"));
            continue;
        };
        let body = changelog
            .split(&format!("\n## {title}\n"))
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .unwrap_or_default();
        let opening = body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        if !(opening.starts_with("Upstream: Python SDK ") && opening.contains(", CLI ")) {
            problems.push(format!("`## {title}` lacks its `Upstream:` line"));
        }
        for group in body.lines().filter_map(|line| line.strip_prefix("### ")) {
            if !["Breaking changes", "Added", "Changed", "Fixed"].contains(&group) {
                problems.push(format!("`## {title}` has the group `### {group}`"));
            }
        }
        if let Some((above, above_order)) = &previous
            && order >= *above_order
        {
            problems.push(format!("`## {title}` is not older than `{above}`"));
        }
        newest.get_or_insert_with(|| released.to_owned());
        previous = Some((released.to_owned(), order));
    }
    if let Some(newest) = newest
        && newest != version
    {
        problems.push(format!(
            "the newest released section is {newest}, but the workspace version is {version}"
        ));
    }
    problems
}

#[test]
fn the_changelog_follows_its_format_and_its_newest_release_is_the_workspace_version() {
    let changelog = read("CHANGELOG.md");
    let problems = changelog_problems(&changelog, &workspace_version());
    assert!(problems.is_empty(), "{problems:?}");
    let unreleased = changelog
        .split("## Unreleased\n")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .unwrap();
    let released = changelog.matches("\n## ").count() > 1;
    assert!(
        released || unreleased.contains("\n- "),
        "before the first release, ## Unreleased holds the summary of what it contains"
    );
    assert!(!changelog.contains('<'), "the changelog holds no HTML");

    let good = "# Changelog\n\n## Unreleased\n\n## 0.1.1 - 2026-11-20\n\nUpstream: Python SDK 0.2.183 to 0.2.184, CLI 2.1.327\n\n### Fixed\n\n- A fix.\n\n## 0.1.0 - 2026-11-19\n\nUpstream: Python SDK 0.2.182, CLI 2.1.325\n\n### Added\n\n- First.\n";
    assert!(
        changelog_problems(good, "0.1.1").is_empty(),
        "{:?}",
        changelog_problems(good, "0.1.1")
    );
    assert_eq!(
        changelog_problems(good, "0.1.2"),
        ["the newest released section is 0.1.1, but the workspace version is 0.1.2"]
    );
    let no_upstream = good.replace("Upstream: Python SDK 0.2.182, CLI 2.1.325\n\n", "");
    assert_eq!(
        changelog_problems(&no_upstream, "0.1.1"),
        ["`## 0.1.0 - 2026-11-19` lacks its `Upstream:` line"]
    );
    let unordered = good.replace("## 0.1.0 - 2026-11-19", "## 0.1.2 - 2026-11-19");
    assert_eq!(
        changelog_problems(&unordered, "0.1.1"),
        ["`## 0.1.2 - 2026-11-19` is not older than `0.1.1`"]
    );
    let candidates = good
        .replace("## 0.1.1 - ", "## 0.1.0 - ")
        .replace("## 0.1.0 - 2026-11-19", "## 0.1.0-rc.2 - 2026-11-19");
    assert!(changelog_problems(&candidates, "0.1.0").is_empty());
    let malformed = good.replace("## 0.1.0 - 2026-11-19", "## 0.1.0 (2026-11-19)");
    assert_eq!(
        changelog_problems(&malformed, "0.1.1"),
        ["`## 0.1.0 (2026-11-19)` is not `## <version> - <YYYY-MM-DD>`"]
    );
    let missing = good.replace("## Unreleased\n\n", "");
    assert!(!changelog_problems(&missing, "0.1.1").is_empty());
}

#[test]
fn the_differences_ledger_stores_the_moving_versions_as_tokens() {
    let ledger = read("parity/differences.toml");
    let entry = ledger
        .split("[wire.sdk-version]\n")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("the ledger has no wire.sdk-version entry");
    assert!(
        entry.contains("python = '\"{upstream_version}\"'"),
        "{entry}"
    );
    assert!(entry.contains("rust = '\"{crate_version}\"'"), "{entry}");
    assert!(!ledger.contains(&format!("'\"{UPSTREAM_SDK_VERSION}\"'")));
    assert!(!ledger.contains(&format!("'\"{}\"'", workspace_version())));
}

/// The lines of `workflow` that read a registry token from the stored
/// secrets.
fn registry_token_from_secrets(workflow: &str) -> Vec<&str> {
    workflow
        .lines()
        .filter(|line| {
            let compacted: String = line.split_whitespace().collect();
            compacted.contains("secrets.CARGO_REGISTRY_TOKEN")
                || compacted.contains("secrets['CARGO_REGISTRY_TOKEN']")
                || compacted.contains("secrets[\"CARGO_REGISTRY_TOKEN\"]")
                || (compacted.starts_with("CARGO_REGISTRY_TOKEN:") && compacted.contains("secrets"))
        })
        .collect()
}

fn workflow_files() -> Vec<(String, String)> {
    let directory = workspace_root().join(".github/workflows");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "yml"))
        .map(|path| {
            let name = format!(
                ".github/workflows/{}",
                path.file_name().unwrap().to_string_lossy()
            );
            let text = read(&name);
            (name, text)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn no_workflow_reads_a_registry_token_from_the_stored_secrets() {
    for (name, workflow) in workflow_files() {
        let hits = registry_token_from_secrets(&workflow);
        assert!(
            hits.is_empty(),
            "{name} reads a stored registry token: {hits:?}"
        );
    }
    assert_eq!(
        registry_token_from_secrets(
            "env:\n  CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}\n  OTHER: ${{ secrets['CARGO_REGISTRY_TOKEN'] }}\n  FINE: ${{ steps.auth.outputs.token }}\n"
        ),
        [
            "  CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}",
            "  OTHER: ${{ secrets['CARGO_REGISTRY_TOKEN'] }}"
        ]
    );
}

/// The job ids of `workflow`, in file order.
fn job_ids(workflow: &str) -> Vec<String> {
    workflow
        .split("\njobs:\n")
        .nth(1)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let id = line.strip_prefix("  ")?.strip_suffix(':')?;
            (!id.starts_with(' ') && !id.starts_with('#')).then(|| id.to_owned())
        })
        .collect()
}

#[test]
fn the_release_workflow_starts_on_a_version_tag_only_and_confines_each_write() {
    let workflow = read(".github/workflows/release.yml");
    let header = compact(workflow.split("\njobs:").next().unwrap());
    assert!(
        header.contains(
            "on: push: tags: - \"v[0-9]+.[0-9]+.[0-9]+\" - \"v[0-9]+.[0-9]+.[0-9]+-*\" permissions: contents: read"
        ),
        "{header}"
    );
    assert!(
        header.contains("concurrency: group: release cancel-in-progress: false"),
        "{header}"
    );
    for forbidden in [
        "pull_request",
        "pull_request_target",
        "workflow_run",
        "schedule:",
        "workflow_dispatch",
        "branches:",
    ] {
        assert!(!workflow.contains(forbidden), "{forbidden}");
    }
    for forbidden in ["git commit", "git tag", "git push", "secrets."] {
        assert!(!workflow.contains(forbidden), "{forbidden}");
        assert!(
            !read(".github/scripts/release.sh").contains(forbidden),
            "{forbidden}"
        );
    }

    assert_eq!(
        job_ids(&workflow),
        ["verify", "ci", "publish", "attest", "github-release"]
    );
    let verify = ci_job(&workflow, "verify");
    assert!(
        verify.contains("if: ${{ !github.event.repository.fork }}"),
        "{verify}"
    );
    assert!(verify.contains("fetch-depth: 0"), "{verify}");
    assert!(
        verify.contains("run: bash .github/scripts/release.sh verify \"$TAG\""),
        "{verify}"
    );
    assert!(!verify.contains("permissions:"), "{verify}");
    for job in ["ci", "publish", "attest", "github-release"] {
        assert!(
            ci_job(&workflow, job).contains("needs:"),
            "{job} must wait for verify"
        );
    }
    let ci = ci_job(&workflow, "ci");
    assert!(
        ci.contains("needs: verify uses: ./.github/workflows/ci.yml"),
        "{ci}"
    );
    assert!(compact(&read(".github/workflows/ci.yml")).contains("branches: - main # release.yml"));
    assert!(read(".github/workflows/ci.yml").contains("\n  workflow_call:\n"));

    let publish = ci_job(&workflow, "publish");
    for step in [
        "needs: [verify, ci]",
        "environment: release",
        "permissions: contents: read id-token: write steps:",
        "id: missing run: bash .github/scripts/release.sh missing",
        "id: auth if: steps.missing.outputs.publish == 'true' uses: rust-lang/crates-io-auth-action@",
        "if: steps.missing.outputs.publish == 'true' run: bash .github/scripts/release.sh publish env: CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}",
    ] {
        assert!(publish.contains(step), "{step}: {publish}");
    }
    assert!(
        !publish.contains("rust-cache"),
        "the publish job restores no cache"
    );
    for job in [&publish, &ci_job(&workflow, "attest")] {
        let actions: Vec<&str> = job
            .split("uses: ")
            .skip(1)
            .filter_map(|rest| rest.split('@').next())
            .collect();
        assert!(
            actions.iter().all(|action| [
                "actions/checkout",
                "rust-lang/crates-io-auth-action",
                "actions/attest",
                "actions/upload-artifact"
            ]
            .contains(action)),
            "a job holding id-token: write uses {actions:?}"
        );
    }
    assert_eq!(workflow.matches("environment:").count(), 1);

    let attest = ci_job(&workflow, "attest");
    assert!(attest.contains("needs: publish"), "{attest}");
    assert!(
        attest.contains(
            "permissions: contents: read id-token: write attestations: write artifact-metadata: write steps:"
        ),
        "{attest}"
    );
    assert!(
        attest.contains("run: bash .github/scripts/release.sh download dist"),
        "{attest}"
    );
    assert!(attest.contains("uses: actions/attest@"), "{attest}");
    assert!(attest.contains("subject-path: dist/*.crate"), "{attest}");
    assert_eq!(
        workflow.matches("id-token: write").count(),
        2,
        "publish and attest only"
    );

    let release = ci_job(&workflow, "github-release");
    assert!(release.contains("needs: [verify, attest]"), "{release}");
    assert!(
        release.contains("permissions: contents: write steps:"),
        "{release}"
    );
    assert!(
        release.contains("bash .github/scripts/release.sh github-release \"$TAG\""),
        "{release}"
    );
    assert!(
        !release.contains("cargo"),
        "the release job runs no build: {release}"
    );
    assert_eq!(workflow.matches("contents: write").count(), 1);
    for other in ["attestations: write", "artifact-metadata: write"] {
        assert_eq!(workflow.matches(other).count(), 1, "{other}");
    }

    let scripts = ci_job(&read(".github/workflows/ci.yml"), "scripts");
    assert!(
        scripts.contains("run: bash .github/scripts/release.test.sh"),
        "{scripts}"
    );
}

/// What `RELEASING.md` fails to name: a job of `workflow`, or one of the
/// release commands.
fn releasing_omissions(releasing: &str, workflow: &str) -> Vec<String> {
    let mut missing: Vec<String> = job_ids(workflow)
        .into_iter()
        .filter(|job| !releasing.contains(&format!("| `{job}` |")))
        .map(|job| format!("the job `{job}` of release.yml"))
        .collect();
    for command in [
        "cargo xtask release prepare",
        "cargo xtask release tag",
        "cargo xtask upstream-sync",
    ] {
        if !releasing.contains(&format!("`{command}")) {
            missing.push(format!("the command `{command}`"));
        }
    }
    missing
}

#[test]
fn releasing_and_security_guides_cover_every_job_command_and_setting() {
    let releasing = read("RELEASING.md");
    let workflow = read(".github/workflows/release.yml");
    let missing = releasing_omissions(&releasing, &workflow);
    assert!(missing.is_empty(), "RELEASING.md does not name {missing:?}");
    assert_eq!(
        releasing_omissions(
            "| `verify` | x |\n`cargo xtask release prepare`\n",
            "name: x\njobs:\n  verify:\n    runs-on: x\n  publish:\n    runs-on: x\n"
        ),
        [
            "the job `publish` of release.yml",
            "the command `cargo xtask release tag`",
            "the command `cargo xtask upstream-sync`"
        ]
    );
    let compacted = compact(&releasing);
    for statement in [
        "## Versioning",
        "## The routine release",
        "## A CLI-only release",
        "## The first publication",
        "## The end-to-end suite",
        "## When something fails",
        "A yank deletes nothing",
        "cargo yank --version",
        "gh attestation verify",
        "--signer-workflow arthjean/claude-agent-rust-sdk/.github/workflows/release.yml",
        "| `release` |",
        "Tag ruleset on `v*`",
        "Release immutability",
        "Private vulnerability reporting",
        "Owner `arthjean`, repository `claude-agent-rust-sdk`, workflow `release.yml`, environment `release`",
        "\"Trusted Publishing Only\"",
        "Before any crate is published, the tag may be deleted",
        "never reuse one",
    ] {
        assert!(compacted.contains(statement), "{statement}");
    }

    let security = compact(&read("SECURITY.md"));
    for statement in [
        "Fixes ship in the newest release only.",
        "GitHub private vulnerability reporting",
        "https://github.com/arthjean/claude-agent-rust-sdk/security/advisories/new",
        "acknowledged within 7 days",
        "goes to Anthropic",
    ] {
        assert!(security.contains(statement), "{statement}");
    }
    for file in ["README.md", "RELEASING.md", "SECURITY.md", "CHANGELOG.md"] {
        assert!(
            !read(file).contains("<img") && !read(file).contains("<br"),
            "{file}"
        );
    }

    let readme = read("README.md");
    for file in ["RELEASING.md", "SECURITY.md", "CHANGELOG.md"] {
        assert!(
            readme.contains(&format!("]({REPOSITORY}/blob/main/{file})")),
            "{file}"
        );
        assert!(workspace_root().join(file).is_file(), "{file}");
    }
    let agents = read("AGENTS.md");
    for command in [
        "cargo xtask release prepare",
        "cargo xtask release tag",
        "cargo xtask upstream-sync",
        "`RELEASING.md`",
        "A pushed `v*` tag starts `release.yml`",
    ] {
        assert!(agents.contains(command), "{command}");
    }
    let usage = read("xtask/src/main.rs");
    for command in [
        "release prepare",
        "release tag",
        "upstream-sync",
        "changelog-section",
    ] {
        assert!(usage.contains(&format!("\n  {command} ")), "{command}");
        assert!(
            readme.contains(&format!("\ncargo xtask {command} ")),
            "{command}"
        );
    }
}

#[test]
fn the_end_to_end_suite_runs_only_on_request_on_linux_after_the_tests_and_never_on_forks() {
    let ci = read(".github/workflows/ci.yml");
    assert!(!ci.contains("secrets.ANTHROPIC_API_KEY"));
    assert!(!ci.contains("cargo xtask e2e"));

    let workflow = read(".github/workflows/e2e.yml");
    assert!(compact(&workflow).starts_with("name: End-to-end on: workflow_dispatch: permissions:"));
    for trigger in ["push:", "pull_request", "schedule:", "workflow_run:"] {
        assert!(!workflow.contains(trigger), "{trigger}");
    }
    assert!(compact(&workflow).contains("permissions: contents: read"));

    let job = ci_job(&workflow, "e2e");
    assert!(job.contains("if: ${{ !github.event.repository.fork }}"));
    assert!(job.contains("runs-on: ubuntu-latest"));
    let tests = job
        .find("run: cargo test --workspace --all-features")
        .expect("the job does not run the tests");
    let suite = job
        .find("run: cargo xtask e2e")
        .expect("the job does not run the suite");
    assert!(tests < suite, "the tests must pass before the suite starts");
    assert!(job.contains("ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}"));
    assert!(!job.contains("CLAUDE_AGENT_RUST_SDK_E2E_AUTH"), "{job}");

    assert!(workflow.contains(&format!("CLAUDE_CODE_VERSION: {CLI_VERSION}")));
    assert!(job.contains("| bash -s \"$CLAUDE_CODE_VERSION\""));
}

#[test]
fn upstream_sdk_is_a_submodule_pinned_to_the_reference_commit() {
    let modules = compact(&read(".gitmodules"));
    assert!(modules.contains(&format!("path = {UPSTREAM_PATH}")));
    assert!(modules.contains("url = https://github.com/anthropics/claude-agent-sdk-python.git"));

    let root = workspace_root();
    if !root.join(".git").exists() {
        return;
    }
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "--stage", "--", UPSTREAM_PATH])
        .output()
    else {
        return;
    };
    assert!(output.status.success());
    let entry = String::from_utf8(output.stdout).unwrap();
    assert!(
        entry.starts_with(&format!("160000 {UPSTREAM_COMMIT} ")),
        "unexpected index entry: {entry:?}"
    );
}

#[test]
fn version_constants_match_the_manifest_and_the_upstream_pin() {
    assert_eq!(claude_agent_rust_sdk::VERSION, env!("CARGO_PKG_VERSION"));
    assert_eq!(claude_agent_rust_sdk::CLI_VERSION, "2.1.329");
    assert_eq!(claude_agent_rust_sdk::UPSTREAM_SDK_VERSION, "0.2.186");
}

#[test]
fn xtask_alias_is_configured() {
    let config = compact(&read(".cargo/config.toml"));
    assert!(config.contains("xtask = \"run --quiet --package xtask --\""));
}
