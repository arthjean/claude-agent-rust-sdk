# Validation: project directory naming matches the CLI

Story: US-023. Date: 2026-11-15. CLI: `2.1.325 (Claude Code)`, native Windows build, Windows 11.

## Question

The CLI stores the transcripts of a working directory under `projects/<name>`, where `<name>` is the directory path with every character outside `[a-zA-Z0-9]` replaced by `-`, cut at 200 units and suffixed with a hash when longer. The upstream Python SDK counts Unicode code points (`src/claude_agent_sdk/_internal/sessions.py:84-114`); the CLI runs on JavaScript, whose strings are UTF-16 code units. `claude-agent-kit` counts UTF-16 code units in one function (`code_units` in `crates/claude-agent-kit/src/sessions/paths.rs`). Does its `project_key_for_directory` give the name the CLI creates?

## Method

The ignored test `crates/claude-agent-kit/tests/project_dir_naming.rs` runs the real CLI once per case:

```text
CLAUDE_AGENT_KIT_E2E=1 cargo test --test project_dir_naming -- --ignored --nocapture
```

For each working directory it runs `claude -p "Reply with the single word OK." --max-turns 1 --model haiku`, lists the directories that appeared under `projects/`, compares them with `project_key_for_directory`, checks that `list_sessions` finds the session, and removes the directories it created.

## Results

| Case | Path length (UTF-16 units) | Directory created by the CLI | `project_key_for_directory` | Match |
|------|---------------------------|------------------------------|-----------------------------|-------|
| ASCII under 200 units | 56 | `C--Users-Arthur-AppData-Local-Temp--tmpqNYwoX-ascii-case` | same | exact |
| Character outside the Basic Multilingual Plane (`😀`, U+1F600) | 59 | `C--Users-Arthur-AppData-Local-Temp--tmpqNYwoX-emoji----case` | same | exact |
| Over 200 units | 250 | `C--Users-...-nested-directory-segment-u9687a` (200-unit prefix, `-`, hash) | same | exact |

`list_sessions` found the session in all three cases. The names observed are pinned by the unit test `names_match_the_directories_created_by_cli_2_1_325` in `crates/claude-agent-kit/src/sessions/paths.rs`, which runs on every platform without the CLI.

## Conclusions

- Counting UTF-16 code units is correct: `😀` is two units, so the CLI writes two hyphens for it (`emoji----case`).
- The Python reference diverges from the CLI on that case. Run against the same path, `_sanitize_path` returns `C--Users-Arthur-AppData-Local-Temp--tmpqNYwoX-emoji---case` (one hyphen for the character), so Python does not find sessions of a directory whose path holds a character outside the Basic Multilingual Plane. `claude-agent-kit` deliberately differs from Python here; the test ledger of US-032 must record this divergence for the Python tests of `_sanitize_path`.
- For the 250-unit path the hash suffix of the native CLI equals the `h = h * 31 + unit` hash, so the exact lookup succeeds; the prefix fallback remains for builds whose hash differs (the comment at `sessions.py:157-184` notes `Bun.hash` builds).
- Not covered: macOS (NFD file names, normalized to NFC before sanitizing) and Linux. The NFC step is unit-tested; a run of this experiment on those systems is the remaining proof.
