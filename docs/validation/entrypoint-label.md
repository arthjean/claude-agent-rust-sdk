# What CLI 2.1.329 does with the entrypoint label

Date: 2026-11-21. CLI: 2.1.329, the native Linux x64 build
(`~/.local/share/claude/versions/2.1.329`). Crate: 0.2.0 on branch
`parity-audit-2`.

The crate sets `CLAUDE_CODE_ENTRYPOINT=sdk-rs`; the Python SDK sets `sdk-py`.
This record measures what the CLI does with each label, without calling the
API.

## Reading the binary

`sdk-rs` occurs nowhere in the binary. The label is compared against fixed
sets:

- the telemetry table of known entrypoints holds `sdk-cli`, `sdk-ts`, and
  `sdk-py`; an unknown label is reported as another client;
- the "is this an SDK session" test is
  `e==="sdk-ts"||e==="sdk-py"||e==="sdk-cli"`: false for `sdk-rs`, which
  switches on behavior of the interactive CLI;
- the client platform sent with API requests maps `sdk-cli`, `sdk-ts`, and
  `sdk-py` to `claude_code_sdk`; any other label falls back to the default of
  the interactive CLI;
- the built-in agents gain `claude-code-guide` when the label is none of
  `sdk-ts`, `sdk-py`, `sdk-cli`;
- one set of request attributes is chosen for `sdk-ts` and `sdk-py` only,
  so `sdk-cli` and `sdk-rs` both differ from Python there;
- a session the SDK starts with no label at all is labeled by the CLI
  itself: `sdk-cli` when it runs non-interactively (`--print`, which the
  SDKs use), `cli` otherwise.

## Measuring the `init` message offline

The CLI was run four times with `-p --output-format stream-json --verbose`,
an empty configuration directory and home, a placeholder API key, and
`ANTHROPIC_BASE_URL=http://127.0.0.1:9`, a closed port: the CLI writes its
`init` message and then fails to reach the API, so nothing is spent. The
`agents` list of the `init` message:

| `CLAUDE_CODE_ENTRYPOINT` | `claude-code-guide` listed |
|---|---|
| `sdk-py` | no |
| `sdk-rs` | yes |
| `sdk-cli` | no |
| unset | no |

The other agents were the same in all four runs.

## Consequence

A session of this crate differs from a Python session on every switch above:
the agents and the description of the Agent tool the model sees, the client
platform of API requests, the behavior reserved for SDK sessions, and
telemetry. Only the CLI can make `sdk-rs` a known label (prerequisite A1 of
the audit). Sending `sdk-py` would report Rust sessions as Python ones and is
ruled out. `sdk-cli`, the label the CLI gives itself under an SDK, matches
Python on all but the request attributes and telemetry; the choice is in
`tasks/parity-audit-2-decisions.md`.
