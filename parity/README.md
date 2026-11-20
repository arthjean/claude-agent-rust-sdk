# Parity

This directory records how `claude-agent-rust-sdk` matches the Python SDK it ports, `claude-agent-sdk` at the commit pinned by the submodule `upstream/claude-agent-sdk-python`.

| File | What it holds | Checked by |
|---|---|---|
| `surface.toml` | Every upstream export and its Rust counterpart | `cargo xtask parity-check` |
| `tests.toml` | Every upstream test and the Rust test that stands for it | `cargo xtask parity-check` |
| `cases/*.json` | Cases of the differential harness | `cargo xtask parity-diff` |
| `cases/coverage.toml` | Option fields no case can set, with the reason | `cargo xtask parity-diff --coverage` |
| `scenarios/*.json` | `fake-claude` scenarios shared by several cases | `cargo xtask parity-diff` |
| `case.schema.json` | JSON Schema of a case | `cargo xtask parity-diff` |
| `differences.toml` | Reviewed differences on the wire (`[wire]`), in what the application observes (`[observed]`), and in behavior (`[behavior]`), and the audit's divergences not yet reviewed (`[unclassified]`) | `cargo xtask parity-diff` (wire, observed), `cargo xtask parity-check` (behavior tests, and the page they render into) |
| `drivers/python_driver.py` | The Python driver | `cargo xtask parity-diff` |
| `requirements.txt` | The Python driver's dependencies, pinned with hashes | `cargo xtask parity-diff --check-requirements` |

## The differential harness

The ledgers state parity; the harness measures it. `cargo xtask parity-diff` plays every case through both SDKs against `fake-claude`, the scripted stand-in for the CLI, and compares both directions: what the stand-in received from each SDK (the command line, the environment, and every frame written to its stdin), and what each SDK handed the application (every message, the return value or error of every call, and the arguments of every callback).

1. `fake-claude` records, when `FAKE_CLAUDE_RECORD` names a file, one JSON object per line: a `spawn` record (arguments, working directory, and every environment variable but those of the list below), then one record per stdin line and one at the end of stdin. The format is documented in `crates/fake-claude/src/record.rs`.
2. Two drivers read the same case. The Rust driver is the workspace binary `crates/parity-driver` (never published). The Python driver is `drivers/python_driver.py`; it imports the SDK from the submodule through `PYTHONPATH` and uses only its public API. Each driver writes a [journal](#the-journal) of what the application observed to the file `PARITY_JOURNAL` names.
3. The harness runs each driver with an environment it builds from scratch (only what the operating system needs, a fresh home and temporary directory inside the run's working directory, and the case's `process_env`), so nothing of the caller's environment reaches the recording. A driver that does not finish within 30 seconds is killed with its children and the case reports a timeout.
4. The two recordings and the two journals are compared after normalization, and every difference must match an entry of `differences.toml`.

The recording leaves out only these variables, which the operating system or the harness puts in every driver's process and no SDK chooses: `PATH`, `HOME`, `USERPROFILE`, `TMPDIR`, `TEMP`, `TMP`, `LANG`, `LC_ALL`, `SYSTEMROOT`, `SYSTEMDRIVE`, `WINDIR`, `COMSPEC`, `PATHEXT`, `__CF_USER_TEXT_ENCODING` (macOS), `PYTHONPATH` and `PYTHONDONTWRITEBYTECODE` (set for the Python driver only), `PARITY_JOURNAL`, and the `FAKE_CLAUDE_*` variables that drive the stand-in. Any other variable an SDK passes on, adds, or fails to remove (such as `CLAUDECODE`) is compared.

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
| `cargo xtask parity-diff --coverage` | Lists every option field with the cases that set it and those in which its effect is observable; fails on a field neither set nor excluded, and on a field set but observable in no case and not listed as unobservable. It plays every case several times, so it needs Python too and takes a few minutes |
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
| `expect_error` | `true` when a call of the SDK is expected to fail: the error is journaled and ends the case, which passes. Without it, an SDK error fails the case |

Every string may hold `${WORK}`, replaced by the run's working directory on each side.

Callbacks cannot travel as data, so a case declares canned ones, and each journals the arguments it received before it answers.

- `can_use_tool` is a decision: `{"behavior": "allow", "updated_input": {...}, "updated_permissions": ...}`, where `updated_permissions` is absent, a list of permission updates in their wire form, or `"suggestions"` to send back as updates the suggestions the callback received (so a case checks both how the SDK parses suggestions and how it writes updates); `{"behavior": "deny", "message": "...", "interrupt": false}`; or `{"behavior": "wait"}`, which never answers, so that the CLI can cancel the request.
- `hooks` maps an event to matchers whose `hooks` list holds hook outputs in their wire form (`continue`, `async`, `hookSpecificOutput`). A hook callback is named `<event>/<matcher index>/<hook index>` in the journal.
- An `mcp_servers` entry of type `sdk` declares an in-process server whose tools return fixed content: `{"type": "sdk", "version": "1.0.0", "tools": [{"name", "description", "input_schema", "annotations", "result": {"content": [...], "is_error": false}}]}`.
- `stderr: true` registers a callback that journals each line, and `session_store: "memory"` uses an in-memory store that journals each append.
- `cli_path` is set by the drivers.

Steps of the interactive client, one action per step: `connect` (`{}` or `{"prompt": ...}`), `query` (`{"prompt": ..., "session_id": "..."}`), `receive_response` (reads until a result), `interrupt`, `set_permission_mode`, `set_model`, `rewind_files`, `reconnect_mcp_server`, `toggle_mcp_server` (`{"name": ..., "enabled": ...}`), `stop_task`, `get_mcp_status`, `get_context_usage`, `get_server_info`, and `disconnect`. The one-shot query and `receive_response` require a result among the messages they read.

A driver exits with status 3 and names the item when a case uses an option, a step, or a value it does not implement; the case is then reported `unsupported`.

The scenario decides what the stand-in answers and when. To keep a recording deterministic, a scenario waits with `expect` for each frame it answers, so that each SDK writes its frames in an order the scenario fixes. When two frames can still arrive in either order, the case lists their labels in an `unordered` group.

Adding a case that uses supported options is one new file and no code change.

## Comparison

Comparison is done on parsed values, never on raw text.

- **Arguments** are read as flags: `--name value`, `--name=value`, or a bare `--name`. The two value forms are kept apart (the equals form under the key `--name=`), because the CLI binds a dash-leading value only in the equals form. Flags are compared by name, not by position, since the CLI does not read them by position; a repeated flag is compared occurrence by occurrence. A value that parses as a JSON object or array is compared as JSON.
- **Environment**: each recorded variable, by name. **Working directory**: as a path.
- **Frames**: each stdin frame gets a label, `control_request:<subtype>`, `control_response:<request_id>`, the frame `type` otherwise, `raw` for a line that is not JSON, and `eof` for the end of stdin, with `#2`, `#3` for a label seen again. Frames are paired by label and compared as JSON; then the order of the labels both sides share is compared, except inside the `unordered` groups of the case, which are compared as sets.
- **JSON** is compared the way the CLI reads it, two numbers being equal when their values are (`5` and `5.0`), and then on the order of the keys both sides share, reported at `<pointer> (key order)`: the order of an output schema's properties reaches the model, and the order of MCP servers or subagents reaches the CLI. Stdin frames are read from the recorded line text, which keeps the order.
- A recording of several processes is compared process by process.

- **The journal** is compared as described in [The journal](#the-journal).

A difference is printed with its location and both values; `<absent>` means that side sent nothing there. Locations are `processes`, `cwd`, `env <NAME>`, `arg <flag>` (with `[i]` for a later occurrence and a JSON pointer into a JSON value), `frame <label>` (with a JSON pointer into the frame), and `frames` for the order; locations of a process after the first start with `process[i] `. Locations in the journal start with `journal `.

### Normalization

Before comparison, the harness replaces these values and nothing else.

| Rule | Replacement | Reason |
|---|---|---|
| Random suffix of a control request id | `req_<n>_<8 hex digits>` becomes `req_<n>_<random>` | Both SDKs draw 8 random hex digits per request (`_internal/query.py:746-749`); the counter is kept and compared. |
| Temporary directory | The run's working directory, in each spelling (as created, canonical), becomes `<work>` | Each side runs in its own temporary directory. |
| CLI binary | The path of `fake-claude` becomes `<cli>` | The path depends on the build directory. |
| Path separators | In a string where one of the two replacements above applied, `\` becomes `/` | Windows paths use backslashes; the rule is limited to strings known to be paths. |

The journal has rules of its own, listed in [The journal](#the-journal).

## The journal

Each driver appends one JSON object per line to the file `PARITY_JOURNAL` names, in the order it observes them. The format is the same for both drivers; each object is an event whose `event` key names its kind.

| `event` | Written when | Fields |
|---|---|---|
| `message` | The application receives a message, from the one-shot query or `receive_response` | `message`, the message; for a result of a case with a session store, `store_appends`, the number of appends the store had received when the result reached the application |
| `call` | A call returns or raises | `step`, the index of the step (`null` for the one-shot query); `method`, the step's action or `query`; `result`, the canonical return value (`null` for a method that returns nothing), or `error`, with `kind`, `exit_code` (of a process error), and `message` |
| `can_use_tool` | The permission callback is called | `tool_name`, `input`, and `context`, the `ToolPermissionContext` with its parsed `suggestions` |
| `hook` | A hook callback is called | `callback`, its name; `input`, the hook input; `tool_use_id`; `context` |
| `tool` | A tool of an in-process MCP server is called | `server`, `tool`, and `arguments` |
| `stderr` | The stderr callback receives a line | `line` |
| `store_append` | The SDK appends to the session store | `key` and `entries` |

Values are written in a canonical form that both drivers produce. An instance of an SDK type becomes an object with `"$type"`, the name of the Python class (`AssistantMessage`, `TextBlock`, `PermissionUpdate`, `ToolPermissionContext`, `ResultMessage`, ...), and one key per field of that class, under its Python name. The Python driver walks the dataclass fields; the Rust driver maps each Rust type onto its Python class by hand (`crates/parity-driver/src/journal.rs`), so a field the Rust type drops, renames, or parses differently shows up as a difference. A value the SDK passes on as raw JSON (a hook input, a `data` frame, a tool input, a `TypedDict` such as `McpStatusResponse` or `SessionKey`) is written as the dictionary Python returns, and as the JSON the Rust type serializes back to. A Rust message or content block of a type the crate does not know is `{"$type": "Unknown", "data": ...}`.

An error is named by its kind, the same on both sides:

| `kind` | Python | Rust `ErrorKind` |
|---|---|---|
| `cli_not_found` | `CLINotFoundError` | `CliNotFound` |
| `cli_connection` | `CLIConnectionError` | `Spawn`, `NotConnected`, `ConnectionClosed` |
| `process` | `ProcessError` | `Process` |
| `result_error` | `ResultError` | `ResultError` |
| `json_decode` | `CLIJSONDecodeError` | `JsonDecode` |
| `message_parse` | `MessageParseError` | `MessageParse` |
| `control` | `Exception`, which the control protocol raises for an error answer or a timeout | `Control`, `ControlTimeout` |

Any other error is `python:<class>` or `rust:<kind>`, which never match each other.

Events are grouped by kind, and kinds are not ordered against each other, since a callback runs while messages are read. Messages are paired like stdin frames: each gets a label, its `$type` followed by `:<subtype>` when it has one (`SystemMessage:init`, `ResultMessage:success`), with `#2`, `#3` for a label seen again; messages are paired by label and compared, and the order of the labels both sides share is compared at `journal messages`. So a message one SDK drops and the other keeps is one difference, at `journal message <label>`. Events of every other kind are compared in the order they were written, at `journal <kind>[<index>]`. A JSON pointer into the event follows the location.

Before comparison, each journal event goes through the normalization of the recording, then these rules, and nothing else:

| Rule | Replacement | Reason |
|---|---|---|
| Null and absent | A key whose value is `null` is removed, at any depth; a `null` inside an array stays | Python gives every field of a dataclass a value, `None` when unset, where Rust leaves an absent `Option` out. The two mean the same to the application. |
| Key order | Keys are sorted | The Rust driver builds each canonical object from typed fields, in an order of its own; key order is compared on the wire, where it reaches the CLI. |
| Error text | `message` is removed from an `error` | The wording of an error is not matched (a non-goal of `tasks/prd-parity-hardening.md`); its kind and exit code are. |
| Stand-in keys | `fake_args`, `fake_env`, `fake_pid`, and `fake_console_window` are removed from the `data` of the `init` system message | Only `fake-claude` writes them: the arguments and variables, which the recording compares, and its process id, which changes with every run. |

## The ledger of differences

`differences.toml` lists the differences that were reviewed and accepted, one entry per location: in the `[wire]` table for what the CLI receives, and in the `[observed]` table, with the same shape, for what the application observes, at a location that starts with `journal `.

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
- `category` is `deliberate` (the port chose to differ), `rust-follows-cli` (Rust matches what the CLI does where Python does not), `rust-more-robust` (Rust keeps going, or keeps the data, where Python fails or drops it), `rust-stricter` (Rust refuses input Python accepts), or `python-defect` (Python is wrong; Rust matches the CLI). A `python-defect` entry must cite the upstream line at fault as `ref = "path:line"` or `ref = "path:first-last"`, relative to the submodule, with an `anchor`: text those lines hold verbatim. Loading fails when the lines do not exist or no longer hold the anchor, so a ref that drifts onto unrelated code when the pin moves is caught. Nothing is reported upstream.
- `reason` explains why the difference is accepted.
- A value that moves with every release is written as a token, so no release edits the ledger: `{crate_version}` is the workspace version of the root `Cargo.toml`, and `{upstream_version}` the version of the submodule's `pyproject.toml`. Both are replaced before the file is read, by the comparison and by the parity page alike.

An entry matches a difference when its case, location, and both values agree. A difference no entry matches fails the run; so does an entry that matched nothing in a full run, which keeps the ledger honest when the crate or upstream changes. A new entry needs the same review as a code change: fix the crate first when the difference is a bug.

### Differences in behavior

The `[behavior]` table holds the divergences the harness cannot see, because neither the recording nor the journal shows them, such as where the child process is placed or what an SDK does with input no case sends:

```toml
[behavior.separate-process-group]
summary = "Where the CLI process is placed"
python = "The CLI is spawned in the process group of the program."
rust = "On Unix the CLI leads its own process group."
ref = "src/claude_agent_sdk/_internal/transport/subprocess_cli.py:828-836"
category = "deliberate"
reason = "A group of its own lets the SDK end the CLI and every process it started in one signal."
test = "crates/claude-agent-rust-sdk/tests/shutdown.rs::the_process_leads_its_own_process_group @unix"
```

- `python` and `rust` are prose here: what each SDK does.
- `ref` and `anchor` are required and checked like those of a `python-defect` entry.
- `test` is a Rust test as `cargo xtask parity-tests` prints it, with the platform suffix of `tests.toml` (` @unix` or ` @linux+windows`, for example) when it is compiled on some platforms only. `cargo xtask parity-check` fails when it does not exist.

The `[unclassified]` table names, with `source` (the section of `docs/research/python-sdk-parity.md`) and `summary`, the divergences of the audit that are not reviewed into `[wire]`, `[observed]`, or `[behavior]` yet. Move an entry out of it once a test or a case shows it, and move a `[behavior]` entry to `[wire]` or `[observed]` once a case shows it.

## Upstream tests rewritten in place

`cargo xtask upstream-diff <tag>` (and `upstream-sync`, which prints it) lists, under `Unit tests changed in place`, each upstream test that keeps its name but whose code or strings changed, with the Rust tests its `tests.toml` entry names after `review:`. The ledger only checks names, so review each listed Rust test against the new upstream test before the port is done.

`crates/claude-agent-rust-sdk/tests/fixtures/fold_session_summary.json` holds what the upstream `fold_session_summary` returns for its batches. `fold_session_summary.py` next to it regenerates it from the pinned submodule, and `--check` fails when it is stale; run it after moving the pin.

## The parity page

`cargo xtask parity-page` writes the region between `<!-- parity:page:begin -->` and `<!-- parity:page:end -->` of `docs/parity.md`: the upstream version, commit, and commit date (read from the submodule with `git`), the counts of both ledgers, the number of cases, of option fields they set, and of fields set but listed as unobservable, one table per category of `differences.toml`, the exclusions of both ledgers, and the `[unclassified]` list with its count. `--check` only compares, and `cargo xtask parity-check` runs that comparison, so CI fails on a stale page. The text outside the region is written by hand; the check also fails when it claims "100%" or "full parity", which only the generated counts may state.

## Coverage of the options

`cargo xtask parity-diff --coverage` reads the fields of `ClaudeAgentOptions` from the submodule's `types.py` and lists, for each, the cases that set it. A field no case sets must be listed in the `[excluded]` table of `cases/coverage.toml` with the reason no case can set it. The command fails on a field neither set nor excluded (such as a field a submodule bump adds), on an exclusion of a field a case now sets, and on a name that is not a field.

Setting a field is not enough: a case covers a field only when the field's effect reaches the recording or the journal, otherwise an SDK could ignore it and the case would still match. The command measures it directly. It plays each case twice per side as written, which must give the same recording and journal (a case that does not is reported, since the measure would mean nothing), then once per side for each field the case sets, with that field removed. The field is observable in the case when, on either side, the run without it differs from the run with it, or no longer completes. Each field is listed as `observable in <cases>`, followed by `set without effect in <cases>` for the cases where removing it changes nothing (`max_thinking_tokens` next to a `thinking` option, for example, which overrides it). A field set by cases but observable in none fails, unless the `[unobservable]` table of `cases/coverage.toml` lists it with the reason no case can observe it; an entry there fails once a case observes the field, and an entry for a field no case sets belongs in `[excluded]` instead. The runs stay in `target/parity-diff/coverage`. The parity page counts the fields the cases set, which needs no run.

## Python dependencies

`requirements.txt` pins every package the Python driver imports, with hashes, for `pip install --require-hashes`. It is compiled from `requirements.in`, which states the upstream ranges with `mcp` held to the 1.x line (the 2.x line is not covered):

```sh
uv pip compile parity/requirements.in --generate-hashes --python-version 3.13 -o parity/requirements.txt
```

`cargo xtask parity-diff --check-requirements` fails when a runtime dependency of the submodule's `pyproject.toml` is not pinned, is pinned outside the range upstream allows, or carries no hash.
