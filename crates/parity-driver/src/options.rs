//! Translation of the `options` object of a case into
//! [`ClaudeAgentOptions`].
//!
//! Keys are the field names of the Python `ClaudeAgentOptions`, and values
//! keep the Python shapes: a `TypedDict` becomes a JSON object with the
//! same keys, a dataclass such as `AgentDefinition` an object keyed by its
//! field names. Callbacks cannot travel as data, so a case declares canned
//! ones:
//!
//! - `can_use_tool`: `{"behavior": "allow", "updated_input": {...},
//!   "updated_permissions": ...}`, where `updated_permissions` is absent,
//!   a list of updates in their wire form, or `"suggestions"` to send back
//!   the suggestions the callback received;
//!   `{"behavior": "deny", "message": "...", "interrupt": false}`; or
//!   `{"behavior": "wait"}`, which never answers, so that the CLI can
//!   cancel the request.
//! - `hooks`: `{"PreToolUse": [{"matcher": "Bash", "timeout": 5,
//!   "hooks": [output, ...]}]}`, where each output is the hook output in
//!   its wire form (`continue`, `hookSpecificOutput`, `async`, ...).
//! - `mcp_servers` entries with `"type": "sdk"`: `{"type": "sdk",
//!   "version": "1.0.0", "tools": [{"name", "description", "input_schema",
//!   "annotations", "result": {"content": [...], "is_error": false}}]}`,
//!   where a content block is text, an image, a resource link, or an
//!   embedded resource; a tool with `"raise": "text"` fails with that
//!   text, and one with `"wait": true` never returns.
//! - `stderr`: `true` registers a callback that journals each line.
//! - `session_store`: `"memory"` uses an in-memory store that journals
//!   each append.
//!
//! Every callback journals the arguments it received before it answers
//! (see the `journal` module): a hook callback names itself
//! `<event>/<matcher index>/<hook index>`.
//!
//! `cli_path` is set by the driver, so a case cannot set it.
//!
//! Options are applied, and hook events registered, in the order the case
//! writes them, as the Python driver does: the order of hook events decides
//! the callback ids the SDK sends in the initialize request.

use std::path::PathBuf;

use claude_agent_rust_sdk::{
    AgentDefinition, AgentEffort, AgentMcpServer, AgentMemory, AsyncHookOutput, CanUseTool,
    ClaudeAgentOptions, ClaudeAgentOptionsBuilder, EffortLevel, HookCallback, HookMatcher,
    HookOutput, McpRemoteServerConfig, McpServerConfig, McpServers, McpStdioServerConfig,
    PermissionMode, PermissionResult, PermissionUpdate, SandboxSettings, SdkBeta, SdkMcpTool,
    SdkPluginConfig, SettingSource, Skills, StderrCallback, SyncHookOutput, SystemPrompt,
    SystemPromptCustom, SystemPromptFile, SystemPromptPreset, TaskBudget, ThinkingConfig,
    ToolAnnotations, ToolContent, ToolResult, Tools, ToolsPreset, create_sdk_mcp_server,
};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::{Failure, journal, store};

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

/// Sets one option of a case on the builder. A `tools` object maps to the
/// `claude_code` preset whatever it holds: Python writes `--tools default`
/// for any object, and that preset, the one Rust models, writes the same.
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
                builder.stderr(StderrCallback::from_sync(|line| {
                    journal::write(
                        "stderr",
                        journal::fields([("line", Value::String(line.to_owned()))]),
                    );
                }))
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
        "agents" => builder.agents(agents(&field)?),
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
            Some("memory") => builder.session_store(store::JournaledStore::new()),
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

/// Builds each agent with the setters a program calls, not through
/// `serde`, so the case shows what those setters send.
fn agents(field: &Field<'_>) -> Result<IndexMap<String, AgentDefinition>, Failure> {
    let Some(agents) = field.value.as_object() else {
        return Err(field.malformed());
    };
    let mut built = IndexMap::new();
    for (name, agent) in agents {
        let agent = agent.as_object().ok_or_else(|| field.malformed())?;
        let text = |key: &str| agent.get(key).and_then(Value::as_str).map(str::to_owned);
        let list = |key: &str| -> Option<Vec<String>> {
            agent.get(key).and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
        };
        let mut definition = AgentDefinition::new(
            text("description").ok_or_else(|| field.malformed())?,
            text("prompt").ok_or_else(|| field.malformed())?,
        );
        for key in agent.keys() {
            definition = match key.as_str() {
                "description" | "prompt" => definition,
                "tools" => definition.tools(list(key).ok_or_else(|| field.malformed())?),
                "disallowedTools" => {
                    definition.disallowed_tools(list(key).ok_or_else(|| field.malformed())?)
                }
                "model" => definition.model(text(key).ok_or_else(|| field.malformed())?),
                "skills" => definition.skills(list(key).ok_or_else(|| field.malformed())?),
                "memory" => definition.memory(AgentMemory::from(
                    text(key).ok_or_else(|| field.malformed())?.as_str(),
                )),
                "mcpServers" => {
                    let servers = agent[key].as_array().ok_or_else(|| field.malformed())?;
                    let mut converted = Vec::new();
                    for server in servers {
                        converted.push(match server {
                            Value::String(name) => AgentMcpServer::from(name.as_str()),
                            Value::Object(inline) => AgentMcpServer::Inline(inline.clone()),
                            _ => return Err(field.malformed()),
                        });
                    }
                    definition.mcp_servers(converted)
                }
                "initialPrompt" => {
                    definition.initial_prompt(text(key).ok_or_else(|| field.malformed())?)
                }
                "maxTurns" => definition.max_turns(
                    agent[key]
                        .as_u64()
                        .and_then(|turns| u32::try_from(turns).ok())
                        .ok_or_else(|| field.malformed())?,
                ),
                "background" => {
                    definition.background(agent[key].as_bool().ok_or_else(|| field.malformed())?)
                }
                "effort" => definition.effort(match &agent[key] {
                    Value::String(level) => AgentEffort::Level(EffortLevel::from(level.as_str())),
                    Value::Number(tokens) => {
                        AgentEffort::Tokens(tokens.as_u64().ok_or_else(|| field.malformed())?)
                    }
                    _ => return Err(field.malformed()),
                }),
                "permissionMode" => definition.permission_mode(PermissionMode::from(
                    text(key).ok_or_else(|| field.malformed())?.as_str(),
                )),
                _ => return Err(field.malformed()),
            };
        }
        built.insert(name.clone(), definition);
    }
    Ok(built)
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
) -> Result<claude_agent_rust_sdk::McpSdkServerConfig, Failure> {
    let version = field.value.get("version").and_then(Value::as_str);
    let mut tools = Vec::new();
    for tool in field
        .value
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        tools.push(sdk_tool(field, name, tool)?);
    }
    create_sdk_mcp_server(name, version, tools)
        .map_err(|error| Failure::Failed(format!("in-process server {name}: {error}")))
}

fn sdk_tool(field: &Field<'_>, server: &str, tool: &Value) -> Result<SdkMcpTool, Failure> {
    let text = |key: &str| tool.get(key).and_then(Value::as_str).map(str::to_owned);
    let name = text("name").ok_or_else(|| field.malformed())?;
    let description = text("description").unwrap_or_default();
    let schema = tool
        .get("input_schema")
        .cloned()
        .ok_or_else(|| field.malformed())?;
    let result = tool_result(field, tool.get("result"))?;
    let failure = text("raise");
    let waits = tool.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let (server, tool_name) = (server.to_owned(), name.clone());
    let mut definition = SdkMcpTool::raw(name, description, schema, move |arguments| {
        journal::write(
            "tool",
            journal::fields([
                ("server", Value::String(server.clone())),
                ("tool", Value::String(tool_name.clone())),
                ("arguments", arguments),
            ]),
        );
        let result = result.clone();
        let failure = failure.clone();
        async move {
            if let Some(failure) = failure {
                return Err(failure.into());
            }
            if waits {
                std::future::pending::<()>().await;
            }
            Ok(result)
        }
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
            Some("resource_link") => {
                ToolContent::resource_link_parts(text("name"), text("uri"), text("description"))
            }
            Some("resource") => {
                let resource = block.get("resource").ok_or_else(|| field.malformed())?;
                let part = |key: &str| resource.get(key).and_then(Value::as_str).map(str::to_owned);
                let uri = part("uri").unwrap_or_default();
                match (part("text"), part("blob")) {
                    (Some(text), _) => ToolContent::text_resource(uri, text),
                    (None, Some(blob)) => ToolContent::blob_resource(uri, blob),
                    (None, None) => return Err(field.malformed()),
                }
            }
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
    Ok(ToolResult::new(content).with_error_flag(is_error))
}

/// What the canned permission callback answers.
#[derive(Clone)]
enum Decision {
    /// A fixed result.
    Fixed(PermissionResult),
    /// An allow that sends back the suggestions the callback received.
    AllowSuggestions(Option<Value>),
    /// Never answers, so that the request can be cancelled.
    Wait,
}

fn can_use_tool(field: &Field<'_>) -> Result<CanUseTool, Failure> {
    let decision = field.value;
    let decision = match decision.get("behavior").and_then(Value::as_str) {
        Some("allow") => {
            let input = decision.get("updated_input").cloned();
            match decision.get("updated_permissions") {
                None => Decision::Fixed(
                    input.map_or_else(PermissionResult::allow, PermissionResult::allow_with_input),
                ),
                Some(Value::String(source)) if source == "suggestions" => {
                    Decision::AllowSuggestions(input)
                }
                Some(Value::Array(updates)) => {
                    let updates = updates
                        .iter()
                        .map(PermissionUpdate::from_wire)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| field.malformed())?;
                    let allow = input
                        .map_or_else(PermissionResult::allow, PermissionResult::allow_with_input);
                    Decision::Fixed(allow.updated_permissions(updates))
                }
                Some(_) => return Err(field.malformed()),
            }
        }
        Some("deny") => {
            let message = decision
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let interrupt = decision
                .get("interrupt")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Decision::Fixed(if interrupt {
                PermissionResult::deny_and_interrupt(message)
            } else {
                PermissionResult::deny(message)
            })
        }
        Some("wait") => Decision::Wait,
        _ => return Err(field.malformed()),
    };
    Ok(CanUseTool::new(move |tool_name, input, context| {
        journal::write(
            "can_use_tool",
            journal::fields([
                ("tool_name", Value::String(tool_name)),
                ("input", input),
                ("context", journal::permission_context(&context)),
            ]),
        );
        let decision = decision.clone();
        async move {
            match decision {
                Decision::Fixed(result) => Ok(result),
                Decision::AllowSuggestions(input) => Ok(input
                    .map_or_else(PermissionResult::allow, PermissionResult::allow_with_input)
                    .updated_permissions(context.suggestions)),
                Decision::Wait => std::future::pending().await,
            }
        }
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
        for (index, matcher) in matchers.iter().enumerate() {
            let name = format!("{event}/{index}");
            builder = builder.hook(event.as_str(), hook_matcher(&field, &name, matcher)?);
        }
    }
    Ok(builder)
}

fn hook_matcher(field: &Field<'_>, name: &str, matcher: &Value) -> Result<HookMatcher, Failure> {
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
    for (index, output) in matcher
        .get("hooks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let output = hook_output(field, output)?;
        let callback = format!("{name}/{index}");
        built = built.hook(HookCallback::new(move |input, tool_use_id, _context| {
            journal::write(
                "hook",
                journal::fields([
                    ("callback", Value::String(callback.clone())),
                    ("input", journal::hook_input(&input)),
                    (
                        "tool_use_id",
                        tool_use_id.map_or(Value::Null, Value::String),
                    ),
                    ("context", Value::Object(Map::new())),
                ]),
            );
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
