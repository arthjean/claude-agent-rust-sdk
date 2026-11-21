# claude-agent-rust-sdk

[![crates.io](https://img.shields.io/crates/v/claude-agent-rust-sdk.svg)](https://crates.io/crates/claude-agent-rust-sdk)
[![docs.rs](https://docs.rs/claude-agent-rust-sdk/badge.svg)](https://docs.rs/claude-agent-rust-sdk)
[![CI](https://github.com/arthjean/claude-agent-rust-sdk/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/arthjean/claude-agent-rust-sdk/actions/workflows/ci.yml)

A Rust SDK for building agents on top of the Claude Code CLI.

The crate starts the `claude` executable as a child process and talks to it over
newline-delimited JSON. You get typed messages, hooks, tool-permission
callbacks, tools served from your own process, session stores, and session
management, all on tokio.

It is a port of the Python Claude Agent SDK, version 0.2.186.
[`docs/migration.md`](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/docs/migration.md) maps every Python name to its Rust
name, and [`docs/parity.md`](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/docs/parity.md) lists what was compared, the
measured counts, each reviewed difference with its reason and its test, and the
differences found but not yet classified.

> **This is not an Anthropic product.** `claude-agent-rust-sdk` is an independent,
> community project. It is not affiliated with, endorsed by, or supported by
> Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic, PBC.

## Requirements

- Rust 1.88 or newer, with tokio as the async runtime.
- The Claude Code CLI installed on the machine that runs your program. The
  crate does not bundle it. Version 2.0.0 is the minimum; the crate is tested
  against 2.1.329 and logs a warning when the CLI reports another version.
- Linux, macOS, or Windows.

## Authentication

The crate never asks for credentials and sends them nowhere but to the CLI it
runs. The CLI authenticates on its own, with what the person running your
program has set up: the login of their own CLI installation, or their own API
key in `ANTHROPIC_API_KEY`.

One path handles that login, as the Python SDK does: resuming a session from a
session store runs the CLI under a temporary configuration directory, and the
crate copies the login of the installed CLI into it. It copies the access token
without the refresh token, into a directory that on Unix only the user can
read, removed when the session ends.

**End users authenticate with their own credentials.** An application built on
this crate must not offer a claude.ai login on behalf of its users, and the
crate provides no way to do so.

**Plan API credits may not apply.** The crate identifies itself to the CLI as
`sdk-rs`. When tested on 2026-11-15 with CLI 2.1.325 and an API key, calls
carrying that label were not paid by the monthly API credit of an Ultra plan:
they were billed to the purchased credit balance of the Console organization.
Budget for API usage accordingly.

## Installation

```text
cargo add claude-agent-rust-sdk
```

Or in `Cargo.toml`, with the crates the examples below also use (`serde` and
`schemars` only for the typed tool):

```toml
[dependencies]
claude-agent-rust-sdk = "0.2"
futures-util = "0.3"
schemars = "1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Quick start

`query` runs one prompt and returns the stream of its messages:

```rust,no_run
use claude_agent_rust_sdk::{ClaudeAgentOptions, Message, query};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), claude_agent_rust_sdk::Error> {
    let options = ClaudeAgentOptions::builder().max_turns(1).build()?;
    let mut messages = query("What is 2 + 2?", options);
    while let Some(message) = messages.next().await {
        if let Message::Result(result) = message? {
            println!("{}", result.result.unwrap_or_default());
        }
    }
    Ok(())
}
```

## A conversation

`ClaudeSdkClient` keeps a session open for several turns:

```rust,no_run
use claude_agent_rust_sdk::{ClaudeAgentOptions, ClaudeSdkClient, Message};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), claude_agent_rust_sdk::Error> {
    let options = ClaudeAgentOptions::builder()
        .allowed_tools(["Read", "Grep"])
        .build()?;
    let client = ClaudeSdkClient::new(options);
    client.connect().await?;

    for prompt in ["Summarize src/lib.rs.", "Which function is the entry point?"] {
        client.query(prompt).await?;
        let mut response = client.receive_response();
        while let Some(message) = response.next().await {
            if let Message::Assistant(assistant) = message? {
                for block in &assistant.content {
                    if let Some(text) = block.as_text() {
                        println!("{text}");
                    }
                }
            }
        }
    }

    client.disconnect().await
}
```

## Tools served from your process

A tool is a Rust function. The CLI calls it over the control channel, so there
is no separate MCP server process to run:

```rust,no_run
use claude_agent_rust_sdk::{
    ClaudeAgentOptions, Message, SdkMcpTool, ToolResult, create_sdk_mcp_server, query,
};
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
struct Operands {
    a: f64,
    b: f64,
}

#[tokio::main]
async fn main() -> Result<(), claude_agent_rust_sdk::Error> {
    let add = SdkMcpTool::typed::<Operands>("add", "Add two numbers", |args| async move {
        Ok(ToolResult::text(format!("{}", args.a + args.b)))
    });
    let options = ClaudeAgentOptions::builder()
        .mcp_server("calc", create_sdk_mcp_server("calculator", Some("1.0.0"), [add])?)
        .allowed_tools(["mcp__calc__add"])
        .build()?;

    let mut messages = query("What is 19 + 23? Use the add tool.", options);
    while let Some(message) = messages.next().await {
        if let Message::Result(result) = message? {
            println!("{}", result.result.unwrap_or_default());
        }
    }
    Ok(())
}
```

## Cargo features

`jsonschema` and `preserve-order` are on by default; the other features are off by default.

| Feature | What it adds |
|---|---|
| `jsonschema` | On by default. Validation of tool arguments against the JSON Schema of the tool, as the Python SDK does. Set `default-features = false` to turn it off. |
| `preserve-order` | On by default. JSON objects keep the order their keys were written in, as Python dictionaries do, so tool schemas and copied transcripts keep their key order. Cargo applies it to every `serde_json::Map` of your program: turn it off with `default-features = false` if another part needs sorted maps. |
| `macros` | The `#[tool]` attribute, which turns an async function into a tool. |
| `rmcp` | Serving a server built with the `rmcp` crate inside your process. |
| `otel` | OpenTelemetry trace propagation: the spans of the CLI join the trace that is active when the session starts. |
| `testing` | A scriptable transport for your own tests. Outside the compatibility promise (see below). |

## Examples

Each file under [`crates/claude-agent-rust-sdk/examples`](https://github.com/arthjean/claude-agent-rust-sdk/tree/main/crates/claude-agent-rust-sdk/examples)
is a runnable program:

| Example | Shows |
|---|---|
| `quick_start` | One-shot queries with `query`. |
| `interactive_client` | Multi-turn sessions, interrupts, and prompt streams with `ClaudeSdkClient`. |
| `hooks` | Hooks that observe, block, and steer tool calls. |
| `tool_permission_callback` | Approving, changing, and denying tool calls from a callback. |
| `mcp_calculator` | Tools served from your process. |
| `agents` | Subagent definitions. |
| `system_prompt` | The system prompt variants. |
| `include_partial_messages` | Streaming partial messages. |
| `max_budget_usd` | A spending cap for a run. |
| `stderr_callback` | Receiving the error output of the CLI. |
| `setting_sources` | Choosing which settings files the CLI loads. |
| `session_store_adapter` | Writing a session store and resuming from it. |

Run one with `cargo run -p claude-agent-rust-sdk --example quick_start`. The examples
call the real API with your credentials.

## Versioning and compatibility

The crate follows semantic versioning as Cargo applies it before `1.0`:

- In `0.y.z`, a change of the minor version `y` may break your build. A patch
  release (a change of `z`) never does.
- Raising the minimum supported Rust version, 1.88 today, is a minor change.
- The `testing` feature is outside the compatibility promise: its items may
  change in a patch release.
- Each release states the version of the Python SDK and of the CLI it matches.
  The crate version is independent of both.

[`CHANGELOG.md`](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/CHANGELOG.md) lists what each release changed.

## Development

```text
cargo xtask gates                  format, lint, examples, test, and doc checks
cargo xtask package-check          package both crates and check what the archives hold
cargo xtask parity-check           check the parity ledgers against upstream
cargo xtask parity-report          print how much of upstream is implemented
cargo xtask parity-page            regenerate the measured region of docs/parity.md
cargo xtask upstream-diff <tag>    list what upstream changed up to a tag, with its verdict
cargo xtask upstream-sync <tag>    move the upstream pin and every copy of the pinned versions
cargo xtask release prepare <bump> move the crate version and write the changelog section
cargo xtask release tag            run the checks, then create the release commit and tag
cargo xtask changelog-section <v>  print the changelog section of a version
cargo xtask semver-check           compare the public API with the newest release on crates.io
cargo xtask parity-diff            compare what both SDKs send the CLI and return, case by case
cargo xtask e2e                    run the end-to-end suite against the real CLI
```

The parity ledgers live in [`parity/`](https://github.com/arthjean/claude-agent-rust-sdk/tree/main/parity): `surface.toml` lists every name
the Python SDK exports, and `tests.toml` lists every upstream unit test (under
`tests/`), each with the Rust item that replaces it or the reason it is not
ported. The upstream end-to-end tests (under `e2e-tests/`) are not listed;
`crates/claude-agent-rust-sdk/tests/e2e` holds the Rust suite against the real CLI.

`cargo xtask parity-diff` measures parity instead of stating it: it plays every
case of `parity/cases` through both SDKs against the scripted stand-in CLI and
fails on any difference that `parity/differences.toml` does not list with its
reason, both in what the CLI receives (the command line, the environment, the
frames) and in what the application receives (the messages, the return values,
the arguments of the callbacks). It needs Python
3.10 or later with the pinned dependencies of `parity/requirements.txt`; see
[`parity/README.md`](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/parity/README.md).

Once a day, the `Upstream watch` workflow compares the newest release of the
Python SDK with the submodule pin and, when the release is newer, opens one
issue with the verdict (`cli-only` when only versions moved, `needs-port`
otherwise), the `cargo xtask upstream-sync <tag>` command that starts the port,
what `cargo xtask upstream-diff` lists, and the release's changelog.

A pushed `v*` tag publishes both crates through crates.io trusted publishing;
[`RELEASING.md`](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/RELEASING.md)
gives the procedure and the recovery from each failure, and
[`SECURITY.md`](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/SECURITY.md)
says how to report a vulnerability.

The end-to-end suite spends API credits: about 0.80 USD for a full run. It
needs `ANTHROPIC_API_KEY`, or `CLAUDE_AGENT_RUST_SDK_E2E_AUTH=cli` to use the login
of your installed CLI. In CI it never starts on its own: run the `End-to-end`
workflow by hand when a change needs it.

## License

`claude-agent-rust-sdk` is licensed under either of the
[MIT license](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/LICENSE-MIT) or the
[Apache License, Version 2.0](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/LICENSE-APACHE), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.

The crate is derived from the Python Claude Agent SDK, which is MIT licensed.
Its notice is reproduced in [`THIRD_PARTY_NOTICES`](https://github.com/arthjean/claude-agent-rust-sdk/blob/main/THIRD_PARTY_NOTICES).
