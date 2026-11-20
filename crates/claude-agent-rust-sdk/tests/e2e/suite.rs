//! Shared rules and helpers of the end-to-end suite.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use claude_agent_rust_sdk::{
    BoxError, ClaudeAgentOptions, ClaudeAgentOptionsBuilder, ClaudeSdkClient, ContentBlock,
    HookCallback, HookContext, HookInput, HookOutput, Message, Prompt, ResultMessage,
    SyncHookOutput, UserContent, project_key_for_directory, query,
};
use futures_util::StreamExt;
use serde_json::Value;

/// Variable holding the API key the suite authenticates with.
pub const CREDENTIALS_VARIABLE: &str = "ANTHROPIC_API_KEY";
/// Variable a developer sets to `cli` to use the login of the installed CLI.
pub const LOGIN_VARIABLE: &str = "CLAUDE_AGENT_RUST_SDK_E2E_AUTH";
/// Value of [`LOGIN_VARIABLE`] that selects the login of the installed CLI.
pub const LOGIN_VALUE: &str = "cli";
/// Variable through which the suite pins the default model of the CLI.
pub const MODEL_VARIABLE: &str = "ANTHROPIC_DEFAULT_MODEL";
/// Variable that selects the model of a session ahead of a `model` in the
/// settings files of the machine, which outranks [`MODEL_VARIABLE`].
pub const SESSION_MODEL_VARIABLE: &str = "ANTHROPIC_MODEL";
/// Model the suite last passed on.
pub const PINNED_MODEL: &str = "claude-haiku-5-5";
/// Ceiling for one turn against the API.
pub const TURN_LIMIT: Duration = Duration::from_secs(240);
/// Names of the shell tools, which differ between platforms.
pub const SHELL_TOOLS: [&str; 2] = ["Bash", "PowerShell"];
/// Hook matcher for the shell tools.
pub const SHELL_MATCHER: &str = "Bash|PowerShell";
/// Names the CLI has used for the tool that starts a subagent.
pub const AGENT_TOOLS: [&str; 2] = ["Agent", "Task"];

/// Why the suite cannot start with the variables `lookup` sees, or `None`
/// when credentials are available.
pub fn credentials_error(lookup: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |name: &str| lookup(name).filter(|value| !value.is_empty());
    if set(CREDENTIALS_VARIABLE).is_some() || set(LOGIN_VARIABLE).as_deref() == Some(LOGIN_VALUE) {
        return None;
    }
    Some(format!(
        "{CREDENTIALS_VARIABLE} environment variable is required for the end-to-end suite. \
         Set it before running, or set {LOGIN_VARIABLE}={LOGIN_VALUE} to use the login of the \
         installed CLI."
    ))
}

/// Whether the credentials come from the environment, and so follow the
/// CLI into an empty configuration directory.
pub fn credentials_travel_with_the_environment() -> bool {
    [CREDENTIALS_VARIABLE, "CLAUDE_CODE_OAUTH_TOKEN"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
}

/// The model the sessions of the suite run on: the default model `ambient`
/// already holds, or [`PINNED_MODEL`].
pub fn suite_model(ambient: impl Fn(&str) -> Option<String>) -> String {
    ambient(MODEL_VARIABLE)
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| PINNED_MODEL.to_owned())
}

/// Pins the model of a session. The default model is pinned unless
/// `ambient` already holds one, and the session model follows it unless
/// `ambient` holds one too. The second variable matters on a developer
/// machine, where a `model` in the user settings outranks the default.
pub fn pin_model(
    mut builder: ClaudeAgentOptionsBuilder,
    ambient: impl Fn(&str) -> Option<String>,
) -> ClaudeAgentOptionsBuilder {
    let set = |name: &str| ambient(name).is_some_and(|value| !value.is_empty());
    if !set(MODEL_VARIABLE) {
        builder = builder.env_var(MODEL_VARIABLE, PINNED_MODEL);
    }
    if !set(SESSION_MODEL_VARIABLE) {
        builder = builder.env_var(SESSION_MODEL_VARIABLE, suite_model(&ambient));
    }
    builder
}

/// Options every session of the suite starts from. Fails the test when no
/// credentials are available.
pub fn options() -> ClaudeAgentOptionsBuilder {
    if let Some(message) = credentials_error(|name| std::env::var(name).ok()) {
        panic!("{message}");
    }
    pin_model(ClaudeAgentOptions::builder(), |name| {
        std::env::var(name).ok()
    })
}

/// A fresh working directory, with symbolic links resolved so the path is
/// the one the CLI reports.
pub fn workspace() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(directory.path()).unwrap();
    let path = match path.to_string_lossy().strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => path,
    };
    (directory, path)
}

/// Thread-safe list the callbacks of a test append to.
pub type Log<T> = Arc<Mutex<Vec<T>>>;

/// An empty [`Log`].
pub fn log<T>() -> Log<T> {
    Arc::new(Mutex::new(Vec::new()))
}

/// Copy of the entries of a [`Log`].
pub fn entries<T: Clone>(log: &Log<T>) -> Vec<T> {
    log.lock().unwrap().clone()
}

/// A hook callback that answers with what `respond` returns.
pub fn hook<F>(respond: F) -> HookCallback
where
    F: Fn(&HookInput) -> SyncHookOutput + Send + Sync + 'static,
{
    let respond = Arc::new(respond);
    HookCallback::new(
        move |input: HookInput, _tool_use_id: Option<String>, _context: HookContext| {
            let respond = Arc::clone(&respond);
            async move { Ok::<HookOutput, BoxError>(respond(&input).into()) }
        },
    )
}

/// Tool name and tool-use id of a tool hook input.
pub fn tool_call(input: &HookInput) -> Option<(String, String)> {
    match input {
        HookInput::PreToolUse(input) => Some((input.tool_name.clone(), input.tool_use_id.clone())),
        HookInput::PostToolUse(input) => Some((input.tool_name.clone(), input.tool_use_id.clone())),
        _ => None,
    }
}

/// Runs one query to its end and returns its messages. Panics on an error
/// item and when the run exceeds [`TURN_LIMIT`].
pub async fn run(prompt: impl Into<Prompt>, options: ClaudeAgentOptions) -> Vec<Message> {
    let (messages, error) = run_to_end(prompt, options).await;
    if let Some(error) = error {
        panic!("the query failed: {error}\nmessages: {messages:#?}");
    }
    messages
}

/// Runs one query to its end and returns its messages and its error, if
/// the stream ended with one.
pub async fn run_to_end(
    prompt: impl Into<Prompt>,
    options: ClaudeAgentOptions,
) -> (Vec<Message>, Option<claude_agent_rust_sdk::Error>) {
    let mut stream = query(prompt, options);
    let mut messages = Vec::new();
    let collected = tokio::time::timeout(TURN_LIMIT, async {
        while let Some(item) = stream.next().await {
            match item {
                Ok(message) => messages.push(message),
                Err(error) => return Some(error),
            }
        }
        None
    })
    .await;
    match collected {
        Ok(error) => (messages, error),
        Err(_) => panic!("the query did not end within {TURN_LIMIT:?}: {messages:#?}"),
    }
}

/// Connects a client. Panics when the handshake fails.
pub async fn connect(options: ClaudeAgentOptions) -> ClaudeSdkClient {
    let client = ClaudeSdkClient::new(options);
    tokio::time::timeout(TURN_LIMIT, client.connect())
        .await
        .expect("the handshake did not end in time")
        .expect("the handshake failed");
    client
}

/// Sends one prompt and returns the messages of its response.
pub async fn turn(client: &ClaudeSdkClient, prompt: impl Into<Prompt>) -> Vec<Message> {
    client.query(prompt).await.unwrap();
    response(client).await
}

/// Reads the messages up to and including the next result.
pub async fn response(client: &ClaudeSdkClient) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut stream = client.receive_response();
    let read = tokio::time::timeout(TURN_LIMIT, async {
        while let Some(message) = stream.next().await {
            messages.push(message.expect("the response yielded an error"));
        }
    })
    .await;
    assert!(
        read.is_ok(),
        "the response did not end within {TURN_LIMIT:?}: {messages:#?}"
    );
    messages
}

/// The results of a run, in order.
pub fn results(messages: &[Message]) -> Vec<&ResultMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Result(result) => Some(result),
            _ => None,
        })
        .collect()
}

/// The single result of a run.
pub fn result(messages: &[Message]) -> &ResultMessage {
    let results = results(messages);
    assert_eq!(results.len(), 1, "expected one result: {messages:#?}");
    results[0]
}

/// The raw init frame of a run.
pub fn init(messages: &[Message]) -> &Value {
    messages
        .iter()
        .find_map(|message| match message {
            Message::System(system) if system.subtype() == "init" => Some(system.data()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no init message: {messages:#?}"))
}

/// The strings of a JSON array field of the init frame.
pub fn init_list(messages: &[Message], field: &str) -> Vec<String> {
    init(messages)[field]
        .as_array()
        .unwrap_or_else(|| panic!("init.{field} is not a list: {}", init(messages)))
        .iter()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// Content blocks of the assistant and user messages of a run, with the
/// `parent_tool_use_id` of the message that holds them.
pub fn blocks(messages: &[Message]) -> Vec<(Option<&str>, &ContentBlock)> {
    let mut found = Vec::new();
    for message in messages {
        match message {
            Message::Assistant(assistant) => {
                for block in &assistant.content {
                    found.push((assistant.parent_tool_use_id.as_deref(), block));
                }
            }
            Message::User(user) => {
                if let UserContent::Blocks(content) = &user.content {
                    for block in content {
                        found.push((user.parent_tool_use_id.as_deref(), block));
                    }
                }
            }
            _ => {}
        }
    }
    found
}

/// Text of the top-level assistant messages of a run.
pub fn assistant_text(messages: &[Message]) -> String {
    blocks(messages)
        .into_iter()
        .filter_map(|(_, block)| block.as_text())
        .collect()
}

/// Directory under which the CLI keeps the transcripts of `cwd`.
pub fn project_directory(cwd: &Path) -> PathBuf {
    let config = match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(directory) if !directory.is_empty() => PathBuf::from(directory),
        _ => std::env::home_dir().unwrap().join(".claude"),
    };
    config
        .join("projects")
        .join(project_key_for_directory(Some(cwd)))
}

/// Every entry of the transcript of a session, read from disk.
pub fn transcript(session_id: &str, cwd: &Path) -> Vec<Value> {
    let file = project_directory(cwd).join(format!("{session_id}.jsonl"));
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("no transcript at {}: {error}", file.display()));
    let entries: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert!(
        !entries.is_empty(),
        "the transcript at {} holds no entry",
        file.display()
    );
    entries
}

/// Removes the transcripts the CLI wrote for `cwd`.
pub fn forget(cwd: &Path) {
    let _ = std::fs::remove_dir_all(project_directory(cwd));
}
