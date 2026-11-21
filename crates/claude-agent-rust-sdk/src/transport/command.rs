//! Construction of the CLI argument vector from the session options.

use std::ffi::OsString;
use std::path::Path;

use serde_json::{Map, Value};

use crate::cli::Platform;
use crate::error::{Error, Result};
use crate::types::{
    ClaudeAgentOptions, McpServers, SdkPluginType, Skills, SystemPrompt, ThinkingConfig, Tools,
};

const CMD_EXE_METACHARACTERS: &str = "&|<>^%!\"";
const SKILL_TOOL: &str = "Skill";
const DEFAULT_SKILL_SETTING_SOURCES: [&str; 2] = ["user", "project"];
const BYTE_ORDER_MARK: char = '\u{feff}';

pub(crate) fn build_arguments(
    options: &ClaudeAgentOptions,
    platform: Platform,
) -> Result<Vec<OsString>> {
    let mut arguments = Arguments::default();
    arguments.pair("--output-format", "stream-json");
    arguments.flag("--verbose");

    match &options.system_prompt {
        None => arguments.pair("--system-prompt", ""),
        Some(SystemPrompt::Text(text)) => arguments.pair("--system-prompt", text),
        Some(SystemPrompt::Custom(custom)) => arguments.pair("--system-prompt", &custom.prompt),
        Some(SystemPrompt::File(file)) => arguments.pair("--system-prompt-file", &file.path),
        Some(SystemPrompt::Preset(preset)) => {
            if let Some(append) = &preset.append {
                arguments.pair("--append-system-prompt", append);
            }
        }
    }

    match &options.tools {
        None => {}
        Some(Tools::List(names)) => arguments.pair("--tools", names.join(",")),
        Some(Tools::Preset(_)) => arguments.pair("--tools", "default"),
    }

    let (allowed_tools, setting_sources) = apply_skills_defaults(options)?;
    if !allowed_tools.is_empty() {
        arguments.pair("--allowedTools", allowed_tools.join(","));
    }

    if let Some(max_turns) = options.max_turns.filter(|turns| *turns != 0) {
        arguments.pair("--max-turns", max_turns.to_string());
    }
    if let Some(budget) = options.max_budget_usd {
        arguments.pair("--max-budget-usd", format!("{budget:?}"));
    }
    if !options.disallowed_tools.is_empty() {
        arguments.pair("--disallowedTools", options.disallowed_tools.join(","));
    }
    if let Some(task_budget) = &options.task_budget {
        arguments.pair("--task-budget", task_budget.total.to_string());
    }
    if let Some(model) = non_empty(options.model.as_deref()) {
        arguments.pair("--model", model);
    }
    if let Some(model) = non_empty(options.fallback_model.as_deref()) {
        arguments.pair("--fallback-model", model);
    }
    if !options.betas.is_empty() {
        let betas: Vec<&str> = options.betas.iter().map(|beta| beta.as_str()).collect();
        arguments.pair("--betas", betas.join(","));
    }
    let prompt_tool = non_empty(options.permission_prompt_tool_name.as_deref())
        .or_else(|| options.can_use_tool.is_some().then_some("stdio"));
    if let Some(tool) = prompt_tool {
        arguments.pair("--permission-prompt-tool", tool);
    }
    if let Some(mode) = &options.permission_mode
        && !mode.as_str().is_empty()
    {
        arguments.pair("--permission-mode", mode.as_str());
    }
    if options.continue_conversation {
        arguments.flag("--continue");
    }

    if let Some(resume) = non_empty(options.resume.as_deref()) {
        reject_cmd_metacharacters("resume", resume, platform)?;
        arguments.flag(format!("--resume={resume}"));
    }
    if let Some(session_id) = non_empty(options.session_id.as_deref()) {
        reject_cmd_metacharacters("session_id", session_id, platform)?;
        arguments.flag(format!("--session-id={session_id}"));
    }

    if let Some(settings) = build_settings_value(options)?
        && !settings.is_empty()
    {
        arguments.pair("--settings", settings);
    }

    for directory in &options.add_dirs {
        arguments.flag("--add-dir");
        arguments.flag(directory.as_os_str());
    }

    match &options.mcp_servers {
        McpServers::Map(servers) => {
            if !servers.is_empty() {
                let config = serde_json::json!({ "mcpServers": servers });
                arguments.pair("--mcp-config", config.to_string());
            }
        }
        McpServers::Json(json) => {
            if !json.is_empty() {
                arguments.pair("--mcp-config", json);
            }
        }
        McpServers::Path(path) => {
            if !path.as_os_str().is_empty() {
                arguments.flag("--mcp-config");
                arguments.flag(path.as_os_str());
            }
        }
    }

    if options.include_partial_messages {
        arguments.flag("--include-partial-messages");
    }
    if options.include_hook_events {
        arguments.flag("--include-hook-events");
    }
    if options.strict_mcp_config {
        arguments.flag("--strict-mcp-config");
    }
    if options.fork_session {
        arguments.flag("--fork-session");
    }

    if let Some(at) = non_empty(options.resume_session_at.as_deref()) {
        reject_cmd_metacharacters("resume_session_at", at, platform)?;
        arguments.flag(format!("--resume-session-at={at}"));
    }
    if let Some(drops) = options.resume_drops_turn.as_deref() {
        reject_cmd_metacharacters("resume_drops_turn", drops, platform)?;
        arguments.flag(format!("--resume-drops-turn={drops}"));
    }

    if options.session_store.is_some() {
        arguments.flag("--session-mirror");
    }

    if let Some(sources) = &setting_sources {
        arguments.flag(format!("--setting-sources={}", sources.join(",")));
    }

    for plugin in &options.plugins {
        match &plugin.kind {
            SdkPluginType::Local => arguments.pair("--plugin-dir", &plugin.path),
            other => {
                return Err(Error::config(format!("Unsupported plugin type: {other}")));
            }
        }
    }

    for (flag, value) in &options.extra_args {
        match value {
            None => arguments.flag(format!("--{flag}")),
            Some(value) if value.starts_with('-') => arguments.flag(format!("--{flag}={value}")),
            Some(value) => arguments.pair(format!("--{flag}"), value),
        }
    }

    match &options.thinking {
        Some(ThinkingConfig::Adaptive { display }) => {
            arguments.pair("--thinking", "adaptive");
            if let Some(display) = display {
                arguments.pair("--thinking-display", display.as_str());
            }
        }
        Some(ThinkingConfig::Enabled {
            budget_tokens,
            display,
        }) => {
            arguments.pair("--max-thinking-tokens", budget_tokens.to_string());
            if let Some(display) = display {
                arguments.pair("--thinking-display", display.as_str());
            }
        }
        Some(ThinkingConfig::Disabled) => arguments.pair("--thinking", "disabled"),
        None => {
            if let Some(tokens) = options.max_thinking_tokens {
                arguments.pair("--max-thinking-tokens", tokens.to_string());
            }
        }
    }

    if let Some(effort) = &options.effort {
        arguments.pair("--effort", effort.as_str());
    }

    if let Some(Value::Object(format)) = &options.output_format
        && format.get("type").and_then(Value::as_str) == Some("json_schema")
        && let Some(schema) = format.get("schema").filter(|schema| !schema.is_null())
    {
        arguments.pair("--json-schema", schema.to_string());
    }

    arguments.pair("--input-format", "stream-json");
    Ok(arguments.0)
}

#[derive(Default)]
struct Arguments(Vec<OsString>);

impl Arguments {
    fn flag(&mut self, flag: impl Into<OsString>) {
        self.0.push(flag.into());
    }

    fn pair(&mut self, flag: impl Into<OsString>, value: impl Into<OsString>) {
        self.0.push(flag.into());
        self.0.push(value.into());
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn apply_skills_defaults(
    options: &ClaudeAgentOptions,
) -> Result<(Vec<String>, Option<Vec<String>>)> {
    let mut allowed_tools = options.allowed_tools.clone();
    let mut setting_sources: Option<Vec<String>> = options.setting_sources.as_ref().map(|list| {
        list.iter()
            .map(|source| source.as_str().to_owned())
            .collect()
    });

    let Some(skills) = &options.skills else {
        return Ok((allowed_tools, setting_sources));
    };

    match skills {
        Skills::All => {
            if !allowed_tools.iter().any(|tool| tool == SKILL_TOOL) {
                allowed_tools.push(SKILL_TOOL.to_owned());
            }
        }
        Skills::List(names) => {
            for name in names {
                validate_skill_name(name)?;
                let pattern = format!("{SKILL_TOOL}({name})");
                if !allowed_tools.contains(&pattern) {
                    allowed_tools.push(pattern);
                }
            }
        }
    }

    if setting_sources.is_none() {
        setting_sources = Some(
            DEFAULT_SKILL_SETTING_SOURCES
                .iter()
                .map(|source| (*source).to_owned())
                .collect(),
        );
    }
    Ok((allowed_tools, setting_sources))
}

pub(crate) fn validate_skill_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::config("Skill names must be non-empty strings"));
    }
    if name != name.trim() {
        return Err(Error::config(format!(
            "Invalid skill name {name:?}: leading or trailing whitespace can never match - the \
             Skill tool trims the invoked name."
        )));
    }
    if name.chars().any(is_invalid_skill_character) {
        return Err(Error::config(format!(
            "Invalid skill name {name:?}: parentheses, commas, control characters, and \
             byte-order marks are not allowed. Names match the skill's directory name, or \
             'plugin:skill' for plugin-qualified skills."
        )));
    }
    if name == "*" {
        return Err(Error::config(
            "Invalid skill name '*': use Skills::All to enable every skill.",
        ));
    }
    if name.ends_with(":*") || name.ends_with(" *") {
        return Err(Error::config(format!(
            "Invalid skill name {name:?}: wildcard-suffix names are not allowed; list each skill \
             by its exact name."
        )));
    }
    if name.starts_with('/') {
        return Err(Error::config(format!(
            "Invalid skill name {name:?}: skill names may not start with '/'. The skills option \
             takes the canonical name, not the slash-command form."
        )));
    }
    if name.contains("\\\\") {
        return Err(Error::config(format!(
            "Invalid skill name {name:?}: consecutive backslashes are not allowed - the per-rule \
             parser collapses them, so the rule would name a different skill."
        )));
    }
    if name.ends_with('\\') {
        return Err(Error::config(format!(
            "Invalid skill name {name:?}: names may not end with an unpaired backslash."
        )));
    }
    Ok(())
}

fn is_invalid_skill_character(character: char) -> bool {
    matches!(character, '(' | ')' | ',' | BYTE_ORDER_MARK)
        || matches!(u32::from(character), 0x00..=0x1f | 0x7f..=0x9f)
}

pub(crate) fn reject_cmd_metacharacters(
    option_name: &str,
    value: &str,
    platform: Platform,
) -> Result<()> {
    if platform != Platform::Windows {
        return Ok(());
    }
    let mut bad: Vec<char> = value
        .chars()
        .filter(|character| {
            CMD_EXE_METACHARACTERS.contains(*character) || matches!(character, '\r' | '\n')
        })
        .collect();
    bad.sort_unstable();
    bad.dedup();
    if bad.is_empty() {
        return Ok(());
    }
    Err(Error::config(format!(
        "{option_name} value {value:?} contains characters that are unsafe to pass on a Windows \
         command line: {bad:?}"
    )))
}

fn build_settings_value(options: &ClaudeAgentOptions) -> Result<Option<String>> {
    let Some(sandbox) = &options.sandbox else {
        return Ok(options.settings.clone());
    };
    let mut settings = match options.settings.as_deref().map(str::trim) {
        None => Map::new(),
        Some(inline) if inline.starts_with('{') && inline.ends_with('}') => {
            parse_settings_object(inline).map_err(settings_error)?
        }
        Some(path) => {
            let path = options
                .cwd
                .as_deref()
                .unwrap_or_else(|| Path::new(""))
                .join(path);
            let text = std::fs::read_to_string(&path)
                .map_err(|error| settings_error(format!("{}: {error}", path.display())))?;
            let text = text.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&text);
            parse_settings_object(text).map_err(settings_error)?
        }
    };
    let sandbox =
        serde_json::to_value(sandbox).map_err(|error| settings_error(error.to_string()))?;
    settings.insert("sandbox".to_owned(), sandbox);
    Ok(Some(Value::Object(settings).to_string()))
}

fn parse_settings_object(text: &str) -> std::result::Result<Map<String, Value>, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(settings)) => Ok(settings),
        Ok(_) => Err("settings must be a JSON object".to_owned()),
        Err(error) => Err(error.to_string()),
    }
}

fn settings_error(cause: String) -> Error {
    Error::spawn(format!("Failed to load settings: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::{Platform, reject_cmd_metacharacters, validate_skill_name};
    use crate::error::ErrorKind;

    fn config_message(result: crate::error::Result<()>) -> String {
        match result {
            Err(error) if error.kind() == ErrorKind::Config => error.to_string(),
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn metacharacters_are_rejected_on_windows_only() {
        for value in [
            "x&calc", "x|whoami", "x<in", "x>out", "x^y", "x%PATH%y", "x!VAR!y", "x\"y", "x\ny",
            "x\ry",
        ] {
            let message = config_message(reject_cmd_metacharacters(
                "resume",
                value,
                Platform::Windows,
            ));
            assert!(message.contains("unsafe"), "{value:?}");
            assert!(message.starts_with("resume value"), "{value:?}");
            assert!(reject_cmd_metacharacters("resume", value, Platform::Unix).is_ok());
        }
    }

    #[test]
    fn rejected_characters_are_listed_sorted_and_once() {
        let message = config_message(reject_cmd_metacharacters(
            "session_id",
            "b|a&c&d|",
            Platform::Windows,
        ));
        assert!(message.ends_with("['&', '|']"), "{message}");
    }

    #[test]
    fn ordinary_title_is_accepted_on_windows() {
        let title = "My project - daily notes (v2) #3";
        assert!(reject_cmd_metacharacters("resume", title, Platform::Windows).is_ok());
    }

    #[test]
    fn skill_name_checks_run_in_the_upstream_order() {
        assert_eq!(
            config_message(validate_skill_name("   ")),
            "Skill names must be non-empty strings"
        );
        assert!(config_message(validate_skill_name(" pdf")).contains("whitespace"));
        assert!(config_message(validate_skill_name("a(b")).contains("parentheses"));
        assert!(config_message(validate_skill_name("*")).contains("Skills::All"));
        assert!(config_message(validate_skill_name("pdf:*")).contains("wildcard-suffix"));
        assert!(config_message(validate_skill_name("/pdf")).contains("start with '/'"));
        assert!(config_message(validate_skill_name("a\\\\b")).contains("consecutive backslashes"));
        assert!(config_message(validate_skill_name("a\\")).contains("unpaired backslash"));
    }
}
