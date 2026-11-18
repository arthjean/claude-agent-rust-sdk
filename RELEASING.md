# Releasing

How a version of `claude-agent-rust-sdk` and `claude-agent-rust-sdk-macros`
goes from a merged port to crates.io, and what to do when a step fails.

A publication is permanent: crates.io never overwrites or deletes a version,
so a version number is used once. Every step that leaves the machine (the
push of a `v*` tag, a yank, a change of a setting) is the owner's.

## Versioning

- Both crates share the workspace version and release together; the library
  requires the macros crate at exactly that version.
- In `0.y.z`, a change of `y` may break the public API; a change of `z` never
  does. Raising the minimum supported Rust version is a minor change. The
  `testing` feature is outside the compatibility promise.
- The `semver` job of CI holds the promise: on every pull request and push,
  `cargo xtask semver-check` runs `cargo-semver-checks` on the library with
  every feature but `testing`, against its newest non-yanked release on
  crates.io, and fails on a break the workspace version does not allow.
  Since the version moves only at `release prepare`, a break keeps the job red
  until the next release is prepared with `minor`, and `release prepare patch`
  refuses it.
- The crate version is independent of upstream. Each changelog section opens
  with the upstream versions it covers, for example
  `Upstream: Python SDK 0.2.183 to 0.2.185, CLI 2.1.330`.
- The cadence is one release per ported upstream release, published as soon
  as the port is merged.

## The commands

| Command | What it does | What it never does |
|---|---|---|
| `cargo xtask upstream-sync <tag>` | Moves the submodule pin to `<tag>` (staged), rewrites every copy of the upstream SDK version, the CLI version, and the upstream commit, regenerates `docs/migration.md` and `docs/parity.md`, adds a `### Changed` entry to `## Unreleased`, and prints `verdict: cli-only` or `verdict: needs-port` | Commit, or touch anything under `upstream/` but the pin |
| `cargo xtask release prepare <patch\|minor\|X.Y.Z[-pre]>` | Moves the workspace version, the exact requirement on the macros crate, the README snippets when the minor changes, and `Cargo.lock`; regenerates both pages; turns `## Unreleased` into `## <version> - <today>` with its `Upstream:` line, drafting entries from the conventional commits since the previous tag when the section has no hand-written entry. The section of a release that follows release candidates opens with their entries and their upstream range, and leaves their sections as published. `--dry-run` prints the plan | Commit, tag, or push |
| `cargo xtask release tag` | Runs the gates and the package check, then creates the commit `chore: release v<version>` and the annotated tag `v<version>` with your git identity and signing settings. `--no-gates` skips the gates, not the package check | Push |
| `cargo xtask changelog-section <version>` | Prints the changelog section of a version | |
| `cargo xtask semver-check` | Compares the public API of the library with its newest release on crates.io through `cargo-semver-checks`, at the version and on the toolchain pinned in `xtask/src/semver.rs`, and lists each broken item. `release prepare patch` runs the same comparison. Install the pins with `cargo install cargo-semver-checks@<version> --locked` and `rustup toolchain install <toolchain>` | Pass without having compared |

Each command refuses, changes nothing, and says why when the tree is dirty,
the branch is not `main`, the version or tag is wrong, or a check fails.

## The routine release

1. The `Upstream watch` workflow opens an issue `Port upstream release <tag>`
   with the verdict and the command to run.
2. On an up-to-date `main`: `cargo xtask upstream-sync <tag>`.
3. If the verdict is `needs-port`, port the changes the output lists and
   complete the ledgers until `cargo xtask parity-check --deny-pending`
   passes. Add changelog entries under `## Unreleased` as you go.
4. Commit the port and merge it to `main`.
5. `cargo xtask release prepare patch` (or `minor` when the port breaks the
   API; `patch` is refused after a commit marked breaking). Review the
   changelog section it wrote.
6. `cargo xtask release tag`.
7. `git push --atomic origin main v<version>`, the command `release tag`
   prints. The push starts `release.yml`.

## A CLI-only release

When the verdict is `cli-only`, upstream moved only its versions: step 3 is
empty. `upstream-sync` already wrote the changelog entry, so the release is:

```sh
cargo xtask upstream-sync <tag>
git commit --all --message "chore: port upstream <tag>"
cargo xtask release prepare patch
cargo xtask release tag
git push --atomic origin main v<version>
```

## What `release.yml` does

The workflow starts only on a pushed tag `v<major>.<minor>.<patch>` with an
optional pre-release part, never on a fork, one release at a time.

| Job | What it does | Permissions |
|---|---|---|
| `verify` | Fails unless the tag is `v` plus the workspace version, the tagged commit is on `main`, and `CHANGELOG.md` has a non-empty section for the version; writes the release notes | `contents: read` |
| `ci` | Runs the whole of `ci.yml` on the tagged commit | `contents: read` |
| `publish` | In the `release` environment, asks the registry index which of the two crates lack the version; only when one does, exchanges the job's identity for a short-lived crates.io token (`rust-lang/crates-io-auth-action`) and publishes the macros crate and the library with the lockfile, skipping a crate whose version the index already lists | `contents: read`, `id-token: write` |
| `attest` | Downloads both `.crate` archives from crates.io, checks them against the SHA-256 of the index, and attests them with `actions/attest` | `contents: read`, `id-token: write`, `attestations: write`, `artifact-metadata: write` |
| `github-release` | Creates the GitHub Release `v<version>` with the changelog section, the crates.io and docs.rs links, and the two attested archives, marked as a pre-release for a pre-release version; does nothing when the release exists | `contents: write` |

The decisions live in `.github/scripts/release.sh`, tested offline by
`.github/scripts/release.test.sh` in the `scripts` job of CI.

## Verifying a downloaded archive

```sh
gh attestation verify claude-agent-rust-sdk-<version>.crate \
  --repo arthjean/claude-agent-rust-sdk \
  --signer-workflow arthjean/claude-agent-rust-sdk/.github/workflows/release.yml
```

The archive is the one attached to the GitHub Release, or the one crates.io
serves at `https://static.crates.io/crates/claude-agent-rust-sdk/claude-agent-rust-sdk-<version>.crate`:
both are the same file.

## The first publication

crates.io accepts trusted publishing only for a crate that already exists,
so the first version is published by hand:

1. `0.1.0-rc.1`: after checking that both names are still free, `release
   prepare 0.1.0-rc.1`, `release tag`, and a push of the branch only; then
   `cargo publish --locked -p claude-agent-rust-sdk-macros` and
   `cargo publish --locked -p claude-agent-rust-sdk` from your machine with a
   token created for this step. Push the tag only after both are published:
   the workflow then skips both crates.
2. Configure the settings below, then `0.1.0-rc.2` through the workflow,
   which proves trusted publishing. Set both crates to "Trusted Publishing
   Only" and revoke the token of step 1.
3. `0.1.0` through the workflow.

Each step is recorded in `docs/validation/first-publication.md`.

## Settings outside the repository

| Where | Setting | Expected value |
|---|---|---|
| GitHub, Environments | `release` | Exists; deployments limited to tags matching `v*`; no required reviewer unless the owner decides otherwise |
| GitHub, Rules | Tag ruleset on `v*` | Only the owner may create, update, or delete a matching tag |
| GitHub, General | Release immutability | On: a published release's tag and assets cannot change |
| GitHub, Security | Private vulnerability reporting | On |
| crates.io, each crate, Trusted Publishing | Trusted publisher | Owner `arthjean`, repository `claude-agent-rust-sdk`, workflow `release.yml`, environment `release` |
| crates.io, each crate, Trusted Publishing | "Trusted Publishing Only" | On, from `0.1.0-rc.2` |

A setting GitHub does not offer for this repository is recorded in
`docs/validation/first-publication.md` with the fallback used.

## The end-to-end suite

The suite against the real CLI spends API credits (about 0.80 USD a run)
and stays manual: no release waits for it. Run the `End-to-end` workflow by
hand when a release changes what reaches the CLI or moves the CLI version
past behavior the fake CLI cannot reproduce. Whether it runs for every
release is the owner's decision, not yet taken.

## When something fails

| Failure | What happened | Recovery |
|---|---|---|
| `verify`: "tag v0.1.3 does not match the workspace version 0.1.2" | The tag was made by hand or on the wrong commit | Nothing is published. Delete the tag (`git push origin :refs/tags/v0.1.3`, `git tag -d v0.1.3`) and release with `release prepare` and `release tag` |
| `verify`: "v0.1.3 is not on main" | The tagged commit is not an ancestor of `main` | Nothing is published. Delete the tag, merge to `main`, tag again |
| `verify`: "CHANGELOG.md has no section for 0.1.3" | The tag was pushed without `release prepare` | Nothing is published. Delete the tag, run `release prepare` and `release tag` |
| `ci` fails on the tagged commit | A gate fails on the tag | Nothing is published. Before any crate is published, the tag may be deleted, the fix merged, and the same version tagged again |
| `publish` fails after the macros crate is published | The library publish failed | Rerun the workflow: the macros crate is skipped and only the library is published. If the library cannot be published at this version, release a new version; never reuse one |
| `publish`: "cannot read the registry index" | The registry lookup failed | Nothing more is published. Rerun once crates.io answers |
| `publish`: the token exchange is refused | The trusted publisher fields do not match the job | Compare the claims in the job log with the table above and correct the crates.io configuration, then rerun. Never add a stored token |
| A rerun after success | Both versions exist | Both crates are skipped and the GitHub Release is left as it is; the run is green |
| `attest` or `github-release` fails | The crates are published | Rerun the failed jobs; the release job does nothing when the release exists |
| `release prepare` or `upstream-sync` refuses a dirty tree | Uncommitted files | Commit or stash the files it lists |
| "breaking change since v0.1.2: use minor" | `patch` after a commit marked `!` or `BREAKING CHANGE:`, or after `cargo-semver-checks` reported a break, listed after the message | Use `release prepare minor` |
| `semver`: "breaks the API of its newest release" | The public API changed in a way the workspace version does not allow; each broken item is listed | If the break is wanted, it ships in the next minor release: the job stays red until `release prepare minor`. Otherwise restore the item |
| `semver` or `release prepare patch`: "cannot read the rustdoc JSON of Rust" | A new Rust or tool release changed the rustdoc JSON format | Move `TOOL_VERSION` and `TOOLCHAIN` of `xtask/src/semver.rs` together to a tool release that reads the format, and the same two values in the `semver` job of `ci.yml` |
| `semver`: "crates.io could not be read" | The registry lookup failed; nothing was compared | Rerun once crates.io answers; no pin is at fault |
| "cargo-semver-checks <version> is pinned" on your machine | Another version, or none, is installed | Run the `cargo install` command the message gives |
| "v0.2.100 is not newer than the pin v0.2.182" | The tag is older than the pin, or unknown | Check the tag name; nothing was changed |
| `verdict: needs-port` | Upstream changed code | Port the changes and complete the ledgers before `release prepare` |
| Several upstream releases at once | The pin is several releases behind | Sync once to the newest tag; the changelog line gives the range |
| A crate name is taken before the first publication | Someone published it first | Nothing is published; the owner decides the new name |
| `package-check` fails on a pull request | A stray file or a broken docs.rs build | Remove the file, or fix the documentation; the check names the cause |
| A workflow starts on a fork | A fork pushed a version tag | No job runs |
| A defect is found after publication | A released version is broken | Yank it (below) and ship the fix as a new version |

## Yanking a version

```sh
cargo yank --version 0.1.3 claude-agent-rust-sdk
cargo yank --version 0.1.3 claude-agent-rust-sdk-macros
```

A yank deletes nothing: the version stays downloadable for every
`Cargo.lock` that already names it, and cargo stops choosing it for new
resolutions. `cargo yank --undo` reverses it. Ship the fix as the next
version, add a `### Fixed` entry that names the yanked version, and never
publish the yanked number again.
