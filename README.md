# claude-agent-kit

A Rust SDK for building agents on top of the Claude Code CLI.

The crate starts the `claude` executable as a child process and talks to it over
newline-delimited JSON. You get typed messages, hooks, tool-permission
callbacks, tools served from your own process, session stores, and session
management, all on tokio.

It is a port of the Python Claude Agent SDK, at parity with version 0.2.182.
[`docs/migration.md`](docs/migration.md) maps every Python name to its Rust
name, and [`docs/parity.md`](docs/parity.md) lists what was compared, the
measured counts, and every known difference with its reason and its test.

> **This is not an Anthropic product.** `claude-agent-kit` is an independent,
> community project. It is not affiliated with, endorsed by, or supported by
> Anthropic. "Claude" and "Claude Code" are trademarks of Anthropic, PBC.

## Requirements

- Rust 1.88 or newer, with tokio as the async runtime.
- The Claude Code CLI installed on the machine that runs your program. The
  crate does not bundle it. Version 2.0.0 is the minimum; the crate is tested
  against 2.1.325.
- Linux, macOS, or Windows.

## Authentication

The crate never asks for, stores, or forwards credentials. The CLI
authenticates on its own, with what the person running your program has set up:
the login of their own CLI installation, or their own API key in
`ANTHROPIC_API_KEY`.

**End users authenticate with their own credentials.** An application built on
this crate must not offer a claude.ai login on behalf of its users, and the
crate provides no way to do so.

**Plan API credits may not apply.** The crate identifies itself to the CLI as
`sdk-rs`. When tested on 2026-11-15 with CLI 2.1.325 and an API key, calls
carrying that label were not paid by the monthly API credit of an Ultra plan:
they were billed to the purchased credit balance of the Console organization.
Budget for API usage accordingly.

## Installation

The crate is not published on crates.io yet. Depend on it from git:

```toml
[dependencies]
claude-agent-kit = { git = "https://github.com/arthjean/claude-agent-sdk-rust" }
futures-util = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Quick start

`query` runs one prompt and returns the stream of its messages:

```rust,no_run
use claude_agent_kit::{ClaudeAgentOptions, Message, query};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), claude_agent_kit::Error> {
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
use claude_agent_kit::{ClaudeAgentOptions, ClaudeSdkClient, Message};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), claude_agent_kit::Error> {
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
use claude_agent_kit::{
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
async fn main() -> Result<(), claude_agent_kit::Error> {
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

`jsonschema` is on by default; the other features are off by default.

| Feature | What it adds |
|---|---|
| `jsonschema` | On by default. Validation of tool arguments against the JSON Schema of the tool, as the Python SDK does. Set `default-features = false` to turn it off. |
| `macros` | The `#[tool]` attribute, which turns an async function into a tool. |
| `rmcp` | Serving a server built with the `rmcp` crate inside your process. |
| `otel` | OpenTelemetry trace propagation: the spans of the CLI join the trace that is active when the session starts. |
| `testing` | A scriptable transport for your own tests. |

## Examples

Each file under [`crates/claude-agent-kit/examples`](crates/claude-agent-kit/examples)
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

Run one with `cargo run -p claude-agent-kit --example quick_start`. The examples
call the real API with your credentials.

## Development

```text
cargo xtask gates                  format, lint, examples, test, and doc checks
cargo xtask parity-check           check the parity ledgers against upstream
cargo xtask parity-report          print how much of upstream is implemented
cargo xtask parity-page            regenerate the measured region of docs/parity.md
cargo xtask upstream-diff <tag>    list what upstream changed up to a tag
cargo xtask parity-diff            compare what both SDKs send the CLI, case by case
cargo xtask e2e                    run the end-to-end suite against the real CLI
```

The parity ledgers live in [`parity/`](parity): `surface.toml` lists every name
the Python SDK exports, and `tests.toml` lists every upstream test, each with the
Rust item that replaces it or the reason it is not ported.

`cargo xtask parity-diff` measures parity instead of stating it: it plays every
case of `parity/cases` through both SDKs against the scripted stand-in CLI and
fails on any difference in the command line, the environment, or the frames
that `parity/differences.toml` does not list with its reason. It needs Python
3.10 or later with the pinned dependencies of `parity/requirements.txt`; see
[`parity/README.md`](parity/README.md).

The end-to-end suite spends API credits: about 0.80 USD for a full run. It
needs `ANTHROPIC_API_KEY`, or `CLAUDE_AGENT_KIT_E2E_AUTH=cli` to use the login
of your installed CLI. In CI it never starts on its own: run the `End-to-end`
workflow by hand when a change needs it.

## License

`claude-agent-kit` is licensed under the GNU General Public License, version 3
or later (GPL-3.0-or-later). See [`LICENSE`](LICENSE).

Rust links crates statically, so a program that depends on this crate is a
combined work under the GPL. **If you distribute a program that links
`claude-agent-kit`, you must release that program under the GPL-3.0-or-later or
a license compatible with it, and make its source available.** Programs you run
only for yourself or inside your organization, without distributing them, are
not affected.

The crate is derived from the Python Claude Agent SDK, which is MIT licensed.
Its notice is reproduced in [`THIRD_PARTY_NOTICES`](THIRD_PARTY_NOTICES).
