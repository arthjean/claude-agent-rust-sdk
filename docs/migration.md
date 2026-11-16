# Migrating from the Python SDK

`claude-agent-kit` is at parity with version 0.2.182 of the Python Claude Agent
SDK. Every name the Python package exports has a Rust counterpart, listed in the
table at the end of this page. This guide covers what changes in shape when you
move a program from Python to Rust.

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

The Python exception classes are the variants of one enum, `Error`. Match on
the variant where Python caught a class.

One difference matters when you port an `except` clause: in Python,
`ResultError` is a subclass of `ProcessError`, so `except ProcessError` also
catches it. In Rust, `Error::ResultError` and `Error::Process` are two variants;
match both when you mean both.

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
  machine, or the one `cli_path` names.
- The TypeScript-only features (`startup`, `prewarm`, `setMcpServers`, and
  similar) are out of scope, as they are for the Python SDK.

## Name by name

This table is generated from [`parity/surface.toml`](../parity/surface.toml) by
`cargo xtask migration-table`; `cargo xtask parity-check` fails when it is out
of date. Paths are written from the crate root.

<!-- parity:table:begin -->
| Python | Rust | Notes |
|---|---|---|
| `AgentDefinition` | `claude_agent_kit::AgentDefinition` |  |
| `AssistantMessage` | `claude_agent_kit::AssistantMessage` |  |
| `BackgroundTaskSummary` | `claude_agent_kit::BackgroundTaskSummary` |  |
| `BaseHookInput` | `claude_agent_kit::HookInputBase` |  |
| `CLIConnectionError` | `claude_agent_kit::Error::CliConnection` | Variant of `Error`. |
| `CLIJSONDecodeError` | `claude_agent_kit::Error::JsonDecode` | Variant of `Error`. |
| `CLINotFoundError` | `claude_agent_kit::Error::CliNotFound` | Variant of `Error`. |
| `CanUseTool` | `claude_agent_kit::CanUseTool` |  |
| `CanUseToolShadowedWarning` | `claude_agent_kit::ClaudeAgentOptions::can_use_tool_shadowed_warning` | Rust has no warning categories: the method returns the text, which the SDK logs through `tracing` at connect. |
| `ClaudeAgentOptions` | `claude_agent_kit::ClaudeAgentOptions` |  |
| `ClaudeSDKClient` | `claude_agent_kit::ClaudeSdkClient` |  |
| `ClaudeSDKError` | `claude_agent_kit::Error` | One enum holds every error; match on its variants. |
| `ContentBlock` | `claude_agent_kit::ContentBlock` |  |
| `ContextUsageCategory` | `claude_agent_kit::ContextUsageCategory` |  |
| `ContextUsageResponse` | `claude_agent_kit::ContextUsageResponse` |  |
| `ConversationResetMessage` | `claude_agent_kit::ConversationResetMessage` |  |
| `CwdChangedHookSpecificOutput` | `claude_agent_kit::CwdChangedHookSpecificOutput` |  |
| `DeferredToolUse` | `claude_agent_kit::DeferredToolUse` |  |
| `EffortLevel` | `claude_agent_kit::EffortLevel` |  |
| `ElicitationHookSpecificOutput` | `claude_agent_kit::ElicitationHookSpecificOutput` |  |
| `ElicitationResultHookSpecificOutput` | `claude_agent_kit::ElicitationResultHookSpecificOutput` |  |
| `FastModeDisabledReason` | `claude_agent_kit::FastModeDisabledReason` |  |
| `FastModeState` | `claude_agent_kit::FastModeState` |  |
| `FileChangedHookSpecificOutput` | `claude_agent_kit::FileChangedHookSpecificOutput` |  |
| `ForkSessionResult` | `claude_agent_kit::ForkSessionResult` |  |
| `HookCallback` | `claude_agent_kit::HookCallback` |  |
| `HookContext` | `claude_agent_kit::HookContext` |  |
| `HookEffort` | `claude_agent_kit::HookEffort` |  |
| `HookEventMessage` | `claude_agent_kit::HookEventMessage` |  |
| `HookInput` | `claude_agent_kit::HookInput` |  |
| `HookJSONOutput` | `claude_agent_kit::HookOutput` |  |
| `HookMatcher` | `claude_agent_kit::HookMatcher` |  |
| `InMemorySessionStore` | `claude_agent_kit::InMemorySessionStore` |  |
| `McpSdkServerConfig` | `claude_agent_kit::McpSdkServerConfig` |  |
| `McpServerConfig` | `claude_agent_kit::McpServerConfig` |  |
| `McpServerConnectionStatus` | `claude_agent_kit::McpServerConnectionStatus` |  |
| `McpServerInfo` | `claude_agent_kit::McpServerInfo` |  |
| `McpServerStatus` | `claude_agent_kit::McpServerStatus` |  |
| `McpServerStatusConfig` | `claude_agent_kit::McpServerStatusConfig` |  |
| `McpStatusResponse` | `claude_agent_kit::McpStatusResponse` |  |
| `McpToolAnnotations` | `claude_agent_kit::McpToolAnnotations` |  |
| `McpToolInfo` | `claude_agent_kit::McpToolInfo` |  |
| `Message` | `claude_agent_kit::Message` |  |
| `MessageDisplayHookSpecificOutput` | `claude_agent_kit::MessageDisplayHookSpecificOutput` |  |
| `MessageOrigin` | `claude_agent_kit::MessageOrigin` |  |
| `MessageOriginKind` | `claude_agent_kit::MessageOriginKind` |  |
| `MirrorErrorMessage` | `claude_agent_kit::MirrorErrorMessage` |  |
| `ModelUsage` | `claude_agent_kit::ModelUsage` |  |
| `NotificationHookInput` | `claude_agent_kit::NotificationHookInput` |  |
| `NotificationHookSpecificOutput` | `claude_agent_kit::NotificationHookSpecificOutput` |  |
| `PermissionDeniedHookSpecificOutput` | `claude_agent_kit::PermissionDeniedHookSpecificOutput` |  |
| `PermissionMode` | `claude_agent_kit::PermissionMode` |  |
| `PermissionRequestHookInput` | `claude_agent_kit::PermissionRequestHookInput` |  |
| `PermissionRequestHookSpecificOutput` | `claude_agent_kit::PermissionRequestHookSpecificOutput` |  |
| `PermissionResult` | `claude_agent_kit::PermissionResult` |  |
| `PermissionResultAllow` | `claude_agent_kit::PermissionResult::Allow` | Variant of `PermissionResult`. |
| `PermissionResultDeny` | `claude_agent_kit::PermissionResult::Deny` | Variant of `PermissionResult`. |
| `PermissionUpdate` | `claude_agent_kit::PermissionUpdate` |  |
| `PostModelSwitchHookSpecificOutput` | `claude_agent_kit::PostModelSwitchHookSpecificOutput` |  |
| `PostToolBatchHookSpecificOutput` | `claude_agent_kit::PostToolBatchHookSpecificOutput` |  |
| `PostToolUseFailureHookInput` | `claude_agent_kit::PostToolUseFailureHookInput` |  |
| `PostToolUseFailureHookSpecificOutput` | `claude_agent_kit::PostToolUseFailureHookSpecificOutput` |  |
| `PostToolUseHookInput` | `claude_agent_kit::PostToolUseHookInput` |  |
| `PostToolUseHookSpecificOutput` | `claude_agent_kit::PostToolUseHookSpecificOutput` |  |
| `PreCompactHookInput` | `claude_agent_kit::PreCompactHookInput` |  |
| `PreModelSwitchHookSpecificOutput` | `claude_agent_kit::PreModelSwitchHookSpecificOutput` |  |
| `PreToolUseHookInput` | `claude_agent_kit::PreToolUseHookInput` |  |
| `PreToolUseHookSpecificOutput` | `claude_agent_kit::PreToolUseHookSpecificOutput` |  |
| `ProcessError` | `claude_agent_kit::Error::Process` | Variant of `Error`. |
| `RateLimitEvent` | `claude_agent_kit::RateLimitEvent` |  |
| `RateLimitInfo` | `claude_agent_kit::RateLimitInfo` |  |
| `RateLimitStatus` | `claude_agent_kit::RateLimitStatus` |  |
| `RateLimitType` | `claude_agent_kit::RateLimitType` |  |
| `ResultError` | `claude_agent_kit::Error::ResultError` | Variant of `Error`; its fields are in `ResultErrorData`. |
| `ResultMessage` | `claude_agent_kit::ResultMessage` |  |
| `SDKSessionInfo` | `claude_agent_kit::SdkSessionInfo` |  |
| `SandboxIgnoreViolations` | `claude_agent_kit::SandboxIgnoreViolations` |  |
| `SandboxNetworkConfig` | `claude_agent_kit::SandboxNetworkConfig` |  |
| `SandboxSettings` | `claude_agent_kit::SandboxSettings` |  |
| `SdkBeta` | `claude_agent_kit::SdkBeta` |  |
| `SdkMcpTool` | `claude_agent_kit::SdkMcpTool` |  |
| `SdkPluginConfig` | `claude_agent_kit::SdkPluginConfig` |  |
| `ServerToolName` | `claude_agent_kit::ServerToolName` |  |
| `ServerToolResultBlock` | `claude_agent_kit::ServerToolResultBlock` |  |
| `ServerToolUseBlock` | `claude_agent_kit::ServerToolUseBlock` |  |
| `SessionCronSummary` | `claude_agent_kit::SessionCronSummary` |  |
| `SessionKey` | `claude_agent_kit::SessionKey` |  |
| `SessionListSubkeysKey` | `claude_agent_kit::SessionListSubkeysKey` |  |
| `SessionMessage` | `claude_agent_kit::SessionMessage` |  |
| `SessionStartHookSpecificOutput` | `claude_agent_kit::SessionStartHookSpecificOutput` |  |
| `SessionStore` | `claude_agent_kit::SessionStore` |  |
| `SessionStoreEntry` | `claude_agent_kit::SessionStoreEntry` |  |
| `SessionStoreFlushMode` | `claude_agent_kit::SessionStoreFlushMode` |  |
| `SessionStoreListEntry` | `claude_agent_kit::SessionStoreListEntry` |  |
| `SessionSummaryEntry` | `claude_agent_kit::SessionSummaryEntry` |  |
| `SettingSource` | `claude_agent_kit::SettingSource` |  |
| `StopHookInput` | `claude_agent_kit::StopHookInput` |  |
| `StopHookSpecificOutput` | `claude_agent_kit::StopHookSpecificOutput` |  |
| `StreamEvent` | `claude_agent_kit::StreamEvent` |  |
| `SubagentStartHookInput` | `claude_agent_kit::SubagentStartHookInput` |  |
| `SubagentStartHookSpecificOutput` | `claude_agent_kit::SubagentStartHookSpecificOutput` |  |
| `SubagentStopHookInput` | `claude_agent_kit::SubagentStopHookInput` |  |
| `SubagentStopHookSpecificOutput` | `claude_agent_kit::SubagentStopHookSpecificOutput` |  |
| `SystemInitData` | `claude_agent_kit::SystemInitData` |  |
| `SystemInitMcpServer` | `claude_agent_kit::SystemInitMcpServer` |  |
| `SystemInitPlugin` | `claude_agent_kit::SystemInitPlugin` |  |
| `SystemInitPluginError` | `claude_agent_kit::SystemInitPluginError` |  |
| `SystemMessage` | `claude_agent_kit::SystemMessage` |  |
| `TERMINAL_TASK_STATUSES` | `claude_agent_kit::TERMINAL_TASK_STATUSES` |  |
| `TaskBudget` | `claude_agent_kit::TaskBudget` |  |
| `TaskNotificationMessage` | `claude_agent_kit::TaskNotificationMessage` |  |
| `TaskNotificationOriginSubkind` | `claude_agent_kit::TaskNotificationOriginSubkind` |  |
| `TaskNotificationStatus` | `claude_agent_kit::TaskNotificationStatus` |  |
| `TaskProgressMessage` | `claude_agent_kit::TaskProgressMessage` |  |
| `TaskStartedMessage` | `claude_agent_kit::TaskStartedMessage` |  |
| `TaskUpdatedMessage` | `claude_agent_kit::TaskUpdatedMessage` |  |
| `TaskUpdatedStatus` | `claude_agent_kit::TaskUpdatedStatus` |  |
| `TaskUsage` | `claude_agent_kit::TaskUsage` |  |
| `TextBlock` | `claude_agent_kit::TextBlock` |  |
| `ThinkingBlock` | `claude_agent_kit::ThinkingBlock` |  |
| `ThinkingConfig` | `claude_agent_kit::ThinkingConfig` |  |
| `ThinkingConfigAdaptive` | `claude_agent_kit::ThinkingConfig::Adaptive` | Variant of `ThinkingConfig`. |
| `ThinkingConfigDisabled` | `claude_agent_kit::ThinkingConfig::Disabled` | Variant of `ThinkingConfig`. |
| `ThinkingConfigEnabled` | `claude_agent_kit::ThinkingConfig::Enabled` | Variant of `ThinkingConfig`. |
| `ToolAnnotations` | `claude_agent_kit::ToolAnnotations` |  |
| `ToolPermissionContext` | `claude_agent_kit::ToolPermissionContext` |  |
| `ToolResultBlock` | `claude_agent_kit::ToolResultBlock` |  |
| `ToolUseBlock` | `claude_agent_kit::ToolUseBlock` |  |
| `Transport` | `claude_agent_kit::Transport` | A trait; `SubprocessTransport` is the implementation that runs the CLI. |
| `UserMessage` | `claude_agent_kit::UserMessage` |  |
| `UserPromptExpansionHookSpecificOutput` | `claude_agent_kit::UserPromptExpansionHookSpecificOutput` |  |
| `UserPromptSubmitHookInput` | `claude_agent_kit::UserPromptSubmitHookInput` |  |
| `UserPromptSubmitHookSpecificOutput` | `claude_agent_kit::UserPromptSubmitHookSpecificOutput` |  |
| `WorktreeCreateHookSpecificOutput` | `claude_agent_kit::WorktreeCreateHookSpecificOutput` |  |
| `__version__` | `claude_agent_kit::VERSION` |  |
| `create_sdk_mcp_server` | `claude_agent_kit::create_sdk_mcp_server` |  |
| `delete_session` | `claude_agent_kit::delete_session` |  |
| `delete_session_via_store` | `claude_agent_kit::delete_session_via_store` |  |
| `fold_session_summary` | `claude_agent_kit::fold_session_summary` |  |
| `fork_session` | `claude_agent_kit::fork_session` |  |
| `fork_session_via_store` | `claude_agent_kit::fork_session_via_store` |  |
| `get_session_info` | `claude_agent_kit::get_session_info` |  |
| `get_session_info_from_store` | `claude_agent_kit::get_session_info_from_store` |  |
| `get_session_messages` | `claude_agent_kit::get_session_messages` |  |
| `get_session_messages_from_store` | `claude_agent_kit::get_session_messages_from_store` |  |
| `get_subagent_messages` | `claude_agent_kit::get_subagent_messages` |  |
| `get_subagent_messages_from_store` | `claude_agent_kit::get_subagent_messages_from_store` |  |
| `import_session_to_store` | `claude_agent_kit::import_session_to_store` |  |
| `list_sessions` | `claude_agent_kit::list_sessions` |  |
| `list_sessions_from_store` | `claude_agent_kit::list_sessions_from_store` |  |
| `list_subagents` | `claude_agent_kit::list_subagents` |  |
| `list_subagents_from_store` | `claude_agent_kit::list_subagents_from_store` |  |
| `project_key_for_directory` | `claude_agent_kit::project_key_for_directory` |  |
| `query` | `claude_agent_kit::query` | Returns a `Stream` of `Result<Message, Error>`. |
| `rename_session` | `claude_agent_kit::rename_session` |  |
| `rename_session_via_store` | `claude_agent_kit::rename_session_via_store` |  |
| `tag_session` | `claude_agent_kit::tag_session` |  |
| `tag_session_via_store` | `claude_agent_kit::tag_session_via_store` |  |
| `tool` | `claude_agent_kit::tool` | Attribute macro behind the `macros` feature; `SdkMcpTool::typed` and `SdkMcpTool::raw` need no feature. |
<!-- parity:table:end -->
