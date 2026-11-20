#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;

use claude_agent_rust_sdk::ErrorKind;
use claude_agent_rust_sdk::{
    AgentDefinition, ClaudeAgentOptions, ClaudeAgentOptionsBuilder, EffortLevel, Error, McpServers,
    McpStdioServerConfig, PermissionMode, SandboxNetworkConfig, SandboxSettings, SdkBeta,
    SdkPluginConfig, SessionKey, SessionStore, SessionStoreEntry, SessionStoreError, SettingSource,
    Skills, SubprocessTransport, SystemPrompt, SystemPromptCustom, SystemPromptFile,
    SystemPromptPreset, TaskBudget, ThinkingConfig, ThinkingDisplay, Tools, ToolsPreset,
};
use serde_json::{Value, json};

use common::{Fake, next_message, strings};

const SHELL_SYNTAX: &str =
    "a \"quoted\" 'word' $HOME %PATH% && echo injected | more; `id` > out.txt \\ ^ *";

fn options() -> ClaudeAgentOptionsBuilder {
    ClaudeAgentOptions::builder().cli_path("/usr/bin/claude")
}

fn try_command(builder: ClaudeAgentOptionsBuilder) -> Result<Vec<String>, Error> {
    SubprocessTransport::new(builder.build().unwrap())
        .command_arguments()
        .map(|arguments| strings(&arguments))
}

fn build(builder: ClaudeAgentOptionsBuilder) -> Vec<String> {
    try_command(builder).unwrap()
}

fn config_error(builder: ClaudeAgentOptionsBuilder) -> String {
    match try_command(builder) {
        Err(error) if error.kind() == ErrorKind::Config => error.to_string(),
        other => panic!("expected a config error, got {other:?}"),
    }
}

fn has(command: &[String], argument: &str) -> bool {
    command.iter().any(|existing| existing == argument)
}

fn value_of<'a>(command: &'a [String], flag: &str) -> &'a str {
    let index = command
        .iter()
        .position(|argument| argument == flag)
        .unwrap_or_else(|| panic!("{flag} is missing from {command:?}"));
    &command[index + 1]
}

fn has_prefix(command: &[String], prefix: &str) -> bool {
    command.iter().any(|argument| argument.starts_with(prefix))
}

#[test]
fn basic_command_starts_and_ends_with_the_stream_json_flags() {
    let command = build(options());
    assert_eq!(
        command[..3],
        ["--output-format", "stream-json", "--verbose"]
    );
    assert_eq!(
        command[command.len() - 2..],
        ["--input-format", "stream-json"]
    );
    assert!(!has(&command, "--print"));
    assert_eq!(value_of(&command, "--system-prompt"), "");
    assert_eq!(
        command,
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--system-prompt",
            "",
            "--input-format",
            "stream-json"
        ]
    );
}

#[test]
fn flags_are_emitted_in_the_upstream_order() {
    let command = build(
        options()
            .system_prompt("Be helpful")
            .tools(["Read", "Edit"])
            .allowed_tools(["Read", "Write"])
            .max_turns(5)
            .max_budget_usd(2.5)
            .disallowed_tools(["Bash"])
            .task_budget(TaskBudget::new(100_000))
            .model("claude-sonnet-4-5")
            .fallback_model("sonnet")
            .betas([SdkBeta::Context1m20250807])
            .permission_prompt_tool_name("mcp__approver__ask")
            .permission_mode(PermissionMode::AcceptEdits)
            .continue_conversation(true)
            .resume("abc123")
            .session_id("8f8b1c0e-2b1e-4a3f-9c2d-5e6f7a8b9c0d")
            .settings("/path/to/settings.json")
            .add_dirs(["/first", "/second"])
            .mcp_servers(McpServers::Json("{\"mcpServers\": {}}".to_owned()))
            .include_partial_messages(true)
            .include_hook_events(true)
            .strict_mcp_config(true)
            .fork_session(true)
            .resume_session_at("at-uuid")
            .resume_drops_turn("drop-uuid")
            .setting_sources([SettingSource::User, SettingSource::Local])
            .plugins([SdkPluginConfig::local("/plugins/one")])
            .extra_arg("future-flag", Some("value"))
            .thinking(ThinkingConfig::adaptive())
            .effort(EffortLevel::Xhigh)
            .output_format(json!({"type": "json_schema", "schema": {"type": "object"}})),
    );
    assert_eq!(
        command,
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--system-prompt",
            "Be helpful",
            "--tools",
            "Read,Edit",
            "--allowedTools",
            "Read,Write",
            "--max-turns",
            "5",
            "--max-budget-usd",
            "2.5",
            "--disallowedTools",
            "Bash",
            "--task-budget",
            "100000",
            "--model",
            "claude-sonnet-4-5",
            "--fallback-model",
            "sonnet",
            "--betas",
            "context-1m-2025-08-07",
            "--permission-prompt-tool",
            "mcp__approver__ask",
            "--permission-mode",
            "acceptEdits",
            "--continue",
            "--resume=abc123",
            "--session-id=8f8b1c0e-2b1e-4a3f-9c2d-5e6f7a8b9c0d",
            "--settings",
            "/path/to/settings.json",
            "--add-dir",
            "/first",
            "--add-dir",
            "/second",
            "--mcp-config",
            "{\"mcpServers\": {}}",
            "--include-partial-messages",
            "--include-hook-events",
            "--strict-mcp-config",
            "--fork-session",
            "--resume-session-at=at-uuid",
            "--resume-drops-turn=drop-uuid",
            "--setting-sources=user,local",
            "--plugin-dir",
            "/plugins/one",
            "--future-flag",
            "value",
            "--thinking",
            "adaptive",
            "--effort",
            "xhigh",
            "--json-schema",
            "{\"type\":\"object\"}",
            "--input-format",
            "stream-json",
        ]
    );
}

#[test]
fn include_hook_events_and_strict_mcp_config_are_opt_in() {
    assert!(has(
        &build(options().include_hook_events(true)),
        "--include-hook-events"
    ));
    assert!(has(
        &build(options().strict_mcp_config(true)),
        "--strict-mcp-config"
    ));
    let plain = build(options());
    assert!(!has(&plain, "--include-hook-events"));
    assert!(!has(&plain, "--strict-mcp-config"));
    assert!(!has(&plain, "--include-partial-messages"));
    assert!(!has(&plain, "--fork-session"));
    assert!(!has(&plain, "--continue"));
}

#[test]
fn resume_and_session_id_use_the_equals_form() {
    let session_id = "8f8b1c0e-2b1e-4a3f-9c2d-5e6f7a8b9c0d";
    let command = build(options().resume("abc123").session_id(session_id));
    assert!(has(&command, "--resume=abc123"));
    assert!(has(&command, &format!("--session-id={session_id}")));
    assert!(!has(&command, "--resume"));
    assert!(!has(&command, "--session-id"));
    assert!(!has(&command, "abc123"));
    assert!(!has(&command, session_id));
}

#[test]
fn truncating_resume_options_use_the_equals_form() {
    let at = "0d78eb23-2d48-4741-b970-4ed0a3356cce";
    let drops = "ce0a8011-2c8d-40f2-86e5-d6e1b0c041c0";
    let command = build(
        options()
            .resume("abc123")
            .fork_session(true)
            .resume_session_at(at)
            .resume_drops_turn(drops),
    );
    assert!(has(&command, &format!("--resume-session-at={at}")));
    assert!(has(&command, &format!("--resume-drops-turn={drops}")));
    assert!(!has(&command, "--resume-session-at"));
    assert!(!has(&command, "--resume-drops-turn"));
    assert!(!has(&command, at));
    assert!(!has(&command, drops));
}

#[test]
fn resume_drops_turn_is_omitted_by_default_and_forwarded_when_empty() {
    let command_without = build(options().resume("abc123").resume_session_at("x"));
    assert!(has(&command_without, "--resume-session-at=x"));
    assert!(!has_prefix(&command_without, "--resume-drops-turn"));

    let command_empty = build(
        options()
            .resume("abc123")
            .resume_session_at("x")
            .resume_drops_turn(""),
    );
    assert!(has(&command_empty, "--resume-drops-turn="));
}

#[test]
fn dash_leading_resume_values_do_not_inject_flags() {
    let command = build(options().resume("--evil").session_id("-r"));
    assert!(has(&command, "--resume=--evil"));
    assert!(has(&command, "--session-id=-r"));
    assert!(!has(&command, "--evil"));
    assert!(!has(&command, "-r"));
    assert!(!has(&command, "--resume"));
    assert!(!has(&command, "--session-id"));
}

#[test]
fn session_id_and_resume_are_absent_by_default() {
    let command = build(options());
    assert!(!has_prefix(&command, "--session-id"));
    assert!(!has_prefix(&command, "--resume"));
}

#[test]
fn session_continuation() {
    let command = build(options().continue_conversation(true).resume("session-123"));
    assert!(has(&command, "--continue"));
    assert!(has(&command, "--resume=session-123"));
}

#[test]
fn system_prompt_forms() {
    let text = build(options().system_prompt("Be helpful"));
    assert_eq!(value_of(&text, "--system-prompt"), "Be helpful");

    let preset = build(options().system_prompt(SystemPromptPreset::new()));
    assert!(!has(&preset, "--system-prompt"));
    assert!(!has(&preset, "--append-system-prompt"));

    let appended = build(options().system_prompt(SystemPromptPreset::new().append("Be concise.")));
    assert!(!has(&appended, "--system-prompt"));
    assert_eq!(value_of(&appended, "--append-system-prompt"), "Be concise.");

    let file = build(options().system_prompt(SystemPromptFile::new("/path/to/prompt.md")));
    assert!(!has(&file, "--system-prompt"));
    assert_eq!(
        value_of(&file, "--system-prompt-file"),
        "/path/to/prompt.md"
    );

    let custom =
        build(options().system_prompt(SystemPromptCustom::new("You are terse.").snapshot(false)));
    assert_eq!(value_of(&custom, "--system-prompt"), "You are terse.");
    assert!(!has(&custom, "--system-prompt-file"));

    let explicit = build(options().system_prompt(SystemPrompt::Text(String::new())));
    assert_eq!(value_of(&explicit, "--system-prompt"), "");
}

#[test]
fn common_options() {
    let command = build(
        options()
            .allowed_tools(["Read", "Write"])
            .disallowed_tools(["Bash"])
            .model("claude-sonnet-4-5")
            .permission_mode(PermissionMode::AcceptEdits)
            .max_turns(5),
    );
    assert_eq!(value_of(&command, "--allowedTools"), "Read,Write");
    assert_eq!(value_of(&command, "--disallowedTools"), "Bash");
    assert_eq!(value_of(&command, "--model"), "claude-sonnet-4-5");
    assert_eq!(value_of(&command, "--permission-mode"), "acceptEdits");
    assert_eq!(value_of(&command, "--max-turns"), "5");
}

#[test]
fn permission_mode_fallback_model_effort_and_budgets() {
    let command = build(
        options()
            .permission_mode(PermissionMode::DontAsk)
            .model("opus")
            .fallback_model("sonnet")
            .effort(EffortLevel::Xhigh)
            .task_budget(TaskBudget::new(100_000))
            .max_budget_usd(10.0),
    );
    assert_eq!(value_of(&command, "--permission-mode"), "dontAsk");
    assert_eq!(value_of(&command, "--model"), "opus");
    assert_eq!(value_of(&command, "--fallback-model"), "sonnet");
    assert_eq!(value_of(&command, "--effort"), "xhigh");
    assert_eq!(value_of(&command, "--task-budget"), "100000");
    assert_eq!(value_of(&command, "--max-budget-usd"), "10.0");

    let plain = build(options());
    assert!(!has(&plain, "--task-budget"));
    assert!(!has(&plain, "--max-budget-usd"));
    assert!(!has(&plain, "--max-turns"));
    assert!(!has(&build(options().max_turns(0)), "--max-turns"));
}

#[test]
fn max_thinking_tokens_alone() {
    let command = build(options().max_thinking_tokens(5000));
    assert_eq!(value_of(&command, "--max-thinking-tokens"), "5000");
    assert!(!has(&command, "--thinking"));
}

#[test]
fn thinking_forms() {
    let adaptive = build(options().thinking(ThinkingConfig::adaptive()));
    assert_eq!(value_of(&adaptive, "--thinking"), "adaptive");
    assert!(!has(&adaptive, "--max-thinking-tokens"));
    assert!(!has(&adaptive, "--thinking-display"));

    let enabled = build(options().thinking(ThinkingConfig::enabled(20_000)));
    assert_eq!(value_of(&enabled, "--max-thinking-tokens"), "20000");
    assert!(!has(&enabled, "--thinking"));

    let disabled = build(options().thinking(ThinkingConfig::Disabled));
    assert_eq!(value_of(&disabled, "--thinking"), "disabled");
    assert!(!has(&disabled, "--thinking-display"));
}

#[test]
fn thinking_display_is_forwarded_for_adaptive_and_enabled() {
    let adaptive =
        build(options().thinking(ThinkingConfig::adaptive().display(ThinkingDisplay::Summarized)));
    assert_eq!(value_of(&adaptive, "--thinking"), "adaptive");
    assert_eq!(value_of(&adaptive, "--thinking-display"), "summarized");

    let enabled = build(
        options().thinking(ThinkingConfig::enabled(20_000).display(ThinkingDisplay::Omitted)),
    );
    assert_eq!(value_of(&enabled, "--max-thinking-tokens"), "20000");
    assert_eq!(value_of(&enabled, "--thinking-display"), "omitted");
}

#[test]
fn thinking_takes_precedence_over_max_thinking_tokens() {
    let command = build(
        options()
            .max_thinking_tokens(5000)
            .thinking(ThinkingConfig::enabled(15_000)),
    );
    assert_eq!(value_of(&command, "--max-thinking-tokens"), "15000");
    assert_eq!(
        command
            .iter()
            .filter(|argument| *argument == "--max-thinking-tokens")
            .count(),
        1
    );

    let adaptive = build(
        options()
            .max_thinking_tokens(5000)
            .thinking(ThinkingConfig::adaptive()),
    );
    assert!(!has(&adaptive, "--max-thinking-tokens"));
}

#[test]
fn add_dirs_emit_one_flag_per_directory() {
    let first = PathBuf::from("/path/to/dir1");
    let second = PathBuf::from("/path/to/dir2");
    let command = build(options().add_dirs([first.clone(), second.clone()]));
    let positions: Vec<usize> = command
        .iter()
        .enumerate()
        .filter(|(_, argument)| *argument == "--add-dir")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(positions.len(), 2);
    assert_eq!(command[positions[0] + 1], first.to_string_lossy());
    assert_eq!(command[positions[1] + 1], second.to_string_lossy());
}

#[test]
fn settings_are_passed_through_without_a_sandbox() {
    let file = build(options().settings("/path/to/settings.json"));
    assert_eq!(value_of(&file, "--settings"), "/path/to/settings.json");

    let inline = "{\"permissions\": {\"allow\": [\"Bash(ls:*)\"]}}";
    let json = build(options().settings(inline));
    assert_eq!(value_of(&json, "--settings"), inline);

    assert!(!has(&build(options()), "--settings"));
}

#[test]
fn setting_sources_forms() {
    assert!(!has_prefix(&build(options()), "--setting-sources"));
    let none = build(options().setting_sources(Vec::<SettingSource>::new()));
    assert!(has(&none, "--setting-sources="));
    let some = build(options().setting_sources([SettingSource::User, SettingSource::Project]));
    assert!(has(&some, "--setting-sources=user,project"));
    assert!(!has(&some, "--setting-sources"));
}

#[test]
fn skills_option_matrix() {
    struct Case {
        skills: Option<Skills>,
        allowed_tools: &'static [&'static str],
        setting_sources: Option<&'static [SettingSource]>,
        want_tools: Option<&'static str>,
        want_sources: Option<&'static str>,
    }
    let cases = [
        Case {
            skills: None,
            allowed_tools: &[],
            setting_sources: None,
            want_tools: None,
            want_sources: None,
        },
        Case {
            skills: None,
            allowed_tools: &["Skill", "Read"],
            setting_sources: Some(&[SettingSource::User, SettingSource::Project]),
            want_tools: Some("Skill,Read"),
            want_sources: Some("user,project"),
        },
        Case {
            skills: Some(Skills::All),
            allowed_tools: &[],
            setting_sources: None,
            want_tools: Some("Skill"),
            want_sources: Some("user,project"),
        },
        Case {
            skills: Some(Skills::list(["pdf", "docx"])),
            allowed_tools: &[],
            setting_sources: None,
            want_tools: Some("Skill(pdf),Skill(docx)"),
            want_sources: Some("user,project"),
        },
        Case {
            skills: Some(Skills::list(["pdf"])),
            allowed_tools: &[],
            setting_sources: Some(&[SettingSource::Project]),
            want_tools: Some("Skill(pdf)"),
            want_sources: Some("project"),
        },
        Case {
            skills: Some(Skills::list(["pdf"])),
            allowed_tools: &["Read", "Bash"],
            setting_sources: None,
            want_tools: Some("Read,Bash,Skill(pdf)"),
            want_sources: Some("user,project"),
        },
        Case {
            skills: Some(Skills::List(Vec::new())),
            allowed_tools: &[],
            setting_sources: None,
            want_tools: None,
            want_sources: Some("user,project"),
        },
    ];
    for (index, case) in cases.into_iter().enumerate() {
        let mut builder = options().allowed_tools(case.allowed_tools.iter().copied());
        if let Some(skills) = case.skills {
            builder = builder.skills(skills);
        }
        if let Some(sources) = case.setting_sources {
            builder = builder.setting_sources(sources.iter().cloned());
        }
        let command = build(builder);
        match case.want_tools {
            None => assert!(!has(&command, "--allowedTools"), "case {index}"),
            Some(tools) => assert_eq!(value_of(&command, "--allowedTools"), tools, "case {index}"),
        }
        match case.want_sources {
            None => assert!(!has_prefix(&command, "--setting-sources"), "case {index}"),
            Some(sources) => assert!(
                has(&command, &format!("--setting-sources={sources}")),
                "case {index}"
            ),
        }
    }
}

#[test]
fn skills_do_not_mutate_the_options_or_duplicate_entries() {
    let options = options()
        .allowed_tools(["Read"])
        .skills(Skills::list(["pdf"]))
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options.clone());
    transport.command_arguments().unwrap();
    assert_eq!(options.allowed_tools, ["Read"]);
    assert_eq!(options.setting_sources, None);

    let all = build(self::options().allowed_tools(["Skill"]).skills(Skills::All));
    assert_eq!(value_of(&all, "--allowedTools"), "Skill");
    let listed = build(
        self::options()
            .allowed_tools(["Skill(pdf)"])
            .skills(Skills::list(["pdf"])),
    );
    assert_eq!(value_of(&listed, "--allowedTools"), "Skill(pdf)");
}

#[test]
fn skill_names_with_rule_delimiters_or_control_characters_are_rejected() {
    for name in [
        "x),Bash(*",
        "safe),Bash,Skill(dummy",
        "name,with,commas",
        "unbalanced(",
        "unbalanced)",
        "()",
        "with\nnewline",
        "with\ttab",
        "nul\u{0}byte",
        "del\u{7f}char",
        "nel\u{85}end",
        "csi\u{9b}end",
        "\u{feff}pdf",
        "pdf\u{feff}",
    ] {
        let message = config_error(options().skills(Skills::list([name])));
        assert!(
            message.contains("Invalid skill name"),
            "{name:?}: {message}"
        );
    }
}

#[test]
fn other_invalid_skill_names_are_rejected_with_their_reason() {
    let cases: [(&[&str], &str); 7] = [
        (&["", " ", "  \t "], "non-empty"),
        (&["pdf:*", "my skill *", ":*"], "wildcard-suffix"),
        (&["*"], "Skills::All"),
        (&["name\\"], "unpaired backslash"),
        (
            &["name\\\\", "name\\\\\\", "mid\\\\dle"],
            "consecutive backslashes",
        ),
        (&[" pdf", "pdf ", "\tpdf", " pdf "], "whitespace"),
        (&["/pdf", "/myplugin:pdf"], "may not start with"),
    ];
    for (names, reason) in cases {
        for name in names {
            let message = config_error(options().skills(Skills::list([*name])));
            assert!(message.contains(reason), "{name:?}: {message}");
        }
    }
}

#[test]
fn ordinary_skill_names_are_accepted() {
    for name in [
        "pdf-tools",
        "my_skill.v2",
        "myplugin:pdf",
        "skill with spaces",
        "dir\\sub",
        "\u{65e5}\u{672c}\u{8a9e}\u{30b9}\u{30ad}\u{30eb}",
    ] {
        let command = build(options().skills(Skills::list([name])));
        assert_eq!(
            value_of(&command, "--allowedTools"),
            format!("Skill({name})")
        );
    }
}

#[test]
fn extra_args_forms() {
    let command = build(
        options()
            .extra_arg("new-flag", Some("value"))
            .extra_arg("boolean-flag", None)
            .extra_arg("another-option", Some("test-value")),
    );
    assert_eq!(value_of(&command, "--new-flag"), "value");
    assert_eq!(value_of(&command, "--another-option"), "test-value");
    assert_eq!(value_of(&command, "--boolean-flag"), "--another-option");
}

#[test]
fn dash_leading_extra_arg_value_uses_the_equals_form() {
    let command = build(options().extra_arg("future-flag", Some("--evil")));
    assert!(has(&command, "--future-flag=--evil"));
    assert!(!has(&command, "--evil"));
    assert!(!has(&command, "--future-flag"));
}

#[test]
fn mcp_servers_map_is_wrapped_and_the_sdk_instance_is_removed() {
    let command = build(
        options()
            .mcp_server(
                "test-server",
                McpStdioServerConfig::new("/path/to/server").args(["--option", "value"]),
            )
            .mcp_server(
                "in-process",
                claude_agent_rust_sdk::create_sdk_mcp_server(
                    "calculator",
                    None,
                    Vec::<claude_agent_rust_sdk::SdkMcpTool>::new(),
                )
                .unwrap(),
            ),
    );
    let config: Value = serde_json::from_str(value_of(&command, "--mcp-config")).unwrap();
    assert_eq!(
        config,
        json!({
            "mcpServers": {
                "test-server": {"command": "/path/to/server", "args": ["--option", "value"]},
                "in-process": {"type": "sdk", "name": "calculator"},
            }
        })
    );
}

#[test]
fn mcp_servers_as_path_or_json_string_are_passed_as_is() {
    let path = PathBuf::from("/path/to/mcp-config.json");
    let from_path = build(options().mcp_servers(McpServers::Path(path.clone())));
    assert_eq!(value_of(&from_path, "--mcp-config"), path.to_string_lossy());

    let json = "{\"mcpServers\": {\"server\": {\"type\": \"stdio\", \"command\": \"test\"}}}";
    let from_json = build(options().mcp_servers(McpServers::Json(json.to_owned())));
    assert_eq!(value_of(&from_json, "--mcp-config"), json);

    assert!(!has(&build(options()), "--mcp-config"));
}

fn settings_value(command: &[String]) -> Value {
    serde_json::from_str(value_of(command, "--settings")).unwrap()
}

#[test]
fn sandbox_alone_becomes_the_settings_object() {
    let sandbox = SandboxSettings::new()
        .enabled(true)
        .auto_allow_bash_if_sandboxed(true)
        .network(
            SandboxNetworkConfig::new()
                .allow_local_binding(true)
                .allow_unix_sockets(["/var/run/docker.sock"]),
        );
    let command = build(options().sandbox(sandbox));
    assert_eq!(
        settings_value(&command),
        json!({
            "sandbox": {
                "enabled": true,
                "autoAllowBashIfSandboxed": true,
                "network": {
                    "allowLocalBinding": true,
                    "allowUnixSockets": ["/var/run/docker.sock"],
                },
            }
        })
    );

    let minimal = build(options().sandbox(SandboxSettings::new().enabled(true)));
    assert_eq!(
        settings_value(&minimal),
        json!({"sandbox": {"enabled": true}})
    );
}

#[test]
fn sandbox_is_merged_into_inline_settings() {
    let command = build(
        options()
            .settings("{\"permissions\": {\"allow\": [\"Bash(ls:*)\"]}, \"verbose\": true}")
            .sandbox(
                SandboxSettings::new()
                    .enabled(true)
                    .excluded_commands(["git", "docker"]),
            ),
    );
    assert_eq!(
        settings_value(&command),
        json!({
            "permissions": {"allow": ["Bash(ls:*)"]},
            "verbose": true,
            "sandbox": {"enabled": true, "excludedCommands": ["git", "docker"]},
        })
    );
}

#[test]
fn sandbox_network_and_ignored_violations_pass_through() {
    let sandbox = SandboxSettings::new()
        .enabled(true)
        .network(
            SandboxNetworkConfig::new()
                .allow_unix_sockets(["/tmp/ssh-agent.sock"])
                .allow_all_unix_sockets(false)
                .allow_local_binding(true)
                .http_proxy_port(8080)
                .socks_proxy_port(8081),
        )
        .ignore_violations(
            [("git".to_owned(), vec!["/etc/hosts".to_owned()])]
                .into_iter()
                .collect::<claude_agent_rust_sdk::SandboxIgnoreViolations>(),
        );
    let parsed = settings_value(&build(options().sandbox(sandbox)));
    assert_eq!(
        parsed["sandbox"]["network"],
        json!({
            "allowUnixSockets": ["/tmp/ssh-agent.sock"],
            "allowAllUnixSockets": false,
            "allowLocalBinding": true,
            "httpProxyPort": 8080,
            "socksProxyPort": 8081,
        })
    );
    assert_eq!(
        parsed["sandbox"]["ignoreViolations"],
        json!({"git": ["/etc/hosts"]})
    );
}

#[test]
fn sandbox_reads_a_relative_settings_file_from_the_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("settings.json"),
        "{\"permissions\": {\"deny\": [\"Bash\"]}}",
    )
    .unwrap();
    let command = build(
        options()
            .settings("settings.json")
            .sandbox(SandboxSettings::new().enabled(true))
            .cwd(directory.path()),
    );
    assert_eq!(
        settings_value(&command),
        json!({"permissions": {"deny": ["Bash"]}, "sandbox": {"enabled": true}})
    );
}

#[test]
fn sandbox_reads_a_relative_settings_file_from_the_process_directory_without_a_cwd() {
    let file = tempfile::Builder::new()
        .prefix("sandbox-settings-")
        .suffix(".json")
        .tempfile_in(".")
        .unwrap();
    std::fs::write(file.path(), "{\"model\": \"m\"}").unwrap();
    let name = file
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let command = build(
        options()
            .settings(name)
            .sandbox(SandboxSettings::new().enabled(true)),
    );
    assert_eq!(
        settings_value(&command),
        json!({"model": "m", "sandbox": {"enabled": true}})
    );
}

#[test]
fn sandbox_reads_an_absolute_settings_path_whatever_the_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("settings.json");
    std::fs::write(&file, "{\"permissions\": {\"deny\": [\"Bash\"]}}").unwrap();
    let command = build(
        options()
            .settings(file.to_string_lossy())
            .sandbox(SandboxSettings::new().enabled(true))
            .cwd(directory.path().join("elsewhere")),
    );
    assert_eq!(
        settings_value(&command)["permissions"],
        json!({"deny": ["Bash"]})
    );
}

#[test]
fn sandbox_tolerates_a_byte_order_mark_in_the_settings_file() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("settings.json"),
        "\u{feff}{\"permissions\": {\"deny\": [\"Bash\"]}}",
    )
    .unwrap();
    let command = build(
        options()
            .settings("settings.json")
            .sandbox(SandboxSettings::new().enabled(true))
            .cwd(directory.path()),
    );
    assert_eq!(
        settings_value(&command)["permissions"],
        json!({"deny": ["Bash"]})
    );
}

#[test]
fn sandbox_with_unloadable_settings_is_an_error_that_hides_inline_content() {
    let cases: [(&str, Option<&str>); 5] = [
        ("missing.json", None),
        (".", None),
        ("settings.json", Some("{not json")),
        ("settings.json", Some("[1, 2]")),
        ("{\"env\": {\"KEY\": \"secret-value\",}}", None),
    ];
    for (settings, contents) in cases {
        let directory = tempfile::tempdir().unwrap();
        if let Some(contents) = contents {
            std::fs::write(directory.path().join(settings), contents).unwrap();
        }
        let result = try_command(
            options()
                .settings(settings)
                .sandbox(SandboxSettings::new().enabled(true))
                .cwd(directory.path()),
        );
        match result {
            Err(error) if error.kind() == ErrorKind::Spawn => {
                let message = error.to_string();
                assert!(message.contains("settings"), "{settings}: {message}");
                assert!(!message.contains("secret-value"), "{settings}: {message}");
            }
            other => panic!("expected a settings error for {settings}, got {other:?}"),
        }
    }
}

#[test]
fn tools_forms() {
    let list = build(options().tools(["Read", "Edit", "Bash"]));
    assert_eq!(value_of(&list, "--tools"), "Read,Edit,Bash");

    let empty = build(options().tools(Tools::List(Vec::new())));
    assert_eq!(value_of(&empty, "--tools"), "");

    let preset = build(options().tools(ToolsPreset::ClaudeCode));
    assert_eq!(value_of(&preset, "--tools"), "default");

    assert!(!has(&build(options()), "--tools"));
}

#[test]
fn agents_and_the_prompt_never_appear_in_the_arguments() {
    let large_prompt = "x".repeat(250_000);
    let command = build(
        options()
            .agent(
                "test-agent",
                AgentDefinition::new("A test agent", "You are a test agent"),
            )
            .agent(
                "large-agent",
                AgentDefinition::new("Large", large_prompt.clone()),
            ),
    );
    assert!(!has(&command, "--agents"));
    assert!(!has(&command, "--print"));
    assert!(!has(&command, "-p"));
    assert_eq!(value_of(&command, "--input-format"), "stream-json");
    assert!(
        command
            .iter()
            .all(|argument| !argument.contains("test agent"))
    );
    assert!(command.iter().all(|argument| argument.len() < 1000));
}

#[test]
fn plugins_must_be_local() {
    let command = build(options().plugins([
        SdkPluginConfig::local("/plugins/one"),
        SdkPluginConfig::local("/plugins/two"),
    ]));
    let directories: Vec<&String> = command
        .iter()
        .zip(command.iter().skip(1))
        .filter(|(flag, _)| *flag == "--plugin-dir")
        .map(|(_, value)| value)
        .collect();
    assert_eq!(directories, ["/plugins/one", "/plugins/two"]);

    let message = config_error(options().plugins([SdkPluginConfig::new("remote", "/plugins")]));
    assert_eq!(message, "Unsupported plugin type: remote");
}

#[test]
fn output_format_emits_the_schema_only_for_json_schema() {
    let schema = json!({"type": "object", "properties": {"answer": {"type": "string"}}});
    let command_with =
        build(options().output_format(json!({"type": "json_schema", "schema": schema.clone()})));
    let emitted: Value = serde_json::from_str(value_of(&command_with, "--json-schema")).unwrap();
    assert_eq!(emitted, schema);

    for other in [
        json!({"type": "text", "schema": {"type": "object"}}),
        json!({"type": "json_schema"}),
        json!({"type": "json_schema", "schema": null}),
        json!("json_schema"),
    ] {
        assert!(!has(
            &build(options().output_format(other)),
            "--json-schema"
        ));
    }
}

#[test]
fn json_values_reach_the_cli_with_their_keys_in_the_order_written() {
    let schema = json!({
        "type": "object",
        "properties": {"reasoning": {"type": "string"}, "answer": {"type": "string"}},
        "required": ["reasoning", "answer"],
    });
    let command =
        build(options().output_format(json!({"type": "json_schema", "schema": schema.clone()})));
    assert_eq!(value_of(&command, "--json-schema"), schema.to_string());
    assert!(
        value_of(&command, "--json-schema")
            .starts_with(r#"{"type":"object","properties":{"reasoning""#)
    );
}

struct NullStore;

impl SessionStore for NullStore {
    async fn append(
        &self,
        _key: &SessionKey,
        _entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        Ok(())
    }

    async fn load(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        Ok(None)
    }
}

#[test]
fn session_mirror_flag_follows_the_session_store() {
    assert!(has(
        &build(options().session_store(NullStore)),
        "--session-mirror"
    ));
    assert!(!has(&build(options()), "--session-mirror"));
}

#[cfg(windows)]
#[test]
fn cmd_metacharacters_in_resume_values_are_rejected_on_windows() {
    for value in [
        "x&calc", "x|whoami", "x<in", "x>out", "x^y", "x%PATH%y", "x!VAR!y", "x\"y", "x\ny", "x\ry",
    ] {
        let message = config_error(options().resume(value));
        assert!(message.contains("unsafe"), "{value:?}");
    }
    assert!(config_error(options().session_id("x&ver")).contains("session_id"));
    assert!(
        config_error(options().resume("abc").resume_session_at("x&calc"))
            .contains("resume_session_at")
    );
    assert!(
        config_error(options().resume("abc").resume_drops_turn("x&calc"))
            .contains("resume_drops_turn")
    );

    let title = "My project - daily notes (v2) #3";
    assert!(has(
        &build(options().resume(title)),
        &format!("--resume={title}")
    ));
}

#[cfg(not(windows))]
#[test]
fn cmd_metacharacters_are_allowed_off_windows() {
    let command = build(options().resume("title & % | notes").session_id("a>b"));
    assert!(has(&command, "--resume=title & % | notes"));
    assert!(has(&command, "--session-id=a>b"));
}

#[tokio::test]
async fn the_spawned_process_receives_exactly_the_built_arguments() {
    let fake = Fake::new(json!({"steps": [{"exit": 0}]}));
    let options = fake
        .options()
        .system_prompt(SHELL_SYNTAX)
        .allowed_tools(["Read", "Write"])
        .resume("--evil")
        .extra_arg("future-flag", Some("-x"))
        .extra_arg("bare", None)
        .skills(Skills::list(["pdf"]))
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    let expected = strings(&transport.command_arguments().unwrap());
    assert!(has(&expected, "--resume=--evil"));
    assert_eq!(value_of(&expected, "--system-prompt"), SHELL_SYNTAX);

    transport.connect().await.unwrap();
    let mut stream = transport.read_messages();
    let init = next_message(&mut stream).await;
    let received: Vec<String> = serde_json::from_value(init["fake_args"].clone()).unwrap();
    assert_eq!(received, expected);
    transport.close().await.unwrap();
}

#[tokio::test]
async fn an_invalid_skill_name_fails_connect_before_anything_is_spawned() {
    let fake = Fake::new(json!({"steps": [{"exit": 0}]}));
    let options = fake
        .options()
        .skills(Skills::list(["/pdf"]))
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    let error = transport.connect().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config, "{error:?}");
    assert_eq!(transport.process_id(), None);
    assert!(!transport.is_ready());

    let options = fake
        .options()
        .plugins([SdkPluginConfig::new("remote", "/plugins")])
        .build()
        .unwrap();
    let transport = SubprocessTransport::new(options);
    let error = transport.connect().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config, "{error:?}");
    assert_eq!(transport.process_id(), None);
}

#[test]
fn keys_the_crate_does_not_model_reach_the_cli_in_sandbox_and_mcp_settings() {
    let sandbox = SandboxSettings::new()
        .enabled(true)
        .extra_field("allowNestedUserNamespaces", true)
        .network(SandboxNetworkConfig::new().extra_field("futureNetworkKey", "on"));
    let command = build(options().sandbox(sandbox).mcp_server(
        "slow",
        McpStdioServerConfig::new("/path/to/server").extra_field("timeout", 5000),
    ));
    assert_eq!(
        settings_value(&command),
        json!({
            "sandbox": {
                "enabled": true,
                "network": {"futureNetworkKey": "on"},
                "allowNestedUserNamespaces": true,
            }
        })
    );
    let config: Value = serde_json::from_str(value_of(&command, "--mcp-config")).unwrap();
    assert_eq!(
        config,
        json!({"mcpServers": {"slow": {"command": "/path/to/server", "timeout": 5000}}})
    );
}

#[test]
fn a_server_config_of_a_kind_the_crate_does_not_model_sits_next_to_an_in_process_server() {
    let serde_json::Value::Object(raw) =
        json!({"type": "ws", "url": "wss://example.test/mcp", "timeout": 5000})
    else {
        unreachable!()
    };
    let raw = claude_agent_rust_sdk::McpServerConfig::Raw(raw);
    assert_eq!(raw.type_name(), "ws");
    let command = build(
        options().mcp_server("future", raw).mcp_server(
            "in-process",
            claude_agent_rust_sdk::create_sdk_mcp_server(
                "calculator",
                None,
                Vec::<claude_agent_rust_sdk::SdkMcpTool>::new(),
            )
            .unwrap(),
        ),
    );
    let config: Value = serde_json::from_str(value_of(&command, "--mcp-config")).unwrap();
    assert_eq!(
        config,
        json!({"mcpServers": {
            "future": {"type": "ws", "url": "wss://example.test/mcp", "timeout": 5000},
            "in-process": {"type": "sdk", "name": "calculator"},
        }})
    );
}
