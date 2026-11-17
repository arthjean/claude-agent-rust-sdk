"""Python driver of the differential parity harness.

Usage: ``python python_driver.py <case.json> --cli <path>``

Plays one case of ``parity/cases`` through the Python SDK, imported from
the submodule ``upstream/claude-agent-sdk-python/src`` through
``PYTHONPATH``, with ``cli_path`` set to ``<path>``. The Rust driver,
``crates/parity-driver``, plays the same case through ``claude-agent-kit``,
and ``cargo xtask parity-diff`` compares what the CLI received from each.
Both drivers read the same format, described in ``parity/README.md`` and
``parity/case.schema.json``; the canned callbacks are documented in the
``options`` module of the Rust driver.

Only the public API of the SDK is used.

Exit status: 0 when the case ran, 1 when it failed, 2 on a usage error, and
3 when the case uses an option, a step, or a value this driver does not
support; the unsupported item is printed on stderr.
"""

import json
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


def canned(value: Any) -> Any:
    """Return a coroutine function that always returns ``value``."""

    async def callback(*_arguments: Any, **_keywords: Any) -> Any:
        return value

    return callback


def can_use_tool(decision: dict[str, Any]) -> Any:
    """Build the canned permission callback of a case."""
    from claude_agent_sdk import PermissionResultAllow, PermissionResultDeny

    behavior = decision.get("behavior")
    if behavior == "allow":
        result: Any = PermissionResultAllow(updated_input=decision.get("updated_input"))
    elif behavior == "deny":
        result = PermissionResultDeny(
            message=decision.get("message", ""),
            interrupt=decision.get("interrupt", False),
        )
    else:
        raise Failed(f"option can_use_tool has an unexpected value: {decision}")
    return canned(result)


def hooks(events: dict[str, Any]) -> dict[str, Any]:
    """Build the canned hook matchers of a case."""
    from claude_agent_sdk import HookMatcher

    built: dict[str, Any] = {}
    for event, matchers in events.items():
        built[event] = []
        for matcher in matchers:
            outputs = [
                {WIRE_TO_PYTHON_HOOK_KEYS.get(key, key): value for key, value in output.items()}
                for output in matcher.get("hooks", [])
            ]
            built[event].append(
                HookMatcher(
                    matcher=matcher.get("matcher"),
                    hooks=[canned(output) for output in outputs],
                    timeout=matcher.get("timeout"),
                    on_failure=matcher.get("on_failure"),
                )
            )
    return built


def tool_content(block: dict[str, Any]) -> dict[str, Any]:
    """Check one canned content block of a tool result."""
    if block.get("type") not in ("text", "image"):
        raise Unsupported(f"tool content type {block.get('type', '(none)')}")
    return block


def sdk_server(name: str, config: dict[str, Any]) -> Any:
    """Build an in-process MCP server whose tools return fixed content."""
    from mcp.types import ToolAnnotations

    from claude_agent_sdk import SdkMcpTool, create_sdk_mcp_server

    tools = []
    for definition in config.get("tools", []):
        result = definition.get("result", {})
        response = {
            "content": [tool_content(block) for block in result.get("content", [])],
            "is_error": result.get("is_error", False),
        }
        annotations = definition.get("annotations")
        tools.append(
            SdkMcpTool(
                name=definition["name"],
                description=definition.get("description", ""),
                input_schema=definition["input_schema"],
                handler=canned(response),
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
    from claude_agent_sdk import AgentDefinition, ClaudeAgentOptions, InMemorySessionStore

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
                keywords[name] = lambda _line: None
        elif name == "session_store":
            if value != "memory":
                raise Failed(f"option session_store has an unexpected value: {value}")
            keywords[name] = InMemorySessionStore()
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


async def one_shot(case: dict[str, Any], options: Any) -> None:
    """Run the one-shot ``query()`` and require a final result."""
    from claude_agent_sdk import ResultMessage, query

    last = None
    async for message in query(prompt=prompt(case.get("prompt")), options=options):
        last = message
    if not isinstance(last, ResultMessage):
        raise Failed("the stream did not end with a result")


async def play(client: Any, action: str, argument: Any) -> None:
    """Play one step of a client case."""
    from claude_agent_sdk import ResultMessage

    if action == "connect":
        if isinstance(argument, dict) and "prompt" in argument:
            await client.connect(prompt(argument["prompt"]))
        else:
            await client.connect()
    elif action == "query":
        if "session_id" in argument:
            await client.query(prompt(argument.get("prompt")), session_id=argument["session_id"])
        else:
            await client.query(prompt(argument.get("prompt")))
    elif action == "receive_response":
        last = None
        async for message in client.receive_response():
            last = message
        if not isinstance(last, ResultMessage):
            raise Failed("the response did not end with a result")
    elif action == "interrupt":
        await client.interrupt()
    elif action == "set_permission_mode":
        await client.set_permission_mode(argument)
    elif action == "set_model":
        await client.set_model(argument)
    elif action == "rewind_files":
        await client.rewind_files(argument)
    elif action == "reconnect_mcp_server":
        await client.reconnect_mcp_server(argument)
    elif action == "toggle_mcp_server":
        await client.toggle_mcp_server(argument.get("name", ""), argument.get("enabled", True))
    elif action == "stop_task":
        await client.stop_task(argument)
    elif action == "get_mcp_status":
        await client.get_mcp_status()
    elif action == "get_context_usage":
        await client.get_context_usage()
    elif action == "disconnect":
        await client.disconnect()
    else:
        raise Unsupported(f"step {action}")


async def client_steps(steps: list[Any], options: Any) -> None:
    """Drive a ``ClaudeSDKClient`` through the steps of a case."""
    from claude_agent_sdk import ClaudeSDKClient

    client = ClaudeSDKClient(options)
    for index, step in enumerate(steps):
        if not isinstance(step, dict) or len(step) != 1:
            raise Failed(f"step {index} must hold exactly one action")
        ((action, argument),) = step.items()
        try:
            await play(client, action, argument)
        except Unsupported:
            raise
        except Exception as error:
            raise Failed(f"step {index} ({action}): {error}") from error


async def run(case: dict[str, Any], cli: str) -> None:
    """Play a case."""
    options = build_options(case.get("options", {}), cli)
    steps = case.get("steps")
    if steps is None:
        await one_shot(case, options)
    else:
        await client_steps(steps, options)


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
