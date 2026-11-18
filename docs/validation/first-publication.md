# First publication

The record of the first publication of `claude-agent-rust-sdk` and
`claude-agent-rust-sdk-macros` (EP-014 of
`tasks/prd-distribution-release.md`), one section per step, with every
deviation from `RELEASING.md`.

## 0.1.0-rc.1: by hand (US-071)

### Before publication (2026-11-18)

- Owner's go: given on 2026-11-18 for US-071, with the instruction to
  prepare the release commit and to review the changelog section with the
  release-notes guidance before tagging.
- GitHub repository: renamed on 2026-11-18 from
  `arthjean/claude-agent-sdk-rust` to `arthjean/claude-agent-rust-sdk` with
  `gh repo rename`, on the owner's go. The old URL answers `301` to
  `https://github.com/arthjean/claude-agent-rust-sdk`, and `origin` of the
  local checkout was moved to the new URL.
- Crate names: `https://crates.io/api/v1/crates/claude-agent-rust-sdk` and
  `https://crates.io/api/v1/crates/claude-agent-rust-sdk-macros` both
  answered `404`, and so did the sparse index entries
  `https://index.crates.io/cl/ai/claude-agent-rust-sdk` and
  `https://index.crates.io/cl/ai/claude-agent-rust-sdk-macros`, on
  2026-11-18 before the release commit.
- Preflight on `a63fdcf`: `cargo xtask gates`, `cargo xtask package-check`,
  and the docs.rs-style build
  (`RUSTDOCFLAGS="--cfg docsrs -D warnings" cargo +nightly doc --package claude-agent-rust-sdk --all-features --no-deps`)
  passed.
- `cargo xtask release prepare 0.1.0-rc.1` moved `0.1.0 -> 0.1.0-rc.1` in
  `Cargo.toml`, `crates/claude-agent-rust-sdk/Cargo.toml`, `Cargo.lock`,
  `docs/parity.md`, and `CHANGELOG.md`. The section kept its hand-written
  entries and gained three by hand before the tag: the requirements, the
  license and the macros crate, and the explicit-version install of a
  release candidate.
- `cargo xtask release tag` created the commit `9474a14`
  (`chore: release v0.1.0-rc.1`) and the annotated tag `v0.1.0-rc.1`,
  author and tagger `arthjean`, after the gates and the package check
  passed. Its first attempt failed: see Deviations.

### Publication (2026-11-18, UTC)

On the owner's go, given after he ran `cargo login` himself in his own
terminal with a token he created for this step: name
`claude-agent-rust-sdk-bootstrap-rc1`, scope `publish-new` only, crate
pattern `claude-agent-rust-sdk*`, expiry 7 days (2026-11-25). The token
never entered the session.

| Time | Step |
|---|---|
| 11:21:38 | Both names rechecked: `404` on the API and the sparse index |
| 11:21:38 to 11:21:47 | `git push origin main` (`a63fdcf..9474a14`), without the tag |
| 11:21:48 | `cargo publish --locked -p claude-agent-rust-sdk-macros`: 8 files, 8.4 KiB compressed |
| 11:21:56 | `cargo publish --locked -p claude-agent-rust-sdk`: 70 files, 207.7 KiB compressed |
| 11:22:00 | `git push origin v0.1.0-rc.1`, which started `release.yml` run `44294042369` |

Registry checksums (SHA-256 of the `.crate` archives):

- `claude-agent-rust-sdk` 0.1.0-rc.1:
  `b1be69246430662754da97fb53bb12a9c87dc63db62e23059e9e247dd3e38b13`
- `claude-agent-rust-sdk-macros` 0.1.0-rc.1:
  `7c46bac94e33d55e5036b5628e6dae999974a32b3f26a26734acfda636668869`

### After publication

- URLs: `https://crates.io/crates/claude-agent-rust-sdk`,
  `https://crates.io/crates/claude-agent-rust-sdk-macros`,
  `https://docs.rs/claude-agent-rust-sdk`,
  `https://docs.rs/claude-agent-rust-sdk-macros`.
- crates.io API for both crates at `0.1.0-rc.1`: license
  `MIT OR Apache-2.0`, repository and homepage
  `https://github.com/arthjean/claude-agent-rust-sdk`, documentation on
  docs.rs, not yanked, no stable version yet. The README endpoint serves
  the rendered README.
- README links: all 15 distinct `https://` URLs of `README.md` were
  fetched (more than the five sampled the story asks for). The GitHub
  links (`CHANGELOG.md`, `RELEASING.md`, `SECURITY.md`, both licenses,
  `THIRD_PARTY_NOTICES`, `docs/migration.md`, `docs/parity.md`,
  `parity/`, `parity/README.md`, the examples directory, the CI workflow
  and its badge) and the badges answered `200`. The crates.io page
  answers `200` to a browser and `404` to a bare client, which is how
  the crates.io front end treats non-browser requests.
- Quick start at an explicit version: in an empty project,
  `cargo add claude-agent-rust-sdk@0.1.0-rc.1 futures-util@0.3` and
  `cargo add tokio@1 --features macros,rt-multi-thread`, with the README's
  quick start as `src/main.rs`, resolved `claude-agent-rust-sdk 0.1.0-rc.1`
  and built.
- `cargo add claude-agent-rust-sdk` without a version, in an empty
  project: exit `0`, and it wrote `claude-agent-rust-sdk = "0.1.0-rc.1"`.
  While no stable version exists, cargo selects the newest pre-release.
  The changelog's advice to name the version explicitly stays correct,
  and once `0.1.0` exists the same command is expected to select it
  (checked in US-073).
- docs.rs: both builds succeeded (`status.json` gives `doc_status: true`;
  the macros crate within minutes, the library by 11:31 after a wait in
  the queue). The `#[tool]` attribute page shows "Available on crate
  feature `macros` only".
- `release.yml` run `44294042369`
  (`https://github.com/arthjean/claude-agent-rust-sdk/actions/runs/44294042369`):
  green from the tag push at 11:22:00 to 11:31:48, 9 min 48 s. `verify`
  passed; every job of the reusable CI passed on Linux, macOS, and
  Windows; `publish` logged "claude-agent-rust-sdk-macros 0.1.0-rc.1 is
  already published" and the same for the library, and published nothing;
  `attest` and `github-release` passed.
- GitHub Release
  `https://github.com/arthjean/claude-agent-rust-sdk/releases/tag/v0.1.0-rc.1`:
  marked as a pre-release, with the two `.crate` archives. Their SHA-256
  (downloaded with `gh release download`) equal the registry checksums
  above.
- Attestations: `gh attestation verify` with the command of `RELEASING.md`
  exits `0` for both archives, signer
  `.github/workflows/release.yml@refs/tags/v0.1.0-rc.1`. Attestation was
  planned from `0.1.0-rc.2` on; `0.1.0-rc.1` already has it, because the
  tag was pushed after the hand publication.

## 0.1.0-rc.2: through the workflow (US-072)

### Settings (2026-11-18)

- crates.io: the owner added to both crates a GitHub trusted publisher
  with owner `arthjean`, repository `claude-agent-rust-sdk`, workflow
  `release.yml`, and environment `release`. crates.io found the workflow
  file on the default branch.
- GitHub, on the owner's go, through the REST API:
  - Environment `release`: it already existed, created by the
    `0.1.0-rc.1` run. Its deployments are now limited to custom policies,
    with one policy: tags matching `v*`. No required reviewer (owner
    decision of 2026-11-18).
  - Tag ruleset `Release tags` (id `27544168`): active on `refs/tags/v*`,
    rules `creation`, `update`, and `deletion`, bypassed only by the
    repository admin role. On this personal-account repository the
    owner is the only admin. GitHub offers tag rulesets here, so no
    fallback was needed.
  - Release immutability: enabled.
  - Private vulnerability reporting: enabled.

### Publication (2026-11-18, UTC)

- `cargo xtask release prepare 0.1.0-rc.2` and `cargo xtask release tag`
  made the commit `9c19e90` and the tag `v0.1.0-rc.2` (gates and package
  check green). The section holds one hand-written `### Changed` entry on
  the trusted publication.
- 11:41:17: `git push --atomic origin main v0.1.0-rc.2`, accepted under
  the tag ruleset through the owner's admin bypass. It started
  `release.yml` run `44296951626`
  (`https://github.com/arthjean/claude-agent-rust-sdk/actions/runs/44296951626`).
- `publish`: the index lookup reported both crates missing at
  `0.1.0-rc.2`. `rust-lang/crates-io-auth-action` exchanged the job's
  identity token for a crates.io token on the first try ("Retrieved token
  successfully"), and both crates were published. No stored token
  exists in the repository or its environment.
- The crates.io API records, for both versions, `published_by: null` and
  `trustpub_data` with provider `github`, repository
  `arthjean/claude-agent-rust-sdk`, run `44296951626`, and commit
  `9c19e9059cd34e5566f86a53218f2ed61c74f928`.
- Times: macros crate on the registry at 11:50:30, library at 11:50:59,
  so 9 min 42 s from the tag push to an installable version. The whole
  run ended green at 11:52:06 (10 min 49 s).

Registry checksums (SHA-256):

- `claude-agent-rust-sdk` 0.1.0-rc.2:
  `6e311bb8ab11798b0be589ad987a65ae6b9ff55dd2fbe6cde4e9188b9557c0ed`
- `claude-agent-rust-sdk-macros` 0.1.0-rc.2:
  `2883ef678e9b76d0974817aa0b055d6ccafde570cdfa4365f4bfee094d95a8f9`

### After publication

- GitHub Release
  `https://github.com/arthjean/claude-agent-rust-sdk/releases/tag/v0.1.0-rc.2`:
  a pre-release and immutable, with the two archives. Their SHA-256, both
  as GitHub reports them and after download, equal the registry
  checksums. With immutability on, `gh release create` attached the
  assets and published the release without error.
- Attestations: the `RELEASING.md` command exits `0` for both archives.
- Rerun: at 11:52:49, `gh run rerun 44296951626` started attempt 2, which
  ended green at 12:11:49. `publish` logged "claude-agent-rust-sdk-macros
  0.1.0-rc.2 is already published" and the same for the library, and
  requested no token. `github-release` found the release and left it
  as it was: same `publishedAt` (11:52:03), same two assets. `attest`
  attested the same archives again, which adds attestations and changes
  nothing published.
- "Trusted Publishing Only": the owner checked "Require trusted
  publishing for all new versions" on both crates. The crates.io API
  then gave `trustpub_only: true` for both.
- Bootstrap token: the owner reported on 2026-11-18 that he revoked
  `claude-agent-rust-sdk-bootstrap-rc1`. The session cannot read the
  owner's token list, so this rests on his report. Both crates refuse a
  token publication either way.

## 0.1.0 (US-073)

- Changelog: the owner chose on 2026-11-18 that the section of a release
  gathers the entries of its release candidates (`0708600`). `release
  prepare 0.1.0` wrote the section from those of `0.1.0-rc.1` and
  `0.1.0-rc.2`. On review, before the tag, the candidate-only entry
  ("This is a release candidate...") was removed and the trusted
  publication entry moved from `### Changed` to `### Added`, since the
  section is read against no earlier release.
- `release tag` made the commit `5087d4e` and the tag `v0.1.0` (gates and
  package check green). At 12:58:58 UTC, `git push --atomic origin main
  v0.1.0`, on the owner's go, started run `44309118563`
  (`https://github.com/arthjean/claude-agent-rust-sdk/actions/runs/44309118563`).
- The run ended green at 13:11:19, all 16 jobs successful. Macros crate
  on the registry at 13:10:15 and library at 13:10:39, so 11 min 41 s
  from the tag push to an installable version and 12 min 21 s for the
  whole run, within the 45-minute Month-1 target.
- crates.io: `trustpub_data` names run `44309118563` for both crates,
  `max_stable_version` is `0.1.0`, and `trustpub_only` is still `true`.
  URLs: `https://crates.io/crates/claude-agent-rust-sdk/0.1.0`,
  `https://crates.io/crates/claude-agent-rust-sdk-macros/0.1.0`,
  `https://docs.rs/claude-agent-rust-sdk/0.1.0`,
  `https://docs.rs/claude-agent-rust-sdk-macros/0.1.0` (both docs.rs
  builds succeeded).
- Registry checksums (SHA-256):
  - `claude-agent-rust-sdk` 0.1.0:
    `ea36769b8d6590de3ef63090bb83f1e010e1bec07f258348cebfe030262f1796`
  - `claude-agent-rust-sdk-macros` 0.1.0:
    `b1a8b3d8cda61e74f99420ded799d3a618075680714f97215193622542c35d56`
- GitHub Release
  `https://github.com/arthjean/claude-agent-rust-sdk/releases/tag/v0.1.0`:
  not a pre-release, immutable. Its notes are the `0.1.0` section of the
  changelog followed by the crates.io and docs.rs links. Its two archives
  match the registry checksums after download, and both attestations
  verify with the `RELEASING.md` command.
- In an empty project, `cargo add claude-agent-rust-sdk` wrote
  `claude-agent-rust-sdk = "0.1.0"`. A second empty project used the
  README snippet (`claude-agent-rust-sdk = "0.1"`, `futures-util`,
  `tokio`) with the README quick start: it resolved
  `claude-agent-rust-sdk 0.1.0` and built. Nothing was run.
- No defect was found after publication, so nothing was yanked. The
  procedure is the "Yanking a version" section of `RELEASING.md`.

## Deviations

- The first `cargo xtask release tag` failed in the `test` gate: two tests
  of `xtask/tests/release.rs` generated the fixture's parity page with the
  crate version of the checkout running the test instead of the fixture's.
  Once the checkout was at `0.1.0-rc.1`, prepare in the fixture no longer
  changed `docs/parity.md`. Nothing was committed or tagged. The parity
  commands gained `--root`, the fixture passes it, and the fix landed as
  `1e1272e` before the release commit.
- The story asks for the quick start to compile with no CLI call. After the
  build, the binary was also run, to see it fail without a CLI. It found
  the CLI installed on the machine and made one real query (one turn,
  "What is 2 + 2?"), which spent a few cents of API credits. The build
  itself made no call. Next time, only build.
- The first attempt to add the dependencies in the empty project failed
  in the check script (`--features tokio/macros` given together with
  several crates). It was rerun with tokio added separately. This was an
  error in the script, not in the crate.
