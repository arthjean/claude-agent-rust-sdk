[PRD]
# PRD: Distribution and release (`claude-agent-rust-sdk`)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-18 | Arthur Jean | Initial draft, from `docs/research/distribution-release.md` and the owner decisions of 2026-11-18 |

## Problem Statement

The crate works and is measured against upstream, but nothing carries it to a user. `docs/research/distribution-release.md` (2026-11-17) inventoried the 45 distribution mechanisms of the Python SDK at commit `4720d36` and compared each with this repository.

1. **The crate cannot be installed from a registry.** There is no tag, no release workflow, and no changelog. The README tells users to depend on the git repository (`README.md:46-50`). `tasks/prd-claude-agent-rust-sdk.md:1149` deferred distribution to this PRD.
2. **The package is not publishable as it stands.** A real `cargo package` run on 2026-11-17 showed four defects: the archive holds neither `LICENSE` nor `THIRD_PARTY_NOTICES`; four untracked `.profraw` files make `cargo package` fail on a clean checkout; no manifest has a `repository` field; 67 test files are packaged although they need `fake-claude`, which is not published.
3. **The name and the license block adoption.** `claude-agent-kit` is a working name that does not say what the crate is. GPL-3.0-or-later excludes every distributed closed-source program, because Rust links statically, while the seven competing crates checked on crates.io are all MIT.
4. **Upstream ships every two days and the port has no release path that keeps up.** Upstream published 13 to 23 releases a month from June to October 2026, and 102 of its 183 changelog sections hold only a bump of the bundled CLI. The upstream watch runs weekly. Each Rust release would today need hand edits in a dozen files, including `parity/differences.toml`, which stores both version numbers as literals.

**Why now:** the repository has been public since 2026-11-16, the parity work of `tasks/prd-parity-hardening.md` is certified on Linux, and the most downloaded competing crate (`anthropic-agent-sdk`) has had no release since 2025-12-23. A crate that follows upstream within days is the gap to fill, and a crate name is only held once it is published.

## Overview

This PRD delivers the path from a merged port to a version on crates.io. First, the crates are renamed to `claude-agent-rust-sdk` and `claude-agent-rust-sdk-macros`, relicensed under `MIT OR Apache-2.0`, and made publishable, with a CI job that packages them on every pull request. Second, a release is reduced to three owner commands: `cargo xtask release prepare`, `cargo xtask release tag`, and a push. The pushed tag starts `release.yml`, which reruns the whole CI, publishes both crates through crates.io trusted publishing, attests the published archives, and creates the GitHub Release from the changelog. `cargo xtask upstream-sync <tag>` moves the upstream pin and every copy of the pinned versions in one command, so a CLI-only upstream release is ported in minutes. Third, the first publication is done in three steps that prove the pipeline before `0.1.0`. Fourth, an API compatibility check and a scheduled advisory check run once a version exists on the registry.

Key decisions. The release logic lives in `xtask`, not in a third-party tool: release-plz creates tags itself and is built around bot pull requests, cargo-release writes no changelog, and neither knows the copies this repository keeps in step (the differences ledger, the generated parity page, the README). The trigger is a tag pushed by the owner, because crates.io refuses the `workflow_run` trigger that the Python SDK uses and because the owner is the only visible contributor: no workflow commits, tags, or pushes. The cadence is one Rust release per ported upstream release, published as soon as the port is merged; the crate version is independent (`0.y.z`), and each changelog section states the upstream versions it covers. The Python SDK's bundling of the CLI, its PyPI quota checks, and its Claude-in-CI jobs are not ported.

Stories continue the numbering of `tasks/prd-parity-hardening.md` (which ends at EP-011 and US-057). The remaining open epic of that PRD, EP-011, is unaffected except that its paths read with the new crate names once US-058 lands. Delivery is phased in four releases, one per epic: A (EP-012), B (EP-013), C (EP-014), D (EP-015).

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Median delay between an upstream release and the Rust release that ports it | 5 days or less | 2 days or less, 90th percentile 7 days or less |
| Owner commands for a release once the port is merged | 3 (`release prepare`, `release tag`, push) | 3 |
| Releases published through trusted publishing, with attested archives | 100% from `0.1.0-rc.2` on | 100% |
| Delay between the tag push and the version being installable from crates.io | 45 minutes or less | 30 minutes or less |
| Owner time for a CLI-only upstream release, from `upstream-sync` to the pushed tag | 15 minutes or less | 10 minutes or less |

## Target Users

### Rust application developer
- **Role:** builds an agent service on Claude and wants a maintained Rust SDK.
- **Behaviors:** runs `cargo add`, reads crates.io and docs.rs, runs `cargo deny` in CI, pins versions with `Cargo.lock`.
- **Pain points:** the crate is not on crates.io; a git dependency has no versions, no changelog, and no docs.rs page; a GPL dependency fails the license check of most companies.
- **Current workaround:** depends on the git repository at a commit, or picks a competing MIT crate that lags upstream by months.
- **Success looks like:** `cargo add claude-agent-rust-sdk` works, the changelog says which upstream version each release matches, and a new upstream feature is available within days.

### SDK maintainer
- **Role:** ports each upstream release and publishes it (today: the owner, working with coding agents).
- **Behaviors:** reads the upstream-watch issue, runs `cargo xtask upstream-diff`, moves the pin, updates the ledgers, runs the gates.
- **Pain points:** a release would need hand edits in about a dozen files; 56% of upstream releases are CLI bumps that still need every copy moved; no step publishes anything.
- **Current workaround:** none; nothing has been released.
- **Success looks like:** a CLI-only upstream release goes from issue to crates.io in under 15 minutes of his time, and no release needs a stored registry token.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Python Claude Agent SDK (the reference):** a bot pushes a CLI bump, `auto-release.yml` computes patch+1, `build-and-publish.yml` builds five wheels and an sdist, uploads them through PyPI trusted publishing, has Claude write the changelog, pushes the version commit and the tag to `main` with a deploy key, and creates a GitHub Release with no asset. It verifies no checksum of the bundled binary, attests nothing, and its changelog lags four versions behind because that job may fail silently.
- **release-plz:** `release-plz update` edits `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` locally without committing; `release-plz release` creates the git tag itself and a forge release. Handling of a version shared through `[workspace.package]` is not documented.
- **cargo-release:** runs `version`, `replace`, `commit`, `publish`, `tag`, `push` as separable steps, supports `pre-release-replacements` and a shared workspace version, and writes no changelog.
- **git-cliff:** generates a changelog section from conventional commits and can prepend it to an existing file.
- **Competing Rust crates:** `anthropic-agent-sdk`, `claude-agent-sdk-rs`, `claude-code-agent-sdk`, `cc-sdk`, `claude-code-sdk-rust`, `claude-agent-sdk`, `claude-agent-sdk-rust` are all MIT; none publishes a parity proof or an attested release.
- **Market gap:** a Rust SDK that follows upstream within days, states its upstream version in every release, and publishes without a stored token.

### Best Practices Applied
- crates.io trusted publishing: a job with `id-token: write` exchanges its GitHub identity token for a short-lived registry token through `rust-lang/crates-io-auth-action`. The first version of a crate must be published by hand. A crate can then refuse token-based publishing ("Trusted Publishing Only"). The `pull_request_target` and `workflow_run` triggers are refused.
- crates.io reads the workflow filename from the `workflow_ref` claim, which names the top-level workflow, so the publishing job stays in `release.yml` itself.
- A publish is permanent: a version is never overwritten or deleted, and a yank removes nothing. `cargo publish` of several crates is ordered by dependency and not atomic.
- In `0.y.z`, a change of `y` is the breaking change; a change of `z` must stay compatible.
- `actions/attest` attests any file by path and needs `id-token: write`, `attestations: write`, and `artifact-metadata: write`. GitHub immutable releases lock the tag and the assets of a release.
- docs.rs builds on nightly with `--cfg docsrs`; `doc_auto_cfg` was merged into `doc_cfg`.
- `cargo-semver-checks` reads rustdoc JSON, whose format changes with the compiler: version 0.47.0 rejects the format of Rust 1.100.0 (probe run on 2026-11-18, "unsupported rustdoc format v61"). The tool and the toolchain of its job are pinned together.

*Full research sources: `docs/research/distribution-release.md` (inventory, evidence, and the URLs of every external fact), plus the tool documentation read on 2026-11-18 (release-plz, cargo-release, git-cliff, cargo-semver-checks) and the crates.io source files `crates/crates_io_trustpub/src/github/workflows.rs` and `src/rate_limiter.rs`.*

## Assumptions & Constraints

### Assumptions (to validate)
- `claude-agent-rust-sdk` and `claude-agent-rust-sdk-macros` are still free when US-071 runs. The crates.io API answered 404 for both on 2026-11-18; a name is held only once published.
- crates.io accepts trusted publishing from `release.yml` when that workflow also calls `ci.yml` as a reusable workflow, because the publishing job is in `release.yml`. Inferred from the registry source; validated by US-072.
- A tag ruleset can restrict who creates `v*` tags on a public repository owned by a personal account. Not confirmed in the GitHub documentation read; validated by US-072, with immutable releases and the `release` environment as the fallback.
- crates.io resolves relative README links in a way that can break for a crate in a subdirectory. Not confirmed; the README uses absolute links, so the question does not matter.
- `cargo add` does not select a pre-release by default. Indirect evidence only; checked in US-071.
- Relicensing is the owner's right: he is the author of all 66 commits, and upstream is MIT. Not legal advice.
- GitHub redirects the old repository URL after the rename.
- The window between US-061 (README says `cargo add`) and US-071 (first publication) is acceptable: the repository has no tagged release and no known user.

### Hard Constraints
- The crate names are `claude-agent-rust-sdk` and `claude-agent-rust-sdk-macros`, and the license is `MIT OR Apache-2.0` (owner decisions of 2026-11-18). These replace the working name and the GPL constraint of `tasks/prd-claude-agent-rust-sdk.md`, which stays unedited as a closed record.
- The owner is the only visible contributor: no workflow or tool commits, tags, or pushes under another identity, and no commit carries AI attribution or a `Co-authored-by` line. A bot-authored issue or GitHub Release is acceptable.
- Nothing is published to crates.io, posted on GitHub, or changed in the settings of either without the owner's explicit go for that item. Every story of EP-014 stops for him.
- Runs against the real CLI spend API credits (about 0.80 USD per full run) and stay manual. No story requires one to pass its quality gates.
- No long-lived registry token is stored in GitHub. The one token of US-071 is created by the owner, used on his machine, and revoked in US-072.
- A published version is never reused, overwritten, or republished.
- GitHub Actions are referenced by tag and locked through `gh actions-lock` in `.github/workflows/actions.lock`.
- The floors stay: line coverage at or above 91.5%, the lint levels of the root `Cargo.toml` and `clippy.toml`, Rust 1.88 as the minimum, edition 2024.
- Doc comments only in library sources, `unsafe` only in `src/process/sys.rs`, and every other rule of `AGENTS.md`.
- Every artifact is written in English.
- The repository is public: no secret is read in a job triggered by `pull_request`, and the release workflow never runs on a fork.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --all --check` - formatting
- `cargo clippy --workspace --all-targets --all-features --no-deps` - lint policy
- `cargo test --workspace --all-features` - unit, integration, fake-CLI, and repository policy tests
- `cargo doc --workspace --all-features --no-deps` - documentation builds with `RUSTDOCFLAGS="-D warnings"`
- `cargo xtask parity-check --deny-pending` - ledgers complete, every Rust path exists, generated pages fresh

For stories that change `parity/`, `crates/parity-driver`, `crates/fake-claude`, or what reaches the CLI (US-058, US-064, US-068):
- `cargo xtask parity-diff` - every differential case matches and no ledger entry is stale
- `cargo xtask parity-diff --coverage` - the option coverage is unchanged

For stories that change a manifest or a dependency (US-058, US-059, US-060, US-074, US-075):
- `cargo deny check licenses` - every dependency license is on the allow list
- `cargo +1.88.0 check --workspace --all-features --locked` - the minimum Rust version still builds

For stories that change `.github/workflows/*.yml` (US-062, US-066, US-067, US-069, US-074, US-075):
- `gh actions-lock --verify` - every action is locked to a commit

For stories that add or change a script under `.github/scripts` (US-066, US-067, US-069):
- `bash .github/scripts/<name>.test.sh` - the offline test of that script

From US-062 on:
- `cargo xtask package-check` - both crates package, verify, and hold exactly the expected files

## Epics & User Stories

Reference root for every `Python reference` block: `/home/arthur/dev/claude-agent-sdk-python` on Fedora, `C:/dev/claude-agent-sdk-python` on Windows, at commit `4720d36` (the same tree as the submodule `upstream/claude-agent-sdk-python`). Line numbers are valid for that commit only. Rust paths are relative to the repository root and use the names that exist before US-058; after US-058, read `crates/claude-agent-kit` as `crates/claude-agent-rust-sdk`.

### EP-012: Rename, relicense, and make the package publishable

Give the crates their final names and license, and make `cargo package` produce an archive that is complete, minimal, and checked on every pull request. Release A. Nothing is published.

**Definition of Done:** the workspace builds and passes every gate under the new names; no tracked file outside dated records and closed PRDs holds the old name or a GPL statement; `cargo xtask package-check` passes in CI from a clean checkout.

**Python reference:**
- `pyproject.toml`: 5-60 (registry metadata), 62-77 (what the wheel and the sdist contain)
- `LICENSE`: 1-3 (MIT, copyright Anthropic, PBC)
- `README.md`: 1-18 (installation), 375-377 (license and terms)
- `.github/workflows/build-wheel-check.yml`: 5-124 (package verification on pull requests)

#### US-058: Rename the crates to `claude-agent-rust-sdk`
**Description:** As a Rust application developer, I want the crate named after what it is so that I find it when I search for a Claude Agent SDK in Rust.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Python reference:**
- `pyproject.toml`: 6 (the package is named `claude-agent-sdk`)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 767-769 (the entrypoint label and the version the SDK sends)

**Acceptance Criteria:**
- [ ] The packages are named `claude-agent-rust-sdk` and `claude-agent-rust-sdk-macros`, their library names are `claude_agent_rust_sdk` and `claude_agent_rust_sdk_macros`, and their directories are `crates/claude-agent-rust-sdk` and `crates/claude-agent-rust-sdk-macros`, moved with `git mv`
- [ ] Every Rust path in `parity/surface.toml`, `parity/tests.toml`, and `parity/differences.toml` uses the new names, and `docs/migration.md` and `docs/parity.md` are regenerated by their xtask commands
- [ ] Names used at run time follow: the managed directory (`MANAGED_DIR_NAME` in `src/cli.rs`) becomes `claude-agent-rust-sdk`, the spawn thread becomes `claude-agent-rust-sdk-spawn`, and the eight environment variables prefixed `CLAUDE_AGENT_KIT_` take the prefix `CLAUDE_AGENT_RUST_SDK_`
- [ ] `xtask`, the four workflows, `AGENTS.md`, `README.md`, `parity/README.md`, the examples, and the rustdoc use the new names, and links to the repository use `https://github.com/arthjean/claude-agent-rust-sdk`
- [ ] The entrypoint label sent to the CLI stays `sdk-rs`, and the differential harness reports no new difference
- [ ] A repository policy test fails when the old name, in any of its three spellings (`claude-agent-kit`, `claude_agent_kit`, `CLAUDE_AGENT_KIT`), appears in a tracked file outside `docs/research/`, `docs/validation/`, `tasks/`, and `upstream/`
- [ ] Given a ledger entry that still names an old Rust path, when the ledgers are checked, then the check fails naming the entry
- [ ] The open questions record that the owner renames the GitHub repository; no criterion of this story depends on that rename having happened

#### US-059: Relicense to `MIT OR Apache-2.0`
**Description:** As a Rust application developer, I want the crate under the license the Rust ecosystem uses so that my company's license check accepts it and I can ship a closed-source program.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-058

**Python reference:**
- `pyproject.toml`: 11, 18 (`license = {text = "MIT"}` and its classifier)
- `LICENSE`: 1-3

**Acceptance Criteria:**
- [ ] `[workspace.package]` sets `license = "MIT OR Apache-2.0"`, inherited by every member
- [ ] The repository root holds `LICENSE-MIT` (copyright line `Copyright (c) 2026 Arthur Jean`) and `LICENSE-APACHE` (the unmodified Apache License 2.0 text), and the GPL `LICENSE` file is removed
- [ ] `THIRD_PARTY_NOTICES` states the new license of the project and keeps the upstream MIT notice word for word, with the pinned commit and version
- [ ] The README License section names both licenses, links both files, carries the usual contribution sentence (a contribution is dual licensed unless stated otherwise), and keeps the paragraph on the upstream notice; the GPL consequence paragraph is removed
- [ ] `deny.toml` no longer allows `GPL-3.0-or-later`
- [ ] The repository policy tests on the license are rewritten for the new state: both license files, the upstream notice, the README sentences, and the allow list
- [ ] Given `deny.toml` allows an identifier of the GPL, AGPL, LGPL, SSPL, or BUSL families, when the policy test runs, then it fails naming the identifier
- [ ] No tracked file outside `docs/research/`, `docs/validation/`, `tasks/`, and `upstream/` states that the project is under the GPL

#### US-060: Registry metadata and package contents
**Description:** As a Rust application developer, I want the published archive to hold the license files, the sources, and nothing else so that the crate is legally complete and small.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-059

**Python reference:**
- `pyproject.toml`: 5-60 (name, description, readme, license, classifiers, keywords, urls), 66-77 (sdist contents)

**Acceptance Criteria:**
- [ ] Both published manifests carry `description`, `license`, `repository`, `homepage`, `documentation`, `readme`, at most 5 `keywords`, and at most 5 `categories` that are valid crates.io slugs; `repository` and `homepage` are inherited from `[workspace.package]`
- [ ] `claude-agent-rust-sdk-macros` has its own `README.md` saying that it is used through the `macros` feature of `claude-agent-rust-sdk`
- [ ] The library depends on the macros crate with an exact requirement (`=<workspace version>`) next to its `path`
- [ ] Each published crate directory holds `LICENSE-MIT` and `LICENSE-APACHE`, the library also holds `THIRD_PARTY_NOTICES`, and a policy test fails when a copy differs from the file at the repository root
- [ ] Both manifests declare an explicit `include` list, and the library archive lists only the manifest files cargo generates, `README.md`, the two license files, `THIRD_PARTY_NOTICES`, `src/**`, and `examples/**`; no file under `tests/` is packaged
- [ ] No source file under `src/` reaches a file outside the package by `#[path]`: the log-capture helper shared with the integration tests moves into the package
- [ ] `.gitignore` ignores `*.profraw`
- [ ] Given a checkout where a coverage run left `.profraw` files, when the library is packaged without `--allow-dirty`, then packaging succeeds
- [ ] `[package.metadata.docs.rs]` keeps `all-features = true` and adds `rustdoc-args = ["--cfg", "docsrs"]`, the library enables `doc_cfg` under `cfg(docsrs)` only, and the build on Rust 1.88 is unchanged
- [ ] `fake-claude`, `parity-driver`, and `xtask` stay `publish = false`, and a policy test fails when one of them becomes publishable

#### US-061: README and crate documentation for registry readers
**Description:** As a Rust application developer, I want the crates.io page and the docs.rs page to tell me how to install the crate and what compatibility to expect so that I can adopt it without opening the repository.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-060

**Python reference:**
- `README.md`: 1-18 (installation and prerequisites), 95 (minimum CLI statement), 299-306 (migration notes point to the changelog)
- `pyproject.toml`: 9 (the README is the registry page), 57-60 (project URLs)

**Acceptance Criteria:**
- [ ] The Installation section shows `cargo add claude-agent-rust-sdk` and a `Cargo.toml` snippet whose requirement is the `major.minor` of the workspace version; the sentence saying the crate is not on crates.io is removed
- [ ] A policy test fails when the `major.minor` of the README snippet differs from the workspace version
- [ ] The README opens with badges for the crates.io version, the docs.rs page, and the CI status
- [ ] Every link from the README to a file of the repository is an absolute `https://github.com/arthjean/claude-agent-rust-sdk/...` URL
- [ ] Given the README gains a relative link target, when the policy tests run, then one fails naming the link
- [ ] A "Versioning and compatibility" section states: a change of the minor version may break in `0.y.z`, a patch never does; raising the minimum Rust version is a minor change; the `testing` feature is outside the compatibility promise; each release states the upstream version it matches; it links `CHANGELOG.md`
- [ ] The crate documentation in `src/lib.rs` marks the `testing` feature as outside the compatibility promise, and an item gated by a feature shows that feature on docs.rs
- [ ] The README sentences on authentication and non-affiliation stay word for word, and the README code blocks still compile as documentation tests

#### US-062: Package check in CI
**Description:** As an SDK maintainer, I want every pull request to package both crates and check the result so that a packaging defect is found before a release, not during it.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-060

**Python reference:**
- `.github/workflows/build-wheel-check.yml`: 5-20 (trigger), 60-101 (name, contents, install, and import checks), 109-124 (the check matrix must match the release matrix)
- `scripts/build_wheel.py`: 514-544 (`twine check` only warns)

**Acceptance Criteria:**
- [ ] `cargo xtask package-check` packages `claude-agent-rust-sdk-macros` and `claude-agent-rust-sdk` with the lockfile, and the verification build of both archives passes
- [ ] The command fails, naming the cause, when an archive lists a file outside the allowed set, misses a license file, misses `repository`, `license`, `description`, or `readme` in its packaged manifest, or exceeds 1 MiB compressed
- [ ] Given an untracked file inside a published crate directory, when the command runs, then it fails with cargo's uncommitted-changes message; the command never passes `--allow-dirty`
- [ ] The checker's rules are covered by unit tests on fixture file lists: a stray file, a missing license, an oversized archive, a missing manifest field
- [ ] A CI job `package` runs the command on Linux for every pull request and every push to `main`, with `contents: read` and no secret
- [ ] The same job builds the library documentation the way docs.rs does (nightly toolchain, all features, `--cfg docsrs`, warnings denied)
- [ ] The repository policy test on the shape of CI covers the `package` job
- [ ] The usage text of `xtask` and the Development section of the README list the command

---

### EP-013: Release machinery

Reduce a release to three owner commands and a workflow that publishes from the pushed tag, and make a CLI-only upstream release a one-command port. Release B. Nothing is published: the workflow is proven by offline script tests until EP-014.

**Definition of Done:** on a clean `main`, `cargo xtask release prepare patch` followed by `cargo xtask release tag` produces one release commit and one annotated tag locally, with no hand edit; the offline test of the release script passes every case; `cargo xtask upstream-sync` on a CLI-only test tag leaves the workspace test suite green with no hand edit.

**Python reference:**
- `RELEASING.md`: 23-86 (versioning, the two release paths, scripts, secrets)
- `scripts/update_version.py`: 16-40; `scripts/update_cli_version.py`: 22-59; `scripts/_cli_version_validation.py`: 30, 92-119
- `.github/workflows/auto-release.yml`: 3-56; `.github/workflows/publish.yml`: 3-85
- `.github/workflows/build-and-publish.yml`: 45-99 (publish job), 220-230 (upload), 246-324 (commit, tag, GitHub Release)
- `CHANGELOG.md`: 1-18; `.github/workflows/generate-changelog.yml`: 46-97; `.claude/commands/generate-changelog.md`: 6-19

#### US-063: `CHANGELOG.md` and its policy
**Description:** As a Rust application developer, I want a changelog that says what changed and which upstream version each release matches so that I can decide when to upgrade.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-058

**Python reference:**
- `CHANGELOG.md`: 1-18 (one `## <version>` section per release, grouped under `###` headings, no date)
- `.claude/commands/generate-changelog.md`: 10-17 (the section names)

**Acceptance Criteria:**
- [ ] `CHANGELOG.md` at the repository root opens with `# Changelog`, then `## Unreleased`, then released sections titled `## <version> - <YYYY-MM-DD>`, newest first
- [ ] Each released section starts with one line `Upstream: Python SDK <version>` or `Upstream: Python SDK <first> to <last>`, followed by `, CLI <version>`
- [ ] Entries are grouped under `### Breaking changes`, `### Added`, `### Changed`, and `### Fixed`, and only the groups that have entries appear
- [ ] `## Unreleased` holds a hand-written summary of what the first release contains
- [ ] A policy test fails when a released section lacks its `Upstream:` line, when versions are not strictly decreasing, or when a section title does not match the format
- [ ] A policy test fails when released sections exist and the newest one is not the workspace version
- [ ] `cargo xtask changelog-section <version>` prints the body of that section
- [ ] Given a version with no section, when `cargo xtask changelog-section` runs, then it exits non-zero naming the version
- [ ] The README links the changelog

#### US-064: `cargo xtask release prepare`
**Description:** As an SDK maintainer, I want one command to move the version everywhere and draft the changelog section so that a release needs no hand edit and no forgotten copy.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-060, US-063

**Python reference:**
- `scripts/update_version.py`: 16-40 (rewrites the two copies of the version, without validating the input)
- `.github/workflows/auto-release.yml`: 31-38 (patch+1)
- `.github/workflows/generate-changelog.yml`: 46-66 (the changelog is written by a model in CI; not ported)

**Acceptance Criteria:**
- [ ] `cargo xtask release prepare <patch|minor|X.Y.Z[-pre]>` computes the next version from the workspace version and rewrites the workspace version, the exact requirement on the macros crate, the README snippet when the minor changes, and `Cargo.lock` through cargo
- [ ] The `[wire.sdk-version]` entry of `parity/differences.toml` stores the tokens `{crate_version}` and `{upstream_version}` instead of literal versions, and the differential comparison and the parity page resolve them from the workspace manifest and the submodule's `pyproject.toml`
- [ ] The command regenerates the migration table and the parity page, so both are fresh after it runs
- [ ] The command turns `## Unreleased` into `## <version> - <today>` and inserts a new empty `## Unreleased`; the `Upstream:` line gives the range from the version after the previous section's last one to `UPSTREAM_SDK_VERSION`, and the current `CLI_VERSION`
- [ ] Given `## Unreleased` has no entry, when the command runs, then it drafts entries from the conventional commits since the previous tag: `feat` under Added, `fix` under Fixed, `perf` and `refactor` under Changed, a `!` marker or a `BREAKING CHANGE:` footer under Breaking changes, every other type left out
- [ ] The command refuses, exits non-zero, and writes nothing when the working tree is dirty, the branch is not `main`, the version is not greater than the current one, the tag `v<version>` already exists, or `patch` is requested while a commit since the previous tag is marked breaking
- [ ] The command makes no commit, creates no tag, and pushes nothing; it ends by printing the changed files and the next command
- [ ] `--dry-run` prints the planned changes and writes nothing
- [ ] Given a failure after some files were computed, when the command exits, then no file is left partly rewritten
- [ ] Integration tests run the command in a temporary git repository, in the style of `xtask/tests/upstream_diff.rs`, and cover each refusal and each bump kind

#### US-065: `cargo xtask release tag`
**Description:** As an SDK maintainer, I want one command to check the prepared release and create its commit and tag under my identity so that the only step left is the push.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-062, US-064

**Python reference:**
- `.github/workflows/build-and-publish.yml`: 269-277 (a bot commits `chore: release v$VERSION`), 318-319 (annotated tag); the Rust tool runs on the owner's machine instead

**Acceptance Criteria:**
- [ ] `cargo xtask release tag` runs the gates and the package check, then creates one commit `chore: release v<version>` holding the files `release prepare` changed and one annotated tag `v<version>`
- [ ] The commit and the tag use the git identity and the signing settings configured on the machine, and carry no trailer
- [ ] The command pushes nothing and ends by printing the one push command that sends the branch and the tag together
- [ ] `--no-gates` skips the gates and still runs the package check
- [ ] The command refuses, exits non-zero, and creates nothing when the changelog has no non-empty section for the workspace version, when a changed file is outside the set `release prepare` writes (the files are listed), or when the tag exists
- [ ] Given a gate or the package check fails, when the command runs, then it exits non-zero and neither the commit nor the tag exists afterwards
- [ ] Integration tests in a temporary git repository cover the commit content, the tag, and each refusal

#### US-066: Release workflow: verify and publish
**Description:** As an SDK maintainer, I want a pushed tag to rerun the whole CI and publish both crates without a stored token so that publishing is one push and no secret can leak.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-062, US-065

**Python reference:**
- `.github/workflows/publish.yml`: 12-58 (tests and lint before a manual release), 75-85 (call of the reusable workflow)
- `.github/workflows/build-and-publish.yml`: 45-52 (environment `production`, `id-token: write`), 73-90 (artifact completeness), 220-230 (upload with `--skip-existing`, no token)
- `.github/workflows/auto-release.yml`: 3-15 (`workflow_run` trigger; not usable with crates.io)

**Acceptance Criteria:**
- [ ] `.github/workflows/release.yml` starts only on a pushed tag of the form `v<major>.<minor>.<patch>` with an optional pre-release part; it has no `pull_request`, `pull_request_target`, `workflow_run`, `schedule`, or `workflow_dispatch` trigger, and no job runs on a fork
- [ ] The workflow's default permission is `contents: read`, and one `concurrency` group serializes releases without cancelling a running one
- [ ] A `verify` job fails with a message when the tag is not `v` plus the workspace version, when the tagged commit is not an ancestor of `main`, or when the changelog has no non-empty section for that version
- [ ] `ci.yml` gains a `workflow_call` trigger, and the release workflow runs it in full on the tagged commit before publishing
- [ ] A `publish` job, the only one in the `release` environment and the only one with `id-token: write`, obtains a short-lived token through `rust-lang/crates-io-auth-action` and publishes the macros crate, then the library, with the lockfile
- [ ] Before each crate, the job asks the registry whether that name and version exist, and skips the crate when they do
- [ ] Given the macros crate is published and the library fails, when the workflow is rerun, then only the library is published
- [ ] Given the registry lookup fails, when the job runs, then it fails and publishes nothing
- [ ] The workflow references no stored registry token, and a policy test fails when `CARGO_REGISTRY_TOKEN` is read from `secrets`
- [ ] No step commits, tags, or pushes to the repository
- [ ] The decisions are in `.github/scripts/release.sh`, with an offline test `.github/scripts/release.test.sh` run by the `scripts` job of CI and covering: tag and version mismatch, tag off `main`, missing changelog section, crate already published, second crate failing, registry lookup error
- [ ] A repository policy test pins the shape of the workflow: triggers, fork guard, default permission, the environment on the publish job only, and `id-token: write` on no job that does not need it

#### US-067: GitHub Release with attested crate archives
**Description:** As a Rust application developer, I want each release to have a GitHub Release with its notes and the attested archives so that I can read what changed and verify what I downloaded.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-066

**Python reference:**
- `.github/workflows/build-and-publish.yml`: 321-324 (notes extracted from the changelog, a registry link, no asset)
- No attestation, signature, or component list exists upstream (`docs/research/distribution-release.md`, section 2.4)

**Acceptance Criteria:**
- [ ] After publishing, the workflow downloads both `.crate` archives of the released version from crates.io and attests them with `actions/attest`, in a job whose write permissions are `id-token`, `attestations`, and `artifact-metadata` only
- [ ] A last job, the only one with `contents: write`, creates the GitHub Release `v<version>` whose notes are the changelog section of that version followed by links to the crates.io and docs.rs pages of that version, and attaches the two attested archives
- [ ] A version with a pre-release part produces a GitHub Release marked as a pre-release
- [ ] Given a GitHub Release for the tag already exists, when the workflow is rerun, then the job ends green and changes nothing
- [ ] Given publishing failed, when the workflow ends, then no GitHub Release and no attestation exist for the tag
- [ ] The offline script test gains the cases: existing release, pre-release flag, content of the notes
- [ ] The policy test on the workflow shape covers the two new jobs and their permissions

#### US-068: `cargo xtask upstream-sync <tag>`
**Description:** As an SDK maintainer, I want one command to move the upstream pin and every copy of the pinned versions so that a CLI-only upstream release is ported in minutes.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-063, US-064

**Python reference:**
- `src/claude_agent_sdk/_cli_version.py`: 3 (the bundled CLI version); `pyproject.toml`: 7 (the SDK version)
- `scripts/update_cli_version.py`: 22-59 (the upstream bump script)
- `RELEASING.md`: 32-49 (every CLI bump yields an SDK patch release)
- `CHANGELOG.md`: 3-7 (a CLI-only section)

**Acceptance Criteria:**
- [ ] `cargo xtask upstream-sync <tag>` fetches the tag in the submodule and moves the pin to it; the pin is its only change under `upstream/`
- [ ] The command reads the SDK version and the CLI version from the submodule and rewrites every copy the policy tests pin: the constants and the crate documentation of the library, the README, `docs/migration.md`, `THIRD_PARTY_NOTICES`, the CLI version of `e2e.yml`, the constants of `tests/workspace.rs`, and the default version of `fake-claude` with its tests
- [ ] The command regenerates the migration table and the parity page, and adds one entry under `### Changed` of `## Unreleased` naming the new upstream and CLI versions
- [ ] The command prints `verdict: cli-only` when the files changed between the old pin and the tag are among `src/claude_agent_sdk/_cli_version.py`, `src/claude_agent_sdk/_version.py`, `CHANGELOG.md`, and the version line of `pyproject.toml`; otherwise it prints `verdict: needs-port` followed by what `upstream-diff` lists
- [ ] Given a `cli-only` verdict, when the command ends, then the workspace test suite passes with no hand edit
- [ ] Given a `needs-port` verdict, when the command ends, then the pin and the copies are moved and the output says the ledgers must be completed before a release
- [ ] The command accepts a tag several releases ahead of the pin
- [ ] The command refuses, exits non-zero, and changes nothing when the tag is unknown, when it is not newer than the pin, or when the working tree is dirty
- [ ] The command makes no commit, runs no real CLI, and uses the network only to fetch the submodule
- [ ] Integration tests against a local stand-in upstream repository cover both verdicts and each refusal

#### US-069: Daily upstream watch
**Description:** As an SDK maintainer, I want to learn about an upstream release within a day, with the command to run, so that the port starts the day upstream ships.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-068

**Python reference:**
- `.github/workflows/auto-release.yml`: 3-15 (upstream releases on each CLI bump)
- `git log --grep='^chore: release v'` on the reference checkout: 13 to 23 releases a month from June to October 2026

**Acceptance Criteria:**
- [ ] `upstream-watch.yml` runs once a day and on manual dispatch
- [ ] The issue it opens also carries the verdict (`cli-only` or `needs-port`) and the exact `cargo xtask upstream-sync <tag>` command
- [ ] The workflow still opens at most one issue per upstream tag, makes no commit, and grants `issues: write` to one job only
- [ ] Given the tag lookup fails, when the workflow runs, then the run fails and never reports that the pin is up to date
- [ ] The repository policy test on the watch is renamed and pins the daily schedule
- [ ] The offline test of `upstream-watch.sh` covers the two verdicts in the issue body
- [ ] The README and `AGENTS.md` say "daily" where they said "weekly"

#### US-070: `RELEASING.md` and `SECURITY.md`
**Description:** As an SDK maintainer, I want the release procedure and the recovery steps written down so that a release, a failed release, and a yank are done the same way each time.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-065, US-067, US-068

**Python reference:**
- `RELEASING.md`: 1-86 (the whole guide)
- No `SECURITY.md`, contribution guide, or issue template exists upstream (`docs/research/distribution-release.md`, section 2.7)

**Acceptance Criteria:**
- [ ] `RELEASING.md` covers: the versioning policy; the routine release (sync, port, prepare, tag, push); the CLI-only release; what each job of `release.yml` does; the first publication; when to run the paid end-to-end suite
- [ ] It gives the recovery for each failure of the Edge Cases table of this PRD, including: before any crate is published the tag may be deleted and recreated; after one crate is published the fix is a rerun or a new version, never a reused one
- [ ] It gives the yank procedure and states that a yank deletes nothing
- [ ] It lists every setting outside the repository with its expected value: the `release` environment, the tag ruleset, release immutability, the trusted publisher fields of both crates, "Trusted Publishing Only", and private vulnerability reporting
- [ ] It gives the command that verifies the attestation of a downloaded archive
- [ ] `SECURITY.md` states which versions receive fixes, that reports go through GitHub private vulnerability reporting, an acknowledgment target of 7 days, and that defects of the Claude CLI itself go to Anthropic
- [ ] Given `RELEASING.md` omits a job of `release.yml` or one of the commands `release prepare`, `release tag`, `upstream-sync`, when the policy tests run, then one fails naming what is missing
- [ ] The README links both files, and `AGENTS.md` describes the new commands and the files they own

---

### EP-014: First publication

Publish the crates for the first time and prove the pipeline end to end before `0.1.0`. Release C. Every story here ends in an irreversible, outward-facing action: the implementer prepares and checks, the owner gives his go and runs the step.

**Definition of Done:** `claude-agent-rust-sdk` 0.1.0 and `claude-agent-rust-sdk-macros` 0.1.0 are on crates.io, published by `release.yml` through trusted publishing; both crates refuse token-based publishing; the bootstrap token is revoked; `docs/validation/first-publication.md` records each step.

**Python reference:**
- `RELEASING.md`: 73-86 (required secrets, trusted publishing, the `production` environment)
- `.github/workflows/build-and-publish.yml`: 45-52, 220-230

#### US-071: Validate assumption: both crates publish by hand as `0.1.0-rc.1`
**Description:** As an SDK maintainer, I want a first hand-made publication of a release candidate so that the names are held, crates.io allows trusted publishing afterwards, and the registry pages are checked before `0.1.0`.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-061, US-067, US-070

**Python reference:**
- `pyproject.toml`: 5-60 (what the registry page shows)
- `RELEASING.md`: 80-86

**Acceptance Criteria:**
- [ ] Before anything is published, the record states: the owner's go for this story, that the GitHub repository is renamed, and that both crate names still answer "not found" on crates.io
- [ ] Given a name was taken in the meantime, when the check runs, then nothing is published and the story is blocked for an owner decision
- [ ] The version is set to `0.1.0-rc.1` with `release prepare`, and the owner publishes the macros crate, then the library, from his machine with a token he created for this step
- [ ] The crates.io page of each crate shows the README, the license `MIT OR Apache-2.0`, and the repository link, and five sampled README links open the intended file
- [ ] The docs.rs build of both crates succeeds, and an item gated by a feature shows that feature
- [ ] In an empty project, adding `claude-agent-rust-sdk` at `0.1.0-rc.1` and compiling the quick-start program succeeds, with no CLI call
- [ ] In an empty project, `cargo add claude-agent-rust-sdk` without a version is recorded with its outcome, to settle whether a pre-release is selected by default
- [ ] Given the docs.rs build fails, when the story is reviewed, then the log is recorded and the fix ships as the next release candidate before US-072
- [ ] `docs/validation/first-publication.md` records the date, the versions, the URLs, and every deviation

#### US-072: Validate assumption: the workflow publishes `0.1.0-rc.2` through trusted publishing
**Description:** As an SDK maintainer, I want the release workflow to publish a release candidate with no stored token so that the pipeline is proven before the first stable version.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-071

**Python reference:**
- `RELEASING.md`: 80-86 (the registry trusts the workflow that runs the upload job, in one environment)
- `.github/workflows/build-and-publish.yml`: 48-52, 225-230

**Acceptance Criteria:**
- [ ] With the owner's go, both crates get a trusted publisher on crates.io naming the owner `arthjean`, the repository `claude-agent-rust-sdk`, the workflow `release.yml`, and the environment `release`
- [ ] With the owner's go, the repository has the `release` environment, a ruleset that restricts who creates, updates, and deletes `v*` tags, release immutability, and private vulnerability reporting; a setting GitHub does not offer for this repository is recorded with the fallback used
- [ ] The owner pushes the tag `v0.1.0-rc.2`, and the workflow ends green with both crates at that version on crates.io
- [ ] The GitHub Release is marked as a pre-release and holds two archives whose SHA-256 equal those served by crates.io
- [ ] The attestation of each archive verifies with the command given in `RELEASING.md`
- [ ] Rerunning the workflow publishes nothing and ends green
- [ ] Given the token exchange is refused, when the job fails, then the mismatching claim is recorded from the job log and the configuration is corrected; no stored token is used as a fallback
- [ ] With the owner's go, both crates are set to "Trusted Publishing Only" and the token of US-071 is revoked
- [ ] The record gains the delay between the tag push and the version being installable

#### US-073: Release `0.1.0`
**Description:** As a Rust application developer, I want a stable `0.1.0` on crates.io so that `cargo add claude-agent-rust-sdk` gives me a version I can depend on.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-072

**Python reference:**
- `README.md`: 5-9 (`pip install claude-agent-sdk`)
- `.github/workflows/build-and-publish.yml`: 321-324

**Acceptance Criteria:**
- [ ] With the owner's go, `0.1.0` is released with `release prepare`, `release tag`, and a push, and the workflow ends green
- [ ] In an empty project, `cargo add claude-agent-rust-sdk` selects `0.1.0`, and the README snippet resolves to it
- [ ] The GitHub Release `v0.1.0` is not a pre-release and its notes are the `0.1.0` section of the changelog
- [ ] Given a defect is found after publication, when it is handled, then the version is yanked as `RELEASING.md` describes and the fix ships as `0.1.1`
- [ ] `AGENTS.md` lists "pushing a `v*` tag" among the actions that wait for the owner, with the reason that a publication is permanent
- [ ] The record is completed with the `0.1.0` URLs and the measured delays

---

### EP-015: Compatibility and supply-chain checks

Check, once a version exists on the registry, that a release keeps its compatibility promise and that no dependency carries a known vulnerability. Release D.

**Definition of Done:** a pull request that breaks the public API without a minor bump fails CI; a weekly job fails when an advisory affects a dependency.

**Python reference:**
- `.github/workflows/test.yml`: 44-70 (the only compatibility floor upstream tests is `mcp==1.23.0`)
- No API compatibility check and no automated dependency update exist upstream (`docs/research/distribution-release.md`, sections 2.4 and 2.7)

#### US-074: API compatibility check
**Description:** As a Rust application developer, I want a patch release never to break my build so that I can take every upstream port without reading the diff.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-073

**Python reference:**
- `CHANGELOG.md`: 364, 1505 (breaking changes upstream are announced in the changelog only)

**Acceptance Criteria:**
- [ ] A CI job `semver` runs `cargo-semver-checks` on every pull request, comparing the library with its newest non-yanked version on crates.io
- [ ] The job checks every feature except `testing`, named explicitly, and a policy test fails when the job's feature list differs from the manifest's features minus `testing`
- [ ] The tool is installed at one pinned version with its lockfile, and the job pins the Rust toolchain that this version supports
- [ ] Given a pull request breaks the public API while the workspace minor version equals the published one, when the job runs, then it fails naming the broken items
- [ ] Given the registry is unreachable or the tool rejects the rustdoc format, when the job runs, then it fails with a message saying which pin to update; it never passes without having compared
- [ ] `cargo xtask release prepare patch` refuses when the installed tool reports a breaking change, and says to use `minor`
- [ ] The macros crate is left out of the check, with the reason written in the workflow
- [ ] The repository policy test on the shape of CI covers the `semver` job

#### US-075: Scheduled advisory check
**Description:** As a Rust application developer, I want the maintainer to learn within a week that a dependency has a known vulnerability so that a fixed version follows.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None

**Python reference:**
- No Dependabot or Renovate configuration exists upstream; build tools are hash-pinned in `scripts/requirements-build.txt`: 1-6

**Acceptance Criteria:**
- [ ] `deny.toml` gains an `[advisories]` section
- [ ] A workflow runs `cargo deny check advisories` once a week and on manual dispatch, with `contents: read`, no secret, and no job on a fork
- [ ] Given an advisory affects a dependency in `Cargo.lock`, when the workflow runs, then the run fails and names the advisory
- [ ] Given the advisory database cannot be fetched, when the workflow runs, then the run fails; it never reports a clean result
- [ ] The workflow opens no issue and makes no commit
- [ ] A repository policy test pins the schedule, the permission, and the fork guard
- [ ] `RELEASING.md` says what to do when the run fails

## Functional Requirements

- FR-01: The workspace must publish exactly two crates, `claude-agent-rust-sdk` and `claude-agent-rust-sdk-macros`, at the same version, the library requiring the macros crate at that exact version.
- FR-02: Every published archive must contain `LICENSE-MIT` and `LICENSE-APACHE`, and the library archive must contain `THIRD_PARTY_NOTICES`.
- FR-03: A published archive must NOT contain a file under `tests/`, a coverage profile, or any file outside the allowed set.
- FR-04: `cargo xtask package-check` must fail on any archive that breaks FR-02 or FR-03, and must NOT pass `--allow-dirty`.
- FR-05: `cargo xtask release prepare` must rewrite every copy of the crate version and write the changelog section, and must NOT commit, tag, or push.
- FR-06: `cargo xtask release tag` must create the release commit and the annotated tag under the local git identity, and must NOT push.
- FR-07: `cargo xtask upstream-sync <tag>` must move the upstream pin and every copy of the upstream and CLI versions, and print whether the upstream change is CLI-only.
- FR-08: The release workflow must start only on a pushed version tag, and must NOT run on a fork.
- FR-09: The release workflow must publish only after the whole CI passed on the tagged commit and the tag, the workspace version, and the changelog section agree.
- FR-10: The release workflow must publish with a short-lived token obtained through trusted publishing, and must NOT read a stored registry token.
- FR-11: The release workflow must skip a crate whose name and version already exist on the registry, so a rerun publishes only what is missing.
- FR-12: The release workflow must attest the archives served by the registry and attach them to a GitHub Release whose notes are the changelog section.
- FR-13: No workflow and no tool of this PRD may commit, tag, or push to the repository under an identity other than the owner's.
- FR-14: The differential harness must NOT depend on a literal crate or upstream version stored in a ledger.
- FR-15: The upstream watch must run daily and open at most one issue per upstream tag.
- FR-16: A pull request that breaks the public API without a minor version bump must fail CI once a version is published.

## Non-Functional Requirements

- **Performance:** the release workflow completes in 30 minutes or less from the tag push at Month-6 (45 at Month-1). `cargo xtask release prepare` and `cargo xtask upstream-sync` each complete in 60 seconds or less, excluding the submodule fetch. `cargo xtask package-check` completes in 5 minutes or less on the CI Linux runner.
- **Security:** 0 long-lived registry tokens are stored in GitHub. `id-token: write` is granted to 2 jobs (publish, attest) and `contents: write` to 1 job (GitHub Release). 100% of actions are locked to a commit. The release workflow has 1 trigger. 100% of published archives carry an attestation from `0.1.0-rc.2` on.
- **Reliability:** rerunning the release workflow publishes 0 duplicate versions and changes 0 existing GitHub Releases. A failed lookup of the registry or of upstream produces a failed run in 100% of cases, never a clean result. A refused command (`release prepare`, `release tag`, `upstream-sync`) leaves 0 files changed.
- **Scalability:** the procedure holds 25 releases a month (the registry allows a burst of 30 new versions, then 1 a minute). Adding a copy of a pinned version is 1 entry in the list the commands rewrite and 1 assertion in the policy tests.
- **Maintainability:** line coverage stays at or above 91.5%, with `xtask` still left out of the measure. Each release decision of the workflow is in 1 script with an offline test. The compressed library archive stays at or below 1 MiB (354 KiB on 2026-11-17; the registry limit is 10 MB).
- **Accessibility:** `CHANGELOG.md`, `RELEASING.md`, and `SECURITY.md` are plain Markdown with 0 images and 0 HTML, and 100% of README links resolve from crates.io without the repository as a base.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Tag and version disagree | The owner tags `v0.1.3` on a tree at `0.1.2` | `verify` fails; nothing is published | "tag v0.1.3 does not match the workspace version 0.1.2" |
| 2 | Tag off `main` | A tag is pushed on a commit that is not an ancestor of `main` | `verify` fails; nothing is published | "v0.1.3 is not on main" |
| 3 | Missing changelog section | The tag is pushed without a section for the version | `verify` fails; `release tag` would have refused first | "CHANGELOG.md has no section for 0.1.3" |
| 4 | Partial publication | The macros crate is published, the library fails | The run fails; a rerun publishes only the library | The failing cargo output |
| 5 | Version already on the registry | The workflow is rerun after success | Both crates are skipped; the run is green; the GitHub Release is unchanged | "claude-agent-rust-sdk 0.1.3 is already published" |
| 6 | Registry unreachable | The lookup before publishing fails | The job fails closed; nothing is published | "cannot read the registry index: ..." |
| 7 | Token exchange refused | The trusted publisher fields do not match the job | The publish job fails; no stored token is tried | The action's error, recorded in `RELEASING.md` terms |
| 8 | CI fails on the tagged commit | A gate fails in the reusable CI run | Nothing is published; before any crate is published the tag may be deleted and recreated | The failing job |
| 9 | Dirty tree at prepare | Uncommitted files when `release prepare` or `upstream-sync` runs | The command refuses and writes nothing | The list of changed files |
| 10 | Patch asked for a breaking change | `release prepare patch` after a commit marked `!`, or after the compatibility tool reports a break | The command refuses | "breaking change since v0.1.2: use minor" |
| 11 | Unknown or older upstream tag | `upstream-sync v0.2.100` when the pin is `v0.2.182` | The command refuses and changes nothing | "v0.2.100 is not newer than the pin v0.2.182" |
| 12 | Upstream release with code changes | `upstream-sync` on a tag that changes more than version files | The pin and the copies move; verdict `needs-port`; the ledger check stays red until the port is done | "verdict: needs-port" and the upstream diff |
| 13 | Several upstream releases at once | The pin is three releases behind | One sync to the newest tag; the changelog line gives the range | "Upstream: Python SDK 0.2.183 to 0.2.185, CLI ..." |
| 14 | Name taken before the first publication | Another crate takes the name between 2026-11-18 and US-071 | Nothing is published; the story is blocked for the owner | "claude-agent-rust-sdk is no longer available" |
| 15 | Stray file in the package | A new top-level file enters a crate directory | `package-check` fails on the pull request | The unexpected path |
| 16 | docs.rs build breaks | A nightly change or a new item breaks the docs.rs-style build | The `package` job fails on the pull request | The rustdoc error |
| 17 | Defect found after publication | A released version is broken | The version is yanked and a new version ships; the version is never reused | None; procedure in `RELEASING.md` |
| 18 | Workflow started on a fork | A fork pushes a version tag | No job runs | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | A wrong version is published and cannot be removed | Med | High | Three agreement checks (tag, manifest, changelog) locally and in `verify`; the whole CI reruns on the tag; two release candidates before `0.1.0`; yank procedure written |
| 2 | The release cadence (13 to 23 a month) wears the procedure down and steps get skipped | Med | High | Three commands, no hand edit; `upstream-sync` handles the 56% of CLI-only releases; refusals are enforced by the tools, not by a checklist |
| 3 | The crate name is taken before the first publication | Low | High | EP-014 comes right after EP-013; US-071 rechecks the name first; a taken name blocks for an owner decision |
| 4 | Trusted publishing does not accept the workflow as designed | Low | Med | The publish job is in the top-level workflow, as the registry source requires; US-072 proves it on a release candidate; no token fallback |
| 5 | The rename misses an occurrence and breaks the ledgers or a test | Med | Med | A policy test forbids the old name outside dated records; the ledger check resolves every Rust path |
| 6 | The README states "tested against" a CLI version no end-to-end run covered | High | Med | Open question for the owner on running the paid suite per release; `RELEASING.md` says when to run it; the differential harness still proves what is sent |
| 7 | A nightly or tool change breaks the docs.rs-style build or the compatibility check without a change in the repository | Med | Low | The compatibility tool and its toolchain are pinned together; a failure names the pin to update |
| 8 | A workflow change widens what the release job can do | Low | High | Policy tests pin triggers, permissions, the environment, and the fork guard; actions locked by commit; no secret stored |
| 9 | Trademark objection to a name that contains "claude" | Low | Med | The README keeps its non-affiliation statement word for word; the name describes compatibility, as the seven competing crates do |
| 10 | Line references into upstream go stale as the pin moves every few days | High | Low | References in this PRD are tied to commit `4720d36`; `upstream-sync` regenerates the pages that state the pinned version |
| 11 | The reusable call of the whole CI makes a release slow | Med | Low | Target of 45 then 30 minutes measured in US-072; a slimmer job list is an engineering option if the target is missed |

## Non-Goals

What this version explicitly does NOT include:

- An installer for the Claude CLI with checksum and signature verification. It needs its own research on what Anthropic's install script verifies; revisited after EP-014.
- Bundling the CLI in the crate, per-platform artifacts, recompression, and registry quota checks. Ruled out by the 10 MB limit and the redistribution terms (`tasks/prd-claude-agent-rust-sdk.md:1150`).
- Issue triage, pull request review, or changelog writing by Claude in CI, and the egress firewall that protects such jobs upstream. Each run would spend API credits.
- Slack notifications. GitHub's own failure notification reaches the single maintainer.
- A bot that commits version bumps or opens release pull requests. The owner is the only visible contributor.
- Running the paid end-to-end suite automatically at each release. Left to the owner (see Open Questions).
- Mirroring the upstream version number. The crate version is independent.
- Pre-release channels after `0.1.0`, nightly builds, and maintenance branches. Upstream has none either.
- Renaming the feature `testing`.
- Asking the owner of the abandoned `claude-agent-sdk` crate for a transfer.
- Renaming the local checkout directory, which is the owner's choice on each machine.

## Files NOT to Modify

- `upstream/claude-agent-sdk-python/**`: read-only reference; only the submodule pin moves, through `cargo xtask upstream-sync`.
- `/home/arthur/dev/claude-agent-sdk-python/**` (Fedora) and `C:/dev/claude-agent-sdk-python/**` (Windows): the reference checkout stays untouched.
- `.github/workflows/actions.lock`: changed only through `gh actions-lock`.
- `Cargo.lock`: changed only by cargo.
- The generated regions of `docs/migration.md` and `docs/parity.md`: changed only by their xtask commands.
- `docs/research/*.md` and `docs/validation/*.md` that already exist: dated records keep the old crate name and the old license; new findings go into new files.
- `tasks/prd-claude-agent-rust-sdk.md` and its status file: certified and closed.
- `tasks/prd-parity-hardening.md` and its status file: owned by that PRD; its open epic EP-011 is not renumbered.
- The `[workspace.lints.clippy]` block of the root `Cargo.toml`, `clippy.toml`, and the 91.5% coverage floor in `.github/workflows/ci.yml`.
- The exit codes (64, 65, 66, 67) and the init-frame keys of `crates/fake-claude`: existing tests rely on them.
- The entrypoint label `sdk-rs` sent to the CLI.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** should the release decisions of the workflow live in a shell script with fake commands, as `upstream-watch.sh` does, or in `xtask`? Recommended: the script for what runs only in CI (registry lookup, GitHub Release), `xtask` for what the owner also runs locally (version, changelog, package check). Engineering to confirm the split keeps each rule in one place.
- **Changelog draft:** native parsing of `git log` in `xtask`, or `git-cliff` 2.14 as a pinned binary? Recommended: native, because the 66 commits already follow `type(scope): subject`, the mapping is five rules, and one fewer tool has to be pinned and installed. Trade-off: `git-cliff` handles footers and odd commit shapes that a small parser may miss.
- **Version in the differences ledger:** tokens resolved at comparison time (recommended, FR-14), or literals rewritten by `release prepare` and `upstream-sync`? Trade-off: tokens add one substitution step to the comparison; literals add two files to every release commit.
- **README documentation test:** `src/lib.rs` includes `../../../README.md` under `cfg(doctest)`, a path that leaves the package. Recommended: accept that the README documentation test runs in the workspace only, since `cargo publish` and docs.rs build without running tests. Alternative: a canonical README inside the crate directory, which GitHub would not show at the repository root.
- **Reusing CI in the release:** calling `ci.yml` in full reruns coverage and the differential job on the tag. Recommended: the full call, for one definition of "green". Is a job filter needed to meet the 30-minute target?
- **Exact pin on the macros crate:** `=x.y.z` forces both crates to move together (the practice of `thiserror`); a caret requirement lets a macros fix ship alone (the practice of `serde_derive`). Recommended: the exact pin, since both crates always release at the shared workspace version.
- **docs.rs-style build in CI:** an unpinned nightly can fail without a change in the repository, as the `minimal-versions` job already can. Recommended: unpinned, in the `package` job, so a docs.rs breakage is seen before a release. Alternative: a pinned nightly bumped by hand.
- **Dependencies:** no new Rust dependency is planned for the library. `xtask` may need a TOML editor that keeps formatting (`toml_edit`) for manifest rewrites; alternative: line-anchored text replacement with a policy test on the result. `cargo-semver-checks` and `cargo-deny` are CI tools, pinned by version.
- **Migration:** no release exists, so the rename, the new environment variable prefix, and the new managed directory name need no compatibility layer. Rollback of a publication is a yank plus a new version; rollback of the rename is not planned.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Versions installable from crates.io | 0 | `0.1.0` published | Month-1 | crates.io page; `docs/validation/first-publication.md` |
| Median delay from an upstream release to the Rust release that ports it | Unbounded (no release) | 5 days, then 2 days | Month-1, Month-6 | Upstream release date against the Rust tag date, per changelog `Upstream:` line |
| Owner commands per release after the port is merged | No procedure | 3 | Month-1 | `RELEASING.md` routine release |
| Hand-edited files in a CLI-only release | About 12 | 0 | Month-1 | `git show --stat` of the release commits |
| Delay from tag push to installable version | N/A (new) | 45 minutes, then 30 minutes | Month-1, Month-6 | Workflow run duration |
| Releases published with a stored token | N/A (new) | 1 (`0.1.0-rc.1`), then 0 | Month-1 | crates.io "Trusted Publishing Only" setting; workflow logs |
| Published archives with a verifiable attestation | 0 | 100% from `0.1.0-rc.2` | Month-1 | `gh attestation verify` on each GitHub Release asset |
| Packaging defects found after a release (missing license, stray file) | 4 found before any release | 0 | Month-6 | `package-check` history and issues |
| Patch releases that break a dependent build | N/A (new) | 0 | Month-6 | The `semver` job; issues |

## Open Questions

- Should the paid end-to-end suite run against the real CLI for each release (about 0.80 USD a run, 10 to 18 USD a month at upstream's cadence), only for releases that port code changes, or on a schedule? Owner to decide before US-070 writes the rule; until then the suite stays manual and no release depends on it. Risk 6 depends on the answer.
- Should the `release` environment require the owner's approval on each run? It adds one click to 13 to 23 releases a month, on top of a tag only he can push. Recommended: no required reviewer, with the environment limited to `v*` tags. Owner to decide before US-072.
- When does the owner rename the GitHub repository to `claude-agent-rust-sdk`? It must be done before US-071, because the trusted publisher and the `repository` link name it. GitHub redirects the old URL.
- Should release tags be signed? `release tag` follows the git configuration of the machine; owner to decide whether a signature is required and checked by `verify`.
- Does `0.1.0` wait for EP-011 (Windows parity proof) of `tasks/prd-parity-hardening.md`? Nothing in this PRD depends on it; owner to decide the order.
- Is a tag ruleset available for a public repository on a personal account? Answered by US-072; the fallback is release immutability plus the environment restriction.
- What does the owner want as the copyright line of `LICENSE-MIT`: `Arthur Jean` or another holder name? Assumed `Copyright (c) 2026 Arthur Jean`; owner to confirm before US-059.
[/PRD]
