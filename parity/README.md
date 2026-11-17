# Parity

This directory records how `claude-agent-kit` matches the Python SDK it ports, `claude-agent-sdk` at the commit pinned by the submodule `upstream/claude-agent-sdk-python`.

| File | What it holds | Checked by |
|---|---|---|
| `surface.toml` | Every upstream export and its Rust counterpart | `cargo xtask parity-check` |
| `tests.toml` | Every upstream test and the Rust test that stands for it | `cargo xtask parity-check` |
| `cases/*.json` | Cases of the differential harness | `cargo xtask parity-diff` |
| `cases/coverage.toml` | Option fields no case can set, with the reason | `cargo xtask parity-diff --coverage` |
| `scenarios/*.json` | `fake-claude` scenarios shared by several cases | `cargo xtask parity-diff` |
| `case.schema.json` | JSON Schema of a case | `cargo xtask parity-diff` |
| `differences.toml` | Reviewed differences on the wire (`[wire]`) and in behavior (`[behavior]`), and the audit's divergences not yet reviewed (`[unclassified]`) | `cargo xtask parity-diff` (wire), `cargo xtask parity-check` (behavior tests, and the page they render into) |
| `drivers/python_driver.py` | The Python driver | `cargo xtask parity-diff` |
| `requirements.txt` | The Python driver's dependencies, pinned with hashes | `cargo xtask parity-diff --check-requirements` |

## The differential harness

The ledgers state parity; the harness measures it. `cargo xtask parity-diff` plays every case through both SDKs against `fake-claude`, the scripted stand-in for the CLI, and compares what the stand-in received from each: the command line, the environment, and every frame written to its stdin.

1. `fake-claude` records, when `FAKE_CLAUDE_RECORD` names a file, one JSON object per line: a `spawn` record (arguments, working directory, the variables named `CLAUDE_*` or `ANTHROPIC_*`, `PWD`, `MAX_THINKING_TOKENS`, `TRACEPARENT`, `TRACESTATE`), then one record per stdin line and one at the end of stdin. The format is documented in `crates/fake-claude/src/record.rs`.
2. Two drivers read the same case. The Rust driver is the workspace binary `crates/parity-driver` (never published). The Python driver is `drivers/python_driver.py`; it imports the SDK from the submodule through `PYTHONPATH` and uses only its public API.
3. The harness runs each driver with an environment it builds from scratch (only what the operating system needs, a fresh home and temporary directory inside the run's working directory, and the case's `process_env`), so nothing of the caller's environment reaches the recording. A driver that does not finish within 30 seconds is killed with its children and the case reports a timeout.
4. The two recordings are compared after normalization, and every difference must match an entry of `differences.toml`.

Each case prints one line: `match`, `differ` (with each unexplained difference), `unsupported` (a driver does not implement something the case uses), `timeout`, or `failed`. Anything but `match` fails the run, so does a stale ledger entry, and so does a missing Python or Python dependency: the harness never reports a match for a side that did not run. The recordings of the last run stay in `target/parity-diff`.

### Running it

The harness needs Python 3.10 or later with the dependencies of `requirements.txt`, installed outside both repositories. The SDK itself is not installed.

```sh
git submodule update --init
python3 -m venv ~/.venvs/parity
~/.venvs/parity/bin/pip install --require-hashes -r parity/requirements.txt
PARITY_PYTHON=~/.venvs/parity/bin/python cargo xtask parity-diff
```

| Command | Effect |
|---|---|
| `cargo xtask parity-diff` | Runs every case; fails on any unexplained difference or stale entry |
| `cargo xtask parity-diff <case>...` | Runs the named cases only; stale entries are not checked |
| `cargo xtask parity-diff --repeat 20` | Runs the corpus 20 times and fails unless every run gives the same results |
| `cargo xtask parity-diff --coverage` | Lists every option field with the cases that set it; fails on a field neither set nor excluded |
| `cargo xtask parity-diff --check-requirements` | Fails when `requirements.txt` and the submodule's `pyproject.toml` disagree on a dependency |

`--python <path>` (or `PARITY_PYTHON`) picks the interpreter; the default tries `python3`, then `python`. `--timeout <seconds>` changes the limit per driver run.

## Cases

A case is one JSON file of `cases/`, named after the case, and checked against `case.schema.json`.

| Key | Meaning |
|---|---|
| `name` | The case name, equal to the file name without `.json` |
| `description` | What the case exercises |
| `scenario` | The `fake-claude` scenario, inline, or the name of a file of `scenarios/` |
| `options` | Options keyed by the field names of the Python `ClaudeAgentOptions`, in their Python shapes |
| `prompt` | For a one-shot `query`: a string, or a list of user messages sent as a stream |
| `steps` | For the interactive client instead: the steps played in order |
| `process_env` | Variables of the driver process itself, which the SDK inherits |
| `unordered` | Groups of frame labels whose order is not compared |

Every string may hold `${WORK}`, replaced by the run's working directory on each side.

Callbacks cannot travel as data, so a case declares canned ones. `can_use_tool` is a decision (`{"behavior": "allow", "updated_input": {...}}` or `{"behavior": "deny", "message": "...", "interrupt": false}`). `hooks` maps an event to matchers whose `hooks` list holds hook outputs in their wire form (`continue`, `async`, `hookSpecificOutput`). An `mcp_servers` entry of type `sdk` declares an in-process server whose tools return fixed content: `{"type": "sdk", "version": "1.0.0", "tools": [{"name", "description", "input_schema", "annotations", "result": {"content": [...], "is_error": false}}]}`. `stderr: true` registers a callback that discards each line, and `session_store: "memory"` uses an in-memory store. `cli_path` is set by the drivers.

Steps of the interactive client, one action per step: `connect` (`{}` or `{"prompt": ...}`), `query` (`{"prompt": ..., "session_id": "..."}`), `receive_response` (reads until a result), `interrupt`, `set_permission_mode`, `set_model`, `rewind_files`, `reconnect_mcp_server`, `toggle_mcp_server` (`{"name": ..., "enabled": ...}`), `stop_task`, `get_mcp_status`, `get_context_usage`, and `disconnect`.

A driver exits with status 3 and names the item when a case uses an option, a step, or a value it does not implement; the case is then reported `unsupported`.

The scenario decides what the stand-in answers and when. To keep a recording deterministic, a scenario waits with `expect` for each frame it answers, so that each SDK writes its frames in an order the scenario fixes. When two frames can still arrive in either order, the case lists their labels in an `unordered` group.

Adding a case that uses supported options is one new file and no code change.

## Comparison

Comparison is done on parsed values, never on raw text.

- **Arguments** are read as flags: `--name value`, `--name=value`, or a bare `--name`. The two value forms are kept apart (the equals form under the key `--name=`), because the CLI binds a dash-leading value only in the equals form. Flags are compared by name, not by position, since the CLI does not read them by position; a repeated flag is compared occurrence by occurrence. A value that parses as a JSON object or array is compared as JSON.
- **Environment**: each recorded variable, by name. **Working directory**: as a path.
- **Frames**: each stdin frame gets a label, `control_request:<subtype>`, `control_response:<request_id>`, the frame `type` otherwise, `raw` for a line that is not JSON, and `eof` for the end of stdin, with `#2`, `#3` for a label seen again. Frames are paired by label and compared as JSON; then the order of the labels both sides share is compared, except inside the `unordered` groups of the case, which are compared as sets.
- **JSON** is compared the way the CLI reads it: object key order never matters, and two numbers are equal when their values are (`5` and `5.0`).
- A recording of several processes is compared process by process.

A difference is printed with its location and both values; `<absent>` means that side sent nothing there. Locations are `processes`, `cwd`, `env <NAME>`, `arg <flag>` (with `[i]` for a later occurrence and a JSON pointer into a JSON value), `frame <label>` (with a JSON pointer into the frame), and `frames` for the order; locations of a process after the first start with `process[i] `.

### Normalization

Before comparison, the harness replaces these values and nothing else.

| Rule | Replacement | Reason |
|---|---|---|
| Random suffix of a control request id | `req_<n>_<8 hex digits>` becomes `req_<n>_<random>` | Both SDKs draw 8 random hex digits per request (`_internal/query.py:746-749`); the counter is kept and compared. |
| Temporary directory | The run's working directory, in each spelling (as created, canonical), becomes `<work>` | Each side runs in its own temporary directory. |
| CLI binary | The path of `fake-claude` becomes `<cli>` | The path depends on the build directory. |
| Path separators | In a string where one of the two replacements above applied, `\` becomes `/` | Windows paths use backslashes; the rule is limited to strings known to be paths. |

## The ledger of differences

`differences.toml` lists the differences that were reviewed and accepted, in the `[wire]` table, one entry per location:

```toml
[wire.entrypoint-label]
case = "*"
location = "env CLAUDE_CODE_ENTRYPOINT"
python = '"sdk-py"'
rust = '"sdk-rs"'
category = "deliberate"
reason = "Each SDK names itself to the CLI."
```

- `case` is a case name, or `*` for every case.
- `python` and `rust` are JSON text; a missing key means that side sent nothing there.
- `category` is `deliberate` (the port chose to differ), `rust-follows-cli` (Rust matches what the CLI does where Python does not), `rust-more-robust` (Rust keeps going, or keeps the data, where Python fails or drops it), `rust-stricter` (Rust refuses input Python accepts), or `python-defect` (Python is wrong; Rust matches the CLI). A `python-defect` entry must cite the upstream line at fault as `ref = "path:line"` or `ref = "path:first-last"`, relative to the submodule, and loading fails when those lines do not exist. Nothing is reported upstream.
- `reason` explains why the difference is accepted.

An entry matches a difference when its case, location, and both values agree. A difference no entry matches fails the run; so does an entry that matched nothing in a full run, which keeps the ledger honest when the crate or upstream changes. A new entry needs the same review as a code change: fix the crate first when the difference is a bug.

### Differences in behavior

The `[behavior]` table holds the divergences the harness cannot see because they are not on the wire, such as how an inbound message is parsed or where the child process is placed:

```toml
[behavior.unknown-messages-kept]
summary = "An unknown message type"
python = "The parser returns `None` and the message is skipped."
rust = "Kept as `Message::Unknown` with its raw JSON."
ref = "src/claude_agent_sdk/_internal/message_parser.py:397-401"
category = "rust-more-robust"
reason = "A newer CLI must not make an older program lose data."
test = "crates/claude-agent-kit/tests/message_parser.rs::parse_unknown_message_type_is_kept_as_raw_json"
```

- `python` and `rust` are prose here: what each SDK does.
- `ref` is required and checked like the one of a `python-defect` entry.
- `test` is a Rust test as `cargo xtask parity-tests` prints it, with the platform suffix of `tests.toml` (` @unix`, for example) when it is compiled on one platform only. `cargo xtask parity-check` fails when it does not exist.

The `[unclassified]` table names, with `source` (the section of `docs/research/python-sdk-parity.md`) and `summary`, the divergences of the audit that are not reviewed into `[wire]` or `[behavior]` yet. Move an entry out of it once a test or a case shows it.

## The parity page

`cargo xtask parity-page` writes the region between `<!-- parity:page:begin -->` and `<!-- parity:page:end -->` of `docs/parity.md`: the upstream version, commit, and commit date (read from the submodule with `git`), the counts of both ledgers, the number of cases and of option fields they cover, one table per category of `differences.toml`, the exclusions of both ledgers, and the `[unclassified]` list with its count. `--check` only compares, and `cargo xtask parity-check` runs that comparison, so CI fails on a stale page. The text outside the region is written by hand; the check also fails when it claims "100%" or "full parity", which only the generated counts may state.

## Coverage of the options

`cargo xtask parity-diff --coverage` reads the fields of `ClaudeAgentOptions` from the submodule's `types.py` and lists, for each, the cases that set it. A field no case sets must be listed in the `[excluded]` table of `cases/coverage.toml` with the reason no case can set it. The command fails on a field neither set nor excluded (such as a field a submodule bump adds), on an exclusion of a field a case now sets, and on a name that is not a field.

## Python dependencies

`requirements.txt` pins every package the Python driver imports, with hashes, for `pip install --require-hashes`. It is compiled from `requirements.in`, which states the upstream ranges with `mcp` held to the 1.x line (the 2.x line is not covered):

```sh
uv pip compile parity/requirements.in --generate-hashes --python-version 3.13 -o parity/requirements.txt
```

`cargo xtask parity-diff --check-requirements` fails when a runtime dependency of the submodule's `pyproject.toml` is not pinned, is pinned outside the range upstream allows, or carries no hash.
