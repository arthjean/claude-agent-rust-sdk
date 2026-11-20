# Migrating from the Python SDK

`claude-agent-rust-sdk` is a port of version 0.2.186 of the Python Claude Agent
SDK. Every name the Python package exports is listed in the table at the end of
this page, with its Rust counterpart or the reason it has none. This guide
covers what changes in shape when you move a program from Python to Rust.
[`parity.md`](parity.md) lists each reviewed difference in behavior, with its
reason and the test that shows it, and the differences not yet classified.

## The two entry points

`query()` and `ClaudeSDKClient` keep their roles.

```python
async for message in query(prompt="What is 2 + 2?", options=options):
    if isinstance(message, ResultMessage):
        print(message.result)
```

```rust,ignore
let mut messages = query("What is 2 + 2?", options);
while let Some(message) = messages.next().await {
    if let Message::Result(result) = message? {
        println!("{}", result.result.unwrap_or_default());
    }
}
```

- `query` returns a `Stream` (the Rust word for an async iterator) of
  `Result<Message, Error>`. An error is the last item of the stream instead of
  an exception raised from the loop.
- `async with ClaudeSDKClient(options) as client:` becomes
  `ClaudeSdkClient::new(options)`, `client.connect().await?`, and
  `client.disconnect().await?` when you are done.
- `client.receive_response()` and `client.receive_messages()` return streams
  too.
- The `get_` prefix is dropped, as Rust names its getters:
  `get_server_info()`, `get_mcp_status()`, and `get_context_usage()` become
  `server_info()`, `mcp_status()`, and `context_usage()`.

## Options

`ClaudeAgentOptions(model="...", max_turns=1)` becomes a builder:

```rust,ignore
let options = ClaudeAgentOptions::builder()
    .model("claude-sonnet-5-5")
    .max_turns(1)
    .allowed_tools(["Read", "Grep"])
    .build()?;
```

The setters carry the names of the Python fields. A few of them take one entry
at a time next to the form that takes the whole collection: `env_var` next to
`env`, `extra_arg` next to `extra_args`, `mcp_server` next to `mcp_servers`,
`hook` next to `hooks`, and `agent` next to `agents`. `build` checks the
combinations Python checks at connect, such as `can_use_tool` together with
`permission_prompt_tool_name`.

## Messages and content blocks

Python tells message classes apart with `isinstance`. Rust has one `Message`
enum with a variant per class (`Message::Assistant`, `Message::User`,
`Message::System`, `Message::Result`, `Message::StreamEvent`, and so on), and one
`ContentBlock` enum (`ContentBlock::Text`, `ContentBlock::ToolUse`,
`ContentBlock::ToolResult`, `ContentBlock::Thinking`). Match on them.

A message or a block this version does not know is kept as raw JSON instead of
being dropped, so a newer CLI does not break an older program.

## Errors

Every function returns one error type, `Error`. Its `kind()` says what went
wrong, where Python raises a class: match on `ErrorKind` where Python caught a
class, and read the data of the cause through accessors (`exit_code()`,
`stderr()`, `result()`, `line()`). `is_retryable()` says whether trying again
can succeed.

```rust,ignore
match error.kind() {
    ErrorKind::ConnectionClosed => reconnect().await?,
    ErrorKind::Process => eprintln!("exit code {:?}", error.exit_code()),
    _ if error.is_retryable() => retry().await?,
    _ => return Err(error),
}
```

One difference matters when you port an `except` clause: in Python,
`ResultError` is a subclass of `ProcessError`, so `except ProcessError` also
catches it. In Rust, `ErrorKind::ResultError` and `ErrorKind::Process` are two
kinds, so a `match` arm on `ErrorKind::Process` alone misses a result error.
The same holds for `CLINotFoundError`, a subclass of `CLIConnectionError`, which
Rust splits into four kinds by cause. Two predicates keep the Python meaning:

| Python | Rust |
|---|---|
| `except ProcessError:` | `if error.is_process()`: true for `ErrorKind::Process` and `ErrorKind::ResultError` |
| `except CLIConnectionError:` | `if error.is_cli_connection()`: true for `ErrorKind::CliNotFound`, `Spawn`, `NotConnected`, `ConnectionClosed`, and `Unsupported` |

## Hooks and the permission callback

A Python hook is an `async def` that returns a dictionary. A Rust hook is a
`HookCallback` that returns a `HookOutput`, usually built from `SyncHookOutput`:

```rust,ignore
let hook = HookCallback::new(|input, _tool_use_id, _context| async move {
    let output = SyncHookOutput::default().hook_specific_output(
        PreToolUseHookSpecificOutput::default()
            .permission_decision(HookPermissionDecision::Deny)
            .permission_decision_reason("Not in this project"),
    );
    Ok(output.into())
});
let options = ClaudeAgentOptions::builder()
    .hook("PreToolUse", HookMatcher::new().matcher("Bash").hook(hook))
    .build()?;
```

The hook input is the `HookInput` enum, with one variant per event, where
Python gives a dictionary. The keys Python spells `continue_` and `async_`
keep those names in Rust.

`can_use_tool` takes a closure that receives the tool name, its input, and a
`ToolPermissionContext`, and returns `PermissionResult::allow()` or
`PermissionResult::deny("reason")`.

Python warns with `CanUseToolShadowedWarning` when the callback can never be
reached. Rust has no warning categories:
`ClaudeAgentOptions::can_use_tool_shadowed_warning` returns the text, and the
SDK logs it through `tracing` when a session starts.

## Tools served from your process

`@tool(...)` plus `create_sdk_mcp_server(...)` become `SdkMcpTool` plus
`create_sdk_mcp_server`:

- `SdkMcpTool::typed::<Args>(name, description, handler)` derives the input
  schema from a Rust type (`Args` implements `Deserialize` and `JsonSchema`).
- `SdkMcpTool::raw(name, description, schema, handler)` takes a JSON Schema you
  wrote. As in Python, the arguments of each call are validated against it
  before the handler runs, through the `jsonschema` feature, which is on by
  default. A build with `default-features = false` turns validation off, and
  the handler then receives the arguments as the model sent them. A remote
  `$ref` is never fetched: a schema that needs one makes every call an error
  result.
- With the `macros` feature, `#[tool]` on an async function does the same as
  the Python decorator.

A handler returns `ToolResult::text("...")` or `ToolResult::error("...")` where
Python returns a `{"content": [...]}` dictionary.

## Sessions and session stores

The session functions keep their names (`list_sessions`, `get_session_messages`,
`rename_session`, `tag_session`, `fork_session`, and the `*_from_store` and
`*_via_store` variants). Keyword arguments with defaults become plain
parameters: `directory` is an `Option<&Path>`, `limit` an `Option<usize>`, and
`offset` a `usize`.

`SessionStore` is a trait. `InMemorySessionStore` ships with the crate; the
`session_store_adapter` example shows how to write your own. The Postgres,
Redis, and S3 adapters of the Python repository are not ported.

## Logging and tracing

The SDK logs through the `tracing` crate where Python uses `logging`. With the
`otel` feature, the trace context that is active when a session starts is handed
to the CLI, as the Python `otel` extra does.

## What has no counterpart

- anyio and trio: tokio is the only runtime.
- The bundled CLI: the crate runs the `claude` executable installed on the
  machine, or the one `cli_path` names, and logs a warning when its version
  is not the tested one (`CLI_VERSION`).
- The TypeScript-only features (`startup`, `prewarm`, `setMcpServers`, and
  similar) are out of scope, as they are for the Python SDK.

## Name by name

This table is generated from [`parity/surface.toml`](../parity/surface.toml) by
`cargo xtask migration-table`; `cargo xtask parity-check` fails when it is out
of date. Paths are written from the crate root. A note that starts with
"Adapted" marks a name whose Rust counterpart is a different kind of item, such
as an enum variant where Python has a class: the line that uses it changes
shape, not only spelling.

<!-- parity:table:begin -->
| Python | Rust | Notes |
|---|---|---|
| `AgentDefinition` | `claude_agent_rust_sdk::AgentDefinition` |  |
| `AssistantMessage` | `claude_agent_rust_sdk::AssistantMessage` |  |
| `BackgroundTaskSummary` | `claude_agent_rust_sdk::BackgroundTaskSummary` |  |
| `BaseHookInput` | `claude_agent_rust_sdk::HookInputBase` |  |
| `CLIConnectionError` | `claude_agent_rust_sdk::Error::is_cli_connection`, `claude_agent_rust_sdk::ErrorKind::Spawn`, `claude_agent_rust_sdk::ErrorKind::NotConnected`, `claude_agent_rust_sdk::ErrorKind::ConnectionClosed`, `claude_agent_rust_sdk::ErrorKind::Unsupported` | Adapted: Four kinds tell its causes apart. `except CLIConnectionError`, which also catches `CLINotFoundError`, becomes `Error::is_cli_connection()`. |
| `CLIJSONDecodeError` | `claude_agent_rust_sdk::ErrorKind::JsonDecode` | Adapted: A kind of `Error`; `Error::line()` holds the line. |
| `CLINotFoundError` | `claude_agent_rust_sdk::ErrorKind::CliNotFound` | Adapted: A kind of `Error`, read with `Error::kind()`. |
| `CanUseTool` | `claude_agent_rust_sdk::CanUseTool` |  |
| `CanUseToolShadowedWarning` | `claude_agent_rust_sdk::ClaudeAgentOptions::can_use_tool_shadowed_warning` | Adapted: Rust has no warning categories: the method returns the text, which the SDK logs through `tracing` at connect. |
| `ClaudeAgentOptions` | `claude_agent_rust_sdk::ClaudeAgentOptions` |  |
| `ClaudeSDKClient` | `claude_agent_rust_sdk::ClaudeSdkClient` |  |
| `ClaudeSDKError` | `claude_agent_rust_sdk::Error` | One opaque error type holds every error; branch on `Error::kind()`. |
| `ContentBlock` | `claude_agent_rust_sdk::ContentBlock` |  |
| `ContextUsageCategory` | `claude_agent_rust_sdk::ContextUsageCategory` |  |
| `ContextUsageResponse` | `claude_agent_rust_sdk::ContextUsageResponse` | Typed struct where Python returns the dictionary; it and its categories keep the keys they do not model in `extra`. |
| `ConversationResetMessage` | `claude_agent_rust_sdk::ConversationResetMessage` |  |
| `CwdChangedHookSpecificOutput` | `claude_agent_rust_sdk::CwdChangedHookSpecificOutput` |  |
| `DeferredToolUse` | `claude_agent_rust_sdk::DeferredToolUse` |  |
| `EffortLevel` | `claude_agent_rust_sdk::EffortLevel` |  |
| `ElicitationHookSpecificOutput` | `claude_agent_rust_sdk::ElicitationHookSpecificOutput` |  |
| `ElicitationResultHookSpecificOutput` | `claude_agent_rust_sdk::ElicitationResultHookSpecificOutput` |  |
| `FastModeDisabledReason` | `claude_agent_rust_sdk::FastModeDisabledReason` |  |
| `FastModeState` | `claude_agent_rust_sdk::FastModeState` |  |
| `FileChangedHookSpecificOutput` | `claude_agent_rust_sdk::FileChangedHookSpecificOutput` |  |
| `ForkSessionResult` | `claude_agent_rust_sdk::ForkSessionResult` |  |
| `HookCallback` | `claude_agent_rust_sdk::HookCallback` |  |
| `HookContext` | `claude_agent_rust_sdk::HookContext` |  |
| `HookEffort` | `claude_agent_rust_sdk::HookEffort` |  |
| `HookEventMessage` | `claude_agent_rust_sdk::HookEventMessage` |  |
| `HookInput` | `claude_agent_rust_sdk::HookInput` |  |
| `HookJSONOutput` | `claude_agent_rust_sdk::HookOutput` |  |
| `HookMatcher` | `claude_agent_rust_sdk::HookMatcher` |  |
| `InMemorySessionStore` | `claude_agent_rust_sdk::InMemorySessionStore` |  |
| `McpSdkServerConfig` | `claude_agent_rust_sdk::McpSdkServerConfig` |  |
| `McpServerConfig` | `claude_agent_rust_sdk::McpServerConfig` |  |
| `McpServerConnectionStatus` | `claude_agent_rust_sdk::McpServerConnectionStatus` |  |
| `McpServerInfo` | `claude_agent_rust_sdk::McpServerInfo` |  |
| `McpServerStatus` | `claude_agent_rust_sdk::McpServerStatus` |  |
| `McpServerStatusConfig` | `claude_agent_rust_sdk::McpServerStatusConfig` | Enum where Python returns the dictionary. A configuration of a known type that carries a key the type does not model is kept whole as `Unknown`, so the key stays reachable. |
| `McpStatusResponse` | `claude_agent_rust_sdk::McpStatusResponse` | Typed struct where Python returns the dictionary; every nested struct keeps the keys it does not model in `extra`. |
| `McpToolAnnotations` | `claude_agent_rust_sdk::McpToolAnnotations` |  |
| `McpToolInfo` | `claude_agent_rust_sdk::McpToolInfo` |  |
| `Message` | `claude_agent_rust_sdk::Message` |  |
| `MessageDisplayHookSpecificOutput` | `claude_agent_rust_sdk::MessageDisplayHookSpecificOutput` |  |
| `MessageOrigin` | `claude_agent_rust_sdk::MessageOrigin` |  |
| `MessageOriginKind` | `claude_agent_rust_sdk::MessageOriginKind` |  |
| `MirrorErrorMessage` | `claude_agent_rust_sdk::MirrorErrorMessage` |  |
| `ModelUsage` | `claude_agent_rust_sdk::ModelUsage` | Typed struct where Python keeps the dictionary; keys it does not model are kept in `extra`, and an entry that is not an object is skipped. |
| `NotificationHookInput` | `claude_agent_rust_sdk::NotificationHookInput` |  |
| `NotificationHookSpecificOutput` | `claude_agent_rust_sdk::NotificationHookSpecificOutput` |  |
| `PermissionDeniedHookSpecificOutput` | `claude_agent_rust_sdk::PermissionDeniedHookSpecificOutput` |  |
| `PermissionMode` | `claude_agent_rust_sdk::PermissionMode` |  |
| `PermissionRequestHookInput` | `claude_agent_rust_sdk::PermissionRequestHookInput` |  |
| `PermissionRequestHookSpecificOutput` | `claude_agent_rust_sdk::PermissionRequestHookSpecificOutput` |  |
| `PermissionResult` | `claude_agent_rust_sdk::PermissionResult` |  |
| `PermissionResultAllow` | `claude_agent_rust_sdk::PermissionResult::Allow` | Adapted: Variant of `PermissionResult`. |
| `PermissionResultDeny` | `claude_agent_rust_sdk::PermissionResult::Deny` | Adapted: Variant of `PermissionResult`. |
| `PermissionUpdate` | `claude_agent_rust_sdk::PermissionUpdate` |  |
| `PostModelSwitchHookSpecificOutput` | `claude_agent_rust_sdk::PostModelSwitchHookSpecificOutput` |  |
| `PostToolBatchHookSpecificOutput` | `claude_agent_rust_sdk::PostToolBatchHookSpecificOutput` |  |
| `PostToolUseFailureHookInput` | `claude_agent_rust_sdk::PostToolUseFailureHookInput` |  |
| `PostToolUseFailureHookSpecificOutput` | `claude_agent_rust_sdk::PostToolUseFailureHookSpecificOutput` |  |
| `PostToolUseHookInput` | `claude_agent_rust_sdk::PostToolUseHookInput` |  |
| `PostToolUseHookSpecificOutput` | `claude_agent_rust_sdk::PostToolUseHookSpecificOutput` |  |
| `PreCompactHookInput` | `claude_agent_rust_sdk::PreCompactHookInput` |  |
| `PreModelSwitchHookSpecificOutput` | `claude_agent_rust_sdk::PreModelSwitchHookSpecificOutput` |  |
| `PreToolUseHookInput` | `claude_agent_rust_sdk::PreToolUseHookInput` |  |
| `PreToolUseHookSpecificOutput` | `claude_agent_rust_sdk::PreToolUseHookSpecificOutput` |  |
| `ProcessError` | `claude_agent_rust_sdk::ErrorKind::Process` | Adapted: A kind of `Error`; `Error::exit_code()` and `Error::stderr()` hold its fields. `except ProcessError`, which also catches `ResultError`, becomes `Error::is_process()`. |
| `RateLimitEvent` | `claude_agent_rust_sdk::RateLimitEvent` |  |
| `RateLimitInfo` | `claude_agent_rust_sdk::RateLimitInfo` |  |
| `RateLimitStatus` | `claude_agent_rust_sdk::RateLimitStatus` |  |
| `RateLimitType` | `claude_agent_rust_sdk::RateLimitType` |  |
| `ResultError` | `claude_agent_rust_sdk::ErrorKind::ResultError` | Adapted: A kind of `Error`; `Error::result()` returns its fields as `ResultErrorData`. |
| `ResultMessage` | `claude_agent_rust_sdk::ResultMessage` |  |
| `SDKSessionInfo` | `claude_agent_rust_sdk::SdkSessionInfo` |  |
| `SandboxIgnoreViolations` | `claude_agent_rust_sdk::SandboxIgnoreViolations` |  |
| `SandboxNetworkConfig` | `claude_agent_rust_sdk::SandboxNetworkConfig` |  |
| `SandboxSettings` | `claude_agent_rust_sdk::SandboxSettings` |  |
| `SdkBeta` | `claude_agent_rust_sdk::SdkBeta` |  |
| `SdkMcpTool` | `claude_agent_rust_sdk::SdkMcpTool` |  |
| `SdkPluginConfig` | `claude_agent_rust_sdk::SdkPluginConfig` |  |
| `ServerToolName` | `claude_agent_rust_sdk::ServerToolName` |  |
| `ServerToolResultBlock` | `claude_agent_rust_sdk::ServerToolResultBlock` |  |
| `ServerToolUseBlock` | `claude_agent_rust_sdk::ServerToolUseBlock` |  |
| `SessionCronSummary` | `claude_agent_rust_sdk::SessionCronSummary` |  |
| `SessionKey` | `claude_agent_rust_sdk::SessionKey` |  |
| `SessionListSubkeysKey` | `claude_agent_rust_sdk::SessionListSubkeysKey` |  |
| `SessionMessage` | `claude_agent_rust_sdk::SessionMessage` |  |
| `SessionStartHookSpecificOutput` | `claude_agent_rust_sdk::SessionStartHookSpecificOutput` |  |
| `SessionStore` | `claude_agent_rust_sdk::SessionStore` |  |
| `SessionStoreEntry` | `claude_agent_rust_sdk::SessionStoreEntry` |  |
| `SessionStoreFlushMode` | `claude_agent_rust_sdk::SessionStoreFlushMode` |  |
| `SessionStoreListEntry` | `claude_agent_rust_sdk::SessionStoreListEntry` |  |
| `SessionSummaryEntry` | `claude_agent_rust_sdk::SessionSummaryEntry` |  |
| `SettingSource` | `claude_agent_rust_sdk::SettingSource` |  |
| `StopHookInput` | `claude_agent_rust_sdk::StopHookInput` |  |
| `StopHookSpecificOutput` | `claude_agent_rust_sdk::StopHookSpecificOutput` |  |
| `StreamEvent` | `claude_agent_rust_sdk::StreamEvent` |  |
| `SubagentStartHookInput` | `claude_agent_rust_sdk::SubagentStartHookInput` |  |
| `SubagentStartHookSpecificOutput` | `claude_agent_rust_sdk::SubagentStartHookSpecificOutput` |  |
| `SubagentStopHookInput` | `claude_agent_rust_sdk::SubagentStopHookInput` |  |
| `SubagentStopHookSpecificOutput` | `claude_agent_rust_sdk::SubagentStopHookSpecificOutput` |  |
| `SystemInitData` | `claude_agent_rust_sdk::SystemInitData` |  |
| `SystemInitMcpServer` | `claude_agent_rust_sdk::SystemInitMcpServer` |  |
| `SystemInitPlugin` | `claude_agent_rust_sdk::SystemInitPlugin` |  |
| `SystemInitPluginError` | `claude_agent_rust_sdk::SystemInitPluginError` |  |
| `SystemMessage` | `claude_agent_rust_sdk::SystemMessage` |  |
| `TERMINAL_TASK_STATUSES` | `claude_agent_rust_sdk::TERMINAL_TASK_STATUSES` |  |
| `TaskBudget` | `claude_agent_rust_sdk::TaskBudget` |  |
| `TaskNotificationMessage` | `claude_agent_rust_sdk::TaskNotificationMessage` |  |
| `TaskNotificationOriginSubkind` | `claude_agent_rust_sdk::TaskNotificationOriginSubkind` |  |
| `TaskNotificationStatus` | `claude_agent_rust_sdk::TaskNotificationStatus` |  |
| `TaskProgressMessage` | `claude_agent_rust_sdk::TaskProgressMessage` |  |
| `TaskStartedMessage` | `claude_agent_rust_sdk::TaskStartedMessage` |  |
| `TaskUpdatedMessage` | `claude_agent_rust_sdk::TaskUpdatedMessage` |  |
| `TaskUpdatedStatus` | `claude_agent_rust_sdk::TaskUpdatedStatus` |  |
| `TaskUsage` | `claude_agent_rust_sdk::TaskUsage` |  |
| `TextBlock` | `claude_agent_rust_sdk::TextBlock` |  |
| `ThinkingBlock` | `claude_agent_rust_sdk::ThinkingBlock` |  |
| `ThinkingConfig` | `claude_agent_rust_sdk::ThinkingConfig` |  |
| `ThinkingConfigAdaptive` | `claude_agent_rust_sdk::ThinkingConfig::Adaptive` | Adapted: Variant of `ThinkingConfig`. |
| `ThinkingConfigDisabled` | `claude_agent_rust_sdk::ThinkingConfig::Disabled` | Adapted: Variant of `ThinkingConfig`. |
| `ThinkingConfigEnabled` | `claude_agent_rust_sdk::ThinkingConfig::Enabled` | Adapted: Variant of `ThinkingConfig`. |
| `ToolAnnotations` | `claude_agent_rust_sdk::ToolAnnotations` |  |
| `ToolPermissionContext` | `claude_agent_rust_sdk::ToolPermissionContext` |  |
| `ToolResultBlock` | `claude_agent_rust_sdk::ToolResultBlock` |  |
| `ToolUseBlock` | `claude_agent_rust_sdk::ToolUseBlock` |  |
| `Transport` | `claude_agent_rust_sdk::Transport` | A trait; `SubprocessTransport` is the implementation that runs the CLI. |
| `UserMessage` | `claude_agent_rust_sdk::UserMessage` |  |
| `UserPromptExpansionHookSpecificOutput` | `claude_agent_rust_sdk::UserPromptExpansionHookSpecificOutput` |  |
| `UserPromptSubmitHookInput` | `claude_agent_rust_sdk::UserPromptSubmitHookInput` |  |
| `UserPromptSubmitHookSpecificOutput` | `claude_agent_rust_sdk::UserPromptSubmitHookSpecificOutput` |  |
| `WorktreeCreateHookSpecificOutput` | `claude_agent_rust_sdk::WorktreeCreateHookSpecificOutput` |  |
| `__version__` | `claude_agent_rust_sdk::VERSION` |  |
| `create_sdk_mcp_server` | `claude_agent_rust_sdk::create_sdk_mcp_server` |  |
| `delete_session` | `claude_agent_rust_sdk::delete_session` |  |
| `delete_session_via_store` | `claude_agent_rust_sdk::delete_session_via_store` |  |
| `fold_session_summary` | `claude_agent_rust_sdk::fold_session_summary` |  |
| `fork_session` | `claude_agent_rust_sdk::fork_session` |  |
| `fork_session_via_store` | `claude_agent_rust_sdk::fork_session_via_store` |  |
| `get_session_info` | `claude_agent_rust_sdk::get_session_info` |  |
| `get_session_info_from_store` | `claude_agent_rust_sdk::get_session_info_from_store` |  |
| `get_session_messages` | `claude_agent_rust_sdk::get_session_messages` |  |
| `get_session_messages_from_store` | `claude_agent_rust_sdk::get_session_messages_from_store` |  |
| `get_subagent_messages` | `claude_agent_rust_sdk::get_subagent_messages` |  |
| `get_subagent_messages_from_store` | `claude_agent_rust_sdk::get_subagent_messages_from_store` |  |
| `import_session_to_store` | `claude_agent_rust_sdk::import_session_to_store` |  |
| `list_sessions` | `claude_agent_rust_sdk::list_sessions` |  |
| `list_sessions_from_store` | `claude_agent_rust_sdk::list_sessions_from_store` |  |
| `list_subagents` | `claude_agent_rust_sdk::list_subagents` |  |
| `list_subagents_from_store` | `claude_agent_rust_sdk::list_subagents_from_store` |  |
| `project_key_for_directory` | `claude_agent_rust_sdk::project_key_for_directory` |  |
| `query` | `claude_agent_rust_sdk::query` | Returns a `Stream` of `Result<Message, Error>`. |
| `rename_session` | `claude_agent_rust_sdk::rename_session` |  |
| `rename_session_via_store` | `claude_agent_rust_sdk::rename_session_via_store` |  |
| `tag_session` | `claude_agent_rust_sdk::tag_session` |  |
| `tag_session_via_store` | `claude_agent_rust_sdk::tag_session_via_store` |  |
| `tool` | `claude_agent_rust_sdk::tool` | Adapted: Attribute macro behind the `macros` feature; `SdkMcpTool::typed` and `SdkMcpTool::raw` need no feature. |
<!-- parity:table:end -->
