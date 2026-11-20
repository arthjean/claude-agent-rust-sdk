"""Regenerates the `outputs` of fold_session_summary.json from its `batches`.

The outputs are what the upstream `fold_session_summary` returns when it
folds each batch into the summary of the previous one, for the key
`proj`/`sess`, with the `mtime` a store would stamp (1000 plus the batch
index), as `tests/session_store_helpers.rs` replays them.

Run it from the repository root with the Python of `parity/README.md`:

    $PARITY_PYTHON crates/claude-agent-rust-sdk/tests/fixtures/fold_session_summary.py
    $PARITY_PYTHON crates/claude-agent-rust-sdk/tests/fixtures/fold_session_summary.py --check

It imports the SDK from the pinned submodule, so moving the pin and running
it again shows any change in the upstream fold. `--check` exits with 1 when
the file differs from what the upstream fold gives, and writes nothing.
"""

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[4]
FIXTURE = pathlib.Path(__file__).with_suffix(".json")
sys.path.insert(0, str(ROOT / "upstream" / "claude-agent-sdk-python" / "src"))

from claude_agent_sdk._internal.session_summary import (  # noqa: E402
    fold_session_summary,
)


def outputs(batches):
    key = {"project_key": "proj", "session_id": "sess"}
    summary = None
    folded = []
    for index, batch in enumerate(batches):
        summary = fold_session_summary(summary, key, batch)
        summary = {**summary, "mtime": 1000 + index}
        folded.append(summary)
    return folded


def main():
    fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
    expected = {"batches": fixture["batches"], "outputs": outputs(fixture["batches"])}
    if "--check" in sys.argv[1:]:
        if expected != fixture:
            print(f"{FIXTURE} differs from the upstream fold; run without --check")
            sys.exit(1)
        print(f"{FIXTURE} matches the upstream fold")
        return
    FIXTURE.write_text(json.dumps(expected, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {FIXTURE}")


if __name__ == "__main__":
    main()
