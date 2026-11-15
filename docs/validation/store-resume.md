# Validation: resume from a store against the real CLI

Epic: EP-005 (definition of done). Date: 2026-11-15. CLI: `2.1.325 (Claude Code)`, native Windows build, Windows 11.

## Question

Can a session that exists only in a `SessionStore` be resumed by the real CLI, with the transcript first copied to the store by the live mirror?

## Method

The ignored test `crates/claude-agent-kit/tests/store_resume_e2e.rs` runs the real CLI twice through `query()`:

```text
CLAUDE_AGENT_KIT_E2E=1 cargo test --test store_resume_e2e -- --ignored --nocapture
```

1. A first query with an `InMemorySessionStore` set asks the model (`haiku`, one turn) to remember a code word. The CLI runs with `--session-mirror`, and the batcher copies the transcript to the store.
2. The local project directory under `projects/` is removed, so the store holds the only copy.
3. A second query with the same store and `resume` set to the session id asks for the code word. The session is materialized into a `claude-resume-*` directory and the child runs with `CLAUDE_CONFIG_DIR` pointing there.

## Results

| Check | Observed |
|-------|----------|
| Entries mirrored by the first turn | 32 to 33 per run, including the user turn |
| Session id of the resumed turn | same as the first turn |
| Reply of the resumed turn | the code word (`PELICAN-4417`) |
| Second turn mirrored to the store | yes, the entry count grew |
| `claude-resume-*` directories left behind | none |

## Keychain on macOS

On macOS, with no config directory, API key, or OAuth token set, the credentials come from the Keychain through `security find-generic-password`. The upstream SDK only stubs this reader in its unit tests (`tests/test_session_resume.py:217-246`), and its end-to-end jobs authenticate through workload identity federation, so the Keychain branch is never taken there.

Here the macOS job of `.github/workflows/ci.yml` creates a throwaway keychain holding a `Claude Code-credentials` item for `$USER` with fake tokens, then runs the ignored test `keychain_credentials_are_copied_without_the_refresh_token` of `crates/claude-agent-kit/tests/session_resume.rs`. The test resumes through `query()` against the fake CLI and checks that the child's `.credentials.json` holds the Keychain access token, without the refresh token, with mode `0o600`. It proves the real `security` call on a real Keychain. It does not prove that the CLI names its item `Claude Code-credentials`; that name is taken from upstream.

## Definition of done: surface ledger

The definition of done asks that every session function exported by Python has a Rust counterpart in the surface ledger. The ledger file (`parity/surface.toml`) is delivered by US-032 in EP-006. Decision of 2026-11-15: for EP-005 the clause is met by checking the exports directly. The 39 session-related names of upstream `__all__` (`src/claude_agent_sdk/__init__.py:648-824`), and `run_session_store_conformance` of `claude_agent_sdk.testing`, each have a public Rust counterpart exported from `crates/claude-agent-kit/src/lib.rs` or `claude_agent_kit::testing`. US-032 records them in the ledger.

## Not covered

- Linux: not run.
