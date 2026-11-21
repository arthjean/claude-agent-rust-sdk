# How calls carrying the `sdk-rs` label were billed

Date of the observation: 2026-11-15. Recorded: 2026-11-21, when the
statement left the README. CLI: 2.1.325. Authentication: an API key.

## Observation

Calls made through the crate, which identifies itself to the CLI as
`sdk-rs`, were not paid by the monthly API credit of an Ultra plan: they were
billed to the purchased credit balance of the Console organization.

## Context

`entrypoint-label.md` shows that CLI 2.1.329 does not know the `sdk-rs`
label: its SDK switch lists `sdk-ts`, `sdk-py`, and `sdk-cli`. The crate
keeps `sdk-rs` and never sends `sdk-py` in its place (`[wire.entrypoint-label]`
in `parity/differences.toml`). This observation is evidence for prerequisite
A1 of the second parity audit: asking Anthropic to add `sdk-rs` to the CLI.
