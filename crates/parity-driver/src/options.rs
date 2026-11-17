//! Translation of the `options` object of a case into
//! [`ClaudeAgentOptions`].
//!
//! Keys are the field names of the Python `ClaudeAgentOptions`, and values
//! keep the Python shapes: a `TypedDict` becomes a JSON object with the
//! same keys, a dataclass such as `AgentDefinition` an object keyed by its
//! field names. Callbacks cannot travel as data, so a case declares canned
//! ones:
//!
//! - `can_use_tool`: `{"behavior": "allow", "updated_input": {...}}` or
//!   `{"behavior": "deny", "message": "...", "interrupt": false}`.
//! - `hooks`: `{"PreToolUse": [{"matcher": "Bash", "timeout": 5,
//!   "hooks": [output, ...]}]}`, where each output is the hook output in
//!   its wire form (`continue`, `hookSpecificOutput`, `async`, ...).
//! - `mcp_servers` entries with `"type": "sdk"`: `{"type": "sdk",
//!   "version": "1.0.0", "tools": [{"name", "description", "input_schema",
//!   "annotations", "result": {"content": [...], "is_error": false}}]}`.
//! - `stderr`: `true` registers a callback that discards each line.
//! - `session_store`: `"memory"` uses an in-memory store.
//!
//! `cli_path` is set by the driver, so a case cannot set it.
//!
//! Options are applied, and hook events registered, in the order the case
//! writes them, as the Python driver does: the order of hook events decides
//! the callback ids the SDK sends in the initialize request.

use std::path::PathBuf;

use claude_agent_kit::{
    AgentDefinition, AsyncHookOutput, CanUseTool, ClaudeAgentOptions, ClaudeAgentOptionsBuilder,
    HookCallback, HookMatcher, HookOutput, InMemorySessionStore, McpRemoteServerConfig,
    McpServerConfig, McpServers, McpStdioServerConfig, PermissionResult, SandboxSettings, SdkBeta,
    SdkMcpTool, SdkPluginConfig, SettingSource, Skills, StderrCallback, SyncHookOutput,
    SystemPrompt, SystemPromptCustom, SystemPromptFile, SystemPromptPreset, TaskBudget,
    ThinkingConfig, ToolAnnotations, ToolContent, ToolResult, Tools, ToolsPreset,
    create_sdk_mcp_server,
};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::Failure;

/// Builds the options of a case. `cli` is the path of the CLI to run.
pub(crate) fn build(text: &str, cli: PathBuf) -> Result<ClaudeAgentOptions, Failure> {
    let unreadable =
        |error: serde_json::Error| Failure::Failed(format!("`options` cannot be read: {error}"));
    let ordered: Ordered = serde_json::from_str(text).map_err(unreadable)?;
    let events: HookEvents = serde_json::from_str(text).map_err(unreadable)?;
    let mut builder = ClaudeAgentOptions::builder().cli_path(cli);
    for (name, value) in &ordered.options {
        builder = if name == "hooks" {
            hooks(builder, &events.options.hooks, value)?
        } else {
            apply(builder, name, value)?
        };
    }
    builder
        .build()
        .map_err(|error| Failure::Failed(format!("invalid options: {error}")))
}

fn apply(
    builder: ClaudeAgentOptionsBuilder,
    name: &str,
    value: &Value,
) -> Result<ClaudeAgentOptionsBuilder, Failure> {
    let field = Field { name, value };
    Ok(match name {
        "tools" => match value {
            Value::Array(_) => builder.tools(Tools::List(field.strings()?)),
            Value::Object(_) => builder.tools(ToolsPreset::ClaudeCode),
            _ => return Err(field.malformed()),
        },
        "allowed_tools" => builder.allowed_tools(field.strings()?),
        "disallowed_tools" => builder.disallowed_tools(field.strings()?),
        "system_prompt" => match value {
            Value::Null => builder,
            Value::String(text) => builder.system_prompt(SystemPrompt::Text(text.clone())),
            Value::Object(prompt) => builder.system_prompt(system_prompt(&field, prompt)?),
            _ => return Err(field.malformed()),
        },
        "mcp_servers" => builder.mcp_servers(mcp_servers(&field)?),
        "strict_mcp_config" => builder.strict_mcp_config(field.flag()?),
        "permission_mode" => builder.permission_mode(field.text()?),
        "continue_conversation" => builder.continue_conversation(field.flag()?),
        "resume" => builder.resume(field.text()?),
        "session_id" => builder.session_id(field.text()?),
        "max_turns" => builder.max_turns(field.integer()?),
        "max_budget_usd" => {
            builder.max_budget_usd(value.as_f64().ok_or_else(|| field.malformed())?)
        }
        "model" => builder.model(field.text()?),
        "fallback_model" => builder.fallback_model(field.text()?),
        "betas" => builder.betas(field.strings()?.into_iter().map(SdkBeta::from)),
        "permission_prompt_tool_name" => builder.permission_prompt_tool_name(field.text()?),
        "cwd" => builder.cwd(PathBuf::from(field.text()?)),
        "settings" => builder.settings(field.text()?),
        "add_dirs" => builder.add_dirs(field.strings()?.into_iter().map(PathBuf::from)),
        "env" => builder.env(field.string_map()?),
        "extra_args" => builder.extra_args(field.optional_string_map()?),
        "max_buffer_size" => builder.max_buffer_size(field.integer()?),
        "stderr" => {
            if field.flag()? {
                builder.stderr(StderrCallback::from_sync(|_| {}))
            } else {
                builder
            }
        }
        "can_use_tool" => builder.can_use_tool(can_use_tool(&field)?),
        "user" => builder.user(field.text()?),
        "include_partial_messages" => builder.include_partial_messages(field.flag()?),
        "include_hook_events" => builder.include_hook_events(field.flag()?),
        "forward_subagent_text" => builder.forward_subagent_text(field.flag()?),
        "verbatim_prompts" => builder.verbatim_prompts(field.flag()?),
        "fork_session" => builder.fork_session(field.flag()?),
        "resume_session_at" => builder.resume_session_at(field.text()?),
        "resume_drops_turn" => builder.resume_drops_turn(field.text()?),
        "agents" => {
            let agents: IndexMap<String, AgentDefinition> = field.decode()?;
            builder.agents(agents)
        }
        "setting_sources" => {
            builder.setting_sources(field.strings()?.into_iter().map(SettingSource::from))
        }
        "skills" => match value {
            Value::String(all) if all == "all" => builder.skills(Skills::All),
            Value::Array(_) => builder.skills(Skills::List(field.strings()?)),
            _ => return Err(field.malformed()),
        },
        "sandbox" => builder.sandbox(field.decode::<SandboxSettings>()?),
        "plugins" => builder.plugins(field.decode::<Vec<SdkPluginConfig>>()?),
        "max_thinking_tokens" => builder.max_thinking_tokens(field.integer()?),
        "thinking" => builder.thinking(field.decode::<ThinkingConfig>()?),
        "effort" => builder.effort(field.text()?),
        "output_format" => builder.output_format(value.clone()),
        "enable_file_checkpointing" => builder.enable_file_checkpointing(field.flag()?),
        "session_store" => match value.as_str() {
            Some("memory") => builder.session_store(InMemorySessionStore::new()),
            _ => return Err(field.malformed()),
        },
        "session_store_flush" => builder.session_store_flush(field.text()?.into()),
        "load_timeout_ms" => builder.load_timeout_ms(field.integer()?),
        "task_budget" => builder.task_budget(field.decode::<TaskBudget>()?),
        "client_app" => builder.client_app(field.text()?),
        _ => return Err(Failure::Unsupported(format!("option {name}"))),
    })
}

/// The options of a case, in the order the case writes them.
#[derive(serde::Deserialize)]
struct Ordered {
    #[serde(default)]
    options: IndexMap<String, Value>,
}

/// The hook events of a case, in the order the case writes them.
#[derive(serde::Deserialize, Default)]
struct HookEvents {
    #[serde(default)]
    options: HookOptions,
}

#[derive(serde::Deserialize, Default)]
struct HookOptions {
    #[serde(default)]
    hooks: IndexMap<String, Vec<Value>>,
}

struct Field<'a> {
    name: &'a str,
    value: &'a Value,
}

impl Field<'_> {
    fn malformed(&self) -> Failure {
        Failure::Failed(format!(
            "option {} has an unexpected value: {}",
            self.name, self.value
        ))
    }

    fn text(&self) -> Result<String, Failure> {
        self.value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| self.malformed())
    }

    fn flag(&self) -> Result<bool, Failure> {
        self.value.as_bool().ok_or_else(|| self.malformed())
    }

    fn integer<T: TryFrom<u64>>(&self) -> Result<T, Failure> {
        self.value
            .as_u64()
            .and_then(|number| T::try_from(number).ok())
            .ok_or_else(|| self.malformed())
    }

    fn strings(&self) -> Result<Vec<String>, Failure> {
        self.decode()
    }

    fn string_map(&self) -> Result<IndexMap<String, String>, Failure> {
        self.decode()
    }

    fn optional_string_map(&self) -> Result<IndexMap<String, Option<String>>, Failure> {
        self.decode()
    }

    fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, Failure> {
        serde_json::from_value(self.value.clone()).map_err(|_| self.malformed())
    }
}

fn system_prompt(field: &Field<'_>, prompt: &Map<String, Value>) -> Result<SystemPrompt, Failure> {
    let text = |key: &str| prompt.get(key).and_then(Value::as_str).map(str::to_owned);
    let flag = |key: &str| prompt.get(key).and_then(Value::as_bool);
    match prompt.get("type").and_then(Value::as_str) {
        Some("preset") => {
            let mut preset = SystemPromptPreset::new();
            if let Some(append) = text("append") {
                preset = preset.append(append);
            }
            if let Some(exclude) = flag("exclude_dynamic_sections") {
                preset = preset.exclude_dynamic_sections(exclude);
            }
            if let Some(snapshot) = flag("snapshot") {
                preset = preset.snapshot(snapshot);
            }
            Ok(SystemPrompt::Preset(preset))
        }
        Some("custom") => {
            let mut custom =
                SystemPromptCustom::new(text("prompt").ok_or_else(|| field.malformed())?);
            if let Some(snapshot) = flag("snapshot") {
                custom = custom.snapshot(snapshot);
            }
            Ok(SystemPrompt::Custom(custom))
        }
        Some("file") => Ok(SystemPrompt::File(SystemPromptFile::new(
            text("path").ok_or_else(|| field.malformed())?,
        ))),
        _ => Err(field.malformed()),
    }
}

fn mcp_servers(field: &Field<'_>) -> Result<McpServers, Failure> {
    let servers = match field.value {
        Value::String(text) => {
            return Ok(
                if serde_json::from_str::<Value>(text).is_ok_and(|value| value.is_object()) {
                    McpServers::Json(text.clone())
                } else {
                    McpServers::Path(PathBuf::from(text))
                },
            );
        }
        Value::Object(servers) => servers,
        _ => return Err(field.malformed()),
    };
    let mut configs = IndexMap::new();
    for (name, config) in servers {
        let server = Field {
            name: field.name,
            value: config,
        };
        let config = match config.get("type").and_then(Value::as_str) {
            None | Some("stdio") => {
                McpServerConfig::Stdio(server.decode::<McpStdioServerConfig>()?)
            }
            Some("sse") => McpServerConfig::Sse(server.decode::<McpRemoteServerConfig>()?),
            Some("http") => McpServerConfig::Http(server.decode::<McpRemoteServerConfig>()?),
            Some("sdk") => McpServerConfig::Sdk(sdk_server(&server, name)?),
            Some(other) => return Err(Failure::Unsupported(format!("mcp server type {other}"))),
        };
        configs.insert(name.clone(), config);
    }
    Ok(McpServers::Map(configs))
}

fn sdk_server(
    field: &Field<'_>,
    name: &str,
) -> Result<claude_agent_kit::McpSdkServerConfig, Failure> {
    let version = field.value.get("version").and_then(Value::as_str);
    let mut tools = Vec::new();
    for tool in field
        .value
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        tools.push(sdk_tool(field, tool)?);
    }
    create_sdk_mcp_server(name, version, tools)
        .map_err(|error| Failure::Failed(format!("in-process server {name}: {error}")))
}

fn sdk_tool(field: &Field<'_>, tool: &Value) -> Result<SdkMcpTool, Failure> {
    let text = |key: &str| tool.get(key).and_then(Value::as_str).map(str::to_owned);
    let name = text("name").ok_or_else(|| field.malformed())?;
    let description = text("description").unwrap_or_default();
    let schema = tool
        .get("input_schema")
        .cloned()
        .ok_or_else(|| field.malformed())?;
    let result = tool_result(field, tool.get("result"))?;
    let mut definition = SdkMcpTool::raw(name, description, schema, move |_| {
        let result = result.clone();
        async move { Ok(result) }
    })
    .map_err(|error| Failure::Failed(error.to_string()))?;
    if let Some(annotations) = tool.get("annotations").and_then(Value::as_object) {
        definition = definition.annotations(tool_annotations(annotations));
    }
    Ok(definition)
}

fn tool_annotations(annotations: &Map<String, Value>) -> ToolAnnotations {
    let flag = |key: &str| annotations.get(key).and_then(Value::as_bool);
    let mut built = ToolAnnotations::new();
    if let Some(title) = annotations.get("title").and_then(Value::as_str) {
        built = built.title(title);
    }
    if let Some(hint) = flag("readOnlyHint") {
        built = built.read_only_hint(hint);
    }
    if let Some(hint) = flag("destructiveHint") {
        built = built.destructive_hint(hint);
    }
    if let Some(hint) = flag("idempotentHint") {
        built = built.idempotent_hint(hint);
    }
    if let Some(hint) = flag("openWorldHint") {
        built = built.open_world_hint(hint);
    }
    built
}

fn tool_result(field: &Field<'_>, result: Option<&Value>) -> Result<ToolResult, Failure> {
    let Some(result) = result else {
        return Ok(ToolResult::new(Vec::<ToolContent>::new()));
    };
    let mut content = Vec::new();
    for block in result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let text = |key: &str| block.get(key).and_then(Value::as_str).map(str::to_owned);
        content.push(match block.get("type").and_then(Value::as_str) {
            Some("text") => ToolContent::text(text("text").ok_or_else(|| field.malformed())?),
            Some("image") => ToolContent::image(
                text("data").ok_or_else(|| field.malformed())?,
                text("mimeType").ok_or_else(|| field.malformed())?,
            ),
            other => {
                return Err(Failure::Unsupported(format!(
                    "tool content type {}",
                    other.unwrap_or("(none)")
                )));
            }
        });
    }
    let is_error = result
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(ToolResult::new(content).is_error(is_error))
}

fn can_use_tool(field: &Field<'_>) -> Result<CanUseTool, Failure> {
    let decision = field.value;
    let result = match decision.get("behavior").and_then(Value::as_str) {
        Some("allow") => match decision.get("updated_input") {
            Some(input) => PermissionResult::allow_with_input(input.clone()),
            None => PermissionResult::allow(),
        },
        Some("deny") => {
            let message = decision
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let interrupt = decision
                .get("interrupt")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            PermissionResult::Deny {
                message: message.to_owned(),
                interrupt,
            }
        }
        _ => return Err(field.malformed()),
    };
    Ok(CanUseTool::new(move |_, _, _| {
        let result = result.clone();
        async move { Ok(result) }
    }))
}

fn hooks(
    mut builder: ClaudeAgentOptionsBuilder,
    events: &IndexMap<String, Vec<Value>>,
    value: &Value,
) -> Result<ClaudeAgentOptionsBuilder, Failure> {
    let field = Field {
        name: "hooks",
        value,
    };
    for (event, matchers) in events {
        for matcher in matchers {
            builder = builder.hook(event.as_str(), hook_matcher(&field, matcher)?);
        }
    }
    Ok(builder)
}

fn hook_matcher(field: &Field<'_>, matcher: &Value) -> Result<HookMatcher, Failure> {
    let mut built = HookMatcher::new();
    if let Some(pattern) = matcher.get("matcher").and_then(Value::as_str) {
        built = built.matcher(pattern);
    }
    if let Some(seconds) = matcher.get("timeout").and_then(Value::as_f64) {
        built = built.timeout(std::time::Duration::from_secs_f64(seconds));
    }
    if let Some(mode) = matcher.get("on_failure").and_then(Value::as_str) {
        built = built.on_failure(mode);
    }
    for output in matcher
        .get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let output = hook_output(field, output)?;
        built = built.hook(HookCallback::new(move |_, _, _| {
            let output = output.clone();
            async move { Ok(output) }
        }));
    }
    Ok(built)
}

fn hook_output(field: &Field<'_>, output: &Value) -> Result<HookOutput, Failure> {
    let canned = Field {
        name: field.name,
        value: output,
    };
    if output.get("async").is_some() {
        Ok(HookOutput::Async(canned.decode::<AsyncHookOutput>()?))
    } else {
        Ok(HookOutput::Sync(canned.decode::<SyncHookOutput>()?))
    }
}
