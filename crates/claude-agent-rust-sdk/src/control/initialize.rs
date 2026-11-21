//! What a session takes from the options: the `initialize` request, the
//! hook callbacks it registers, and the settings of the control channel.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use super::TranscriptMirror;
use super::run_end::{parse_python_int, run_end_ceiling_ms};
use crate::error::{Error, Result};
use crate::mcp::McpBridges;
use crate::mirror::TranscriptMirrorBatcher;
use crate::sessions::projects_dir;
use crate::types::{
    CanUseTool, ClaudeAgentOptions, HookCallback, HookEvent, HookFailureMode, HookMatcher,
    McpServerConfig, McpServers, Skills, SystemPrompt,
};

pub(crate) const STREAM_CLOSE_TIMEOUT_VARIABLE: &str = "CLAUDE_CODE_STREAM_CLOSE_TIMEOUT";
const MINIMUM_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// Settings of one session, read once from the options.
pub(crate) struct SessionConfig {
    pub(crate) can_use_tool: Option<CanUseTool>,
    pub(crate) hook_callbacks: HashMap<String, HookCallback>,
    pub(crate) initialize_request: Value,
    pub(crate) blocks_on_failure: bool,
    pub(crate) initialize_timeout: Duration,
    pub(crate) verbatim_prompts: bool,
    pub(crate) run_end_ceiling_ms: u64,
    pub(crate) bidirectional: bool,
    pub(crate) mcp: McpBridges,
    pub(crate) mirror: Option<Arc<dyn TranscriptMirror>>,
}

impl SessionConfig {
    pub(crate) fn from_options(options: &ClaudeAgentOptions) -> Result<Self> {
        let mcp = McpBridges::from_servers(&options.mcp_servers)?;
        let hooks = register_hooks(options.hooks.as_ref());
        let has_hooks = options
            .hooks
            .as_ref()
            .is_some_and(|hooks| !hooks.is_empty());
        Ok(Self {
            can_use_tool: options.can_use_tool.clone(),
            initialize_request: initialize_request(options, hooks.config),
            hook_callbacks: hooks.callbacks,
            blocks_on_failure: hooks.blocks_on_failure,
            initialize_timeout: initialize_timeout(
                std::env::var(STREAM_CLOSE_TIMEOUT_VARIABLE).ok().as_deref(),
            ),
            verbatim_prompts: options.verbatim_prompts,
            run_end_ceiling_ms: run_end_ceiling_ms(&options.env),
            bidirectional: has_hooks
                || options.can_use_tool.is_some()
                || has_sdk_mcp_server(&options.mcp_servers),
            mcp,
            mirror: options
                .session_store
                .as_ref()
                .map(|store| {
                    let projects = projects_dir(Some(&options.env)).ok_or_else(|| {
                        Error::spawn(
                            "Cannot mirror transcripts: CLAUDE_CONFIG_DIR is not set and \
                             the home directory cannot be found",
                        )
                    })?;
                    let mirror: Arc<dyn TranscriptMirror> = TranscriptMirrorBatcher::new(
                        store.clone(),
                        projects,
                        &options.session_store_flush,
                    );
                    Ok::<_, Error>(mirror)
                })
                .transpose()?,
        })
    }
}

/// The initialize timeout: the stream close timeout of the environment, in
/// milliseconds, and never less than 60 s. MCP servers may take time to
/// start.
///
/// The value is read as Python's `int()` reads it, so `120_000` is two
/// minutes. Where `int()` raises, upstream fails the connect; this gives
/// 60 s and logs a warning.
pub(crate) fn initialize_timeout(raw: Option<&str>) -> Duration {
    let Some(raw) = raw else {
        return MINIMUM_INITIALIZE_TIMEOUT;
    };
    let Some(milliseconds) = parse_python_int(raw) else {
        tracing::warn!(
            "Ignoring {STREAM_CLOSE_TIMEOUT_VARIABLE}={raw:?}, which is not an integer; \
             the initialize timeout stays at 60 s"
        );
        return MINIMUM_INITIALIZE_TIMEOUT;
    };
    u64::try_from(milliseconds)
        .map_or(Duration::ZERO, Duration::from_millis)
        .max(MINIMUM_INITIALIZE_TIMEOUT)
}

fn has_sdk_mcp_server(servers: &McpServers) -> bool {
    match servers {
        McpServers::Map(servers) => servers
            .values()
            .any(|server| matches!(server, McpServerConfig::Sdk(_))),
        McpServers::Json(_) | McpServers::Path(_) => false,
    }
}

struct HookRegistration {
    config: Option<Value>,
    callbacks: HashMap<String, HookCallback>,
    blocks_on_failure: bool,
}

fn register_hooks(hooks: Option<&IndexMap<HookEvent, Vec<HookMatcher>>>) -> HookRegistration {
    let mut config = Map::new();
    let mut callbacks = HashMap::new();
    let mut blocks_on_failure = false;
    for (event, matchers) in hooks.into_iter().flatten() {
        if matchers.is_empty() {
            continue;
        }
        let mut matcher_configs = Vec::with_capacity(matchers.len());
        for matcher in matchers {
            let mut callback_ids = Vec::with_capacity(matcher.hooks.len());
            for callback in &matcher.hooks {
                let callback_id = format!("hook_{}", callbacks.len());
                callbacks.insert(callback_id.clone(), callback.clone());
                callback_ids.push(callback_id);
            }
            let mut matcher_config = Map::new();
            matcher_config.insert("matcher".to_owned(), json!(matcher.matcher));
            matcher_config.insert("hookCallbackIds".to_owned(), json!(callback_ids));
            if let Some(timeout) = matcher.timeout {
                matcher_config.insert("timeout".to_owned(), seconds(timeout));
            }
            if let Some(on_failure) = &matcher.on_failure {
                matcher_config.insert("onFailure".to_owned(), json!(on_failure.as_str()));
                blocks_on_failure |= *on_failure != HookFailureMode::Continue;
            }
            matcher_configs.push(Value::Object(matcher_config));
        }
        config.insert(event.as_str().to_owned(), Value::Array(matcher_configs));
    }
    HookRegistration {
        config: (!config.is_empty()).then_some(Value::Object(config)),
        callbacks,
        blocks_on_failure,
    }
}

fn seconds(duration: Duration) -> Value {
    if duration.subsec_nanos() == 0 {
        json!(duration.as_secs())
    } else {
        json!(duration.as_secs_f64())
    }
}

fn initialize_request(options: &ClaudeAgentOptions, hooks: Option<Value>) -> Value {
    let mut request = Map::new();
    request.insert("subtype".to_owned(), json!("initialize"));
    request.insert("hooks".to_owned(), hooks.unwrap_or(Value::Null));
    if let Some(agents) = options.agents.as_ref().filter(|agents| !agents.is_empty()) {
        request.insert("agents".to_owned(), json!(agents));
    }
    let (exclude_dynamic_sections, snapshot) = match &options.system_prompt {
        Some(SystemPrompt::Preset(preset)) => (preset.exclude_dynamic_sections, preset.snapshot),
        Some(SystemPrompt::Custom(custom)) => (None, custom.snapshot),
        Some(SystemPrompt::Text(_) | SystemPrompt::File(_)) | None => (None, None),
    };
    if let Some(exclude) = exclude_dynamic_sections {
        request.insert("excludeDynamicSections".to_owned(), json!(exclude));
    }
    if let Some(snapshot) = snapshot {
        request.insert("systemPromptSnapshot".to_owned(), json!(snapshot));
    }
    if let Some(Skills::List(skills)) = &options.skills {
        request.insert("skills".to_owned(), json!(skills));
    }
    if options.forward_subagent_text {
        request.insert("forwardSubagentText".to_owned(), json!(true));
    }
    Value::Object(request)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::initialize_timeout;

    #[test]
    fn initialize_timeout_is_the_larger_of_sixty_seconds_and_the_variable() {
        let cases: [(Option<&str>, u64); 8] = [
            (None, 60_000),
            (Some("120000"), 120_000),
            (Some(" 90000 "), 90_000),
            (Some("60001"), 60_001),
            (Some("5000"), 60_000),
            (Some("0"), 60_000),
            (Some("-90000"), 60_000),
            (Some("soon"), 60_000),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                initialize_timeout(raw),
                Duration::from_millis(expected),
                "raw {raw:?}"
            );
        }
    }

    #[test]
    fn the_stream_close_timeout_is_read_with_the_grammar_of_python_int() {
        let cases: [(&str, u64); 5] = [
            ("120_000", 120_000),
            ("+90000", 90_000),
            ("\t75000\n", 75_000),
            ("1__000000", 60_000),
            ("90000_", 60_000),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                initialize_timeout(Some(raw)),
                Duration::from_millis(expected),
                "raw {raw:?}"
            );
        }
    }
}
