#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use claude_agent_kit::{
    ClaudeAgentOptions, ClaudeAgentOptionsBuilder, ClaudeSdkClient, Error, InMemorySessionStore,
    Message, QueryStream, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreCapabilities, SessionStoreEntry, SessionStoreError, SessionStoreListEntry,
    project_key_for_directory, query,
};
use futures_util::{Stream, StreamExt};
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
        Self::with(scenario(), caller_config())
    }

    fn with(fake: Fake, caller: TempDir) -> Self {
        let cwd = tempfile::tempdir().unwrap();
        let project_key = project_key_for_directory(Some(cwd.path()));
        Self {
            fake,
            cwd,
            caller,
            project_key,
        }
    }

    fn with_caller_files(files: &[(&str, &[u8])]) -> Self {
        let caller = tempfile::tempdir().unwrap();
        for (name, content) in files {
            std::fs::write(caller.path().join(name), content).unwrap();
        }
        Self::with(scenario(), caller)
    }

    fn without_store(&self) -> ClaudeAgentOptionsBuilder {
        self.fake
            .options()
            .cwd(self.cwd.path())
            .env_var("CLAUDE_CONFIG_DIR", self.caller.path().to_string_lossy())
    }

    fn options(&self, store: impl SessionStore) -> ClaudeAgentOptionsBuilder {
        self.without_store().session_store(store)
    }

    fn resume_dirs(&self) -> Vec<PathBuf> {
        resume_dirs()
            .into_iter()
            .filter(|directory| directory.join("projects").join(&self.project_key).is_dir())
            .collect()
    }

    async fn resumed(&self) -> (QueryStream, PathBuf, String) {
        let store = InMemorySessionStore::new();
        let id = uuid::Uuid::new_v4().to_string();
        store
            .append(&self.key(&id), &transcript(&id))
            .await
            .unwrap();
        let options = self.options(store).resume(id.as_str()).build().unwrap();
        let mut messages = query("continue", options);
        let config = child_config_dir(&until_init(&mut messages).await);
        assert_ne!(config, self.caller.path());
        (messages, config, id)
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
async fn until_init<S>(messages: &mut S) -> Value
where
    S: Stream<Item = Result<Message, Error>> + Unpin,
{
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

#[derive(Clone, Copy)]
enum Fault {
    Hang,
    Fail(&'static str),
}

async fn apply(fault: Option<Fault>) -> Result<(), SessionStoreError> {
    match fault {
        None => Ok(()),
        Some(Fault::Hang) => std::future::pending().await,
        Some(Fault::Fail(message)) => Err(message.into()),
    }
}

#[derive(Clone)]
struct ProbeStore {
    inner: InMemorySessionStore,
    project_key: String,
    load: Option<Fault>,
    list_sessions: Option<Fault>,
    list_subkeys: Option<Fault>,
    listed: Option<Vec<SessionStoreListEntry>>,
    reads: Arc<AtomicUsize>,
    directories: Arc<Mutex<Vec<PathBuf>>>,
    entered: Arc<tokio::sync::Notify>,
}

impl ProbeStore {
    fn new(setup: &Setup) -> Self {
        Self {
            inner: InMemorySessionStore::new(),
            project_key: setup.project_key.clone(),
            load: None,
            list_sessions: None,
            list_subkeys: None,
            listed: None,
            reads: Arc::default(),
            directories: Arc::default(),
            entered: Arc::default(),
        }
    }

    async fn holding(setup: &Setup, id: &str) -> Self {
        let store = Self::new(setup);
        store
            .inner
            .append(&setup.key(id), &transcript(id))
            .await
            .unwrap();
        store
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn directories(&self) -> Vec<PathBuf> {
        self.directories.lock().unwrap().clone()
    }

    fn enter(&self, fault: Option<Fault>) {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if fault.is_some() {
            self.entered.notify_one();
        }
    }
}

impl SessionStore for ProbeStore {
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
        self.enter(self.load);
        apply(self.load).await?;
        self.inner.load(key).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::none()
            .with_list_sessions()
            .with_list_subkeys()
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>, SessionStoreError> {
        self.enter(self.list_sessions);
        apply(self.list_sessions).await?;
        match &self.listed {
            Some(listed) => Ok(listed.clone()),
            None => self.inner.list_sessions(project_key).await,
        }
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        let existing: Vec<PathBuf> = resume_dirs()
            .into_iter()
            .filter(|directory| directory.join("projects").join(&self.project_key).is_dir())
            .collect();
        self.directories.lock().unwrap().extend(existing);
        self.enter(self.list_subkeys);
        apply(self.list_subkeys).await?;
        self.inner.list_subkeys(key).await
    }
}

struct RenamedFake {
    _directory: TempDir,
    path: PathBuf,
    name: String,
}

fn renamed_fake() -> RenamedFake {
    let directory = tempfile::tempdir().unwrap();
    let name = format!("fc-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let path = directory
        .path()
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(common::fake_claude(), &path).unwrap();
    RenamedFake {
        _directory: directory,
        path,
        name,
    }
}

fn missing_cli(setup: &Setup) -> PathBuf {
    setup.cwd.path().join("no-such-claude")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn transcript_file(setup: &Setup, config: &Path, id: &str) -> PathBuf {
    config
        .join("projects")
        .join(&setup.project_key)
        .join(format!("{id}.jsonl"))
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_without_a_store_leaves_the_config_dir_and_the_resume_value_alone() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let options = setup.without_store().resume(id.as_str()).build().unwrap();
    let mut messages = query("x", options);
    let init = until_init(&mut messages).await;
    assert_eq!(child_config_dir(&init), setup.caller.path());
    assert_eq!(resume_argument(&init).as_deref(), Some(id.as_str()));
    assert!(setup.resume_dirs().is_empty());
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_is_not_read_without_resume_or_continue() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let store = ProbeStore::holding(&setup, &id).await;
    let options = setup.options(store.clone()).build().unwrap();
    let mut messages = query("x", options);
    let init = until_init(&mut messages).await;
    assert_eq!(child_config_dir(&init), setup.caller.path());
    assert_eq!(resume_argument(&init), None);
    assert!(setup.resume_dirs().is_empty());
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
    assert_eq!(store.reads(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn resuming_a_session_the_store_holds_nothing_for_materializes_nothing() {
    let _serial = SERIAL.lock().await;
    for stored_empty in [false, true] {
        let setup = Setup::new();
        let store = InMemorySessionStore::new();
        let id = uuid::Uuid::new_v4().to_string();
        if stored_empty {
            store.append(&setup.key(&id), &[]).await.unwrap();
            assert_eq!(store.load(&setup.key(&id)).await.unwrap(), Some(Vec::new()));
        } else {
            assert_eq!(store.load(&setup.key(&id)).await.unwrap(), None);
        }
        let options = setup.options(store).resume(id.as_str()).build().unwrap();
        let mut messages = query("x", options);
        let init = until_init(&mut messages).await;
        assert_eq!(
            child_config_dir(&init),
            setup.caller.path(),
            "stored empty {stored_empty}"
        );
        assert_eq!(resume_argument(&init).as_deref(), Some(id.as_str()));
        assert!(
            setup.resume_dirs().is_empty(),
            "stored empty {stored_empty}"
        );
        let rest: Vec<_> = messages.collect().await;
        assert!(rest.iter().all(Result::is_ok), "{rest:?}");
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_fifo_in_place_of_the_settings_file_is_skipped_without_blocking() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::with_caller_files(&[]);
    let created = std::process::Command::new("mkfifo")
        .arg(setup.caller.path().join("settings.json"))
        .status()
        .unwrap();
    assert!(created.success());
    let (messages, config, id) = tokio::time::timeout(Duration::from_secs(30), setup.resumed())
        .await
        .expect("the FIFO is not opened");
    assert!(!config.join("settings.json").exists());
    assert!(transcript_file(&setup, &config, &id).is_file());
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

struct ProcessVariable {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl ProcessVariable {
    fn set(name: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(name);
        unsafe { std::env::set_var(name, value) }
        Self { name, previous }
    }
}

impl Drop for ProcessVariable {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => unsafe { std::env::set_var(self.name, previous) },
            None => unsafe { std::env::remove_var(self.name) },
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_are_seeded_from_the_config_dir_of_the_process_environment() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::with_caller_files(&[("settings.json", br#"{"apiKeyHelper":"/from/env"}"#)]);
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
    assert!(!options.env.contains_key("CLAUDE_CONFIG_DIR"));
    let variable = ProcessVariable::set("CLAUDE_CONFIG_DIR", setup.caller.path());
    let mut messages = query("continue", options);
    let config = child_config_dir(&until_init(&mut messages).await);
    drop(variable);
    assert_ne!(config, setup.caller.path());
    assert!(
        config
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("claude-resume-")
    );
    assert_eq!(
        read_json(&config.join("settings.json")),
        json!({"apiKeyHelper": "/from/env"})
    );
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_files_the_caller_does_not_have_are_not_written() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::with_caller_files(&[]);
    let (messages, config, id) = setup.resumed().await;
    for name in ["settings.json", "cowork_settings.json", ".claude.json"] {
        assert!(!config.join(name).exists(), "{name}");
    }
    assert!(transcript_file(&setup, &config, &id).is_file());
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_with_a_number_that_overflows_a_float_are_copied_byte_for_byte() {
    let _serial = SERIAL.lock().await;
    let raw: &[u8] = br#"{"enabledPlugins": {"p@m": true}, "threshold": 1e999}"#;
    let setup = Setup::with_caller_files(&[("settings.json", raw)]);
    let (messages, config, _id) = setup.resumed().await;
    assert_eq!(std::fs::read(config.join("settings.json")).unwrap(), raw);
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn directories_in_place_of_the_seed_files_are_skipped_and_the_transcript_is_written() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::with_caller_files(&[]);
    let seeds = ["settings.json", ".credentials.json", ".claude.json"];
    for name in seeds {
        std::fs::create_dir(setup.caller.path().join(name)).unwrap();
    }
    let (messages, config, id) = setup.resumed().await;
    for name in seeds {
        assert!(!config.join(name).exists(), "{name}");
    }
    assert!(transcript_file(&setup, &config, &id).is_file());
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

async fn continued_session(setup: &Setup, store: ProbeStore) -> Option<String> {
    let options = setup
        .options(store)
        .continue_conversation(true)
        .build()
        .unwrap();
    let mut messages = query("again", options);
    let init = until_init(&mut messages).await;
    let resumed = resume_argument(&init);
    let config = child_config_dir(&init);
    drop(messages);
    if resumed.is_some() {
        assert_ne!(config, setup.caller.path());
        wait_until_removed(&config).await;
    }
    resumed
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_resumes_the_session_with_the_highest_mtime() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let older = uuid::Uuid::new_v4().to_string();
    let newer = uuid::Uuid::new_v4().to_string();
    let listings = [
        vec![
            SessionStoreListEntry::new(older.as_str(), 1000),
            SessionStoreListEntry::new(newer.as_str(), 2000),
        ],
        vec![
            SessionStoreListEntry::new(newer.as_str(), 2000),
            SessionStoreListEntry::new(older.as_str(), 1000),
        ],
    ];
    for listed in listings {
        let mut store = ProbeStore::holding(&setup, &older).await;
        store
            .inner
            .append(&setup.key(&newer), &transcript(&newer))
            .await
            .unwrap();
        store.listed = Some(listed.clone());
        assert_eq!(
            continued_session(&setup, store).await.as_deref(),
            Some(newer.as_str()),
            "{listed:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_with_only_sidechain_sessions_starts_a_fresh_session() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let store = InMemorySessionStore::new();
    let sidechain = uuid::Uuid::new_v4().to_string();
    let mut side = transcript(&sidechain);
    side[0].insert("isSidechain".to_owned(), Value::Bool(true));
    store.append(&setup.key(&sidechain), &side).await.unwrap();
    let options = setup
        .options(store)
        .continue_conversation(true)
        .build()
        .unwrap();
    let mut messages = query("fresh", options);
    let init = until_init(&mut messages).await;
    assert_eq!(child_config_dir(&init), setup.caller.path());
    assert_eq!(resume_argument(&init), None);
    assert!(arguments(&init).contains(&"--continue".to_owned()));
    assert!(setup.resume_dirs().is_empty());
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_picks_the_first_listed_of_two_sessions_with_equal_mtimes_every_time() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let mut store = ProbeStore::holding(&setup, &first).await;
    store
        .inner
        .append(&setup.key(&second), &transcript(&second))
        .await
        .unwrap();
    store.listed = Some(vec![
        SessionStoreListEntry::new(first.as_str(), 5000),
        SessionStoreListEntry::new(second.as_str(), 5000),
    ]);
    let once = continued_session(&setup, store.clone()).await;
    let again = continued_session(&setup, store).await;
    assert_eq!(once.as_deref(), Some(first.as_str()));
    assert_eq!(again, once);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_without_list_subkeys_gets_the_main_transcript_only() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let inner = InMemorySessionStore::new();
    let id = uuid::Uuid::new_v4().to_string();
    inner
        .append(&setup.key(&id), &transcript(&id))
        .await
        .unwrap();
    inner
        .append(
            &setup.key(&id).with_subpath("subagents/agent-a1"),
            &[entry(json!({"type": "user", "uuid": "s1"}))],
        )
        .await
        .unwrap();
    let options = setup
        .options(MinimalStore(inner))
        .resume(id.as_str())
        .build()
        .unwrap();
    let mut messages = query("continue", options);
    let config = child_config_dir(&until_init(&mut messages).await);
    assert!(transcript_file(&setup, &config, &id).is_file());
    assert!(
        !config
            .join("projects")
            .join(&setup.project_key)
            .join(&id)
            .exists()
    );
    let rest: Vec<_> = messages.collect().await;
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_list_sessions_fails_continue_with_a_timeout_that_names_it() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let mut store = ProbeStore::holding(&setup, &id).await;
    store.list_sessions = Some(Fault::Hang);
    let options = setup
        .options(store)
        .continue_conversation(true)
        .load_timeout_ms(100)
        .build()
        .unwrap();
    let error = first_error(options).await;
    assert!(matches!(error, Error::SessionStore { .. }), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("list_sessions"), "{message}");
    assert!(message.contains("timed out after 100 ms"), "{message}");
    assert!(setup.resume_dirs().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_query_or_a_connect_while_subkeys_are_listed_removes_the_directory() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let mut store = ProbeStore::holding(&setup, &id).await;
    store.list_subkeys = Some(Fault::Hang);
    let options = setup
        .options(store.clone())
        .resume(id.as_str())
        .build()
        .unwrap();

    let mut messages = query("x", options.clone());
    tokio::select! {
        item = messages.next() => panic!("the query went past list_subkeys: {item:?}"),
        () = store.entered.notified() => {}
    }
    let created = store.directories();
    assert_eq!(created.len(), 1);
    assert!(created[0].join(".credentials.json").is_file());
    drop(messages);
    assert!(!created[0].exists());

    let client = ClaudeSdkClient::new(options);
    tokio::select! {
        outcome = client.connect() => panic!("connect went past list_subkeys: {outcome:?}"),
        () = store.entered.notified() => {}
    }
    let created = store.directories();
    assert_eq!(created.len(), 2);
    assert_ne!(created[0], created[1]);
    assert!(!created[1].exists());
    assert!(setup.resume_dirs().is_empty());
    client.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_store_load_error_fails_the_resume_with_the_message_of_the_store() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let mut store = ProbeStore::holding(&setup, &id).await;
    store.load = Some(Fault::Fail("network down"));
    let options = setup.options(store).resume(id.as_str()).build().unwrap();
    let error = first_error(options).await;
    assert!(matches!(error, Error::SessionStore { .. }), "{error:?}");
    assert!(error.to_string().contains("network down"), "{error}");
    assert!(setup.resume_dirs().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_subkeys_error_fails_the_resume_and_removes_the_directory() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let mut store = ProbeStore::holding(&setup, &id).await;
    store.list_subkeys = Some(Fault::Fail("boom"));
    let options = setup
        .options(store.clone())
        .resume(id.as_str())
        .build()
        .unwrap();
    let error = first_error(options).await;
    assert!(matches!(error, Error::SessionStore { .. }), "{error:?}");
    assert!(error.to_string().contains("boom"), "{error}");
    let created = store.directories();
    assert_eq!(created.len(), 1);
    assert!(!created[0].exists());
    assert!(setup.resume_dirs().is_empty());
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn a_client_over_a_custom_transport_never_reads_the_store_to_resume() {
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let store = ProbeStore::holding(&setup, &id).await;
    let options = setup
        .options(store.clone())
        .resume(id.as_str())
        .build()
        .unwrap();
    let transport = common::mock::cli();
    let client = ClaudeSdkClient::with_transport(options, transport.clone());
    client.connect().await.unwrap();
    assert_eq!(transport.connect_count(), 1);
    assert_eq!(store.reads(), 0);
    assert!(setup.resume_dirs().is_empty());
    client.disconnect().await.unwrap();
    assert_eq!(store.reads(), 0);
    assert!(store.directories().is_empty());
}

#[cfg(feature = "testing")]
#[tokio::test]
async fn a_query_over_a_custom_transport_never_reads_the_store_to_resume() {
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let store = ProbeStore::holding(&setup, &id).await;
    let options = setup
        .options(store.clone())
        .resume(id.as_str())
        .build()
        .unwrap();
    let transport = claude_agent_kit::testing::MockTransport::new();
    transport.fail_connect("spawn failed");
    let mut messages = claude_agent_kit::query_with_transport("hi", options, transport.clone());
    let error = match messages.next().await {
        Some(Err(error)) => error,
        other => panic!("expected an error, got {other:?}"),
    };
    assert!(error.to_string().contains("spawn failed"), "{error}");
    assert!(messages.next().await.is_none());
    assert_eq!(transport.connect_count(), 1);
    assert_eq!(store.reads(), 0);
    assert!(store.directories().is_empty());
    assert!(setup.resume_dirs().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_without_a_store_passes_resume_through_to_the_child() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let client = ClaudeSdkClient::new(setup.without_store().resume(id.as_str()).build().unwrap());
    client.connect().await.unwrap();
    let init = until_init(&mut client.receive_messages()).await;
    assert_eq!(child_config_dir(&init), setup.caller.path());
    assert_eq!(resume_argument(&init).as_deref(), Some(id.as_str()));
    assert!(setup.resume_dirs().is_empty());
    client.disconnect().await.unwrap();
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn removing_the_directory_is_retried_while_a_handle_holds_a_file_open() {
    use std::os::windows::fs::OpenOptionsExt;

    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let (messages, config, _id) = setup.resumed().await;
    let held = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(config.join("zz-held"))
        .unwrap();
    let draining = tokio::spawn(messages.collect::<Vec<_>>());
    let mut attempted = false;
    for _ in 0..4000 {
        if !config.join("projects").exists() {
            attempted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(attempted, "no removal was attempted");
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        config.join("zz-held").exists(),
        "the first removal fails on the file held open"
    );
    drop(held);
    let rest = tokio::time::timeout(Duration::from_secs(30), draining)
        .await
        .expect("the stream ends")
        .unwrap();
    assert!(rest.iter().all(Result::is_ok), "{rest:?}");
    assert!(!config.exists(), "a later attempt removes the directory");
}

#[cfg(windows)]
struct HoldingStore {
    inner: ProbeStore,
    held_for: Duration,
}

#[cfg(windows)]
impl SessionStore for HoldingStore {
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
        self.inner.load(key).await
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        self.inner.capabilities()
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>, SessionStoreError> {
        use std::os::windows::fs::OpenOptionsExt;

        let outcome = self.inner.list_subkeys(key).await;
        for directory in self.inner.directories() {
            let held = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .share_mode(0)
                .open(directory.join("zz-held"))
                .unwrap();
            let held_for = self.held_for;
            std::thread::spawn(move || {
                std::thread::sleep(held_for);
                drop(held);
            });
        }
        outcome
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_materialization_retries_the_removal_while_a_handle_holds_a_file_open() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let mut probe = ProbeStore::holding(&setup, &id).await;
    probe.list_subkeys = Some(Fault::Fail("boom"));
    let store = HoldingStore {
        inner: probe.clone(),
        held_for: Duration::from_millis(150),
    };
    let options = setup.options(store).resume(id.as_str()).build().unwrap();
    let error = first_error(options).await;
    assert!(error.to_string().contains("boom"), "{error}");
    let created = probe.directories();
    assert_eq!(created.len(), 1);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left: Vec<_> = std::fs::read_dir(&created[0])
        .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    assert!(
        !created[0].exists(),
        "{} is left behind with {left:?}",
        created[0].display()
    );
    let _ = std::fs::remove_dir_all(&created[0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_whose_cli_fails_to_spawn_removes_the_directory() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let store = ProbeStore::holding(&setup, &id).await;
    let options = setup
        .options(store.clone())
        .cli_path(missing_cli(&setup))
        .resume(id.as_str())
        .build()
        .unwrap();
    let client = ClaudeSdkClient::new(options);
    let error = client.connect().await.unwrap_err();
    assert!(matches!(error, Error::CliNotFound { .. }), "{error:?}");
    let created = store.directories();
    assert_eq!(created.len(), 1, "the session was materialized first");
    assert!(!created[0].exists());
    assert!(setup.resume_dirs().is_empty());
    assert!(matches!(
        client.query("x").await,
        Err(Error::CliConnection { .. })
    ));
    client.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_whose_initialize_fails_stops_the_child_and_removes_the_directory() {
    let _serial = SERIAL.lock().await;
    let fake = Fake::new(json!({
        "steps": [
            {"expect": {"type": "control_request", "request": {"subtype": "initialize"}}},
            {"control_error": "control timeout"},
        ],
    }));
    let setup = Setup::with(fake, caller_config());
    let cli = renamed_fake();
    let id = uuid::Uuid::new_v4().to_string();
    let store = ProbeStore::holding(&setup, &id).await;
    let options = setup
        .options(store.clone())
        .cli_path(cli.path.as_path())
        .resume(id.as_str())
        .build()
        .unwrap();
    let client = ClaudeSdkClient::new(options);
    let error = client.connect().await.unwrap_err();
    assert!(matches!(error, Error::Control { .. }), "{error:?}");
    assert!(error.to_string().contains("control timeout"), "{error}");
    assert_eq!(
        common::processes_matching(&cli.name),
        Vec::<String>::new(),
        "the child is stopped"
    );
    let created = store.directories();
    assert_eq!(created.len(), 1, "the session was materialized first");
    assert!(!created[0].exists());
    assert!(setup.resume_dirs().is_empty());
    assert!(matches!(
        client.query("x").await,
        Err(Error::CliConnection { .. })
    ));
    client.disconnect().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_connect_while_the_store_loads_spawns_nothing_and_leaves_no_directory() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let cli = renamed_fake();
    let id = uuid::Uuid::new_v4().to_string();
    let mut store = ProbeStore::holding(&setup, &id).await;
    store.load = Some(Fault::Hang);
    let options = setup
        .options(store.clone())
        .cli_path(cli.path.as_path())
        .resume(id.as_str())
        .load_timeout_ms(120_000)
        .build()
        .unwrap();
    let client = Arc::new(ClaudeSdkClient::new(options));
    let connecting = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.connect().await }
    });
    tokio::time::timeout(Duration::from_secs(2), store.entered.notified())
        .await
        .expect("connect reaches the store load");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!connecting.is_finished());
    assert_eq!(common::processes_matching(&cli.name), Vec::<String>::new());
    connecting.abort();
    assert!(connecting.await.unwrap_err().is_cancelled());
    assert_eq!(store.reads(), 1);
    assert_eq!(common::processes_matching(&cli.name), Vec::<String>::new());
    assert!(store.directories().is_empty());
    assert!(setup.resume_dirs().is_empty());
    tokio::time::timeout(Duration::from_secs(2), client.disconnect())
        .await
        .expect("disconnect does not hang")
        .unwrap();

    let loading = ProbeStore::holding(&setup, &id).await;
    let spawning = ClaudeSdkClient::new(
        setup
            .options(loading)
            .cli_path(cli.path.as_path())
            .resume(id.as_str())
            .build()
            .unwrap(),
    );
    spawning.connect().await.unwrap();
    assert_eq!(
        common::processes_matching(&cli.name).len(),
        1,
        "a spawned child is visible under this name"
    );
    spawning.disconnect().await.unwrap();
    assert_eq!(common::processes_matching(&cli.name), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_whose_cli_fails_to_spawn_yields_the_error_and_removes_the_directory() {
    let _serial = SERIAL.lock().await;
    let setup = Setup::new();
    let id = uuid::Uuid::new_v4().to_string();
    let store = ProbeStore::holding(&setup, &id).await;
    let options = setup
        .options(store.clone())
        .cli_path(missing_cli(&setup))
        .resume(id.as_str())
        .build()
        .unwrap();
    let mut messages = query("hi", options);
    let error = match messages.next().await {
        Some(Err(error)) => error,
        other => panic!("expected an error, got {other:?}"),
    };
    assert!(matches!(error, Error::CliNotFound { .. }), "{error:?}");
    assert!(messages.next().await.is_none());
    let created = store.directories();
    assert_eq!(created.len(), 1, "the session was materialized first");
    assert!(!created[0].exists());
    assert!(setup.resume_dirs().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_query_stops_the_child_before_the_directory_is_removed() {
    let _serial = SERIAL.lock().await;
    let fake = Fake::new(json!({
        "report_env": ["CLAUDE_CONFIG_DIR"],
        "auto_reply": true,
        "steps": [{"sleep_ms": 1500}],
    }));
    let setup = Setup::with(fake, caller_config());
    let store = InMemorySessionStore::new();
    let id = uuid::Uuid::new_v4().to_string();
    store
        .append(&setup.key(&id), &transcript(&id))
        .await
        .unwrap();
    let options = setup.options(store).resume(id.as_str()).build().unwrap();
    let mut messages = query("hi", options);
    let init = until_init(&mut messages).await;
    let config = child_config_dir(&init);
    let pid = u32::try_from(init["fake_pid"].as_u64().unwrap()).unwrap();
    assert!(config.is_dir());
    drop(messages);

    let mut existed_while_the_child_ran = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let removed = !config.exists();
        let running = common::process_exists(pid);
        assert!(
            !(removed && running),
            "the directory was removed while the child was still running"
        );
        if removed {
            break;
        }
        existed_while_the_child_ran |= running;
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} was not removed",
            config.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        existed_while_the_child_ran,
        "the directory was still there while the transport closed"
    );
    assert!(!common::process_exists(pid));
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
    let _serial = SERIAL.lock().await;
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
