# Validation: the Python SDK runs against `fake-claude`

Date: 2026-11-16. Story: US-047 of `tasks/prd-parity-hardening.md`.

## Question

Does the unmodified Python SDK complete a session against `fake-claude` when `cli_path` points at the binary, so that the differential harness can drive both SDKs against the same stand-in?

## Setup

| Item | Value |
|---|---|
| Operating system | Linux 7.3.3 (Fedora 45), x86_64 |
| Python | CPython 3.13.12, in a virtual environment outside both repositories |
| Python SDK | `claude-agent-sdk` 0.2.182, commit `4720d36`, imported from `upstream/claude-agent-sdk-python/src` through `PYTHONPATH`, not installed |
| Dependencies | `anyio` 4.16.0, `sniffio` 1.3.1, `mcp` 1.30.0, `jsonschema` 4.26.0, `typing-extensions` 4.16.0, and what they pull in, as pinned with hashes in `parity/requirements.txt` |
| Stand-in | `fake-claude` built from this repository at the commit of this file |
| Script | `docs/validation/python-sdk-against-fake-claude.py` |

The dependencies were first installed with `uv pip install "anyio>=4.0.0" "sniffio>=1.0.0" "mcp>=1.23.0,<2" "jsonschema>=4.20.0"`, the ranges of `pyproject.toml:27-33` with `mcp` held to the 1.x line, and the run was repeated with `pip install --require-hashes -r parity/requirements.txt`, which pins the same versions.

## Command

```sh
cargo build -p fake-claude
env -i PATH=/usr/bin:/bin HOME="$HOME" PYTHONDONTWRITEBYTECODE=1 \
    PYTHONPATH="$PWD/upstream/claude-agent-sdk-python/src" \
    "$VENV/bin/python" docs/validation/python-sdk-against-fake-claude.py "$PWD/target/debug/fake-claude"
```

The script writes a scenario that answers the initialize request, waits for the user message, and emits one assistant message and one result. It sets `FAKE_CLAUDE_SCENARIO` and `FAKE_CLAUDE_RECORD`, calls `query(prompt="Say hello", options=ClaudeAgentOptions(cli_path=binary))`, and checks the recording.

## Observed

Exit status 0. The stream yielded `SystemMessage` (the init frame), `AssistantMessage`, and `ResultMessage`. The recording, with the process id and the random suffix of the request id replaced:

```text
{"args": ["--output-format", "stream-json", "--verbose", "--system-prompt", "", "--input-format", "stream-json"], "cwd": "/home/arthur/dev/claude-agent-sdk-rust", "env": {"CLAUDE_AGENT_SDK_VERSION": "0.2.182", "CLAUDE_CODE_ENTRYPOINT": "sdk-py", "CLAUDE_CODE_SDK_READS_SESSION_STATE": "1"}, "pid": N, "record": "spawn"}
{"frame": {"request": {"hooks": null, "subtype": "initialize"}, "request_id": "req_1_XXXXXXXX", "type": "control_request"}, "pid": N, "record": "stdin"}
{"frame": {"message": {"content": "Say hello", "role": "user"}, "parent_tool_use_id": null, "session_id": "", "type": "user"}, "pid": N, "record": "stdin"}
{"pid": N, "record": "stdin_eof"}
```

Every check passed: the stream ended with a result, the recording starts with a `spawn` record, and it holds an `initialize` control request and a user message frame. The SDK ran `cli_path` as given, did not run `cli_path -v`, and read the version from the init frame, as `subprocess_cli.py:251-252` and `:1153-1167` lead to expect.

## Findings for the harness

- **The inherited environment must be cleaned.** A first run from inside a Claude Code session recorded a dozen inherited `CLAUDE_*` variables, among them a session messaging token. The harness starts each driver with an environment it builds itself: `PATH`, the home directory, the variables the operating system needs, and the variables of the case; no inherited `CLAUDE_*` or `ANTHROPIC_*` variable.
- `PWD` is set by the Python SDK only when `cwd` is set (`subprocess_cli.py:822-823`), so it is absent from this recording.
- The script must not be run with `python -I`: isolated mode ignores `PYTHONPATH`.
- **Python must not write bytecode into the submodule.** A first run left `__pycache__` directories under `upstream/claude-agent-sdk-python/src`, which is read-only by policy; they were removed. The command above and the harness set `PYTHONDONTWRITEBYTECODE=1`.

## Platforms

| Platform | Result |
|---|---|
| Linux | Works, as above. |
| Windows | Not run: this validation was made on the Fedora machine. `subprocess_cli.py:394-423` refuses only `.bat` and `.cmd` paths, and `fake-claude.exe` is neither, so no failure is expected. The Windows run is US-056 of EP-011 in `tasks/prd-parity-hardening.md`, which records it in this row. The differential CI job of EP-009 runs on Linux only, so US-048 is not scoped down. |
