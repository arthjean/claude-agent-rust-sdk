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

**Decided by the owner (2026-11-21):** keep `sdk-rs`; the owner asks Anthropic to add it to the CLI as part of the proposal. `sdk-py` is never sent.

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

## A2: crate name, copyright, and license

**Decided by the owner (2026-11-21):** keep `claude-agent-rust-sdk` and `MIT OR Apache-2.0` for the next publication; the official name is a question for the proposal to Anthropic.

Today: crate `claude-agent-rust-sdk`, license `MIT OR Apache-2.0`, copyright
"2026 Arthur Jean" in both license files, run-time names
`CLAUDE_AGENT_RUST_SDK_*` and the `claude-agent-rust-sdk` install directory.
Upstream: package `claude-agent-sdk`, MIT only, copyright "2025 Anthropic,
PBC".

Checked on crates.io on 2026-11-21:

- `claude-agent-sdk` is taken: version 0.1.1 of 2025-09-30, owned by a single
  personal account, with a repository link to
  `github.com/anthropics/claude-agent-sdk-rust` that answers 404. The natural
  official name would need a transfer from its owner, which only Anthropic
  can credibly ask for.
- `claude-agent-sdk-rs` is taken by an unrelated project (0.6.4,
  2026-02-09).
- `anthropic-claude-agent-sdk` is free.

To settle before the next publication, since a rename after users depend on
the crate breaks their builds: the target name (a transfer of
`claude-agent-sdk`, or keeping `claude-agent-rust-sdk`), the license
(`MIT OR Apache-2.0` is the Rust convention and includes MIT; Anthropic may
want MIT alone to match upstream), and the assignment of rights or a
contributor agreement. A rename moves the crate name, the run-time names
(pinned by `workspace.rs::the_entrypoint_label_and_the_run_time_names_follow_the_crate_name`),
and the macros crate together.

## A3: the billing paragraph of the README

**Decided by the owner (2026-11-21):** removed from the README; the observation is kept in `docs/validation/entrypoint-billing.md` as evidence for A1.

`README.md` ("Plan API credits may not apply") reports how a third party
billed calls carrying `sdk-rs` on 2026-11-15. No policy test pins it
(`tests/workspace.rs` matches the authentication, non-affiliation, and
license sentences only), so removing it is a plain edit. Recommendation:
remove it from the published README and keep it in the proposal as evidence
for A1; `docs/validation/entrypoint-label.md` now explains the cause (the CLI
does not know the label). It stays until you decide, since the README
statement is yours.

## A4: who maintains the tooling, and the end-to-end cadence

**Decided by the owner (2026-11-21):** the recommendation is adopted for the proposal.

Size today: `xtask` about 13,300 lines of Rust, `fake-claude` and
`parity-driver` about 3,100, the `parity/` ledgers about 2,200. The real-CLI end-to-end
workflow runs by hand only (`e2e.yml`, `workflow_dispatch`, about 0.80 USD a
run); upstream runs its end-to-end tests on every push and pull request on
three systems. Recommendation for the proposal: present the differential
harness and the ledgers as what makes the port reviewable (each difference
from Python is measured, classified, and tested), offer to keep maintaining
them, and propose running `e2e.yml` on every push with Anthropic's
credentials once adopted. Since this run, the `offline-cli` job of `ci.yml`
already runs the pinned CLI on every push without credentials (handshake,
options in the init message, the `sdk-rs` agent list, project directory
names), so only the turns that need the model wait for those credentials.

## m34: shape of the public surface, and the native MCP dispatcher

**Decided by the owner (2026-11-21):** keep the public API as it is (flat root, synchronous disk readers, native dispatcher), and keep `IndexMap` (D21) and the variant names (D22); no rename.

Three questions, all of them public API, so none was changed in this run.

**Flat root.** `src/lib.rs` re-exports every type at the root
(`pub use types::*`, about 200 names), the way `claude_agent_sdk` exports
its `__all__`. A Rust reviewer may ask for modules instead
(`claude_agent_rust_sdk::{messages, hooks, options, mcp, sessions}`).
Options:

1. Keep the flat root. Ported code reads like the Python it comes from,
   and `docs/migration.md` maps one name to one name.
2. Group into modules and keep the root re-exports as the compatibility
   path (`#[doc(no_inline)]`). Additive, no break, but two paths per name.
3. Group into modules and drop the root re-exports in a 0.x minor release.
   Cleanest docs, breaks every user.

Recommendation: 2, decided with Anthropic during the review, since it costs
nothing now and can be done in one commit before 1.0.

**Synchronous disk readers.** `list_sessions`, `get_session_info`, and the
other disk readers block and return `Vec` or `Option`, ignoring I/O errors.
This is upstream's contract (synchronous functions, `except OSError` that
skips the file: `_internal/sessions.py:153`, `:182`, `:379`), so it is parity,
not a defect. If a reviewer asks for errors, the additive answer is a
`try_` variant returning `Result` next to each reader; an async variant is
not needed, since the `*_from_store` readers already are.

**Native MCP dispatcher.** Upstream serves an in-process server through the
`mcp` library "rather than re-implemented here"
(`_internal/sdk_mcp_bridge.py:1-10`). The crate does both: a server built
with `create_sdk_mcp_server` runs on a native dispatcher
(`src/mcp/server.rs`), and a hand-written `rmcp::ServerHandler` runs on rmcp
itself (feature `rmcp`, `McpServerConfig::sdk_rmcp`). The reasons for the
native default: it keeps rmcp out of the default build (96 crates in the
normal and build dependency tree on Linux without the feature, 115 with it,
measured 2026-11-21), and the dispatcher
only has to serve `initialize`, `ping`, `tools/list`, `tools/call`, and
`notifications/cancelled`. The cost is the one M1 measured: the answers
must follow the protocol library upstream pins, which moved when `mcp` 2.x
changed its errors. The guard is the differential harness, which runs the
MCP cases against both `mcp` 1.x and 2.x. Options: keep it (recommended,
with the harness as the argument), or route `create_sdk_mcp_server` through
rmcp when the feature is on and keep the native path only without it.


**`IndexMap` in public fields (D21).** `ClaudeAgentOptions::env`,
`extra_args`, `hooks`, `agents`, the `env` and `headers` of MCP configs,
`SandboxIgnoreViolations`, and `ResultMessage::model_usage` are
`indexmap::IndexMap`, re-exported as `claude_agent_rust_sdk::indexmap` and
named in the crate docs as one of the six public dependencies. Upstream's
`dict` keeps insertion order, and that order reaches the CLI: the order of
`extra_args` on the command line, of hook matchers in the initialize
request, and of agents, all compared by the differential harness. So the order is behavior, not taste.
The cost is that an `indexmap` 3.0 is a major release of this crate.
Options:

1. Keep `IndexMap` (recommended). `indexmap` has been at 2.x since 2023,
   and the re-export spares users a version match.
2. Wrap each field in a crate-owned ordered map type (`OrderedMap<K, V>`)
   that derefs to a slice of pairs. Frees the semver, costs a new type
   every user has to learn and a breaking change now.
3. Take `Vec<(K, V)>` in the setters and keep the map private. Simplest
   type, but loses key lookup, which `options.env.get("X")` callers use.

This changes the type of public fields, so it is yours to decide.

**`UnknownError` and `UnknownReason` (D22).** `AssistantMessageError` has
`UnknownError` (the wire value `"unknown"`, a category the CLI sends) next
to `Unknown(String)` (any value this version does not know), and
`FastModeDisabledReason` has `UnknownReason` next to `Unknown(String)`.
They are different things, but a reader sees two "unknown" variants.
Options: rename the wire ones (`Unspecified`, `Other`), which is a public
rename and yours; or keep the names and say it on each variant (done: the
docs of `UnknownError` say "Any other failure; details are in the message
text"). The error texts that name Python types (`got NoneType`) are kept on
purpose: they are upstream's text word for word, now said on
`python_type_name` in `src/message_parser.rs`.

**Terminal access of tools (U1).** The CLI leads its own process group, so
on Unix it runs in the background of the program's terminal: a tool that
opens `/dev/tty` (a `sudo` password, an `ssh` host-key question) is stopped
by `SIGTTIN`, where upstream's child, in the program's own group, prompts.
Measured under a pseudo-terminal; the close no longer stalls on such a tool
(`SIGCONT` follows `SIGTERM`). Options: keep the own group (current: one
signal ends the CLI and everything it started, the guarantee of M5), or
join the program's group when it is the foreground of a terminal, and then
signal only the CLI, as upstream does, losing the sweep of its
descendants in that one case. Recommendation: keep, since a session's CLI
reads its prompts from a pipe and a tool prompting on the host terminal
would fight the host program for it.
