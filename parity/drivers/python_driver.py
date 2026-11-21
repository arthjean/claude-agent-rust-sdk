"""Python driver of the differential parity harness.

Usage: ``python python_driver.py <case.json> --cli <path>``

Plays one case of ``parity/cases`` through the Python SDK, imported from
the submodule ``upstream/claude-agent-sdk-python/src`` through
``PYTHONPATH``, with ``cli_path`` set to ``<path>``. The Rust driver,
``crates/parity-driver``, plays the same case through ``claude-agent-rust-sdk``,
and ``cargo xtask parity-diff`` compares what the CLI received from each.
Both drivers read the same format, described in ``parity/README.md`` and
``parity/case.schema.json``; the canned callbacks are documented in the
``options`` module of the Rust driver.

Only the public API of the SDK is used.

When ``PARITY_JOURNAL`` names a file, the driver writes there what the
application observed: each message, the outcome of each call, and the
arguments of each callback, in the journal format of ``parity/README.md``,
which the Rust driver writes too. An error of the SDK is journaled and ends
the case, which then fails unless it sets ``"expect_error": true``.

Exit status: 0 when the case ran, 1 when it failed, 2 on a usage error, and
3 when the case uses an option, a step, or a value this driver does not
support; the unsupported item is printed on stderr.
"""

import dataclasses
import json
import os
import sys
from typing import Any

EXIT_FAILED = 1
EXIT_USAGE = 2
EXIT_UNSUPPORTED = 3


class Unsupported(Exception):
    """The case needs something this driver does not implement."""


class Failed(Exception):
    """The case ran and failed."""


PASSED_AS_IS = {
    "tools",
    "allowed_tools",
    "disallowed_tools",
    "system_prompt",
    "strict_mcp_config",
    "permission_mode",
    "continue_conversation",
    "resume",
    "session_id",
    "max_turns",
    "max_budget_usd",
    "model",
    "fallback_model",
    "betas",
    "permission_prompt_tool_name",
    "cwd",
    "settings",
    "add_dirs",
    "env",
    "extra_args",
    "max_buffer_size",
    "user",
    "include_partial_messages",
    "include_hook_events",
    "forward_subagent_text",
    "verbatim_prompts",
    "fork_session",
    "resume_session_at",
    "resume_drops_turn",
    "setting_sources",
    "skills",
    "sandbox",
    "plugins",
    "max_thinking_tokens",
    "thinking",
    "effort",
    "output_format",
    "enable_file_checkpointing",
    "session_store_flush",
    "load_timeout_ms",
    "task_budget",
    "client_app",
}

WIRE_TO_PYTHON_HOOK_KEYS = {"continue": "continue_", "async": "async_"}

JOURNAL_VARIABLE = "PARITY_JOURNAL"


def canonical(value: Any) -> Any:
    """Return the canonical form of a value the application received.

    An instance of an SDK dataclass becomes an object with ``"$type"``, its
    class name, and one key per field; a dict, a list, and a scalar stay as
    they are. The Rust driver maps its types onto the same form.
    """
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        result: dict[str, Any] = {"$type": type(value).__name__}
        for field in dataclasses.fields(value):
            result[field.name] = canonical(getattr(value, field.name))
        return result
    if isinstance(value, dict):
        return {str(key): canonical(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [canonical(item) for item in value]
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    return {"$type": "Unmapped", "debug": repr(value)}


def journal(event: str, **fields: Any) -> None:
    """Append one event to the journal; do nothing without a journal."""
    path = os.environ.get(JOURNAL_VARIABLE)
    if not path:
        return
    record = {key: canonical(value) for key, value in fields.items()}
    record["event"] = event
    with open(path, "a", encoding="utf-8") as file:
        file.write(json.dumps(record) + "\n")


ERROR_KINDS = {
    "CLINotFoundError": "cli_not_found",
    "CLIConnectionError": "cli_connection",
    "ProcessError": "process",
    "ResultError": "result_error",
    "CLIJSONDecodeError": "json_decode",
    "MessageParseError": "message_parse",
    "Exception": "control",
}


def error_record(error: BaseException) -> dict[str, Any]:
    """Return the ``error`` of a ``call`` event."""
    name = type(error).__name__
    return {
        "kind": ERROR_KINDS.get(name, f"python:{name}"),
        "exit_code": getattr(error, "exit_code", None),
        "message": str(error),
    }


def can_use_tool(decision: dict[str, Any]) -> Any:
    """Build the canned permission callback of a case."""
    import anyio

    from claude_agent_sdk import (
        PermissionResultAllow,
        PermissionResultDeny,
        PermissionUpdate,
    )

    behavior = decision.get("behavior")
    updates = decision.get("updated_permissions")
    if behavior == "allow":
        if updates is None or updates == "suggestions":
            fixed = None
        elif isinstance(updates, list):
            fixed = [PermissionUpdate.from_dict(update) for update in updates]
        else:
            raise Failed(f"option can_use_tool has an unexpected value: {decision}")
    elif behavior not in ("deny", "wait"):
        raise Failed(f"option can_use_tool has an unexpected value: {decision}")

    async def callback(tool_name: str, tool_input: dict[str, Any], context: Any) -> Any:
        journal("can_use_tool", tool_name=tool_name, input=tool_input, context=context)
        if behavior == "allow":
            return PermissionResultAllow(
                updated_input=decision.get("updated_input"),
                updated_permissions=context.suggestions if updates == "suggestions" else fixed,
            )
        if behavior == "deny":
            return PermissionResultDeny(
                message=decision.get("message", ""),
                interrupt=decision.get("interrupt", False),
            )
        await anyio.sleep_forever()

    return callback


def hook(name: str, output: dict[str, Any]) -> Any:
    """Return a hook callback that journals its arguments and returns ``output``."""

    async def callback(hook_input: Any, tool_use_id: Any, context: Any) -> Any:
        journal(
            "hook",
            callback=name,
            input=hook_input,
            tool_use_id=tool_use_id,
            context=context,
        )
        return output

    return callback


def hooks(events: dict[str, Any]) -> dict[str, Any]:
    """Build the canned hook matchers of a case."""
    from claude_agent_sdk import HookMatcher

    built: dict[str, Any] = {}
    for event, matchers in events.items():
        built[event] = []
        for index, matcher in enumerate(matchers):
            outputs = [
                {WIRE_TO_PYTHON_HOOK_KEYS.get(key, key): value for key, value in output.items()}
                for output in matcher.get("hooks", [])
            ]
            built[event].append(
                HookMatcher(
                    matcher=matcher.get("matcher"),
                    hooks=[
                        hook(f"{event}/{index}/{position}", output)
                        for position, output in enumerate(outputs)
                    ],
                    timeout=matcher.get("timeout"),
                    on_failure=matcher.get("on_failure"),
                )
            )
    return built


def tool_content(block: dict[str, Any]) -> dict[str, Any]:
    """Check one canned content block of a tool result."""
    if block.get("type") not in ("text", "image", "resource_link", "resource"):
        raise Unsupported(f"tool content type {block.get('type', '(none)')}")
    return block


def tool_handler(server: str, tool: str, definition: dict[str, Any]) -> Any:
    """Return a tool handler that journals its arguments, then returns the
    canned result, raises the ``raise`` text, or, with ``wait``, never
    returns."""
    result = definition.get("result", {})
    response = {
        "content": [tool_content(block) for block in result.get("content", [])],
        "is_error": result.get("is_error", False),
    }

    async def handler(arguments: Any) -> Any:
        journal("tool", server=server, tool=tool, arguments=arguments)
        if "raise" in definition:
            raise RuntimeError(definition["raise"])
        if definition.get("wait"):
            import anyio

            await anyio.sleep_forever()
        return response

    return handler


STORE_APPENDS: list[int] = []
"""The number of appends made so far, as its only item once a case has a
session store: the ``message`` event of a result carries it, so the journal
shows how many appends the application could rely on when the result
reached it."""


def journaled_store() -> Any:
    """Return an in-memory session store that journals every append."""
    from claude_agent_sdk import InMemorySessionStore

    class JournaledStore(InMemorySessionStore):
        async def append(self, key: Any, entries: Any) -> None:
            journal("store_append", key=key, entries=entries)
            await super().append(key, entries)
            STORE_APPENDS[0] += 1

    STORE_APPENDS[:] = [0]
    return JournaledStore()


def stderr_line(line: str) -> None:
    """Journal one line the CLI wrote to its error output."""
    journal("stderr", line=line)


def sdk_server(name: str, config: dict[str, Any]) -> Any:
    """Build an in-process MCP server whose tools return fixed content."""
    from mcp.types import ToolAnnotations

    from claude_agent_sdk import SdkMcpTool, create_sdk_mcp_server

    tools = []
    for definition in config.get("tools", []):
        annotations = definition.get("annotations")
        tools.append(
            SdkMcpTool(
                name=definition["name"],
                description=definition.get("description", ""),
                input_schema=definition["input_schema"],
                handler=tool_handler(name, definition["name"], definition),
                annotations=ToolAnnotations(**annotations) if annotations is not None else None,
            )
        )
    return create_sdk_mcp_server(name, version=config.get("version", "1.0.0"), tools=tools)


def mcp_servers(value: Any) -> Any:
    """Translate the ``mcp_servers`` option, building in-process servers."""
    if not isinstance(value, dict):
        return value
    servers = {}
    for name, config in value.items():
        if config.get("type") == "sdk":
            servers[name] = sdk_server(name, config)
        elif config.get("type", "stdio") in ("stdio", "sse", "http"):
            servers[name] = config
        else:
            raise Unsupported(f"mcp server type {config.get('type')}")
    return servers


def build_options(options: dict[str, Any], cli: str) -> Any:
    """Translate the ``options`` object of a case into ``ClaudeAgentOptions``."""
    from claude_agent_sdk import AgentDefinition, ClaudeAgentOptions

    keywords: dict[str, Any] = {"cli_path": cli}
    for name, value in options.items():
        if name in PASSED_AS_IS:
            keywords[name] = value
        elif name == "mcp_servers":
            keywords[name] = mcp_servers(value)
        elif name == "agents":
            keywords[name] = {key: AgentDefinition(**agent) for key, agent in value.items()}
        elif name == "can_use_tool":
            keywords[name] = can_use_tool(value)
        elif name == "hooks":
            keywords[name] = hooks(value)
        elif name == "stderr":
            if value:
                keywords[name] = stderr_line
        elif name == "session_store":
            if value != "memory":
                raise Failed(f"option session_store has an unexpected value: {value}")
            keywords[name] = journaled_store()
        else:
            raise Unsupported(f"option {name}")
    return ClaudeAgentOptions(**keywords)


def prompt(value: Any) -> Any:
    """Translate a prompt: a string, or a list of messages sent as a stream."""
    if isinstance(value, str):
        return value
    if isinstance(value, list):

        async def messages() -> Any:
            for message in value:
                yield message

        return messages()
    raise Failed("a prompt must be a string or a list of messages")


class SdkFailure(Exception):
    """A call of the SDK raised; the error is journaled."""

    def __init__(self, error: BaseException) -> None:
        super().__init__(str(error))
        self.error = error


async def messages(stream: Any, what: str) -> None:
    """Read messages until the stream ends, journaling each one, and require
    a result among them (a session state message may follow it)."""
    from claude_agent_sdk import ResultMessage

    result = False
    try:
        async for message in stream:
            is_result = isinstance(message, ResultMessage)
            appends = STORE_APPENDS[0] if is_result and STORE_APPENDS else None
            journal("message", message=message, store_appends=appends)
            result = result or is_result
    except Exception as error:
        raise SdkFailure(error) from error
    if not result:
        raise Failed(f"{what} held no result")


def journal_call(step: Any, method: str, outcome: Any, expect_error: bool) -> bool:
    """Journal the outcome of one call; return whether the case goes on.

    ``outcome`` is the returned value, or the ``SdkFailure`` the call raised.
    An error of the SDK ends the case: it passes when the case expects an
    error, and fails otherwise.
    """
    if isinstance(outcome, SdkFailure):
        journal("call", step=step, method=method, error=error_record(outcome.error))
        if not expect_error:
            raise Failed(f"{type(outcome.error).__name__}: {outcome.error}")
        return False
    journal("call", step=step, method=method, result=outcome)
    return True


async def one_shot(case: dict[str, Any], options: Any, expect_error: bool) -> None:
    """Run the one-shot ``query()`` and require a final result."""
    from claude_agent_sdk import query

    outcome: Any = None
    try:
        await messages(query(prompt=prompt(case.get("prompt")), options=options), "the stream")
    except SdkFailure as failure:
        outcome = failure
    journal_call(None, "query", outcome, expect_error)


async def play(client: Any, action: str, argument: Any) -> Any:
    """Play one step of a client case and return what the method returned."""
    if action == "connect":
        if isinstance(argument, dict) and "prompt" in argument:
            return await client.connect(prompt(argument["prompt"]))
        return await client.connect()
    if action == "query":
        if "session_id" in argument:
            return await client.query(
                prompt(argument.get("prompt")), session_id=argument["session_id"]
            )
        return await client.query(prompt(argument.get("prompt")))
    if action == "receive_response":
        return await messages(client.receive_response(), "the response")
    if action == "interrupt":
        return await client.interrupt()
    if action == "set_permission_mode":
        return await client.set_permission_mode(argument)
    if action == "set_model":
        return await client.set_model(argument)
    if action == "rewind_files":
        return await client.rewind_files(argument)
    if action == "reconnect_mcp_server":
        return await client.reconnect_mcp_server(argument)
    if action == "toggle_mcp_server":
        return await client.toggle_mcp_server(
            argument.get("name", ""), argument.get("enabled", True)
        )
    if action == "stop_task":
        return await client.stop_task(argument)
    if action == "get_mcp_status":
        return await client.get_mcp_status()
    if action == "get_context_usage":
        return await client.get_context_usage()
    if action == "get_server_info":
        return await client.get_server_info()
    if action == "disconnect":
        return await client.disconnect()
    raise Unsupported(f"step {action}")


async def client_steps(steps: list[Any], options: Any, expect_error: bool) -> None:
    """Drive a ``ClaudeSDKClient`` through the steps of a case."""
    from claude_agent_sdk import ClaudeSDKClient

    client = ClaudeSDKClient(options)
    for index, step in enumerate(steps):
        if not isinstance(step, dict) or len(step) != 1:
            raise Failed(f"step {index} must hold exactly one action")
        ((action, argument),) = step.items()
        try:
            outcome = await play(client, action, argument)
        except (Unsupported, Failed):
            raise
        except SdkFailure as failure:
            outcome = failure
        except Exception as error:
            outcome = SdkFailure(error)
        try:
            go_on = journal_call(index, action, outcome, expect_error)
        except Failed as failure:
            raise Failed(f"step {index} ({action}): {failure}") from failure
        if not go_on:
            try:
                await client.disconnect()
            except Exception:
                pass
            break


async def run(case: dict[str, Any], cli: str) -> None:
    """Play a case."""
    options = build_options(case.get("options", {}), cli)
    expect_error = case.get("expect_error", False) is True
    steps = case.get("steps")
    if steps is None:
        await one_shot(case, options, expect_error)
    else:
        await client_steps(steps, options, expect_error)


def main() -> int:
    """Entry point."""
    if len(sys.argv) != 4 or sys.argv[2] != "--cli":
        print("usage: python_driver.py <case.json> --cli <path>", file=sys.stderr)
        return EXIT_USAGE
    try:
        with open(sys.argv[1], encoding="utf-8") as file:
            case = json.load(file)
    except (OSError, ValueError) as error:
        print(f"python-driver: cannot read the case: {error}", file=sys.stderr)
        return EXIT_USAGE
    if not isinstance(case, dict):
        print("python-driver: a case must be a JSON object", file=sys.stderr)
        return EXIT_USAGE

    import anyio

    try:
        anyio.run(run, case, sys.argv[3])
    except Unsupported as item:
        print(f"python-driver: unsupported {item}", file=sys.stderr)
        return EXIT_UNSUPPORTED
    except Exception as error:
        print(f"python-driver: {type(error).__name__}: {error}", file=sys.stderr)
        return EXIT_FAILED
    return 0


if __name__ == "__main__":
    sys.exit(main())
