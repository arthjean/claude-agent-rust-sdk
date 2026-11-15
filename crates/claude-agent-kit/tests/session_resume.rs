#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use claude_agent_kit::{
    ClaudeAgentOptions, ClaudeAgentOptionsBuilder, ClaudeSdkClient, Error, InMemorySessionStore,
    Message, QueryStream, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, project_key_for_directory,
    query,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tempfile::TempDir;

use common::Fake;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn entry(value: Value) -> SessionStoreEntry {
    value.as_object().cloned().unwrap()
}

fn transcript(session: &str) -> Vec<SessionStoreEntry> {
    vec![
        entry(
            json!({"type": "user", "uuid": "u1", "parentUuid": null, "sessionId": session, "message": {"content": "hello"}}),
        ),
        entry(
            json!({"type": "assistant", "uuid": "a1", "parentUuid": "u1", "sessionId": session, "message": {"content": []}}),
        ),
    ]
}

fn scenario() -> Fake {
    Fake::new(json!({
        "report_env": ["CLAUDE_CONFIG_DIR"],
        "auto_reply": true,
        "steps": [
            {"expect": {"type": "user"}},
            {"emit": {"type": "result", "subtype": "success", "duration_ms": 1, "duration_api_ms": 1,
                      "is_error": false, "num_turns": 1, "session_id": "s", "result": "ok"}},
        ],
    }))
}

/// A caller config directory with credentials, user settings, and the
/// `.claude.json` file.
fn caller_config() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join(".credentials.json"),
        json!({"claudeAiOauth": {"accessToken": "access", "refreshToken": "refresh"}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("settings.json"),
        json!({"enabledPlugins": {"p": true}, "extraKnownMarketplaces": {"m": {}}, "apiKeyHelper": "helper", "env": {"CLAUDE_CONFIG_DIR": "/elsewhere", "KEEP": "1"}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("cowork_settings.json"),
        "[not an object]",
    )
    .unwrap();
    std::fs::write(directory.path().join(".claude.json"), r#"{"userID":"u"}"#).unwrap();
    directory
}

struct Setup {
    fake: Fake,
    cwd: TempDir,
    caller: TempDir,
    project_key: String,
}

impl Setup {
    fn new() -> Self {
        let cwd = tempfile::tempdir().unwrap();
        let project_key = project_key_for_directory(Some(cwd.path()));
        Self {
            fake: scenario(),
            cwd,
            caller: caller_config(),
            project_key,
        }
    }

    fn options(&self, store: impl SessionStore) -> ClaudeAgentOptionsBuilder {
        self.fake
            .options()
            .cwd(self.cwd.path())
            .env_var("CLAUDE_CONFIG_DIR", self.caller.path().to_string_lossy())
            .session_store(store)
    }

    fn key(&self, session: &str) -> SessionKey {
        SessionKey::new(self.project_key.as_str(), session)
    }
}

fn init_frame(message: &Message) -> Option<Value> {
    match message {
        Message::System(system) if system.subtype() == "init" => Some(system.data().clone()),
        _ => None,
    }
}

/// Reads the stream up to the init frame of the fake CLI and returns it.
async fn until_init(messages: &mut QueryStream) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), messages.next())
            .await
            .expect("the fake CLI answers")
            .expect("an init frame")
            .unwrap();
        if let Some(init) = init_frame(&message) {
            return init;
        }
    }
}

fn arguments(init: &Value) -> Vec<String> {
    serde_json::from_value(init["fake_args"].clone()).unwrap()
}

fn resume_argument(init: &Value) -> Option<String> {
    arguments(init)
        .iter()
        .find_map(|argument| argument.strip_prefix("--resume=").map(str::to_owned))
}

fn child_config_dir(init: &Value) -> PathBuf {
    PathBuf::from(init["fake_env"]["CLAUDE_CONFIG_DIR"].as_str().unwrap())
}

async fn wait_until_removed(path: &Path) {
    for _ in 0..400 {
        if !path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{} was not removed", path.display());
}

fn resume_dirs() -> BTreeSet<PathBuf> {
    std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("claude-resume-"))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_runs_the_child_on_a_materialized_config_dir_and_removes_it() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let store = InMemorySessionStore::new();
    let id = uuid::Uuid::new_v4().to_string();
    store
        .append(&setup.key(&id), &transcript(&id))
        .await
        .unwrap();
    let sub = setup.key(&id).with_subpath("subagents/agent-a1");
    store
        .append(&sub, &[entry(json!({"type": "user", "uuid": "s1", "message": {"content": "task"}})), entry(json!({"type": "agent_metadata", "agentType": "general", "toolUseId": "toolu_1"}))])
        .await
        .unwrap();
    store
        .append(
            &setup.key(&id).with_subpath("../escape"),
            &[entry(json!({"type": "user", "uuid": "x"}))],
        )
        .await
        .unwrap();

    let options = setup
        .options(store.clone())
        .resume(id.as_str())
        .build()
        .unwrap();
    let mut messages = query("continue", options);
    let init = until_init(&mut messages).await;

    let config = child_config_dir(&init);
    assert!(
        config
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("claude-resume-")
    );
    assert_ne!(config, setup.caller.path());
    assert_eq!(resume_argument(&init).as_deref(), Some(id.as_str()));
    assert!(!arguments(&init).contains(&"--continue".to_owned()));

    let project = config.join("projects").join(&setup.project_key);
    let written = project.join(format!("{id}.jsonl"));
    let lines: Vec<Value> = std::fs::read_to_string(&written)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let expected: Vec<Value> = transcript(&id).into_iter().map(Value::Object).collect();
    assert_eq!(lines, expected);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&written).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let credentials = std::fs::metadata(config.join(".credentials.json")).unwrap();
        assert_eq!(credentials.permissions().mode() & 0o777, 0o600);
    }

    let credentials: Value =
        serde_json::from_str(&std::fs::read_to_string(config.join(".credentials.json")).unwrap())
            .unwrap();
    assert_eq!(
        credentials,
        json!({"claudeAiOauth": {"accessToken": "access"}})
    );
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(config.join("settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings,
        json!({"apiKeyHelper": "helper", "env": {"KEEP": "1"}})
    );
    assert_eq!(
        std::fs::read_to_string(config.join("cowork_settings.json")).unwrap(),
        "[not an object]"
    );
    assert_eq!(
        std::fs::read_to_string(config.join(".claude.json")).unwrap(),
        r#"{"userID":"u"}"#
    );

    let subagent = project.join(&id).join("subagents").join("agent-a1.jsonl");
    assert_eq!(
        std::fs::read_to_string(&subagent).unwrap().lines().count(),
        1
    );
    let sidecar: Value = serde_json::from_str(
        &std::fs::read_to_string(
            project
                .join(&id)
                .join("subagents")
                .join("agent-a1.meta.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        sidecar,
        json!({"agentType": "general", "toolUseId": "toolu_1"})
    );
    assert!(!project.join("escape.jsonl").exists());

    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
    assert!(
        !config.exists(),
        "the temporary directory is removed after the child exits"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_picks_the_newest_session_that_is_not_a_sidechain() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let store = InMemorySessionStore::new();
    let main = uuid::Uuid::new_v4().to_string();
    let sidechain = uuid::Uuid::new_v4().to_string();
    store
        .append(&setup.key(&main), &transcript(&main))
        .await
        .unwrap();
    store
        .append(&setup.key("not-a-uuid"), &transcript("not-a-uuid"))
        .await
        .unwrap();
    let mut side = transcript(&sidechain);
    side[0].insert("isSidechain".to_owned(), Value::Bool(true));
    store.append(&setup.key(&sidechain), &side).await.unwrap();

    let options = setup
        .options(store)
        .continue_conversation(true)
        .build()
        .unwrap();
    let mut messages = query("again", options);
    let init = until_init(&mut messages).await;
    assert_eq!(resume_argument(&init).as_deref(), Some(main.as_str()));
    assert!(!arguments(&init).contains(&"--continue".to_owned()));
    let config = child_config_dir(&init);
    drop(messages);
    wait_until_removed(&config).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_with_an_empty_store_starts_a_fresh_session() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let before = resume_dirs();
    let options = setup
        .options(InMemorySessionStore::new())
        .continue_conversation(true)
        .build()
        .unwrap();
    let mut messages = query("fresh", options);
    let init = until_init(&mut messages).await;
    assert_eq!(child_config_dir(&init), setup.caller.path());
    assert!(arguments(&init).contains(&"--continue".to_owned()));
    let _: Vec<_> = messages.collect().await;
    assert_eq!(resume_dirs(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_query_removes_the_directory() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let store = InMemorySessionStore::new();
    let id = uuid::Uuid::new_v4().to_string();
    store
        .append(&setup.key(&id), &transcript(&id))
        .await
        .unwrap();
    let options = setup.options(store).resume(id.as_str()).build().unwrap();
    let mut messages = query("cancel me", options);
    let config = child_config_dir(&until_init(&mut messages).await);
    assert!(config.exists());
    drop(messages);
    wait_until_removed(&config).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_client_removes_the_directory_on_disconnect() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let store = InMemorySessionStore::new();
    let id = uuid::Uuid::new_v4().to_string();
    store
        .append(&setup.key(&id), &transcript(&id))
        .await
        .unwrap();
    let before = resume_dirs();
    let client = ClaudeSdkClient::new(setup.options(store).resume(id.as_str()).build().unwrap());
    client.connect().await.unwrap();
    let created: Vec<PathBuf> = resume_dirs().difference(&before).cloned().collect();
    assert_eq!(created.len(), 1);
    assert!(
        created[0]
            .join("projects")
            .join(&setup.project_key)
            .join(format!("{id}.jsonl"))
            .exists()
    );
    client.disconnect().await.unwrap();
    assert!(!created[0].exists());
}

struct SlowStore {
    inner: InMemorySessionStore,
    slow_load: bool,
    slow_subkeys: bool,
}

impl SessionStore for SlowStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.inner.append(key, entries).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        if self.slow_load {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        self.inner.load(key).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::none().with_list_subkeys()
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        if self.slow_subkeys {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        self.inner.list_subkeys(key).await
    }
}

async fn first_error(options: ClaudeAgentOptions) -> Error {
    let mut messages = query("x", options);
    match messages.next().await {
        Some(Err(error)) => error,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_store_fails_the_resume_and_leaves_no_directory() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let before = resume_dirs();
    for (slow_load, slow_subkeys) in [(true, false), (false, true)] {
        let inner = InMemorySessionStore::new();
        inner
            .append(&setup.key(&id), &transcript(&id))
            .await
            .unwrap();
        let store = SlowStore {
            inner,
            slow_load,
            slow_subkeys,
        };
        let options = setup
            .options(store)
            .resume(id.as_str())
            .load_timeout_ms(100)
            .build()
            .unwrap();
        let error = first_error(options).await;
        assert!(matches!(error, Error::SessionStore { .. }), "{error:?}");
        assert!(
            error.to_string().contains("timed out after 100 ms"),
            "{error}"
        );
        assert_eq!(
            resume_dirs(),
            before,
            "slow load {slow_load}, slow subkeys {slow_subkeys}"
        );
    }
}

struct MinimalStore(InMemorySessionStore);

impl SessionStore for MinimalStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<(), SessionStoreError> {
        self.0.append(key, entries).await
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>, SessionStoreError> {
        self.0.load(key).await
    }
}

#[tokio::test]
async fn continue_needs_a_store_that_lists_sessions() {
    let setup = Setup::new();
    let mut options = setup
        .options(MinimalStore(InMemorySessionStore::new()))
        .build()
        .unwrap();
    options.continue_conversation = true;
    assert!(matches!(first_error(options).await, Error::Config { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_that_is_not_a_uuid_reaches_the_cli_unchanged() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let before = resume_dirs();
    let options = setup
        .options(InMemorySessionStore::new())
        .resume("not-a-uuid")
        .build()
        .unwrap();
    let mut messages = query("x", options);
    let init = until_init(&mut messages).await;
    assert_eq!(resume_argument(&init).as_deref(), Some("not-a-uuid"));
    assert_eq!(child_config_dir(&init), setup.caller.path());
    let _: Vec<_> = messages.collect().await;
    assert_eq!(resume_dirs(), before);
}

/// Reads the credentials from the real macOS Keychain through `security`.
///
/// It needs a `Claude Code-credentials` item for `$USER` holding
/// `{"claudeAiOauth":{"accessToken":"ci-access","refreshToken":"ci-refresh"}}`,
/// which the macOS job of `.github/workflows/ci.yml` creates in a
/// throwaway keychain, and none of `CLAUDE_CONFIG_DIR`, `ANTHROPIC_API_KEY`,
/// or `CLAUDE_CODE_OAUTH_TOKEN` in the environment. It is ignored by
/// default so it never reads the Keychain of a developer machine.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "reads the macOS Keychain; set CLAUDE_AGENT_KIT_KEYCHAIN=1 and pass --ignored"]
async fn keychain_credentials_are_copied_without_the_refresh_token() {
    if std::env::var("CLAUDE_AGENT_KIT_KEYCHAIN").as_deref() != Ok("1") {
        eprintln!("CLAUDE_AGENT_KIT_KEYCHAIN is not 1: skipping");
        return;
    }
    for variable in [
        "CLAUDE_CONFIG_DIR",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        assert!(
            std::env::var(variable).is_err(),
            "{variable} must be unset, or the Keychain is not consulted"
        );
    }
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let store = InMemorySessionStore::new();
    let id = uuid::Uuid::new_v4().to_string();
    store
        .append(&setup.key(&id), &transcript(&id))
        .await
        .unwrap();
    let options = setup
        .fake
        .options()
        .cwd(setup.cwd.path())
        .session_store(store)
        .resume(id.as_str())
        .build()
        .unwrap();
    let mut messages = query("continue", options);
    let init = until_init(&mut messages).await;
    let config = child_config_dir(&init);
    let credentials: Value =
        serde_json::from_str(&std::fs::read_to_string(config.join(".credentials.json")).unwrap())
            .unwrap();
    assert_eq!(
        credentials,
        json!({"claudeAiOauth": {"accessToken": "ci-access"}})
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(config.join(".credentials.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let _: Vec<_> = messages.collect().await;
    wait_until_removed(&config).await;
}
