# Validation: project directory naming matches the CLI on Linux

Date: 2026-11-21. CLI: `2.1.329 (Claude Code)`, native Linux x64 build.
Crate: 0.2.0 on branch `parity-audit-2`. Follows
`docs/validation/project-dir-naming.md`, which covered Windows only.

## Method

The ignored test `project_directory_naming_matches_the_cli_without_calling_the_api`
in `crates/claude-agent-rust-sdk/tests/project_dir_naming.rs` runs the real CLI
once per working directory with an emptied environment, an empty configuration
directory and home, a placeholder API key, and `ANTHROPIC_BASE_URL` on a closed
local port. The CLI creates the project directory and the transcript of the
prompt before it calls the API, so the name it chose is observed and no API call
succeeds; the CLI is stopped once the transcript exists.

```text
CLAUDE_AGENT_RUST_SDK_TEST_CLI=~/.local/share/claude/versions/2.1.329 \
  cargo test -p claude-agent-rust-sdk --all-features --test project_dir_naming \
  project_directory_naming_matches_the_cli_without_calling_the_api -- --ignored --nocapture
```

The variable names the executable because the `claude` found on `PATH` may be a
launcher that needs the full environment.

## Results

| Case | Path length (UTF-16 units) | Match with `project_key_for_directory` |
|------|---------------------------|----------------------------------------|
| ASCII under 200 units | 61 | exact |
| Character outside the Basic Multilingual Plane (`😀`) | 64 | exact (`emoji----case`, two hyphens) |
| Precomposed accented character (`é`) | 60 | exact (`caf--case`) |
| Over 200 units | 255 | exact (200-unit prefix, `-`, hash `ilgmlq`) |

## Conclusions

- On Linux, as on Windows, the CLI counts UTF-16 units and its hash suffix is
  the one the crate computes, so `[behavior]` entries of category
  `rust-follows-cli` on project directory names hold on both systems.
- `é` is one UTF-16 unit, so it becomes one hyphen on both sides.
- Not covered: macOS, where file names may come back decomposed (NFD); the NFC
  step is unit-tested only.
