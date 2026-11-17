[PRD]
# PRD: Parity hardening and differential proof (`claude-agent-kit`)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-16 | Arthur Jean | Initial draft, from `docs/research/python-sdk-parity.md` |
| 1.1 | 2026-11-16 | Arthur Jean | Add the Fedora path of the Python SDK checkout next to the Windows one |
| 1.2 | 2026-11-16 | Arthur Jean | Drop EP-008 (upstream contributions to the Python SDK): the PRD covers the Rust crate only. Its identifiers (EP-008, US-042 to US-045) are retired, not reused |
| 1.3 | 2026-11-17 | Arthur Jean | Move every Windows proof out of EP-009 into a new EP-011, so EP-009 is certified on Linux: the Windows criterion of US-047 becomes US-056, and the differential job on Windows becomes US-057 |

## Problem Statement

The parity audit of 2026-11-16 (`docs/research/python-sdk-parity.md`) compared this crate to `claude-agent-sdk` (Python) v0.2.182, commit `4720d36`, subsystem by subsystem. It found the port functionally complete (159 of 159 exports, about 99.5% of capabilities) and listed three problems that remain for this crate.

1. **Five divergences can hurt a user who migrates from Python.** Tool arguments are not validated against a raw JSON Schema unless an optional cargo feature is on, while Python always validates. A `match` on `Error::Process` misses `Error::ResultError`, while Python's `except ProcessError` catches `ResultError`. A resume from a session store continues after the credentials file could not be written, while Python aborts. Unknown keys of `modelUsage`, of the MCP status response, and of the context usage response are dropped, while Python passes those dictionaries through untouched. Seven test assertions are weaker than the Python tests they stand for.
2. **Parity is declared, not proven.** The test ledger maps 933 Python tests to 566 Rust tests by hand. 194 Rust tests stand for several Python tests each, up to 19. The audit sampled 52 pairs and found 4 weaker ones. Nothing checks mechanically that both SDKs send the CLI the same command line, environment, and JSON frames.
3. **The published parity claim is more generous than the facts.** `parity/surface.toml` reports 100% implemented although one entry (`CanUseToolShadowedWarning`) points to a method, not to a type. About 87 behavioral divergences are documented only in a French research note. Upstream drift is detected only when someone runs `cargo xtask upstream-diff` by hand.

**Why now:** the repository became public on 2026-11-16, so every claim is now read by outsiders. Upstream `main` is still at the pinned commit `4720d36` (checked with `git ls-remote` on 2026-11-16), so the differential harness compares against upstream as it stands. The audit is fresh, and its line references are valid for that commit only.

## Overview

This PRD turns the audit into three deliverables. First, close the five risky divergences so the crate behaves like Python where a user would be surprised otherwise. Second, build a differential harness: one language-neutral case file drives both SDKs against the scripted fake CLI, the fake CLI records what it received, and a comparison fails on any difference that is not in a reviewed ledger. Third, publish the divergences in an English page generated from that ledger, make the surface ledger distinguish "implemented" from "adapted", and watch upstream releases with a scheduled job.

Key decisions. Stories continue the numbering of the first PRD (`tasks/prd-claude-agent-rust-sdk.md` ends at EP-006 and US-035), so an identifier stays unique in the repository. Work on the Python SDK itself is out of scope: a Python defect the port finds is recorded as a `python-defect` entry of the differences ledger, and nothing is filed upstream. The differential harness follows the model of the Model Context Protocol conformance suite: a narrow list of expected differences, where an entry that is no longer needed fails the run. Comparison is done on parsed JSON values with a small normalizer written for this harness; the `insta` crate was considered and not chosen, because it compares a value to a stored snapshot and not two live recordings. Raw-schema validation becomes a default cargo feature; `jsonschema` 0.61.2 with its own default features off carries no network code.

The Python source stays the behavioral reference, read from the submodule `upstream/claude-agent-sdk-python` and from the identical checkout at `C:/dev/claude-agent-sdk-python` (Windows) or `/home/arthur/dev/claude-agent-sdk-python` (Fedora). Every epic and story carries a `Python reference` block. Delivery is phased in four releases, one per epic, in the order EP-007, EP-009, EP-010, EP-011.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Risky divergences from the audit still open (of 5) | 0 | 0 |
| Option fields of `ClaudeAgentOptions` that reach the CLI and are exercised by a differential case | 20 of the fields that reach the command line | 100% of the fields that reach the command line, the environment, or the initialize request |
| Divergences listed in `docs/parity.md` that link to a test | 100% of the listed ones | 100%, with every divergence of the audit either listed or closed |
| Delay between an upstream release and an open tracking issue | 7 days or less | 7 days or less |

## Target Users

### Rust application developer migrating from Python
- **Role:** ports an agent service from the Python SDK to this crate.
- **Behaviors:** translates `except ProcessError` blocks into `match` arms, reuses JSON Schemas written for Python tools, reads `docs/migration.md`.
- **Pain points:** a raw-schema tool receives arguments the schema forbids; an error arm silently stops matching; a new `modelUsage` key added by the CLI is invisible.
- **Current workaround:** enables the `jsonschema` feature after hitting the problem, matches both error variants by hand, and reads the raw CLI output to find lost keys.
- **Success looks like:** code ported line by line behaves the same, and every deliberate difference is on one page with its reason.

### SDK maintainer
- **Role:** keeps the crate aligned with upstream (today: the owner).
- **Behaviors:** runs `cargo xtask parity-check`, reads upstream commits, maps tests in `parity/tests.toml`.
- **Pain points:** a hand-kept ledger can claim coverage a folded test does not deliver; drift is noticed late.
- **Current workaround:** manual audits such as `docs/research/python-sdk-parity.md`, which took nine sub-agents and a full verification pass.
- **Success looks like:** one command proves both SDKs send the same bytes, and an issue appears within a week of each upstream release.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Model Context Protocol conformance suite** (`github.com/modelcontextprotocol/conformance`): scenarios with their own checks; each implementation keeps an expected-failures file; a listed scenario that starts passing fails the run; one check can be excused instead of a whole scenario. This is the model for `parity/differences.toml`.
- **Smithy protocol tests (AWS SDKs)**: data fixtures; random values pinned at the source with a fixed token; `requireHeaders` and `forbidHeaders` assert presence and absence without comparing every value.
- **gRPC interop tests and the Temporal `features` repository**: one named case per feature, implemented per language; no formal record of known deviations.
- **Market gap:** no conformance suite exists for the Claude Agent SDKs. Python and TypeScript are kept aligned by hand. A language-neutral case format is useful beyond this crate.

### Best Practices Applied
- Compare parsed JSON values, never raw text; pin nondeterministic values at the source when possible and redact by a narrow selector otherwise (`insta` redaction documentation, Smithy specification).
- Keep the expected-differences list honest: narrow entries, and failure when an entry is stale (MCP conformance).
- A parity page states the upstream version checked, the date, each divergence with its reason and a link to the test that shows it, and keeps "not ported" separate from "deliberately different".
- A scheduled workflow gets no default permission; `issues: write` is granted to the single job that opens the issue.
- pip's hash-checking mode cannot install a local directory or a git URL, so the Python SDK is imported from the submodule through `PYTHONPATH` and only its dependencies are installed, hash-locked.

*Full research sources: `docs/research/python-sdk-parity.md`, and the sources listed in the research brief of this PRD's decision log (MCP conformance, Smithy protocol tests, gRPC interop descriptions, insta redactions, pip secure installs, GitHub changelog of 2026-06-18 on `pull_request_target`).*

## Assumptions & Constraints

### Assumptions (to validate)
- The Python SDK completes a session against `fake-claude` when `cli_path` points at the binary, on Windows and Linux, with no change to either. Based on `ref: src/claude_agent_sdk/_internal/transport/subprocess_cli.py:251-252` (the option is used as given), `:394-423` (only `.bat` and `.cmd` are refused), and `:1153-1167` (the version is read from the init frame, which `fake-claude` fills with `2.1.325`). Validated on Linux by US-047 and on Windows by US-056.
- The CLI writes non-ASCII text raw in transcripts, as JavaScript's `JSON.stringify` does. Inferred, not checked; it matters only for the wording of one divergence in `docs/parity.md`.
- Frame order on the fake CLI's stdin is deterministic when the scenario serializes the exchange with `expect` steps. Checked by the 20-run criterion of US-049.
- `mcp` 1.x is the version line the differential job installs. Upstream also accepts 2.x (`ref: pyproject.toml:31`); that line is not covered.

### Hard Constraints
- Every hard constraint of `tasks/prd-claude-agent-rust-sdk.md` still holds: Rust 1.88 minimum, edition 2024, tokio only, GPL-3.0-or-later, no implementation comments (`//`) in code, rustdoc required on public items.
- The upstream submodule and the local checkout (`C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora) are read-only.
- Nothing is posted on GitHub in the owner's name (issue, pull request, comment) without his explicit go for that item.
- Runs against the real CLI cost money and are manual: an implementer asks the owner before launching one. No story may require a real-CLI run to pass its quality gates.
- The repository is public: no secret is read in a job triggered by `pull_request`.
- The owner is the only visible contributor: no AI attribution, no `Co-authored-by` line, and no bot-authored commit. A bot-authored issue is acceptable; a bot-authored commit is not.
- GitHub Actions are referenced by tag and locked through `gh actions-lock` in `.github/workflows/actions.lock`.
- Every artifact is written in English, including `docs/parity.md`.
- `cargo xtask parity-check --deny-pending` stays green, and line coverage stays at or above 91.5%.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --all --check` - formatting
- `cargo clippy --workspace --all-targets --all-features --no-deps` - lint policy (`panic`, `unimplemented`, `dbg_macro` denied)
- `cargo test --workspace --all-features` - unit and integration tests, including the fake-CLI tests
- `cargo doc --workspace --all-features --no-deps` - documentation builds with `RUSTDOCFLAGS="-D warnings"`
- `cargo xtask parity-check --deny-pending` - both ledgers complete, every Rust path exists, generated tables fresh

For stories of EP-009 and EP-010 that land after US-049, one additional gate:
- `cargo xtask parity-diff` - every differential case matches, and no entry of `parity/differences.toml` is stale

## Epics & User Stories

Reference root for every `Python reference` block and `ref:` note: `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, at commit `4720d36` (the same tree as the submodule `upstream/claude-agent-sdk-python`). Line numbers are valid for that commit only. Rust paths are relative to the repository root (`C:/dev/claude-agent-sdk-rust` on Windows, `/home/arthur/dev/claude-agent-sdk-rust` on Fedora) and valid for commit `c15579f`.

### EP-007: Close the risky behavioral gaps

Make the crate behave like Python in the five places where the audit found a difference that can surprise a migrating user. Release A.

**Definition of Done:** the five divergences of section 4.4 items 1 to 4 and section 6 of `docs/research/python-sdk-parity.md` are closed, each by a test that fails on the code as it stands at commit `c15579f`.

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/__init__.py`: 436-456 (schema building), 604-645 (`create_sdk_mcp_server`, validation at 628-632)
- `src/claude_agent_sdk/_errors.py`: 6-134 (exception hierarchy)
- `src/claude_agent_sdk/_internal/session_resume.py`: 130-219 (`materialize_resume_session`, cleanup at 183-191), 326-393 (`_copy_auth_files`), 431-455 (`_write_redacted_credentials`)
- `src/claude_agent_sdk/_internal/message_parser.py`: 310-345 (result message, pass-through fields)
- `src/claude_agent_sdk/types.py`: 969-1099 (MCP status and context usage shapes), 1702-1786 (`ModelUsage`, `ResultMessage`)
- `src/claude_agent_sdk/client.py`: 471-538 (`get_mcp_status`, `get_context_usage` return the response as received)
- `pyproject.toml`: 27-33 (runtime dependencies, `jsonschema` among them)
- `tests/test_sdk_mcp_integration.py`, `tests/test_errors.py`, `tests/test_session_resume.py`, `tests/test_transcript_mirror.py`, `tests/test_tool_callbacks.py`: tests
- `e2e-tests/test_error_results.py`, `e2e-tests/test_include_partial_messages.py`, `e2e-tests/test_subagent_session_reads.py`: end-to-end tests

#### US-036: Validate raw-schema tool arguments by default
**Description:** As a Rust application developer migrating from Python, I want arguments of a raw-schema tool validated before my handler runs, without enabling a feature, so that a tool ported from Python rejects the same calls.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/__init__.py`: 620-641 (`run_tool`: validation, error mapping)
- `pyproject.toml`: 32 (`jsonschema>=4.20.0` is a required dependency)
- `tests/test_sdk_mcp_integration.py`: 989-1016 (validation error texts)

**Acceptance Criteria:**
- [ ] `crates/claude-agent-kit/Cargo.toml` declares `default = ["jsonschema"]`, and the `jsonschema` feature still exists so that `default-features = false` turns validation off
- [ ] Given a tool built with `SdkMcpTool::raw` and a schema that requires property `b`, when `tools/call` arrives without `b` in a build with default features, then the result has `isError: true`, its text starts with `Input validation error: `, and the handler is not called (ref: `src/claude_agent_sdk/__init__.py:630-632`)
- [ ] Given a raw schema that is not a valid JSON Schema, when the tool is called, then the result is an error result and the session continues (ref: `src/claude_agent_sdk/__init__.py:640-641`)
- [ ] Given a raw schema whose `$ref` points to a remote URL, when the tool is called, then the result is an error result that names the reference, and no network request is made
- [ ] Given a build with `--no-default-features`, when the same invalid call arrives, then the handler is called, and the rustdoc of `SdkMcpTool::raw` states that arguments are not validated in that build
- [ ] The validator of each tool is compiled once when the server is created, not on each call
- [ ] `cargo tree -p claude-agent-kit -e normal` lists neither `reqwest` nor `rustls` with default features
- [ ] CI builds and tests the crate with `--no-default-features` in at least one job, and the `examples`, `msrv`, `minimal-versions`, and `licenses` jobs pass with the new default
- [ ] The rustdoc of the crate root and `docs/migration.md` state the default and the opt-out

#### US-037: Error predicates that mirror the Python hierarchy
**Description:** As a Rust application developer migrating from Python, I want one call that answers "is this a process failure" and one that answers "is this a connection failure" so that an `except ProcessError` or `except CLIConnectionError` block has a direct translation.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_errors.py`: 10-22 (`CLINotFoundError` is a `CLIConnectionError`), 25-56 (`ResultError` is a `ProcessError`)
- `tests/test_errors.py`: 16-100 (hierarchy assertions)
- `e2e-tests/test_error_results.py`: 37-42 (`isinstance(exc, ProcessError)` on a `ResultError`)

**Acceptance Criteria:**
- [ ] `Error::is_process()` returns true for `Error::Process` and `Error::ResultError`, and false for every other variant
- [ ] `Error::is_cli_connection()` returns true for `Error::CliConnection` and `Error::CliNotFound`, and false for every other variant
- [ ] A test iterates one value of every variant of `Error` and asserts both predicates, so a variant added later without a decision fails to compile or fails the test
- [ ] Given `Error::Config`, when both predicates are called, then both return false
- [ ] The rustdoc of `Error` shows the Python `except` block and its Rust translation for both cases
- [ ] `parity/surface.toml` notes for `CLIConnectionError` and `ProcessError` name the predicate, and `docs/migration.md` is regenerated

#### US-038: Fail a resume when the credentials cannot be written
**Description:** As an agent platform engineer, I want a resume from a session store to fail at once when the credentials file cannot be written so that I get the real cause instead of an authentication error from the CLI later.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/session_resume.py`: 431-455 (`_write_redacted_credentials`: the write at 452 is not guarded), 183-191 (any failure removes the temporary directory and re-raises), 476-493 (`_copy_if_present`, the tolerant path used for the other files)
- `tests/test_session_resume.py`: 156-217 (credentials tests)

**Acceptance Criteria:**
- [ ] Given credentials to copy and a target path that cannot be written, when the resume is materialized, then it returns an error whose message names the credentials file and carries the I/O cause
- [ ] Given that failure, when the call returns, then the temporary directory no longer exists (ref: `src/claude_agent_sdk/_internal/session_resume.py:183-191`)
- [ ] Given that failure, when the call returns, then no CLI process was started
- [ ] Given no credentials to copy (no file, no keychain entry), when the resume is materialized, then it succeeds as before
- [ ] Given `.claude.json` or the settings file cannot be copied, when the resume is materialized, then it still succeeds with a warning, as before (ref: `src/claude_agent_sdk/_internal/session_resume.py:476-493`)
- [ ] The existing test that asserts the refresh token is removed from the copied credentials still passes

#### US-039: Keep every key Python passes through
**Description:** As a Rust application developer, I want every key the CLI sends inside a structure that Python hands over untouched to be reachable from the Rust value so that a field added by a newer CLI is not lost.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/message_parser.py`: 322 (`usage`), 325 (`modelUsage`), 326 (`permission_denials`), 324 (`structured_output`), 352 (`event`), 374 (`raw` of the rate limit info)
- `src/claude_agent_sdk/_internal/query.py`: 813-819 (`get_mcp_status` and `get_context_usage` return the response dictionary)
- `src/claude_agent_sdk/client.py`: 471-538 (the client returns that dictionary)
- `src/claude_agent_sdk/types.py`: 969-1099 (documented keys of those dictionaries), 1702-1726 (`ModelUsage`)

**Acceptance Criteria:**
- [ ] `ModelUsage` exposes the keys it does not model, and a `modelUsage` entry parsed from `{"inputTokens": 1, "futureKey": 7}` gives access to `futureKey` with value 7
- [ ] The typed MCP status response and every structure nested in it (server status, server info, tool info, tool annotations) expose the keys they do not model
- [ ] The typed context usage response and its categories expose the keys they do not model
- [ ] A table-driven test covers every place where the Python parser or client passes a dictionary through (the lines listed in the Python reference above): it injects an unknown key at each object level of a fixture and asserts the key is reachable from the parsed Rust value
- [ ] The test holds a list of exempted structures, each with a written reason; an exemption that is no longer needed fails the test
- [ ] Given a `modelUsage` entry that is not an object, when the result message is parsed, then parsing succeeds and the entry is skipped or kept raw, with no panic
- [ ] Serializing a value that was parsed with unknown keys writes those keys back

#### US-040: Restore the unit assertions lost in folded tests
**Description:** As an SDK maintainer, I want the four unit tests the audit rated weaker than their Python originals to assert the same facts so that the test ledger's claim holds for them.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `tests/test_transcript_mirror.py`: 211-219 (`test_empty_entries_batch_skips_append`), 450-463 (`test_unmapped_file_path_is_dropped_silently`)
- `tests/test_sdk_mcp_integration.py`: 422-456 (`test_close_does_not_hang_on_a_tool_blocked_outside_the_event_loop`)
- `tests/test_tool_callbacks.py`: 981-992 (`test_new_hook_events_registered_in_hooks_config`)
- `src/claude_agent_sdk/_internal/transcript_mirror_batcher.py`: 76-84 and 155-160 (batching), 184-219 (failure reporting)

**Acceptance Criteria:**
- [ ] A test asserts that enqueueing an empty batch causes zero calls to the store's `append`, by counting calls (ref: `tests/test_transcript_mirror.py:216-219`)
- [ ] A test asserts that a frame whose path is outside the projects directory causes zero `append` calls and zero reported mirror errors, with an error reporter attached (ref: `tests/test_transcript_mirror.py:462-463`)
- [ ] A test asserts that closing a server whose in-process tool does not stop logs a warning, captured with the existing `capture::Warnings` helper (ref: `tests/test_sdk_mcp_integration.py:449-451`)
- [ ] A test builds options with hooks registered under `Notification`, `SubagentStart`, and `PermissionRequest` and asserts the three keys are present (ref: `tests/test_tool_callbacks.py:989-992`)
- [ ] Given the store's `append` is called with an empty list by a regression, when the first test runs, then it fails
- [ ] `parity/tests.toml` maps each of the four Python tests to the Rust test that now carries the assertion

#### US-041: Restore the end-to-end assertions and route `store_resume_e2e` through the suite
**Description:** As an SDK maintainer, I want the end-to-end tests to compare what their Python originals compare, and every real-CLI test to fail without credentials, so that a green run means what it says.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `e2e-tests/test_error_results.py`: 22-60 (payload equality at 50-59)
- `e2e-tests/test_include_partial_messages.py`: 26-93 (text block looked for in assistant messages only)
- `e2e-tests/test_subagent_session_reads.py`: 30-107 (every result checked for errors)
- `e2e-tests/test_session_store_resume_settings.py`: 63-98 (the store-resume scenario)
- `e2e-tests/conftest.py`: 8-34 (model pin, credentials fixture)

**Acceptance Criteria:**
- [ ] The error-result test asserts that the error's `session_id` and `terminal_reason` equal those of the result message, that the error payload's `uuid` equals the result's `uuid`, and that the error's `terminal_reason` is `api_error` (ref: `e2e-tests/test_error_results.py:52-59`)
- [ ] The partial-messages test looks for the text block in assistant messages only
- [ ] The subagent-reads test asserts `is_error` is false on every result message, not only on the last
- [ ] `tests/store_resume_e2e.rs` is removed and its test lives in the `e2e` target, uses `suite::options()`, the suite model pin, and the 240-second turn limit
- [ ] Given no credentials in the environment, when the store-resume test is run with `--ignored`, then it fails naming the missing variable, instead of returning early
- [ ] `cargo test --workspace --all-features` still passes with all real-CLI tests ignored, and the count of ignored tests is unchanged or higher by the moved test
- [ ] No real-CLI run is launched by the implementer; the story records in its hand-off that a run of `cargo xtask e2e` awaits the owner's go

---

### EP-009: Differential parity harness

Prove with one command that the Rust SDK and the Python SDK send the CLI the same command line, environment, and frames for the same case. Release B.

**Definition of Done:** `cargo xtask parity-diff` runs every case of `parity/cases/` through both SDKs against `fake-claude`, passes with no unexplained difference, fails on a stale entry of `parity/differences.toml`, and runs in CI on every pull request.

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 251-252 (`cli_path`), 268-270 (version check skip), 394-423 (script refusal on Windows), 519-744 (`_build_command`), 746-840 (`connect`: environment at 759-823, spawn at 828-836), 1153-1167 (version read from the init frame)
- `src/claude_agent_sdk/_internal/query.py`: 314-391 (`initialize` request), 606-733 (answers to inbound control requests), 735-780 (control request envelope), 813-897 (outbound control requests), 1122-1162 (`stream_input`)
- `src/claude_agent_sdk/client.py`: 93-258 (`connect`), 272-303 (`query`), 308-469 (control methods)
- `src/claude_agent_sdk/query.py`: 1-132 (one-shot `query`)
- `src/claude_agent_sdk/types.py`: 2392-2878 (`ClaudeAgentOptions` fields)
- `pyproject.toml`: 10 (`requires-python`), 27-33 (dependencies)
- `.github/workflows/test.yml`: 20-61 (Python version and install steps used upstream)

#### US-046: Fake CLI recording
**Description:** As an SDK maintainer, I want `fake-claude` to write what it received to a file so that two SDKs can be compared on what they actually sent.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 759-823 (every environment variable the SDK sets or removes: the names the recording must capture), 828-836 (spawn arguments)

**Acceptance Criteria:**
- [ ] Given the environment variable `FAKE_CLAUDE_RECORD` names a file, when `fake-claude` starts, then it writes one JSON object per line to that file, starting with a `spawn` record that holds the argument list, the working directory, and the environment variables selected by the recording rule
- [ ] The recording rule captures every variable whose name starts with `CLAUDE_` or `ANTHROPIC_`, plus `PWD`, `MAX_THINKING_TOKENS`, `TRACEPARENT`, and `TRACESTATE`, and the rule is documented in the crate's rustdoc
- [ ] Each line read from stdin produces one `stdin` record holding the parsed JSON, in arrival order; a line that is not valid JSON produces a `stdin_raw` record holding the text
- [ ] The end of stdin produces a `stdin_eof` record
- [ ] Each record is flushed when written, so a process killed mid-run leaves a file whose complete lines are valid JSON
- [ ] Given `FAKE_CLAUDE_RECORD` is not set, when `fake-claude` runs, then its behavior, exit codes (64, 65, 66), and init-frame keys are unchanged, shown by the existing fake-CLI tests passing without edits
- [ ] Given the record file cannot be created, when `fake-claude` starts, then it exits with a new documented exit code and a message on stderr naming the path

#### US-047: Validate assumption: the Python SDK runs against the fake CLI
**Description:** As an SDK maintainer, I want to know by a run that the unmodified Python SDK completes a session against `fake-claude` so that the harness design stands on a fact.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-046

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 251-252, 268-270, 394-423, 1153-1167
- `src/claude_agent_sdk/query.py`: 1-132
- `pyproject.toml`: 10, 27-33

**Acceptance Criteria:**
- [ ] A Python script outside the submodule calls `query()` with `cli_path` set to the `fake-claude` binary and `FAKE_CLAUDE_SCENARIO` set to a scenario that answers the initialize request and emits one result
- [ ] The run ends with a result message and exit status 0 on Linux (the Windows run is US-056 of EP-011)
- [ ] The recording holds a `spawn` record, an `initialize` control request, and a user message frame
- [ ] The Python SDK is imported from `upstream/claude-agent-sdk-python/src` through `PYTHONPATH`; its dependencies are installed in a virtual environment outside both repositories' tracked files
- [ ] The result is written to `docs/validation/python-sdk-against-fake-claude.md`: Python version, dependency versions, the command, and what was observed
- [ ] Given the run fails on one platform, when the validation is recorded, then the cause is written down and US-048 is scoped to the platforms that work, with the gap listed in Open Questions

#### US-048: Case format and the two drivers for one-shot queries
**Description:** As an SDK maintainer, I want one case file to drive both SDKs through the same one-shot query so that their recordings can be compared.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-047

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 2392-2878 (option field names, which the case format reuses)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 519-744 (how each option reaches the command line)
- `src/claude_agent_sdk/query.py`: 1-132; `src/claude_agent_sdk/_internal/client.py`: 1-211 (the one-shot path)

**Acceptance Criteria:**
- [ ] A case is one JSON file under `parity/cases/` with a name, a `fake-claude` scenario, an `options` object keyed by the Python field names of `ClaudeAgentOptions`, and a prompt that is a string or a list of user messages
- [ ] A JSON Schema of the case format is checked in, and `cargo xtask parity-diff` rejects a case that does not match it, naming the file and the path of the first error
- [ ] The Rust driver is a workspace binary that is never published; it reads a case, builds the options, runs `query`, and exits 0 when the stream ends with a result
- [ ] The Python driver is one file under `parity/drivers/`; it reads the same case, builds `ClaudeAgentOptions`, runs `query()`, and exits 0 when the stream ends with a result
- [ ] Both drivers support at least these options: `system_prompt` in its four forms, `tools`, `allowed_tools`, `disallowed_tools`, `max_turns`, `max_budget_usd`, `model`, `fallback_model`, `permission_mode`, `continue_conversation`, `resume`, `session_id`, `settings`, `add_dirs`, `mcp_servers` as a map, `include_partial_messages`, `setting_sources`, `extra_args`, `thinking`, `effort`, `output_format`, `env`, `cwd`
- [ ] Given a case names an option a driver does not support, when the driver runs, then it exits with a distinct status and prints the option name, and the harness reports the case as unsupported instead of passing it
- [ ] Given a driver does not finish within 30 seconds, when the harness runs the case, then it kills the driver and its children and reports a timeout for that side
- [ ] Three cases are checked in: default options, a case with every supported command-line option set, and a case with a list prompt

#### US-049: Comparison, normalization, and the ledger of differences
**Description:** As an SDK maintainer, I want the two recordings compared after a documented normalization, against a ledger of reviewed differences, so that any new difference fails and any outdated excuse fails too.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-048

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/query.py`: 746-749 (request id format `req_{n}_{8 hex}`), 333-347 (hook callback ids `hook_{n}`)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 767-769 (`CLAUDE_CODE_ENTRYPOINT` is `sdk-py`, `CLAUDE_AGENT_SDK_VERSION` is the package version: two expected differences), 563 (`--max-budget-usd` uses `str()`, a known difference of format)

**Acceptance Criteria:**
- [ ] `cargo xtask parity-diff` builds `fake-claude` and the Rust driver, runs both drivers on every case, and prints one line per case: match, differ, unsupported, or timeout
- [ ] Comparison is done on parsed values: command-line arguments whose value is JSON are parsed before comparison, and object key order never causes a difference
- [ ] Normalization replaces only these: the random suffix of control request ids, temporary directory prefixes, the path of the CLI binary, and path separators; each rule is listed in `parity/README.md` with the reason
- [ ] `parity/differences.toml` holds a `[wire]` table; each entry names a case or `*`, one location (an argument, a variable, or a JSON pointer into a frame), the value on each side, a category among `deliberate`, `rust-follows-cli`, and `python-defect`, and a reason
- [ ] Given a difference that no entry covers, when `parity-diff` runs, then it exits non-zero and prints the case, the location, and both values
- [ ] Given an entry that matched no difference in the run, when `parity-diff` runs, then it exits non-zero and names the stale entry
- [ ] Given an entry with category `python-defect`, when the ledger is loaded, then it must cite the upstream line at fault as a `ref:` path and line at the pinned commit, or loading fails
- [ ] The ledger starts with the entries for `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_AGENT_SDK_VERSION`, and the `--max-budget-usd` format, and with no entry wider than one location
- [ ] The three cases of US-048 run 20 times in a row with identical results
- [ ] Given Python or a driver dependency is missing, when `parity-diff` runs, then it exits non-zero with a message that names what is missing, and never reports a match

#### US-050: Client sessions, callbacks, and in-process tools in the drivers
**Description:** As an SDK maintainer, I want cases that exercise the interactive client, hooks, the permission callback, and in-process MCP tools so that the control protocol is compared, not only the command line.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-049

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/client.py`: 93-258, 272-303, 308-469, 471-538
- `src/claude_agent_sdk/_internal/query.py`: 314-391 (initialize payload with hooks and agents), 606-733 (responses to `can_use_tool`, `hook_callback`, `mcp_message`), 813-897 (outbound subtypes)
- `src/claude_agent_sdk/__init__.py`: 277-361 (`tool`), 517-645 (`create_sdk_mcp_server`)
- `src/claude_agent_sdk/_internal/sdk_mcp_bridge.py`: 1-384 (bridge to the `mcp` package)

**Acceptance Criteria:**
- [ ] A case may hold a `steps` list for the interactive client: connect, query, interrupt, set permission mode, set model, rewind files, reconnect or toggle an MCP server, stop a task, read the MCP status, read the context usage, disconnect
- [ ] A case may declare canned callbacks: a permission decision (allow, allow with updated input, deny with a message), hook outputs per event, and in-process tools that return fixed content
- [ ] Both drivers implement every step and every canned callback of the format
- [ ] The cases compare the frames each SDK writes in answer to scripted `can_use_tool`, `hook_callback`, and `mcp_message` requests, including `initialize`, `tools/list`, and `tools/call` on an in-process server
- [ ] Given a scenario sends a control request with an unknown subtype, when both drivers run, then both error responses are recorded and compared
- [ ] Given a case whose frames can arrive in either order, when it is declared `unordered` for a named group of frames, then the comparison treats that group as a set, and every other frame stays ordered
- [ ] Every difference these cases reveal is either fixed in the crate or entered in `parity/differences.toml` with a category and a reason
- [ ] At least 8 cases cover this story: one per callback kind, one per in-process tool method, and one with every client step

#### US-051: Case corpus and coverage of the options
**Description:** As an SDK maintainer, I want to know which options no case exercises so that the proof has a measured extent.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-050

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 2392-2878 (the 50 fields of `ClaudeAgentOptions`)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 519-744 (fields that reach the command line), 759-823 (fields that reach the environment)
- `src/claude_agent_sdk/_internal/query.py`: 350-365 (fields that reach the initialize request)

**Acceptance Criteria:**
- [ ] `cargo xtask parity-diff --coverage` lists every field of `ClaudeAgentOptions`, read from the upstream source, with the cases that set it
- [ ] A field that no case sets must appear in an exclusion list in `parity/cases/coverage.toml` with a reason (for example a callback that never reaches the CLI as data), or the command exits non-zero
- [ ] Given an upstream bump adds an option field, when `--coverage` runs, then it exits non-zero naming the field
- [ ] Given an exclusion names a field that a case now sets, when `--coverage` runs, then it exits non-zero naming the stale exclusion
- [ ] Every field that reaches the command line, the environment, or the initialize request is set by at least one case or excluded with a reason
- [ ] The command prints the totals: fields covered, fields excluded, cases run

#### US-052: Differential job in CI
**Description:** As an SDK maintainer, I want the differential harness to run on every pull request so that a change that breaks parity is caught before it merges.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-049

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `.github/workflows/test.yml`: 20-27 (Python 3.13 setup), 53-61 (the pinned `mcp` install)
- `pyproject.toml`: 27-33 (dependencies to lock)

**Acceptance Criteria:**
- [ ] `ci.yml` has a job that checks out submodules, sets up Python 3.13, installs the Python dependencies, and runs `cargo xtask parity-diff` and `cargo xtask parity-diff --coverage`
- [ ] The Python dependencies are installed from a checked-in requirements file with `--require-hashes`, and every package in it carries a hash
- [ ] The Python SDK itself is not installed: the job sets `PYTHONPATH` to the submodule's `src`
- [ ] The job reads no secret and has `contents: read` as its only permission
- [ ] Every action the job uses is referenced by tag and present in `.github/workflows/actions.lock`, and the `lockfile` job passes
- [ ] The job finishes in under 5 minutes on the Linux runner
- [ ] Given the requirements file and the submodule's `pyproject.toml` disagree on a dependency's allowed range, when the job runs, then a check fails naming the package

---

### EP-010: Publish the divergences and track upstream

Make the parity claim exact and public, and learn about upstream releases within a week. Release C.

**Definition of Done:** `docs/parity.md` is generated from the ledgers and checked by `parity-check`; the surface ledger reports adapted entries separately; a scheduled workflow has opened a tracking issue in a dry run.

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/__init__.py`: 648-824 (`__all__`, the 159 exported names)
- `src/claude_agent_sdk/types.py`: 2254-2262 (`CanUseToolShadowedWarning`), 2263-2372 (the shadow warning and the `can_use_tool` conflict check)
- `src/claude_agent_sdk/_errors.py`: 6-134
- `CHANGELOG.md`: upstream change log
- `pyproject.toml`: 7 (version `0.2.182`)

#### US-053: An `adapted` status in the ledgers
**Description:** As a reader of the parity report, I want entries whose Rust counterpart is a different kind of thing reported apart from plain implementations so that "100% implemented" is only printed when it is true.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/types.py`: 2254-2262 (`CanUseToolShadowedWarning` is a warning class), 2338-2344 (how it is emitted)
- `src/claude_agent_sdk/__init__.py`: 648-824

**Acceptance Criteria:**
- [ ] A ledger entry accepts a new key `adapted`, holding a Rust path or a list of paths, and exactly one of `rust`, `adapted`, `excluded`, and `pending` must be set
- [ ] Given an `adapted` entry without a `note`, when the ledger is loaded, then loading fails naming the entry
- [ ] `cargo xtask parity-check` resolves the Rust paths of `adapted` entries the same way as those of `rust` entries
- [ ] `cargo xtask parity-report` prints four lines per ledger: implemented, adapted, excluded, pending
- [ ] `cargo xtask migration-table` renders an adapted entry with its Rust path and its note
- [ ] `CanUseToolShadowedWarning` is `adapted`, and each of the 15 entries of `parity/surface.toml` that carry a note today is reviewed and set to `rust` or `adapted`, with the decision rule written at the top of the file
- [ ] `docs/migration.md` is regenerated and `parity-check --deny-pending` passes

#### US-054: `docs/parity.md`, generated and checked
**Description:** As a Rust application developer, I want one English page that states what was compared, the measured parity, and every deliberate difference with its reason and its test, so that I can decide whether the crate fits before I port.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-049, US-053

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `pyproject.toml`: 7 (the version the page states)
- `src/claude_agent_sdk/_errors.py`: 10-56 (hierarchy, for the error divergence)
- `src/claude_agent_sdk/types.py`: 2338-2372 (shadow warning, conflict check)
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 828-836 (the child is spawned in the parent's process group)
- `src/claude_agent_sdk/_internal/message_parser.py`: 397-401 (unknown messages are skipped)
- `src/claude_agent_sdk/_internal/sessions.py`: 81-114 (directory naming)

**Acceptance Criteria:**
- [ ] `parity/differences.toml` gains a `[behavior]` table for divergences that the differential harness cannot see (for example how inbound messages are parsed); each entry has a Python reference, a category, a reason, and the path of a Rust test that shows the behavior
- [ ] Given a `[behavior]` entry names a Rust test that does not exist, when `parity-check` runs, then it fails naming the entry
- [ ] `cargo xtask parity-page` writes `docs/parity.md` between generation markers: the upstream version and commit, the date of the pinned commit, the counts of both ledgers, the count of differential cases and covered option fields, and one table per category of difference
- [ ] `cargo xtask parity-page --check` only compares, and `parity-check` runs it, so a stale page fails CI
- [ ] The page keeps "not ported" (ledger exclusions) apart from "deliberately different" (the differences ledger), and has a hand-written section for what Rust adds
- [ ] The `[behavior]` table is seeded with at least these divergences from `docs/research/python-sdk-parity.md`: the flat error enum and its predicates, the separate process group, the shadow warning logged on each session, `can_use_tool` accepted with an explicit `stdio` prompt tool, unknown messages kept instead of skipped, strict typing of required fields in the parser, UTF-16 directory naming, and millisecond timestamps
- [ ] Every divergence of sections 4.1 to 4.4 of `docs/research/python-sdk-parity.md` is listed on the page, closed by EP-007, or named in a "not yet classified" list with a count
- [ ] `docs/migration.md` and `README.md` link to the page
- [ ] The page contains no claim of the form "100%" or "full parity" that the generated counts do not support

#### US-055: Scheduled upstream watch
**Description:** As an SDK maintainer, I want an issue opened when upstream publishes a release I have not ported so that drift is seen within a week.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** None

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `CHANGELOG.md`: release notes the issue quotes
- `pyproject.toml`: 7 (version)
- `.github/workflows/auto-release.yml`, `.github/workflows/publish.yml`: how upstream tags releases (tags of the form `v0.2.182`)

**Acceptance Criteria:**
- [ ] A workflow runs weekly and on manual dispatch, resolves the newest upstream release tag, and compares it with the submodule pin
- [ ] Given the newest tag equals the pin, when the workflow runs, then it opens no issue and ends green
- [ ] Given a newer tag, when the workflow runs, then it runs `cargo xtask upstream-diff <tag>` and opens one issue titled with the tag, whose body is the command's output and the matching changelog section
- [ ] Given an open issue for that tag already exists, when the workflow runs again, then it opens none
- [ ] Given the tag lookup or the diff command fails, when the workflow runs, then the run fails, and it never reports "up to date"
- [ ] The workflow's default permission is `contents: read`; only the job that opens the issue has `issues: write`; it has no `pull_request` or `pull_request_target` trigger and reads no secret other than the workflow token
- [ ] The workflow makes no commit and opens no pull request
- [ ] The script behind it has an offline test in the style of `.github/scripts/actions-freshness.test.sh`, run by the `scripts` job, covering: no new tag, new tag, existing issue, lookup failure
- [ ] Every action it uses is referenced by tag and present in `.github/workflows/actions.lock`

---

### EP-011: Windows parity proof

Prove on Windows what EP-009 proves on Linux: the unmodified Python SDK runs against `fake-claude`, and the differential harness passes. Release D.

**Definition of Done:** the validation of US-047 is recorded for Windows, and the `differential` CI job runs `cargo xtask parity-diff` on `windows-latest` on every pull request and passes with no unexplained difference.

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 272-328 (`_find_cli`, Windows branches at 280 and 307), 346-423 (batch file detection and refusal), 429-449 (`cmd.exe` metacharacter check, Windows only), 917-1000 (`close`: terminate on timeout)
- `.github/workflows/test.yml`: 10-14 (upstream unit tests run on `windows-latest`)
- `pyproject.toml`: 10 (`requires-python`), 27-33 (dependencies)

#### US-056: Validate assumption: the Python SDK runs against the fake CLI on Windows
**Description:** As an SDK maintainer, I want to know by a run that the unmodified Python SDK completes a session against `fake-claude.exe` so that the harness can be trusted on Windows.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** None (US-047 is done on Linux)

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 251-252, 346-423, 1153-1167

**Acceptance Criteria:**
- [ ] `docs/validation/python-sdk-against-fake-claude.py`, unchanged, runs on Windows with `fake-claude.exe` as its argument and the SDK imported from the submodule through `PYTHONPATH`, and ends with exit status 0 and its four checks passing
- [ ] No file is written under `upstream/claude-agent-sdk-python` (`PYTHONDONTWRITEBYTECODE=1`)
- [ ] The Windows row of the Platforms table of `docs/validation/python-sdk-against-fake-claude.md` records the Python version, the command, and what was observed
- [ ] Given the run fails, when the validation is recorded, then the cause is written down and US-057 is cancelled or rescoped, with the gap listed in Open Questions

#### US-057: Differential job on Windows
**Description:** As an SDK maintainer, I want the differential harness to run on Windows in CI so that its Windows code paths (process tree kill, `\\?\` prefixes, path separators) are exercised and a Windows-only divergence between the SDKs is caught.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-056

**Python reference** (root `C:/dev/claude-agent-sdk-python` on Windows, `/home/arthur/dev/claude-agent-sdk-python` on Fedora, commit `4720d36`):
- `src/claude_agent_sdk/_internal/transport/subprocess_cli.py`: 429-449, 759-836
- `.github/workflows/test.yml`: 10-14

**Acceptance Criteria:**
- [ ] The `differential` job of `ci.yml` runs on `ubuntu-latest` and `windows-latest`, with the steps of US-052 on each; the Windows venv interpreter is `Scripts\python.exe`
- [ ] The job keeps `contents: read` as its only permission, reads no secret, and the `lockfile` job passes
- [ ] The policy test `ci_runs_the_differential_harness_on_every_pull_request_without_secrets` checks both operating systems
- [ ] `cargo xtask parity-diff` passes on `windows-latest`; every difference only Windows shows is fixed in the crate or entered in `parity/differences.toml`, with a case restricted to that platform when the entry does not apply on Linux
- [ ] A driver timeout on Windows kills the driver and every `fake-claude` process it started, shown by a run that leaves none
- [ ] The Linux leg still finishes in under 5 minutes

## Functional Requirements

- FR-01: The crate must validate the arguments of a raw-schema tool against its JSON Schema before calling the handler, in a build with default features.
- FR-02: The crate must offer a build without that validation, through `default-features = false`.
- FR-03: `Error` must expose `is_process()` and `is_cli_connection()`, which mirror the Python exception hierarchy.
- FR-04: When the credentials file of a resume cannot be written, the crate must return an error, remove the temporary directory, and start no CLI process.
- FR-05: Wherever the Python SDK passes a dictionary from the CLI through untouched, the crate must NOT drop a key of it.
- FR-06: `fake-claude` must record its arguments, selected environment variables, and every stdin line to a file when `FAKE_CLAUDE_RECORD` is set, and must NOT change behavior when it is not set.
- FR-07: `cargo xtask parity-diff` must run each case through both SDKs and fail on any difference not listed in `parity/differences.toml`.
- FR-08: `cargo xtask parity-diff` must fail when an entry of `parity/differences.toml` matches no difference.
- FR-09: `cargo xtask parity-diff` must NOT report a match when a driver did not run.
- FR-10: The ledgers must accept the `adapted` status and report it apart from `implemented`.
- FR-11: `docs/parity.md` must be generated from the ledgers, and `parity-check` must fail when it is stale.
- FR-12: A scheduled workflow must open at most one issue per upstream release tag that is newer than the pin.
- FR-13: No tool or workflow of this PRD may post to the upstream repository.

## Non-Functional Requirements

- **Performance:** `cargo xtask parity-diff` over the full corpus finishes in under 120 seconds on the CI Linux runner, with a 30-second limit per driver run. A tool's validator is compiled once per server, 0 times per call.
- **Security:** the differential job has 1 permission (`contents: read`) and reads 0 secrets. The upstream watch grants `issues: write` to 1 job. 100% of Python packages installed in CI carry a hash. The default feature set adds 0 crates with network capability (`reqwest`, `rustls` absent from the dependency tree).
- **Reliability:** 20 consecutive runs of the differential corpus give identical results. The upstream watch opens at most 1 open issue per upstream tag. A missing Python interpreter produces exit status non-zero in 100% of runs.
- **Scalability:** the case format and both drivers handle at least 100 cases without a change of design; adding a case is 1 new file and 0 code changes when it uses supported options.
- **Maintainability:** line coverage stays at or above 91.5%. Each normalization rule and each ledger entry covers 1 location.
- **Accessibility:** 100% of the tables of `docs/parity.md` are plain Markdown with a header row, and the page uses 0 images and 0 HTML, so it reads without rendering.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Schema with a remote reference | A raw tool schema holds `$ref` to a URL | The call returns an error result; no network request | "Input validation error: ..." naming the reference |
| 2 | Validation switched off | Build with `--no-default-features` | The handler receives the arguments unchecked | None; stated in rustdoc |
| 3 | Unwritable credentials target | Read-only temporary directory during a resume | Error returned, directory removed, no CLI started | Message naming the credentials file and the I/O cause |
| 4 | Unknown key at a nested level | The CLI adds a key inside a `modelUsage` entry | The key is reachable from the parsed value | None |
| 5 | Python missing | `cargo xtask parity-diff` on a machine without Python 3.10 or later | Exit non-zero, no case reported as a match | "parity-diff: python 3.10 or later not found" |
| 6 | Unsupported option in a case | A case sets an option one driver does not implement | The case is reported unsupported and the run fails | The case name and the option name |
| 7 | Stale difference entry | Upstream or the crate changed, an entry matches nothing | Exit non-zero | The entry's location and reason |
| 8 | Frames in either order | Two writers race on the fake CLI's stdin | Only the group declared `unordered` is compared as a set | None |
| 9 | Driver hangs | A driver does not exit within 30 seconds | The driver and its children are killed; the case reports a timeout | The case name and the side that timed out |
| 10 | Submodule not checked out | `parity-diff` run without `git submodule update` | Exit non-zero before any case runs | "upstream submodule is empty: run git submodule update --init" |
| 11 | Upstream tag lookup fails | Network error in the scheduled workflow | The run fails; no "up to date" result | The failing command in the job log |
| 12 | Issue already open | The watch runs twice for one tag | No second issue | None |
| 13 | Upstream fixes a reported defect | A submodule bump removes a difference | The `python-defect` entry becomes stale and fails until removed | The stale entry |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | The differential suite becomes flaky and gets ignored | Med | High | Scenarios serialize exchanges with `expect`; nondeterminism is pinned or narrowly normalized; the 20-run criterion; unordered comparison only for declared groups |
| 2 | The ledger of differences grows into a rubber stamp | Med | Med | One location per entry, a category and a reason required, stale entries fail, `python-defect` entries must cite the upstream line at fault |
| 3 | Two drivers double the maintenance at each upstream change | Med | Med | The Python driver uses only the public API; the case format reuses Python field names; coverage reports new fields automatically |
| 4 | Validation by default breaks a user whose tool relied on unchecked arguments | Low | Med | Documented opt-out; the change is stated in `docs/migration.md` and the rustdoc; the error text matches Python's prefix |
| 5 | A Python toolchain in CI widens the supply chain | Low | High | Hash-locked requirements, the SDK read from the pinned submodule, no secret in the job, one read-only permission |
| 6 | The hash-locked dependency versions drift from what upstream tests | Med | Low | The job checks the locked versions against the ranges in the submodule's `pyproject.toml`; `mcp` 2.x is declared out of scope |
| 7 | The scheduled workflow is abused or misfires | Low | Med | No fork-triggered event, one job with `issues: write`, idempotent title check, fail-closed lookup, offline script test |
| 8 | The Python SDK cannot drive `fake-claude` on Windows | Low | Med | US-047 validates on Linux before the drivers are built and the CI job of EP-009 runs on Linux; US-056 validates Windows, and US-057 runs the harness there |
| 9 | Line references go stale when upstream moves | High | Low | Every reference is tied to commit `4720d36`; the upstream watch flags new releases |

## Non-Goals

What this version explicitly does NOT include:

- Matching the wording of Python's error and warning messages. About 25 divergences are text only; they are documented on `docs/parity.md`, not changed.
- Any contribution to the Python SDK: no report, patch, or pull request is prepared or filed upstream. A Python defect the port finds is recorded as a `python-defect` entry of `parity/differences.toml`, and the crate matches the CLI there.
- A TypeScript driver. The case format is language-neutral so that one can be added, but it is not part of this PRD.
- Re-auditing the 933 test mappings by hand. The differential harness is the mechanical answer; only the assertions the audit found weaker are restored.
- Running the real-CLI suite in CI on pull requests, or any paid run without the owner's go.
- Coverage of `mcp` 2.x in the differential job.
- The derived client-application label, the Postgres, Redis, and S3 store adapters, and publishing to crates.io: non-goals of the first PRD, unchanged.
- Asking Anthropic about the billing of the `sdk-rs` entrypoint label. The owner deferred it on 2026-11-15; it stays in the Open Questions of `tasks/prd-claude-agent-rust-sdk.md`.

## Files NOT to Modify

- `upstream/claude-agent-sdk-python/**`: read-only behavioral reference, changed only by moving the submodule pin.
- `C:/dev/claude-agent-sdk-python/**` (Windows) and `/home/arthur/dev/claude-agent-sdk-python/**` (Fedora): the original checkout stays untouched; nothing is installed into it and no file is written under it.
- `.github/workflows/actions.lock`: changed only through `gh actions-lock`.
- `Cargo.lock`: changed only by cargo.
- The generated regions of `docs/migration.md` and, once it exists, `docs/parity.md`: changed only by their xtask commands.
- `docs/research/*.md` and `docs/validation/project-dir-naming.md`: dated records; new findings go into new files.
- The `[workspace.lints.clippy]` block of the root `Cargo.toml` and `clippy.toml`.
- `tasks/prd-claude-agent-rust-sdk.md` and its status file: the first PRD is certified; this PRD has its own files.
- The existing exit codes (64, 65, 66) and init-frame keys of `crates/fake-claude`: existing tests rely on them.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** where should the Rust driver live? Recommended: a workspace binary crate with `publish = false`, next to `fake-claude`, so it can depend on the crate with its real feature set. Alternative: an example of `claude-agent-kit`. Engineering to confirm the effect on the coverage job, which already ignores `fake-claude` and `xtask`.
- **Data Model:** should unknown keys be kept in a flattened `extra` map, as `HookInputBase` does (`src/types/hook.rs:83-84`), or in a `raw` copy, as `RateLimitInfo` does (`src/types/message.rs:966`)? Trade-off: `extra` serializes back without duplication; `raw` is simpler for the hand-written parser in `src/message_parser.rs:417-435`. Recommended: `extra`, for symmetry on serialization.
- **API Design:** are `is_process()` and `is_cli_connection()` the right names, or should they follow the Python class names (`is_process_error()`)? Recommended: the short forms, consistent with `exit_code()`. Should a third predicate cover the whole SDK error family, or is the enum itself that answer?
- **Dependencies:** `jsonschema` 0.61 is already an optional workspace dependency with its default features off. Does making it a default feature change the minimal-versions or license jobs? No new Rust dependency is planned for the harness: `insta` was considered for redactions and set aside because it compares against a stored snapshot. Python side: `anyio`, `sniffio`, `mcp` 1.x, `jsonschema`, locked with hashes. Which exact `mcp` 1.x version: the floor upstream tests (`1.23.0`) or the newest 1.x?
- **Recording channel:** an environment variable (`FAKE_CLAUDE_RECORD`) or a key of the scenario file? Recommended: the variable, because both drivers already set `FAKE_CLAUDE_SCENARIO` and the scenario file stays identical for both sides.
- **Frame ordering:** is a declared `unordered` group enough, or do some cases need a per-channel order (control responses apart from user messages)? Engineering to decide from the first racing case found in US-050.
- **Platforms:** should the differential job also run on Windows and macOS? Decided: Linux in EP-009, Windows in EP-011 (US-057); macOS is not planned. Path normalization is designed for all three.
- **Migration:** validation by default is a behavior change for raw-schema tools. No release has been published, so no deprecation path is needed; the change is recorded in `docs/migration.md`. Rollback: remove `jsonschema` from `default`.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Risky divergences open (of 5) | 5 | 0 | Month-1 | The tests of EP-007 pass; audit sections 4.4 and 6 rechecked |
| Differential cases passing | 0 | 11 or more | Month-1 | `cargo xtask parity-diff` |
| Option fields covered or excluded with a reason (of 50) | 0 | 50 | Month-6 | `cargo xtask parity-diff --coverage` |
| Stale entries in the differences ledger | N/A (new) | 0 | Month-1 | `cargo xtask parity-diff` exit status |
| Surface entries classified implemented or adapted by a written rule (of 159) | 159 reported implemented, 1 known misclassification | 159 classified | Month-1 | `cargo xtask parity-report` |
| Audit divergences listed on `docs/parity.md` or closed | 0 of about 87 | 100% | Month-6 | The "not yet classified" count on the page reaches 0 |
| Delay from an upstream release to a tracking issue | Manual, unbounded | 7 days or less | Month-1 | Issue creation date against the release date |

## Open Questions

- Does the owner accept a `github-actions[bot]` issue from the upstream watch, given the rule that he is the only visible contributor? This PRD assumes yes, because an issue adds no commit author. Owner to confirm before US-055.
- Predicate names (`is_process` or `is_process_error`): owner to confirm before US-037 ships, since renaming later breaks users.
- Should raw-schema validation also be on for the `rmcp` bridge, where the external server owns its tools? Engineering to answer during US-036.
- Which `mcp` 1.x version does the differential job lock? Engineering to answer during US-052.
- Should the differential harness be offered to Anthropic as a shared conformance suite once it holds a TypeScript driver? Owner to decide after EP-009; nothing in this PRD depends on it.
- A real-CLI run of `cargo xtask e2e` is needed to confirm US-041 against the API. Owner to schedule it; the story does not block on it.
- US-047 was validated on Linux only (`docs/validation/python-sdk-against-fake-claude.md`). The Windows run moved to US-056 of EP-011; nothing in EP-009 or EP-010 depends on it.
[/PRD]
