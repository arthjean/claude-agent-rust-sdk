//! Session options and their builder.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde_json::Value;

use super::config::{
    AgentDefinition, EffortLevel, McpServerConfig, McpServers, SandboxSettings, SdkBeta,
    SdkPluginConfig, SettingSource, Skills, StderrCallback, SystemPrompt, TaskBudget,
    ThinkingConfig, Tools,
};
use super::hook::{HookEvent, HookMatcher};
use super::permission::{CanUseTool, PermissionMode};
use crate::error::Error;
use crate::session_store::{SessionStore, SessionStoreFlushMode, SharedSessionStore};

const STDIO_PERMISSION_PROMPT_TOOL: &str = "stdio";
const DEFAULT_LOAD_TIMEOUT_MS: u64 = 60_000;
const SKILL_TOOL: &str = "Skill";

/// Options of a session.
///
/// Build a value with [`ClaudeAgentOptions::builder`]; the builder checks
/// the combinations the CLI cannot honor. Every field has the default of
/// the upstream Python SDK.
#[derive(Clone)]
#[non_exhaustive]
pub struct ClaudeAgentOptions {
    /// Base set of available built-in tools. To restrict which tools run
    /// without a prompt, use `allowed_tools` instead.
    pub tools: Option<Tools>,
    /// Tool names that run without asking for permission.
    pub allowed_tools: Vec<String>,
    /// System prompt configuration.
    pub system_prompt: Option<SystemPrompt>,
    /// MCP server configurations.
    pub mcp_servers: McpServers,
    /// Uses only the MCP servers given in `mcp_servers`, ignoring every
    /// other MCP configuration the CLI would load.
    pub strict_mcp_config: bool,
    /// Permission mode of the session.
    pub permission_mode: Option<PermissionMode>,
    /// Continues the most recent conversation of the working directory.
    pub continue_conversation: bool,
    /// Id of the session to resume.
    pub resume: Option<String>,
    /// Session id to use instead of a generated one. It must be a UUID.
    pub session_id: Option<String>,
    /// Maximum number of turns before the query stops.
    pub max_turns: Option<u32>,
    /// Maximum budget in US dollars. The query stops when it is exceeded.
    pub max_budget_usd: Option<f64>,
    /// Tool names removed from the model's context.
    pub disallowed_tools: Vec<String>,
    /// Model to use. Defaults to the default model of the CLI.
    pub model: Option<String>,
    /// Model to use when the primary model fails or is unavailable.
    pub fallback_model: Option<String>,
    /// Beta features to enable.
    pub betas: Vec<SdkBeta>,
    /// MCP tool that handles permission prompts. Setting `can_use_tool`
    /// forces this to `stdio`.
    pub permission_prompt_tool_name: Option<String>,
    /// Working directory of the session. Defaults to the directory of this
    /// process, as does an empty path.
    pub cwd: Option<PathBuf>,
    /// Path of the CLI executable. Discovered when not set.
    pub cli_path: Option<PathBuf>,
    /// Path of an additional settings file, or an inline JSON string.
    pub settings: Option<String>,
    /// Additional directories the agent can access.
    pub add_dirs: Vec<PathBuf>,
    /// Environment variables for the CLI process, merged over the inherited
    /// environment.
    pub env: IndexMap<String, String>,
    /// Additional CLI arguments, keyed by flag name without `--`. A `None`
    /// value emits a bare flag.
    pub extra_args: IndexMap<String, Option<String>>,
    /// Maximum number of bytes buffered for one message of CLI output.
    pub max_buffer_size: Option<usize>,
    /// Callback that receives each line of CLI error output.
    pub stderr: Option<StderrCallback>,
    /// Permission handler for tool calls that would otherwise prompt the
    /// user. It does not run for calls that `allowed_tools`, the permission
    /// mode, or allow rules already permit; see
    /// [`ClaudeAgentOptions::can_use_tool_shadowed_warning`].
    pub can_use_tool: Option<CanUseTool>,
    /// Hook callbacks, keyed by event. Matchers registered on the same
    /// event are dispatched concurrently by the CLI.
    pub hooks: Option<IndexMap<HookEvent, Vec<HookMatcher>>>,
    /// Name of the operating-system user the CLI process runs as. The
    /// process takes the user id, the primary group, and the supplementary
    /// groups of that user, as `sudo -u` gives them (an unprivileged
    /// process cannot set groups and keeps its own). The
    /// environment is not changed: set `HOME` through `env` when the CLI
    /// must read that user's configuration. Unix only: switching to
    /// another user needs the privilege to do so, and connecting fails on
    /// Windows when this is set.
    pub user: Option<String>,
    /// Emits a stream event for each partial message update.
    pub include_partial_messages: bool,
    /// Emits hook lifecycle events in the message stream.
    pub include_hook_events: bool,
    /// Forwards the text and thinking blocks of subagents as messages.
    pub forward_subagent_text: bool,
    /// Delivers every prompt as written: no file-mention expansion and no
    /// slash-command dispatch.
    pub verbatim_prompts: bool,
    /// Forks a resumed session to a new session id.
    pub fork_session: bool,
    /// When resuming, loads the conversation only up to and including the
    /// message with this UUID.
    pub resume_session_at: Option<String>,
    /// With `resume_session_at`: UUID of the user prompt whose turn the
    /// truncating resume intends to discard. The CLI refuses the resume
    /// when the discarded range holds anything else.
    pub resume_drops_turn: Option<String>,
    /// Custom subagents, keyed by agent name.
    pub agents: Option<IndexMap<String, AgentDefinition>>,
    /// Filesystem settings to load. `None` loads every source; an empty
    /// list loads none.
    pub setting_sources: Option<Vec<SettingSource>>,
    /// Skills to enable for the main session. `None` leaves the defaults of
    /// the CLI untouched.
    pub skills: Option<Skills>,
    /// Sandbox settings for command execution.
    pub sandbox: Option<SandboxSettings>,
    /// Plugins to load.
    pub plugins: Vec<SdkPluginConfig>,
    /// Maximum thinking tokens. Deprecated in favor of `thinking`, which
    /// takes precedence.
    pub max_thinking_tokens: Option<u64>,
    /// Thinking behavior of the model.
    pub thinking: Option<ThinkingConfig>,
    /// How much effort the model puts into its response.
    pub effort: Option<EffortLevel>,
    /// Output format for structured responses, such as a JSON schema.
    pub output_format: Option<Value>,
    /// Tracks file changes so files can be rewound to an earlier message.
    pub enable_file_checkpointing: bool,
    /// Store that receives a copy of the session transcript.
    pub session_store: Option<SharedSessionStore>,
    /// When mirrored transcript entries are flushed to `session_store`.
    pub session_store_flush: SessionStoreFlushMode,
    /// Timeout of each store load during a resume, in milliseconds.
    pub load_timeout_ms: u64,
    /// API-side task budget in tokens.
    pub task_budget: Option<TaskBudget>,
    /// Identifier of your application, such as `my-app/1.0.0`, sent on API
    /// requests.
    pub client_app: Option<String>,
}

/// Prints every field, with the values of `env` and `extra_args`, and
/// `settings` given as inline JSON, replaced by `<redacted>`: they hold API
/// keys and tokens. The destructuring lists every field, so a new field
/// fails to compile until it is printed here.
impl std::fmt::Debug for ClaudeAgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            tools,
            allowed_tools,
            system_prompt,
            mcp_servers,
            strict_mcp_config,
            permission_mode,
            continue_conversation,
            resume,
            session_id,
            max_turns,
            max_budget_usd,
            disallowed_tools,
            model,
            fallback_model,
            betas,
            permission_prompt_tool_name,
            cwd,
            cli_path,
            settings,
            add_dirs,
            env,
            extra_args,
            max_buffer_size,
            stderr,
            can_use_tool,
            hooks,
            user,
            include_partial_messages,
            include_hook_events,
            forward_subagent_text,
            verbatim_prompts,
            fork_session,
            resume_session_at,
            resume_drops_turn,
            agents,
            setting_sources,
            skills,
            sandbox,
            plugins,
            max_thinking_tokens,
            thinking,
            effort,
            output_format,
            enable_file_checkpointing,
            session_store,
            session_store_flush,
            load_timeout_ms,
            task_budget,
            client_app,
        } = self;
        f.debug_struct("ClaudeAgentOptions")
            .field("tools", tools)
            .field("allowed_tools", allowed_tools)
            .field("system_prompt", system_prompt)
            .field("mcp_servers", mcp_servers)
            .field("strict_mcp_config", strict_mcp_config)
            .field("permission_mode", permission_mode)
            .field("continue_conversation", continue_conversation)
            .field("resume", resume)
            .field("session_id", session_id)
            .field("max_turns", max_turns)
            .field("max_budget_usd", max_budget_usd)
            .field("disallowed_tools", disallowed_tools)
            .field("model", model)
            .field("fallback_model", fallback_model)
            .field("betas", betas)
            .field("permission_prompt_tool_name", permission_prompt_tool_name)
            .field("cwd", cwd)
            .field("cli_path", cli_path)
            .field(
                "settings",
                &super::redact::RedactedSettings(settings.as_deref()),
            )
            .field("add_dirs", add_dirs)
            .field("env", &super::redact::RedactedMap(env))
            .field("extra_args", &super::redact::RedactedArguments(extra_args))
            .field("max_buffer_size", max_buffer_size)
            .field("stderr", stderr)
            .field("can_use_tool", can_use_tool)
            .field("hooks", hooks)
            .field("user", user)
            .field("include_partial_messages", include_partial_messages)
            .field("include_hook_events", include_hook_events)
            .field("forward_subagent_text", forward_subagent_text)
            .field("verbatim_prompts", verbatim_prompts)
            .field("fork_session", fork_session)
            .field("resume_session_at", resume_session_at)
            .field("resume_drops_turn", resume_drops_turn)
            .field("agents", agents)
            .field("setting_sources", setting_sources)
            .field("skills", skills)
            .field("sandbox", sandbox)
            .field("plugins", plugins)
            .field("max_thinking_tokens", max_thinking_tokens)
            .field("thinking", thinking)
            .field("effort", effort)
            .field("output_format", output_format)
            .field("enable_file_checkpointing", enable_file_checkpointing)
            .field("session_store", session_store)
            .field("session_store_flush", session_store_flush)
            .field("load_timeout_ms", load_timeout_ms)
            .field("task_budget", task_budget)
            .field("client_app", client_app)
            .finish()
    }
}

impl Default for ClaudeAgentOptions {
    fn default() -> Self {
        Self {
            tools: None,
            allowed_tools: Vec::new(),
            system_prompt: None,
            mcp_servers: McpServers::default(),
            strict_mcp_config: false,
            permission_mode: None,
            continue_conversation: false,
            resume: None,
            session_id: None,
            max_turns: None,
            max_budget_usd: None,
            disallowed_tools: Vec::new(),
            model: None,
            fallback_model: None,
            betas: Vec::new(),
            permission_prompt_tool_name: None,
            cwd: None,
            cli_path: None,
            settings: None,
            add_dirs: Vec::new(),
            env: IndexMap::new(),
            extra_args: IndexMap::new(),
            max_buffer_size: None,
            stderr: None,
            can_use_tool: None,
            hooks: None,
            user: None,
            include_partial_messages: false,
            include_hook_events: false,
            forward_subagent_text: false,
            verbatim_prompts: false,
            fork_session: false,
            resume_session_at: None,
            resume_drops_turn: None,
            agents: None,
            setting_sources: None,
            skills: None,
            sandbox: None,
            plugins: Vec::new(),
            max_thinking_tokens: None,
            thinking: None,
            effort: None,
            output_format: None,
            enable_file_checkpointing: false,
            session_store: None,
            session_store_flush: SessionStoreFlushMode::default(),
            load_timeout_ms: DEFAULT_LOAD_TIMEOUT_MS,
            task_budget: None,
            client_app: None,
        }
    }
}

impl ClaudeAgentOptions {
    /// Starts a builder with every option at its default.
    pub fn builder() -> ClaudeAgentOptionsBuilder {
        ClaudeAgentOptionsBuilder::default()
    }

    /// Returns a builder that starts from these options.
    ///
    /// The `stdio` permission prompt tool that a previous build forced for
    /// `can_use_tool` is cleared, so the options can be built again.
    pub fn into_builder(mut self) -> ClaudeAgentOptionsBuilder {
        let forced_by_callback = self.can_use_tool.is_some()
            && self.permission_prompt_tool_name.as_deref() == Some(STDIO_PERMISSION_PROMPT_TOOL);
        if forced_by_callback {
            self.permission_prompt_tool_name = None;
        }
        ClaudeAgentOptionsBuilder { options: self }
    }

    /// Explains why `can_use_tool` will not run for some tool calls, or
    /// returns `None` when nothing visibly shadows it.
    ///
    /// The callback is shadowed when the permission mode is
    /// `bypassPermissions`, or when an `allowed_tools` entry allows a whole
    /// tool (`Read`, `Read()`, or `Read(*)`). Enabling every skill counts
    /// as allowing the whole `Skill` tool. The warning is advisory:
    /// shadowing can be intentional.
    #[must_use]
    pub fn can_use_tool_shadowed_warning(&self) -> Option<String> {
        self.can_use_tool.as_ref()?;
        let mut allowed_tools: Vec<&str> = self.allowed_tools.iter().map(String::as_str).collect();
        if matches!(self.skills, Some(Skills::All)) && !allowed_tools.contains(&SKILL_TOOL) {
            allowed_tools.push(SKILL_TOOL);
        }
        shadowed_warning_message(self.permission_mode.as_ref(), &allowed_tools)
    }

    /// Returns the options a session starts from: the checks of
    /// [`ClaudeAgentOptionsBuilder::build`] are applied again, so options
    /// changed after they were built are still consistent.
    pub(crate) fn for_session(&self) -> Result<Self, Error> {
        let mut options = self.clone();
        validate_session_store(&options)?;
        let forced_by_callback = options.can_use_tool.is_some()
            && options.permission_prompt_tool_name.as_deref() == Some(STDIO_PERMISSION_PROMPT_TOOL);
        if !forced_by_callback {
            configure_can_use_tool(&mut options)?;
        }
        Ok(options)
    }
}

fn whole_tool_allowed(entry: &str) -> Option<&str> {
    if entry.trim().is_empty() {
        return None;
    }
    let Some(open_index) = entry.find('(') else {
        return Some(entry);
    };
    if open_index == 0 || !entry.ends_with(')') {
        return None;
    }
    let specifier = &entry[open_index + 1..entry.len() - 1];
    matches!(specifier, "" | "*").then(|| &entry[..open_index])
}

fn shadowed_warning_message(
    permission_mode: Option<&PermissionMode>,
    allowed_tools: &[&str],
) -> Option<String> {
    if permission_mode == Some(&PermissionMode::BypassPermissions) {
        return Some(
            "can_use_tool will not be invoked: permission_mode 'bypassPermissions' \
             auto-approves every tool call (except explicit deny rules) before the callback \
             is consulted. To gate every tool call, use a PreToolUse hook instead."
                .to_owned(),
        );
    }
    let mut shadowed: Vec<&str> = Vec::new();
    for tool in allowed_tools
        .iter()
        .filter_map(|entry| whole_tool_allowed(entry))
    {
        if !shadowed.contains(&tool) {
            shadowed.push(tool);
        }
    }
    if shadowed.is_empty() {
        return None;
    }
    Some(format!(
        "can_use_tool will not be invoked for: {}. An allowed_tools entry that allows a whole \
         tool auto-approves it before the callback is consulted. To gate every tool call, use \
         a PreToolUse hook; or narrow the entry so calls fall through to can_use_tool. Allow \
         rules from settings files can also shadow the callback but are not visible here.",
        shadowed.join(", ")
    ))
}

fn validate_session_store(options: &ClaudeAgentOptions) -> Result<(), Error> {
    let Some(store) = &options.session_store else {
        return Ok(());
    };
    if options.continue_conversation
        && options.resume.is_none()
        && !store.capabilities().list_sessions
    {
        return Err(Error::config(
            "continue_conversation with session_store requires the store to implement \
             list_sessions()",
        ));
    }
    if options.enable_file_checkpointing {
        return Err(Error::config(
            "session_store cannot be combined with enable_file_checkpointing (checkpoints are \
             local-disk only and would diverge from the mirrored transcript)",
        ));
    }
    Ok(())
}

fn configure_can_use_tool(options: &mut ClaudeAgentOptions) -> Result<(), Error> {
    if options.can_use_tool.is_none() {
        return Ok(());
    }
    let prompt_tool_is_set = options
        .permission_prompt_tool_name
        .as_deref()
        .is_some_and(|name| !name.is_empty());
    if prompt_tool_is_set {
        return Err(Error::config(
            "can_use_tool callback cannot be used with permission_prompt_tool_name. Please use \
             one or the other.",
        ));
    }
    options.permission_prompt_tool_name = Some(STDIO_PERMISSION_PROMPT_TOOL.to_owned());
    Ok(())
}

/// Builder of [`ClaudeAgentOptions`].
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct ClaudeAgentOptionsBuilder {
    options: ClaudeAgentOptions,
}

macro_rules! option_setters {
    ($( $(#[$meta:meta])* $kind:ident $field:ident: $ty:ty; )*) => {
        $( option_setters!(@one $(#[$meta])* $kind $field: $ty); )*
    };
    (@one $(#[$meta:meta])* some $field:ident: $ty:ty) => {
        $(#[$meta])*
        pub fn $field(mut self, value: impl Into<$ty>) -> Self {
            self.options.$field = Some(value.into());
            self
        }
    };
    (@one $(#[$meta:meta])* value $field:ident: $ty:ty) => {
        $(#[$meta])*
        pub fn $field(mut self, value: $ty) -> Self {
            self.options.$field = Some(value);
            self
        }
    };
    (@one $(#[$meta:meta])* flag $field:ident: $ty:ty) => {
        $(#[$meta])*
        pub fn $field(mut self, value: $ty) -> Self {
            self.options.$field = value;
            self
        }
    };
    (@one $(#[$meta:meta])* list $field:ident: $ty:ty) => {
        $(#[$meta])*
        pub fn $field(mut self, values: impl IntoIterator<Item = impl Into<$ty>>) -> Self {
            self.options.$field = values.into_iter().map(Into::into).collect();
            self
        }
    };
    (@one $(#[$meta:meta])* some_list $field:ident: $ty:ty) => {
        $(#[$meta])*
        pub fn $field(mut self, values: impl IntoIterator<Item = impl Into<$ty>>) -> Self {
            self.options.$field = Some(values.into_iter().map(Into::into).collect());
            self
        }
    };
}

impl ClaudeAgentOptionsBuilder {
    /// Sets the store that receives a copy of the transcript: any
    /// [`SessionStore`], or a [`SharedSessionStore`] handle.
    pub fn session_store(mut self, store: impl SessionStore) -> Self {
        self.options.session_store = Some(SharedSessionStore::new(store));
        self
    }

    option_setters! {
        /// Sets the base set of built-in tools.
        some tools: Tools;
        /// Sets the tool names that run without asking for permission.
        list allowed_tools: String;
        /// Sets the system prompt.
        some system_prompt: SystemPrompt;
        /// Sets the MCP servers.
        flag mcp_servers: McpServers;
        /// Uses only the MCP servers given in the options.
        flag strict_mcp_config: bool;
        /// Sets the permission mode.
        some permission_mode: PermissionMode;
        /// Continues the most recent conversation of the working directory.
        flag continue_conversation: bool;
        /// Sets the id of the session to resume.
        some resume: String;
        /// Sets the session id to use.
        some session_id: String;
        /// Sets the maximum number of turns.
        value max_turns: u32;
        /// Sets the maximum budget in US dollars.
        value max_budget_usd: f64;
        /// Sets the tool names removed from the model's context.
        list disallowed_tools: String;
        /// Sets the model.
        some model: String;
        /// Sets the fallback model.
        some fallback_model: String;
        /// Sets the beta features to enable.
        list betas: SdkBeta;
        /// Sets the MCP tool that handles permission prompts. It cannot be
        /// combined with `can_use_tool`.
        some permission_prompt_tool_name: String;
        /// Sets the working directory of the session.
        some cwd: PathBuf;
        /// Sets the path of the CLI executable.
        some cli_path: PathBuf;
        /// Sets a settings file path or an inline JSON string.
        some settings: String;
        /// Sets the additional directories the agent can access.
        list add_dirs: PathBuf;
        /// Sets the maximum number of bytes buffered for one message.
        value max_buffer_size: usize;
        /// Sets the callback that receives each line of CLI error output.
        some stderr: StderrCallback;
        /// Sets the tool-permission callback. This forces
        /// `permission_prompt_tool_name` to `stdio`.
        some can_use_tool: CanUseTool;
        /// Sets the user identifier.
        some user: String;
        /// Emits a stream event for each partial message update.
        flag include_partial_messages: bool;
        /// Emits hook lifecycle events in the message stream.
        flag include_hook_events: bool;
        /// Forwards the text and thinking blocks of subagents.
        flag forward_subagent_text: bool;
        /// Delivers every prompt as written.
        flag verbatim_prompts: bool;
        /// Forks a resumed session to a new session id.
        flag fork_session: bool;
        /// Sets the message UUID a resume stops at.
        some resume_session_at: String;
        /// Sets the UUID of the user prompt a truncating resume discards.
        some resume_drops_turn: String;
        /// Sets the filesystem settings to load. An empty list loads none.
        some_list setting_sources: SettingSource;
        /// Sets the skills to enable.
        some skills: Skills;
        /// Sets the sandbox settings.
        some sandbox: SandboxSettings;
        /// Sets the plugins to load.
        list plugins: SdkPluginConfig;
        /// Sets the maximum thinking tokens. Prefer `thinking`.
        value max_thinking_tokens: u64;
        /// Sets the thinking behavior.
        some thinking: ThinkingConfig;
        /// Sets the effort level.
        some effort: EffortLevel;
        /// Sets the output format for structured responses.
        some output_format: Value;
        /// Tracks file changes so files can be rewound.
        flag enable_file_checkpointing: bool;
        /// Sets when mirrored entries are flushed to the store.
        flag session_store_flush: SessionStoreFlushMode;
        /// Sets the timeout of each store load during a resume.
        flag load_timeout_ms: u64;
        /// Sets the API-side task budget.
        some task_budget: TaskBudget;
        /// Sets the identifier of your application.
        some client_app: String;
    }

    /// Adds one MCP server, replacing an inline JSON or path configuration.
    pub fn mcp_server(
        mut self,
        name: impl Into<String>,
        config: impl Into<McpServerConfig>,
    ) -> Self {
        if !matches!(self.options.mcp_servers, McpServers::Map(_)) {
            self.options.mcp_servers = McpServers::default();
        }
        if let McpServers::Map(servers) = &mut self.options.mcp_servers {
            servers.insert(name.into(), config.into());
        }
        self
    }

    /// Sets the environment variables of the CLI process.
    pub fn env(
        mut self,
        env: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.options.env = env
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        self
    }

    /// Adds one environment variable of the CLI process.
    pub fn env_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.env.insert(key.into(), value.into());
        self
    }

    /// Sets the additional CLI arguments.
    pub fn extra_args(
        mut self,
        extra_args: impl IntoIterator<Item = (impl Into<String>, Option<String>)>,
    ) -> Self {
        self.options.extra_args = extra_args
            .into_iter()
            .map(|(flag, value)| (flag.into(), value))
            .collect();
        self
    }

    /// Adds one CLI argument. A `None` value emits a bare flag.
    pub fn extra_arg(mut self, flag: impl Into<String>, value: Option<&str>) -> Self {
        self.options
            .extra_args
            .insert(flag.into(), value.map(str::to_owned));
        self
    }

    /// Sets the hook callbacks.
    pub fn hooks(mut self, hooks: impl IntoIterator<Item = (HookEvent, Vec<HookMatcher>)>) -> Self {
        self.options.hooks = Some(hooks.into_iter().collect());
        self
    }

    /// Adds one matcher to an event.
    pub fn hook(mut self, event: impl Into<HookEvent>, matcher: HookMatcher) -> Self {
        self.options
            .hooks
            .get_or_insert_with(IndexMap::new)
            .entry(event.into())
            .or_default()
            .push(matcher);
        self
    }

    /// Sets the custom subagents.
    pub fn agents(
        mut self,
        agents: impl IntoIterator<Item = (impl Into<String>, AgentDefinition)>,
    ) -> Self {
        self.options.agents = Some(
            agents
                .into_iter()
                .map(|(name, definition)| (name.into(), definition))
                .collect(),
        );
        self
    }

    /// Adds one custom subagent.
    pub fn agent(mut self, name: impl Into<String>, definition: AgentDefinition) -> Self {
        self.options
            .agents
            .get_or_insert_with(IndexMap::new)
            .insert(name.into(), definition);
        self
    }

    /// Checks the options and returns them.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`](crate::ErrorKind::Config) when `can_use_tool` is combined with
    /// `permission_prompt_tool_name`, when `session_store` is combined with
    /// `enable_file_checkpointing`, or when `continue_conversation` is used
    /// without `resume` on a store that does not list sessions.
    pub fn build(self) -> Result<ClaudeAgentOptions, Error> {
        let mut options = self.options;
        validate_session_store(&options)?;
        configure_can_use_tool(&mut options)?;
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::{ClaudeAgentOptions, PermissionMode, shadowed_warning_message, whole_tool_allowed};
    use crate::error::{Error, ErrorKind};
    use crate::types::{CanUseTool, PermissionResult};

    fn allow_everything() -> CanUseTool {
        CanUseTool::new(async |_tool_name, _input, _context| Ok(PermissionResult::allow()))
    }

    #[test]
    fn session_options_force_the_stdio_prompt_tool_for_a_callback_set_after_the_build() {
        let mut options = ClaudeAgentOptions::default();
        assert_eq!(
            options.for_session().unwrap().permission_prompt_tool_name,
            None
        );
        options.can_use_tool = Some(allow_everything());
        assert_eq!(
            options
                .for_session()
                .unwrap()
                .permission_prompt_tool_name
                .as_deref(),
            Some("stdio")
        );

        let built = ClaudeAgentOptions::builder()
            .can_use_tool(allow_everything())
            .build()
            .unwrap();
        assert_eq!(
            built
                .for_session()
                .unwrap()
                .permission_prompt_tool_name
                .as_deref(),
            Some("stdio")
        );

        options.permission_prompt_tool_name = Some("mcp__approver__ask".to_owned());
        assert_eq!(
            options.for_session().as_ref().err().map(Error::kind),
            Some(ErrorKind::Config)
        );
    }

    #[test]
    fn a_callback_next_to_the_stdio_prompt_tool_is_accepted_only_when_the_fields_are_set_directly()
    {
        let mut direct = ClaudeAgentOptions {
            can_use_tool: Some(allow_everything()),
            permission_prompt_tool_name: Some("stdio".to_owned()),
            ..ClaudeAgentOptions::default()
        };
        assert_eq!(
            direct
                .for_session()
                .unwrap()
                .permission_prompt_tool_name
                .as_deref(),
            Some("stdio")
        );

        direct.permission_prompt_tool_name = Some("mcp__approver__ask".to_owned());
        assert_eq!(
            direct.for_session().as_ref().err().map(Error::kind),
            Some(ErrorKind::Config)
        );

        let built = ClaudeAgentOptions::builder()
            .can_use_tool(allow_everything())
            .permission_prompt_tool_name("stdio")
            .build();
        assert_eq!(
            built.as_ref().err().map(Error::kind),
            Some(ErrorKind::Config),
            "the builder refuses any prompt tool next to the callback, as upstream"
        );
    }

    #[test]
    fn whole_tool_allowed_mirrors_the_cli_rule_parser() {
        let cases: [(&str, Option<&str>); 15] = [
            ("Read", Some("Read")),
            ("mcp__server__tool", Some("mcp__server__tool")),
            ("Read(*)", Some("Read")),
            ("Read()", Some("Read")),
            ("mcp__server__tool(*)", Some("mcp__server__tool")),
            ("Bash(ls:*)", None),
            ("Bash(git log:*)", None),
            ("Bash(*.py)", None),
            ("", None),
            ("   ", None),
            ("Bash(ls:*", None),
            ("Bash(ls)x", None),
            ("(foo)", None),
            ("(*)", None),
            ("Read(*x", None),
        ];
        for (entry, expected) in cases {
            assert_eq!(whole_tool_allowed(entry), expected, "entry {entry:?}");
        }
    }

    #[test]
    fn bypass_permissions_message() {
        let message =
            shadowed_warning_message(Some(&PermissionMode::BypassPermissions), &[]).unwrap();
        assert!(message.contains("bypassPermissions"));
        assert!(message.contains("PreToolUse"));
    }

    #[test]
    fn bare_entries_message() {
        let message =
            shadowed_warning_message(None, &["Read", "mcp__server__tool", "Bash(ls:*)"]).unwrap();
        assert!(message.contains("Read, mcp__server__tool"));
        assert!(!message.contains("Bash(ls:*)"));
        assert!(message.contains("PreToolUse"));
        assert!(message.contains("settings files"));
    }

    #[test]
    fn bypass_permissions_takes_precedence_over_bare_entries() {
        let message =
            shadowed_warning_message(Some(&PermissionMode::BypassPermissions), &["Read", "Write"])
                .unwrap();
        assert!(message.contains("bypassPermissions"));
        assert!(!message.contains("Read"));
        assert!(!message.contains("Write"));
    }

    #[test]
    fn preserves_allowed_tools_order() {
        let message = shadowed_warning_message(None, &["Write", "Read"]).unwrap();
        assert!(message.contains("Write, Read"));
    }

    #[test]
    fn accept_edits_without_bare_entries_returns_none() {
        assert_eq!(
            shadowed_warning_message(Some(&PermissionMode::AcceptEdits), &[]),
            None
        );
    }

    #[test]
    fn accept_edits_still_reports_bare_entries() {
        let message =
            shadowed_warning_message(Some(&PermissionMode::AcceptEdits), &["Read"]).unwrap();
        assert!(message.contains("Read"));
        assert!(!message.contains("bypassPermissions"));
    }

    #[test]
    fn wildcard_and_empty_specifiers_are_whole_tool_allows() {
        let message = shadowed_warning_message(None, &["Read(*)", "Write()"]).unwrap();
        assert!(message.contains("invoked for: Read, Write."));
    }

    #[test]
    fn blank_entries_never_reach_the_message() {
        let message = shadowed_warning_message(None, &["   ", "Read"]).unwrap();
        assert!(message.contains("invoked for: Read."));
    }

    #[test]
    fn entries_resolving_to_the_same_tool_are_reported_once() {
        let message = shadowed_warning_message(None, &["Read", "Read()", "Read(*)"]).unwrap();
        assert!(message.contains("invoked for: Read."));
    }

    #[test]
    fn dedup_preserves_first_seen_order() {
        let message = shadowed_warning_message(None, &["Write", "Read", "Write()"]).unwrap();
        assert!(message.contains("invoked for: Write, Read."));
    }

    #[test]
    fn specifier_and_empty_entries_return_none() {
        assert_eq!(
            shadowed_warning_message(
                None,
                &[
                    "Bash(ls:*)",
                    "Bash(git log:*)",
                    "mcp__server__tool(param:value)",
                    ""
                ],
            ),
            None
        );
    }

    #[test]
    fn empty_allowed_tools_default_mode_returns_none() {
        assert_eq!(shadowed_warning_message(None, &[]), None);
    }
}
