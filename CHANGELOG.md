# Changelog

## Unreleased

### Added

- The first release of `claude-agent-rust-sdk`, a Rust port of the Python Claude Agent SDK that runs the Claude Code CLI as a child process and speaks its JSON control protocol over stdio.
- One-shot queries with `query`, and multi-turn sessions with `ClaudeSdkClient`: interrupts, prompt streams, model and permission changes, and typed messages that keep unknown wire data instead of dropping it.
- Hooks, tool-permission callbacks, subagent definitions, system prompt variants, budget limits, partial message streaming, and the CLI's error output through a callback.
- Tools served from your process, written by hand, with the `#[tool]` attribute (feature `macros`), or with an `rmcp` server (feature `rmcp`), with tool arguments validated against their JSON Schema (feature `jsonschema`, on by default).
- Session listing, reading, renaming, tagging, forking, and deletion on the CLI's own disk format, and session stores that resume a conversation from your own storage.
- OpenTelemetry trace propagation into the CLI (feature `otel`), and a scriptable transport for your own tests (feature `testing`, outside the compatibility promise).
- Linux, macOS, and Windows support, with the child process placed in its own process group or Job Object.
