# Security policy

## Supported versions

Fixes ship in the newest release only. Before `1.0`, that is the newest
`0.y.z`: a fix is released as the next patch of the newest minor version,
and older minor versions receive none.

## Reporting a vulnerability

Report a vulnerability through GitHub private vulnerability reporting:
[open a report](https://github.com/arthjean/claude-agent-rust-sdk/security/advisories/new).
Do not open a public issue for it.

A report is acknowledged within 7 days. The fix, its release, and the
advisory are coordinated with you in the report.

## What is not in scope

This crate runs the Claude Code CLI, which it does not bundle. A defect of
the CLI itself, or of the Claude API, goes to Anthropic through its
[security policy](https://github.com/anthropics/claude-code/security/policy),
not here. Report here what this crate does wrong: how it starts the CLI,
what it sends, what it accepts back, and what it writes to disk.
