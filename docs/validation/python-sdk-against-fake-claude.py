"""Run the unmodified Python SDK against fake-claude and check the recording.

Usage:

    PYTHONPATH=upstream/claude-agent-sdk-python/src \
        python docs/validation/python-sdk-against-fake-claude.py <fake-claude binary>

The script writes a scenario and a record file to a temporary directory,
calls ``query()`` with ``cli_path`` set to the binary, prints every message
it receives and the records, and exits with status 0 only when the stream
ended with a result and the recording holds a spawn record, an initialize
control request, and a user message frame.
"""

import json
import os
import sys
import tempfile

import anyio

from claude_agent_sdk import ClaudeAgentOptions, ResultMessage, query

SCENARIO = {
    "steps": [
        {"expect": {"type": "control_request", "request": {"subtype": "initialize"}}},
        {"control_response": {"commands": []}},
        {"expect": {"type": "user"}},
        {
            "emit": {
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "model": "m",
                    "content": [{"type": "text", "text": "hello"}],
                },
                "session_id": "s",
            }
        },
        {
            "emit": {
                "type": "result",
                "subtype": "success",
                "duration_ms": 1,
                "duration_api_ms": 1,
                "is_error": False,
                "num_turns": 1,
                "session_id": "s",
                "result": "hello",
            }
        },
    ]
}


async def run(binary: str, directory: str) -> int:
    scenario_path = os.path.join(directory, "scenario.json")
    record_path = os.path.join(directory, "record.jsonl")
    with open(scenario_path, "w", encoding="utf-8") as file:
        json.dump(SCENARIO, file)
    os.environ["FAKE_CLAUDE_SCENARIO"] = scenario_path
    os.environ["FAKE_CLAUDE_RECORD"] = record_path

    last = None
    async for message in query(
        prompt="Say hello", options=ClaudeAgentOptions(cli_path=binary)
    ):
        print("message:", type(message).__name__)
        last = message

    with open(record_path, encoding="utf-8") as file:
        records = [json.loads(line) for line in file]
    for record in records:
        print("record:", json.dumps(record, sort_keys=True))

    frames = [record["frame"] for record in records if record["record"] == "stdin"]
    checks = {
        "stream ended with a result": isinstance(last, ResultMessage),
        "spawn record": records[0]["record"] == "spawn",
        "initialize control request": any(
            frame.get("type") == "control_request"
            and frame.get("request", {}).get("subtype") == "initialize"
            for frame in frames
        ),
        "user message frame": any(frame.get("type") == "user" for frame in frames),
    }
    for name, passed in checks.items():
        print(f"check: {name}: {'ok' if passed else 'FAILED'}")
    return 0 if all(checks.values()) else 1


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory() as directory:
        return anyio.run(run, sys.argv[1], directory)


if __name__ == "__main__":
    sys.exit(main())
