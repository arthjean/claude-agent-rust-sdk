# Parity audit 2: decisions left to the owner

Each section states what the audit asked, what was done on the branch, and the
choice that remains, with a recommendation. Nothing here is acted on until the
owner decides.

## M7: key order of `serde_json` maps

Done on the branch: `serde_json/preserve_order` is no longer forced on every
dependent. The library turns it on through its own `preserve-order` feature,
on by default, and builds and passes its tests without it (removal goes
through `json_map::RemoveKey`, which keeps the order in both
representations; the tests that check key order are gated on the feature).

Why on by default: the differential harness compares key order at every
depth, because it reaches the model (the properties of a tool schema) and the
on-disk transcripts the CLI reads. Values the program builds are
`serde_json::Value`s; without the feature their keys are sorted the moment
they are built, and no code in the library can restore the order the program
wrote. Off by default would make every program diverge from Python unless it
knew to turn the feature on.

Remaining choice:

- **Keep it (recommended for now).** Parity by default, an opt-out for the
  programs that need sorted maps, documented in the crate docs and README.
- **Own ordered JSON type in the public API.** Replace `serde_json::Value`
  and `Map` in public fields (`extra`, tool input, schemas) with a type that
  always keeps order, so the library never touches the program's
  `serde_json` features. Exact in every build, but a breaking change of most
  of the public surface: a major release, and an owner decision.

## M5: opting out of the process-group sweep

Done on the branch: what is left in the group of the CLI when it exits gets
`SIGTERM`, then `SIGKILL` 5 s later only if it is still running.

Remaining choice: an option to leave background processes alone (Python
signals only the direct child). The current policy was the owner's decision
A12 of the first audit; an opt-out would be a new public option
(`ClaudeAgentOptions::keep_background_processes` or similar), which is a
naming decision. Recommendation: no option until a user asks; `setsid`
already lets an agent detach a process on purpose.

## M6: installing a pinned CLI

Done on the branch: the warning about an untested CLI is logged once per
process for each path and version, and the install directory the crate
searches before `PATH` is documented (README, migration guide, `find_cli`) as
the place to pin a release.

Remaining choice: an installer that fills that directory, the Rust
counterpart of the CLI the Python wheel bundles. The download protocol is the
one `https://claude.ai/install.sh` follows: read
`https://downloads.claude.ai/claude-code-releases/<version>/manifest.json`,
take `platforms["<os>-<arch>[-musl]"].checksum` (SHA-256), download
`<version>/<platform>/claude`, check the digest, and move the file into place
atomically. A design that fits the crate:

- an opt-in feature `install-cli` with `cli::install_cli(version)`, pinned to
  `CLI_VERSION` by default, never called by the crate on its own;
- an HTTPS client (`ureq` with `rustls`) and `sha2`, which add `ring` and a
  certificate root set to the graph: `deny.toml` would have to allow the
  license of the roots (`webpki-roots` is CDLA-Permissive-2.0), or use the
  platform verifier;
- offline tests against a local server serving a manifest and a binary,
  wrong digest and missing platform included.

Why it waits for the owner: it downloads and installs Anthropic's binary on
the user's machine, which is the distribution question of prerequisite A1
(whether Anthropic would rather bundle or host it), and it adds a network
stack to the supply chain of every build that turns the feature on.
Recommendation: build it once A1 is settled, as a feature of this crate
rather than a separate binary.

## B1 and A1: the entrypoint label

Measured (`docs/validation/entrypoint-label.md`): CLI 2.1.329 knows `sdk-ts`,
`sdk-py`, and `sdk-cli` only. With `sdk-rs` the `init` message lists the
`claude-code-guide` agent a Python session does not get, API requests carry
the client platform of the interactive CLI, and telemetry counts another
client. Done on the branch: the ledger entry states these effects, and an
end-to-end test (`a_session_with_the_sdk_rs_entrypoint_lists_the_agent_a_python_session_does_not`)
pins the agent difference for the next real run. `sdk-py` is never sent.

Remaining choice, until the CLI knows `sdk-rs` (A1):

- **Keep `sdk-rs`.** Honest and distinct in telemetry, which is what A1 asks
  Anthropic to support, but every switch above differs from Python.
- **Send `sdk-cli` (recommended as the interim).** It is the label the CLI
  gives itself when an SDK starts it without one, so it claims nothing that
  is not true. Measured: the `init` agents match Python, and the code reading
  shows the SDK switches and the client platform match too. Two differences
  remain: one set of request attributes chosen for `sdk-ts` and `sdk-py`
  only, and telemetry that counts the session as `sdk-cli` instead of a Rust
  SDK. Changing it is one constant (`transport/environment.rs`), the policy
  test that pins it, the README paragraph on billing (A3), and the ledger
  entry; it should follow the conversation with Anthropic, since it changes
  how they would count Rust sessions.
