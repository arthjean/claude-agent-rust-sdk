#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use claude_agent_kit::{
    AgentDefinition, ClaudeAgentOptions, EffortLevel, Error, HookEvent, HookInput, HookMatcher,
    HookOutput, McpRemoteServerConfig, McpSdkServerConfig, McpSdkServerInstance, McpServerConfig,
    McpServers, McpStdioServerConfig, PermissionMode, PermissionResult, SandboxNetworkConfig,
    SandboxSettings, SdkBeta, SdkPluginConfig, SdkPluginType, SessionKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreFlushMode,
    SessionStoreListEntry, SettingSource, SharedSessionStore, Skills, StderrCallback, SystemPrompt,
    SystemPromptCustom, SystemPromptFile, SystemPromptPreset, TaskBudget, ThinkingConfig,
    ThinkingDisplay, ToolPermissionContext, Tools, ToolsPreset, UnsupportedStoreOperation,
};
use indexmap::IndexMap;
use serde_json::{Value, json};

async fn allow_everything(
    _tool_name: String,
    _input: Value,
    _context: ToolPermissionContext,
) -> Result<PermissionResult, claude_agent_kit::BoxError> {
    Ok(PermissionResult::allow())
}

fn config_error(result: Result<ClaudeAgentOptions, Error>) -> String {
    match result {
        Err(Error::Config { message }) => message,
        other => panic!("expected a config error, got {other:?}"),
    }
}

#[derive(Default)]
struct MinimalStore {
    entries: Mutex<Vec<SessionStoreEntry>>,
}

impl SessionStore for MinimalStore {
    async fn append(
        &self,
        _key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.entries.lock().unwrap().extend_from_slice(entries);
        Ok(())
    }

    async fn load(
        &self,
        _key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        let entries = self.entries.lock().unwrap().clone();
        Ok((!entries.is_empty()).then_some(entries))
    }
}

struct ListingStore;

impl SessionStore for ListingStore {
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

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::none().with_list_sessions()
    }

    async fn list_sessions(
        &self,
        _project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        Ok(vec![SessionStoreListEntry::new(
            "session-1",
            1_700_000_000_000,
        )])
    }
}

#[test]
fn default_options_match_upstream_defaults() {
    let built = ClaudeAgentOptions::builder().build().unwrap();
    for options in [built, ClaudeAgentOptions::default()] {
        assert_eq!(options.tools, None);
        assert!(options.allowed_tools.is_empty());
        assert_eq!(options.system_prompt, None);
        assert!(matches!(&options.mcp_servers, McpServers::Map(servers) if servers.is_empty()));
        assert!(options.mcp_servers.is_empty());
        assert!(!options.strict_mcp_config);
        assert_eq!(options.permission_mode, None);
        assert!(!options.continue_conversation);
        assert_eq!(options.resume, None);
        assert_eq!(options.session_id, None);
        assert_eq!(options.max_turns, None);
        assert_eq!(options.max_budget_usd, None);
        assert!(options.disallowed_tools.is_empty());
        assert_eq!(options.model, None);
        assert_eq!(options.fallback_model, None);
        assert!(options.betas.is_empty());
        assert_eq!(options.permission_prompt_tool_name, None);
        assert_eq!(options.cwd, None);
        assert_eq!(options.cli_path, None);
        assert_eq!(options.settings, None);
        assert!(options.add_dirs.is_empty());
        assert!(options.env.is_empty());
        assert!(options.extra_args.is_empty());
        assert_eq!(options.max_buffer_size, None);
        assert!(options.stderr.is_none());
        assert!(options.can_use_tool.is_none());
        assert!(options.hooks.is_none());
        assert_eq!(options.user, None);
        assert!(!options.include_partial_messages);
        assert!(!options.include_hook_events);
        assert!(!options.forward_subagent_text);
        assert!(!options.verbatim_prompts);
        assert!(!options.fork_session);
        assert_eq!(options.resume_session_at, None);
        assert_eq!(options.resume_drops_turn, None);
        assert_eq!(options.agents, None);
        assert_eq!(options.setting_sources, None);
        assert_eq!(options.skills, None);
        assert_eq!(options.sandbox, None);
        assert!(options.plugins.is_empty());
        assert_eq!(options.max_thinking_tokens, None);
        assert_eq!(options.thinking, None);
        assert_eq!(options.effort, None);
        assert_eq!(options.output_format, None);
        assert!(!options.enable_file_checkpointing);
        assert!(options.session_store.is_none());
        assert_eq!(options.session_store_flush, SessionStoreFlushMode::Batched);
        assert_eq!(options.load_timeout_ms, 60_000);
        assert_eq!(options.task_budget, None);
        assert_eq!(options.client_app, None);
    }
}

#[test]
fn builder_sets_every_option() {
    let stderr_lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&stderr_lines);
    let options = ClaudeAgentOptions::builder()
        .tools(["Bash", "Read", "Edit"])
        .allowed_tools(["Bash(ls:*)"])
        .system_prompt("You are a helpful assistant.")
        .mcp_server(
            "files",
            McpStdioServerConfig::new("node").args(["server.js"]),
        )
        .strict_mcp_config(true)
        .permission_mode(PermissionMode::AcceptEdits)
        .continue_conversation(true)
        .resume("session-123")
        .session_id("7b0e5a0e-2b6f-4b0a-9d0c-0d0f6f1d2c3b")
        .max_turns(5)
        .max_budget_usd(1.5)
        .disallowed_tools(["WebFetch"])
        .model("claude-sonnet-5")
        .fallback_model("claude-haiku-5")
        .betas([SdkBeta::Context1m20250807])
        .cwd("/work")
        .cli_path("/usr/local/bin/claude")
        .settings("{\"permissions\": {}}")
        .add_dirs(["/data", "/logs"])
        .env([("A", "1")])
        .env_var("B", "2")
        .extra_arg("debug-to-stderr", None)
        .extra_arg("replay-user-messages", Some("yes"))
        .max_buffer_size(4096)
        .stderr(StderrCallback::from_sync(move |line| {
            sink.lock().unwrap().push(line.to_owned());
        }))
        .can_use_tool(allow_everything)
        .hook(
            HookEvent::PostToolUse,
            HookMatcher::new().matcher("Bash").hook(
                async |_input: HookInput, _tool_use_id: Option<String>, _context| {
                    Ok(HookOutput::default())
                },
            ),
        )
        .hook(HookEvent::PreToolUse, HookMatcher::new())
        .hook(HookEvent::PostToolUse, HookMatcher::new().matcher("Read"))
        .user("user-1")
        .include_partial_messages(true)
        .include_hook_events(true)
        .forward_subagent_text(true)
        .verbatim_prompts(true)
        .fork_session(true)
        .resume_session_at("uuid-keep")
        .resume_drops_turn("uuid-drop")
        .agent(
            "reviewer",
            AgentDefinition::new("Reviews code", "You review"),
        )
        .setting_sources([SettingSource::User, SettingSource::Project])
        .skills(Skills::list(["reviewer"]))
        .sandbox(
            SandboxSettings::new()
                .enabled(true)
                .excluded_commands(["docker"])
                .network(SandboxNetworkConfig::new().allow_local_binding(true)),
        )
        .plugins([SdkPluginConfig::local("/plugins/demo")])
        .max_thinking_tokens(2000)
        .thinking(ThinkingConfig::adaptive())
        .effort(EffortLevel::High)
        .output_format(json!({"type": "json_schema", "schema": {"type": "object"}}))
        .session_store_flush(SessionStoreFlushMode::Eager)
        .load_timeout_ms(5_000)
        .task_budget(TaskBudget::new(50_000))
        .client_app("my-app/1.0.0")
        .build()
        .unwrap();

    assert_eq!(options.tools, Some(Tools::list(["Bash", "Read", "Edit"])));
    assert_eq!(options.allowed_tools, ["Bash(ls:*)"]);
    assert_eq!(
        options.system_prompt,
        Some(SystemPrompt::Text(
            "You are a helpful assistant.".to_owned()
        ))
    );
    let McpServers::Map(servers) = &options.mcp_servers else {
        panic!("expected a server map");
    };
    assert!(matches!(
        servers.get("files"),
        Some(McpServerConfig::Stdio(_))
    ));
    assert!(options.strict_mcp_config);
    assert_eq!(options.permission_mode, Some(PermissionMode::AcceptEdits));
    assert!(options.continue_conversation);
    assert_eq!(options.resume.as_deref(), Some("session-123"));
    assert_eq!(
        options.session_id.as_deref(),
        Some("7b0e5a0e-2b6f-4b0a-9d0c-0d0f6f1d2c3b")
    );
    assert_eq!(options.max_turns, Some(5));
    assert_eq!(options.max_budget_usd, Some(1.5));
    assert_eq!(options.disallowed_tools, ["WebFetch"]);
    assert_eq!(options.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(options.fallback_model.as_deref(), Some("claude-haiku-5"));
    assert_eq!(options.betas, [SdkBeta::Context1m20250807]);
    assert_eq!(options.cwd, Some(PathBuf::from("/work")));
    assert_eq!(
        options.cli_path,
        Some(PathBuf::from("/usr/local/bin/claude"))
    );
    assert_eq!(options.settings.as_deref(), Some("{\"permissions\": {}}"));
    assert_eq!(
        options.add_dirs,
        [PathBuf::from("/data"), PathBuf::from("/logs")]
    );
    assert_eq!(
        options.env.iter().collect::<Vec<_>>(),
        [
            (&"A".to_owned(), &"1".to_owned()),
            (&"B".to_owned(), &"2".to_owned())
        ]
    );
    assert_eq!(
        options.extra_args.iter().collect::<Vec<_>>(),
        [
            (&"debug-to-stderr".to_owned(), &None),
            (&"replay-user-messages".to_owned(), &Some("yes".to_owned())),
        ]
    );
    assert_eq!(options.max_buffer_size, Some(4096));
    assert!(options.stderr.is_some());
    assert!(options.can_use_tool.is_some());
    let hooks = options.hooks.as_ref().unwrap();
    assert_eq!(
        hooks.keys().collect::<Vec<_>>(),
        [&HookEvent::PostToolUse, &HookEvent::PreToolUse]
    );
    assert_eq!(hooks[&HookEvent::PostToolUse].len(), 2);
    assert_eq!(hooks[&HookEvent::PostToolUse][0].hooks.len(), 1);
    assert_eq!(
        hooks[&HookEvent::PostToolUse][1].matcher.as_deref(),
        Some("Read")
    );
    assert_eq!(options.user.as_deref(), Some("user-1"));
    assert!(options.include_partial_messages);
    assert!(options.include_hook_events);
    assert!(options.forward_subagent_text);
    assert!(options.verbatim_prompts);
    assert!(options.fork_session);
    assert_eq!(options.resume_session_at.as_deref(), Some("uuid-keep"));
    assert_eq!(options.resume_drops_turn.as_deref(), Some("uuid-drop"));
    assert_eq!(
        options.agents.as_ref().unwrap()["reviewer"].description,
        "Reviews code"
    );
    assert_eq!(
        options.setting_sources,
        Some(vec![SettingSource::User, SettingSource::Project])
    );
    assert_eq!(
        options.skills,
        Some(Skills::List(vec!["reviewer".to_owned()]))
    );
    assert_eq!(options.sandbox.as_ref().unwrap().enabled, Some(true));
    assert_eq!(options.plugins, [SdkPluginConfig::local("/plugins/demo")]);
    assert_eq!(options.max_thinking_tokens, Some(2000));
    assert_eq!(
        options.thinking,
        Some(ThinkingConfig::Adaptive { display: None })
    );
    assert_eq!(options.effort, Some(EffortLevel::High));
    assert_eq!(
        options.output_format,
        Some(json!({"type": "json_schema", "schema": {"type": "object"}}))
    );
    assert!(!options.enable_file_checkpointing);
    assert_eq!(options.session_store_flush, SessionStoreFlushMode::Eager);
    assert_eq!(options.load_timeout_ms, 5_000);
    assert_eq!(options.task_budget, Some(TaskBudget::new(50_000)));
    assert_eq!(options.client_app.as_deref(), Some("my-app/1.0.0"));

    let rebuilt = options
        .clone()
        .into_builder()
        .model("claude-opus-5")
        .build()
        .unwrap();
    assert_eq!(rebuilt.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(rebuilt.allowed_tools, options.allowed_tools);
    assert_eq!(
        rebuilt.permission_prompt_tool_name.as_deref(),
        Some("stdio")
    );
}

#[tokio::test]
async fn stderr_callback_accepts_async_and_sync_closures() {
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));

    let sink = Arc::clone(&lines);
    let from_async = StderrCallback::new(move |line: String| {
        let sink = Arc::clone(&sink);
        async move {
            tokio::task::yield_now().await;
            sink.lock().unwrap().push(format!("async:{line}"));
        }
    });
    let sink = Arc::clone(&lines);
    let from_sync = StderrCallback::from_sync(move |line| {
        sink.lock().unwrap().push(format!("sync:{line}"));
    });

    let options = ClaudeAgentOptions::builder()
        .stderr(from_async)
        .build()
        .unwrap();
    options
        .stderr
        .as_ref()
        .unwrap()
        .call("first".to_owned())
        .await;
    from_sync.call("second".to_owned()).await;
    assert_eq!(*lines.lock().unwrap(), ["async:first", "sync:second"]);
}

#[test]
fn permission_mode_option_accepts_every_mode() {
    for mode in [
        PermissionMode::BypassPermissions,
        PermissionMode::Plan,
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::DontAsk,
        PermissionMode::Auto,
    ] {
        let options = ClaudeAgentOptions::builder()
            .permission_mode(mode.clone())
            .build()
            .unwrap();
        assert_eq!(options.permission_mode, Some(mode));
    }
}

#[test]
fn system_prompt_union_forms() {
    let prompt = |value: SystemPrompt| {
        ClaudeAgentOptions::builder()
            .system_prompt(value)
            .build()
            .unwrap()
            .system_prompt
            .unwrap()
    };

    assert_eq!(
        prompt("You are a helpful assistant.".into()),
        SystemPrompt::Text("You are a helpful assistant.".to_owned())
    );

    let SystemPrompt::Preset(preset) = prompt(SystemPromptPreset::new().into()) else {
        panic!("expected a preset");
    };
    assert_eq!(preset.append, None);
    assert_eq!(preset.exclude_dynamic_sections, None);
    assert_eq!(preset.snapshot, None);

    let SystemPrompt::Preset(preset) = prompt(
        SystemPromptPreset::new()
            .append("Be concise.")
            .exclude_dynamic_sections(true)
            .snapshot(false)
            .into(),
    ) else {
        panic!("expected a preset");
    };
    assert_eq!(preset.append.as_deref(), Some("Be concise."));
    assert_eq!(preset.exclude_dynamic_sections, Some(true));
    assert_eq!(preset.snapshot, Some(false));

    let SystemPrompt::Custom(custom) = prompt(
        SystemPromptCustom::new("You are a release bot.")
            .snapshot(true)
            .into(),
    ) else {
        panic!("expected a custom prompt");
    };
    assert_eq!(custom.prompt, "You are a release bot.");
    assert_eq!(custom.snapshot, Some(true));

    let SystemPrompt::File(file) = prompt(SystemPromptFile::new("/path/to/prompt.md").into())
    else {
        panic!("expected a file prompt");
    };
    assert_eq!(file.path, "/path/to/prompt.md");
}

#[test]
fn tools_union_forms() {
    let tools = |value: Tools| {
        ClaudeAgentOptions::builder()
            .tools(value)
            .build()
            .unwrap()
            .tools
            .unwrap()
    };
    assert_eq!(
        tools(["Bash", "Read"].into()),
        Tools::List(vec!["Bash".to_owned(), "Read".to_owned()])
    );
    assert_eq!(
        tools(Tools::list(Vec::<String>::new())),
        Tools::List(Vec::new())
    );
    assert_eq!(
        tools(ToolsPreset::ClaudeCode.into()),
        Tools::Preset(ToolsPreset::ClaudeCode)
    );
}

#[test]
fn mcp_servers_union_forms() {
    let mut servers = IndexMap::new();
    servers.insert(
        "zeta".to_owned(),
        McpServerConfig::Stdio(
            McpStdioServerConfig::new("python")
                .args(["-m", "server"])
                .env([("TOKEN", "t")]),
        ),
    );
    servers.insert(
        "alpha".to_owned(),
        McpServerConfig::Sse(McpRemoteServerConfig::new("https://example.com/sse")),
    );
    servers.insert(
        "http".to_owned(),
        McpServerConfig::Http(
            McpRemoteServerConfig::new("https://example.com/mcp").headers([("X-Key", "v")]),
        ),
    );
    servers.insert(
        "calc".to_owned(),
        McpServerConfig::Sdk(McpSdkServerConfig::new(
            "calc",
            McpSdkServerInstance::new(41_u32),
        )),
    );
    let options = ClaudeAgentOptions::builder()
        .mcp_servers(McpServers::Map(servers))
        .build()
        .unwrap();
    let McpServers::Map(servers) = &options.mcp_servers else {
        panic!("expected a server map");
    };
    assert_eq!(
        servers.keys().collect::<Vec<_>>(),
        ["zeta", "alpha", "http", "calc"]
    );
    assert_eq!(
        serde_json::to_value(servers).unwrap(),
        json!({
            "zeta": {"command": "python", "args": ["-m", "server"], "env": {"TOKEN": "t"}},
            "alpha": {"type": "sse", "url": "https://example.com/sse"},
            "http": {"type": "http", "url": "https://example.com/mcp", "headers": {"X-Key": "v"}},
            "calc": {"type": "sdk", "name": "calc"},
        })
    );
    let McpServerConfig::Sdk(sdk) = &servers["calc"] else {
        panic!("expected an sdk server");
    };
    assert_eq!(sdk.instance.downcast_ref::<u32>(), Some(&41));
    assert_eq!(sdk.instance.downcast_ref::<String>(), None);
    assert_eq!(servers["zeta"].type_name(), "stdio");

    let options = ClaudeAgentOptions::builder()
        .mcp_servers(McpServers::Json("{\"mcpServers\": {}}".to_owned()))
        .build()
        .unwrap();
    assert!(
        matches!(&options.mcp_servers, McpServers::Json(json) if json == "{\"mcpServers\": {}}")
    );

    let options = ClaudeAgentOptions::builder()
        .mcp_servers(McpServers::Path(PathBuf::from("/etc/mcp.json")))
        .mcp_server("extra", McpStdioServerConfig::new("node"))
        .build()
        .unwrap();
    let McpServers::Map(servers) = &options.mcp_servers else {
        panic!("adding a server should switch to a server map");
    };
    assert_eq!(servers.len(), 1);

    let options = ClaudeAgentOptions::builder()
        .mcp_servers(PathBuf::from("/etc/mcp.json").into())
        .build()
        .unwrap();
    assert!(
        matches!(&options.mcp_servers, McpServers::Path(path) if path == &PathBuf::from("/etc/mcp.json"))
    );
}

#[test]
fn skills_union_forms() {
    let skills = |value: Skills| {
        ClaudeAgentOptions::builder()
            .skills(value)
            .build()
            .unwrap()
            .skills
            .unwrap()
    };
    assert_eq!(skills(Skills::All), Skills::All);
    assert_eq!(
        skills(["reviewer", "plugin:skill"].into()),
        Skills::List(vec!["reviewer".to_owned(), "plugin:skill".to_owned()])
    );
    assert_eq!(
        skills(Skills::list(Vec::<String>::new())),
        Skills::List(Vec::new())
    );
}

#[test]
fn configuration_types_use_upstream_wire_names() {
    assert_eq!(SettingSource::KNOWN, ["user", "project", "local"]);
    assert_eq!(SdkBeta::KNOWN, ["context-1m-2025-08-07"]);
    assert_eq!(
        EffortLevel::KNOWN,
        ["low", "medium", "high", "xhigh", "max"]
    );

    assert_eq!(
        serde_json::to_value(TaskBudget::new(1000)).unwrap(),
        json!({"total": 1000})
    );

    assert_eq!(
        serde_json::to_value(SdkPluginConfig::local("/plugins/demo")).unwrap(),
        json!({"type": "local", "path": "/plugins/demo"})
    );
    let remote = SdkPluginConfig::new("remote", "https://example.com/plugin");
    assert_eq!(remote.kind, SdkPluginType::Unknown("remote".to_owned()));

    assert_eq!(
        serde_json::to_value(ThinkingConfig::adaptive()).unwrap(),
        json!({"type": "adaptive"})
    );
    assert_eq!(
        serde_json::to_value(ThinkingConfig::Adaptive {
            display: Some(ThinkingDisplay::Summarized)
        })
        .unwrap(),
        json!({"type": "adaptive", "display": "summarized"})
    );
    assert_eq!(
        serde_json::to_value(ThinkingConfig::enabled(8000)).unwrap(),
        json!({"type": "enabled", "budget_tokens": 8000})
    );
    assert_eq!(
        serde_json::to_value(ThinkingConfig::Disabled).unwrap(),
        json!({"type": "disabled"})
    );
    assert_eq!(
        serde_json::from_value::<ThinkingConfig>(
            json!({"type": "enabled", "budget_tokens": 1024, "display": "omitted"})
        )
        .unwrap(),
        ThinkingConfig::Enabled {
            budget_tokens: 1024,
            display: Some(ThinkingDisplay::Omitted)
        }
    );
}

#[test]
fn sandbox_settings_use_camel_case_wire_names() {
    let mut ignore_violations = IndexMap::new();
    ignore_violations.insert("*".to_owned(), vec!["/etc/hosts".to_owned()]);
    let sandbox = SandboxSettings::new()
        .enabled(true)
        .auto_allow_bash_if_sandboxed(true)
        .excluded_commands(["docker"])
        .allow_unsandboxed_commands(false)
        .network(
            SandboxNetworkConfig::new()
                .allowed_domains(["example.com"])
                .denied_domains(["evil.example.com"])
                .allow_managed_domains_only(false)
                .allow_unix_sockets(["/var/run/docker.sock"])
                .allow_all_unix_sockets(false)
                .allow_local_binding(true)
                .allow_mach_lookup(["com.apple.*"])
                .http_proxy_port(8080)
                .socks_proxy_port(1080),
        )
        .ignore_violations(ignore_violations)
        .enable_weaker_nested_sandbox(false);
    let wire = json!({
        "enabled": true,
        "autoAllowBashIfSandboxed": true,
        "excludedCommands": ["docker"],
        "allowUnsandboxedCommands": false,
        "network": {
            "allowedDomains": ["example.com"],
            "deniedDomains": ["evil.example.com"],
            "allowManagedDomainsOnly": false,
            "allowUnixSockets": ["/var/run/docker.sock"],
            "allowAllUnixSockets": false,
            "allowLocalBinding": true,
            "allowMachLookup": ["com.apple.*"],
            "httpProxyPort": 8080,
            "socksProxyPort": 1080,
        },
        "ignoreViolations": {"*": ["/etc/hosts"]},
        "enableWeakerNestedSandbox": false,
    });
    assert_eq!(serde_json::to_value(&sandbox).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<SandboxSettings>(wire).unwrap(),
        sandbox
    );
    assert_eq!(
        serde_json::to_value(SandboxSettings::new()).unwrap(),
        json!({})
    );
}

#[test]
fn can_use_tool_forces_the_stdio_permission_prompt_tool() {
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(allow_everything)
        .build()
        .unwrap();
    assert_eq!(
        options.permission_prompt_tool_name.as_deref(),
        Some("stdio")
    );

    let without_callback = ClaudeAgentOptions::builder()
        .permission_prompt_tool_name("CustomTool")
        .build()
        .unwrap();
    assert_eq!(
        without_callback.permission_prompt_tool_name.as_deref(),
        Some("CustomTool")
    );
}

#[test]
fn can_use_tool_with_permission_prompt_tool_name_is_a_config_error() {
    let message = config_error(
        ClaudeAgentOptions::builder()
            .can_use_tool(allow_everything)
            .permission_prompt_tool_name("CustomTool")
            .build(),
    );
    assert_eq!(
        message,
        "can_use_tool callback cannot be used with permission_prompt_tool_name. Please use one \
         or the other."
    );

    let reversed_order = config_error(
        ClaudeAgentOptions::builder()
            .permission_prompt_tool_name("stdio")
            .can_use_tool(allow_everything)
            .build(),
    );
    assert_eq!(reversed_order, message);
}

#[test]
fn session_store_with_file_checkpointing_is_a_config_error() {
    let message = config_error(
        ClaudeAgentOptions::builder()
            .session_store(MinimalStore::default())
            .enable_file_checkpointing(true)
            .build(),
    );
    assert_eq!(
        message,
        "session_store cannot be combined with enable_file_checkpointing (checkpoints are \
         local-disk only and would diverge from the mirrored transcript)"
    );

    let store_alone = ClaudeAgentOptions::builder()
        .session_store(MinimalStore::default())
        .build()
        .unwrap();
    assert!(store_alone.session_store.is_some());

    let checkpointing_alone = ClaudeAgentOptions::builder()
        .enable_file_checkpointing(true)
        .build()
        .unwrap();
    assert!(checkpointing_alone.enable_file_checkpointing);
}

#[test]
fn continue_conversation_needs_a_store_that_lists_sessions() {
    let message = config_error(
        ClaudeAgentOptions::builder()
            .session_store(MinimalStore::default())
            .continue_conversation(true)
            .build(),
    );
    assert_eq!(
        message,
        "continue_conversation with session_store requires the store to implement \
         list_sessions()"
    );

    ClaudeAgentOptions::builder()
        .session_store(MinimalStore::default())
        .continue_conversation(true)
        .resume("session-123")
        .build()
        .unwrap();

    ClaudeAgentOptions::builder()
        .session_store(ListingStore)
        .continue_conversation(true)
        .build()
        .unwrap();
}

#[tokio::test]
async fn session_store_option_reaches_the_store() {
    let options = ClaudeAgentOptions::builder()
        .session_store(SharedSessionStore::new(MinimalStore::default()))
        .build()
        .unwrap();
    let store = options.session_store.clone().unwrap();
    let key = SessionKey::new("project", "session-1");
    assert_eq!(store.load(&key).await.unwrap(), None);

    let entry: SessionStoreEntry = json!({"type": "user", "uuid": "u1"})
        .as_object()
        .unwrap()
        .clone();
    store
        .append(&key, std::slice::from_ref(&entry))
        .await
        .unwrap();
    assert_eq!(
        options.session_store.unwrap().load(&key).await.unwrap(),
        Some(vec![entry])
    );

    assert_eq!(store.capabilities(), SessionStoreCapabilities::none());
    let error = store.list_sessions("project").await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<UnsupportedStoreOperation>(),
        Some(&UnsupportedStoreOperation {
            operation: "list_sessions"
        })
    );
    assert!(store.list_session_summaries("project").await.is_err());
    assert!(store.delete(&key).await.is_err());
    assert!(
        store
            .list_subkeys(&claude_agent_kit::SessionListSubkeysKey::new(
                "project",
                "session-1"
            ))
            .await
            .is_err()
    );

    let listing = SharedSessionStore::new(ListingStore);
    assert!(listing.capabilities().list_sessions);
    assert_eq!(
        listing.list_sessions("project").await.unwrap(),
        [SessionStoreListEntry::new("session-1", 1_700_000_000_000)]
    );
}

#[test]
fn shadow_warning_requires_a_callback() {
    let options = ClaudeAgentOptions::builder()
        .permission_mode(PermissionMode::BypassPermissions)
        .allowed_tools(["Read", "Bash"])
        .build()
        .unwrap();
    assert_eq!(options.can_use_tool_shadowed_warning(), None);
}

#[test]
fn shadow_warning_for_bypass_permissions() {
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(allow_everything)
        .permission_mode(PermissionMode::BypassPermissions)
        .build()
        .unwrap();
    let message = options.can_use_tool_shadowed_warning().unwrap();
    assert!(message.contains("bypassPermissions"));
    assert!(message.contains("PreToolUse"));
}

#[test]
fn shadow_warning_for_whole_tool_entries() {
    let warning = |allowed_tools: &[&str]| {
        ClaudeAgentOptions::builder()
            .can_use_tool(allow_everything)
            .allowed_tools(allowed_tools.iter().copied())
            .build()
            .unwrap()
            .can_use_tool_shadowed_warning()
    };
    assert!(
        warning(&["Read(*)"])
            .unwrap()
            .contains("invoked for: Read.")
    );
    assert!(warning(&["Read"]).unwrap().contains("invoked for: Read."));
    assert!(
        warning(&["Write", "Read", "Write()"])
            .unwrap()
            .contains("invoked for: Write, Read.")
    );
    assert_eq!(warning(&["Bash(ls:*)"]), None);
    assert_eq!(warning(&[]), None);
}

#[test]
fn shadow_warning_is_silent_for_accept_edits_with_specifier_entries() {
    let options = ClaudeAgentOptions::builder()
        .can_use_tool(allow_everything)
        .permission_mode(PermissionMode::AcceptEdits)
        .allowed_tools(["Bash(ls:*)"])
        .build()
        .unwrap();
    assert_eq!(options.can_use_tool_shadowed_warning(), None);
}

#[test]
fn skills_all_shadows_the_skill_tool() {
    let warning = |skills: Skills, allowed_tools: &[&str]| {
        let options = ClaudeAgentOptions::builder()
            .can_use_tool(allow_everything)
            .skills(skills)
            .allowed_tools(allowed_tools.iter().copied())
            .build()
            .unwrap();
        let warning = options.can_use_tool_shadowed_warning();
        assert_eq!(options.allowed_tools, allowed_tools);
        warning
    };
    assert!(
        warning(Skills::All, &[])
            .unwrap()
            .contains("invoked for: Skill.")
    );
    assert_eq!(warning(Skills::list(["reviewer"]), &[]), None);
    assert!(
        warning(Skills::All, &["Skill"])
            .unwrap()
            .contains("invoked for: Skill.")
    );
    assert!(
        warning(Skills::All, &["Skill(*)"])
            .unwrap()
            .contains("invoked for: Skill.")
    );
    assert!(
        warning(Skills::All, &["Read"])
            .unwrap()
            .contains("invoked for: Read, Skill.")
    );
}
