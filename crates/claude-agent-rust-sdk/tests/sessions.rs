#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

common::transcripts::shared_config_dir_before_main!();

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use claude_agent_rust_sdk::{
    ForkSessionOptions, ImportSessionOptions, ListSessionsOptions, SdkSessionInfo, SessionMessage,
    SessionMessageType, SessionMessagesOptions, get_session_info, get_session_messages,
    get_subagent_messages, list_sessions, list_subagents, project_key_for_directory,
};
use serde_json::{Value, json};

use common::transcripts::{
    Project, assistant, config_dir, conversation, new_id, projects_dir, set_mtime, type_first,
    user, write_lines,
};

fn ids(sessions: &[SdkSessionInfo]) -> Vec<String> {
    sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect()
}

fn uuids(messages: &[SessionMessage]) -> Vec<&str> {
    messages
        .iter()
        .map(|message| message.uuid.as_str())
        .collect()
}

fn plain_session(project: &Project, prompt: &str) -> String {
    let id = new_id();
    project.write(
        &id,
        &[
            json!({"type": "user", "message": {"role": "user", "content": prompt}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": "Hi there!"}}),
            json!({"type": "summary"}),
        ],
    );
    id
}

#[test]
fn config_dir_comes_from_the_environment() {
    let project = Project::new();
    assert!(project.dir.starts_with(config_dir()));
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    let listed = list_sessions(
        ListSessionsOptions::new()
            .directory(project.path())
            .include_worktrees(false),
    );
    assert_eq!(ids(&listed), [id]);
}

#[test]
fn a_single_session_is_listed_with_its_metadata() {
    let project = Project::new();
    let id = new_id();
    let cwd = project.path().to_string_lossy().into_owned();
    let mut first = user("u1", None, &id, Value::from("What is 2+2?"));
    first["gitBranch"] = Value::from("main");
    first["cwd"] = Value::from(cwd.as_str());
    project.write(&id, &[first, assistant("a1", Some("u1"), &id, "4")]);

    let listed = list_sessions(
        ListSessionsOptions::new()
            .directory(project.path())
            .include_worktrees(false),
    );
    assert_eq!(ids(&listed), [id]);
    let session = &listed[0];
    assert_eq!(session.first_prompt.as_deref(), Some("What is 2+2?"));
    assert_eq!(session.summary, "What is 2+2?");
    assert_eq!(session.git_branch.as_deref(), Some("main"));
    assert_eq!(session.cwd.as_deref(), Some(cwd.as_str()));
    assert!(session.file_size.is_some_and(|size| size > 0));
    assert!(session.last_modified > 0);
    assert_eq!(session.custom_title, None);
}

#[test]
fn a_custom_title_wins_over_the_summary_and_the_first_prompt() {
    let project = Project::new();
    let id = new_id();
    let mut lines = conversation(&id, "original question", json!({}));
    lines.push(json!({"type": "summary", "summary": "auto summary"}));
    lines.push(json!({"type": "custom-title", "customTitle": "My Custom Title", "sessionId": id}));
    project.write(&id, &lines);
    let listed = list_sessions(
        ListSessionsOptions::new()
            .directory(project.path())
            .include_worktrees(false),
    );
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].summary, "My Custom Title");
    assert_eq!(listed[0].custom_title.as_deref(), Some("My Custom Title"));
    assert_eq!(listed[0].first_prompt.as_deref(), Some("original question"));
}

#[test]
fn metadata_follows_the_upstream_rules() {
    let project = Project::new();
    let id = new_id();
    let mut lines = vec![json!({"type": "permission-mode", "mode": "default"})];
    let mut first = user("u1", None, &id, Value::from("What is 2 + 2?"));
    first["cwd"] = Value::from("/work/here");
    first["gitBranch"] = Value::from("feature");
    lines.push(first);
    lines.push(assistant("a1", Some("u1"), &id, "4"));
    lines.push(json!({"type": "tool", "input": {"tag": "v1.0"}}));
    lines.push(json!({"type": "summary", "summary": "Arithmetic", "gitBranch": "main"}));
    lines.push(json!({"type":"tag","tag":"important","sessionId": id}));
    project.write(&id, &lines);

    let info = get_session_info(&id, Some(project.path())).unwrap();
    assert_eq!(info.summary, "Arithmetic");
    assert_eq!(info.first_prompt.as_deref(), Some("What is 2 + 2?"));
    assert_eq!(info.custom_title, None);
    assert_eq!(info.git_branch.as_deref(), Some("main"));
    assert_eq!(info.cwd.as_deref(), Some("/work/here"));
    assert_eq!(info.tag.as_deref(), Some("important"));
    assert_eq!(info.created_at, Some(1_768_473_000_000));
    assert!(info.file_size.unwrap() > 0);

    let titled = new_id();
    project.write(
        &titled,
        &conversation(
            &titled,
            "prompt",
            json!({"type": "custom-title", "customTitle": "My title", "lastPrompt": "later"}),
        ),
    );
    let info = get_session_info(&titled, Some(project.path())).unwrap();
    assert_eq!(info.summary, "My title");
    assert_eq!(info.custom_title.as_deref(), Some("My title"));

    let prompted = new_id();
    project.write(
        &prompted,
        &conversation(
            &prompted,
            "first",
            json!({"type": "last-prompt", "lastPrompt": "the latest"}),
        ),
    );
    assert_eq!(
        get_session_info(&prompted, Some(project.path()))
            .unwrap()
            .summary,
        "the latest"
    );
}

#[test]
fn cwd_falls_back_to_the_project_path() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    let info = get_session_info(&id, Some(project.path())).unwrap();
    let canonical = std::fs::canonicalize(project.path()).unwrap();
    let canonical = canonical.to_string_lossy();
    let expected = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    assert_eq!(info.cwd.as_deref(), Some(expected));
}

#[test]
fn first_prompt_is_truncated_and_skips_generated_prompts() {
    let project = Project::new();
    let id = new_id();
    let lines = vec![
        user(
            "u0",
            None,
            &id,
            Value::from("<local-command-stdout>ok</local-command-stdout>"),
        ),
        user("u1", Some("u0"), &id, Value::from("x".repeat(300))),
    ];
    project.write(&id, &lines);
    let info = get_session_info(&id, Some(project.path())).unwrap();
    assert_eq!(
        info.first_prompt.unwrap(),
        format!("{}\u{2026}", "x".repeat(200))
    );
}

#[test]
fn listing_sorts_newest_first_and_paginates() {
    let project = Project::new();
    let mut expected = Vec::new();
    for age in [30, 10, 20, 40] {
        let id = new_id();
        let path = project.write(&id, &conversation(&id, "prompt", json!({})));
        set_mtime(&path, age);
        expected.push((age, id));
    }
    expected.sort();
    let expected: Vec<String> = expected.into_iter().map(|(_, id)| id).collect();
    assert_eq!(
        ids(&list_sessions(
            ListSessionsOptions::new()
                .directory(project.path())
                .include_worktrees(false)
        )),
        expected
    );
    assert_eq!(
        ids(&list_sessions(
            ListSessionsOptions::new()
                .directory(project.path())
                .limit(2)
                .include_worktrees(false)
        )),
        expected[..2]
    );
    assert_eq!(
        ids(&list_sessions(
            ListSessionsOptions::new()
                .directory(project.path())
                .limit(2)
                .offset(1)
                .include_worktrees(false)
        )),
        expected[1..3]
    );
    assert_eq!(
        ids(&list_sessions(
            ListSessionsOptions::new()
                .directory(project.path())
                .limit(0)
                .offset(3)
                .include_worktrees(false)
        )),
        expected[3..]
    );
}

#[test]
fn sidechain_empty_metadata_only_and_misnamed_files_are_skipped() {
    let project = Project::new();
    let kept = new_id();
    project.write(&kept, &conversation(&kept, "real", json!({})));
    let sidechain = new_id();
    let mut lines = conversation(&sidechain, "side", json!({}));
    lines[0]["isSidechain"] = Value::Bool(true);
    project.write(&sidechain, &lines);
    let metadata_only = new_id();
    project.write(
        &metadata_only,
        &[json!({"type": "permission-mode", "mode": "plan"})],
    );
    let meta_prompt_only = new_id();
    let mut lines = conversation(&meta_prompt_only, "ignored meta", json!({}));
    lines[0]["isMeta"] = Value::Bool(true);
    project.write(&meta_prompt_only, &lines);
    let empty = new_id();
    std::fs::write(project.transcript(&empty), "").unwrap();
    std::fs::write(project.dir.join("not-a-uuid.jsonl"), "{}\n").unwrap();
    std::fs::write(project.dir.join(format!("{}.txt", new_id())), "{}\n").unwrap();

    assert_eq!(
        ids(&list_sessions(
            ListSessionsOptions::new().directory(project.path())
        )),
        [kept]
    );
    assert!(get_session_info(&sidechain, Some(project.path())).is_none());
}

#[test]
fn listing_reads_only_the_head_and_the_tail() {
    let project = Project::new();
    let id = new_id();
    let mut lines = conversation(&id, "start", json!({}));
    let filler = "f".repeat(1000);
    for index in 0..200 {
        lines.push(json!({"type": "progress", "uuid": format!("p{index}"), "summary": "hidden middle", "data": filler}));
    }
    lines.push(json!({"type": "summary", "summary": "tail summary"}));
    project.write(&id, &lines);
    let info = get_session_info(&id, Some(project.path())).unwrap();
    assert_eq!(info.summary, "tail summary");
    assert!(info.file_size.unwrap() > 2 * 65_536);
}

#[test]
fn get_session_info_rejects_ids_that_are_not_uuids() {
    let project = Project::new();
    project.write("not-a-uuid", &conversation("x", "hello", json!({})));
    assert!(get_session_info("not-a-uuid", Some(project.path())).is_none());
    assert!(get_session_info(&new_id(), Some(project.path())).is_none());
}

#[test]
fn all_projects_are_listed_and_searched_without_a_directory() {
    let first = Project::new();
    let second = Project::new();
    let shared = new_id();
    let older = first.write(&shared, &conversation(&shared, "older", json!({})));
    set_mtime(&older, 100);
    second.write(&shared, &conversation(&shared, "newer", json!({})));
    let alone = new_id();
    second.write(&alone, &conversation(&alone, "alone", json!({})));

    let all = list_sessions(ListSessionsOptions::new());
    let matching: Vec<_> = all
        .iter()
        .filter(|session| session.session_id == shared)
        .collect();
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].summary, "newer");
    assert!(all.iter().any(|session| session.session_id == alone));
    assert_eq!(get_session_info(&alone, None).unwrap().summary, "alone");
    let older_alone = new_id();
    let path = first.write(
        &older_alone,
        &conversation(&older_alone, "from one", json!({})),
    );
    set_mtime(&path, 50);
    let ours: Vec<String> = list_sessions(ListSessionsOptions::new())
        .into_iter()
        .map(|session| session.session_id)
        .filter(|id| [&alone, &older_alone].contains(&id))
        .collect();
    assert_eq!(ours, [alone, older_alone], "newest first across projects");
}

#[test]
fn missing_project_directory_lists_nothing() {
    let cwd = tempfile::tempdir().unwrap();
    assert!(list_sessions(ListSessionsOptions::new().directory(cwd.path())).is_empty());
    assert!(
        get_session_messages(
            &new_id(),
            SessionMessagesOptions::new().directory(cwd.path())
        )
        .is_empty()
    );
}

#[test]
fn long_paths_fall_back_to_the_prefix_lookup() {
    let base = tempfile::tempdir().unwrap();
    let mut long = base.path().to_path_buf();
    for _ in 0..12 {
        long.push("a-very-long-directory-name");
    }
    std::fs::create_dir_all(&long).unwrap();
    let key = project_key_for_directory(Some(&long));
    assert!(key.len() > 200);
    let other_hash = projects_dir().join(format!("{}-differenthash", &key[..200]));
    let id = new_id();
    write_lines(
        &other_hash.join(format!("{id}.jsonl")),
        &conversation(&id, "long", json!({})),
    );
    assert_eq!(
        ids(&list_sessions(
            ListSessionsOptions::new()
                .directory(&long)
                .include_worktrees(false)
        )),
        std::slice::from_ref(&id)
    );
    assert_eq!(
        get_session_messages(&id, SessionMessagesOptions::new().directory(&long)).len(),
        2
    );
}

fn git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(directory)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn worktrees_are_listed_together_and_deduplicated() {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("main");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "--quiet"]);
    git(&main, &["commit", "--quiet", "--allow-empty", "-m", "init"]);
    git(&main, &["worktree", "add", "--quiet", "../linked"]);
    let linked = base.path().join("linked");

    let main_dir = projects_dir().join(project_key_for_directory(Some(&main)));
    let linked_dir = projects_dir().join(project_key_for_directory(Some(&linked)));
    let in_main = new_id();
    write_lines(
        &main_dir.join(format!("{in_main}.jsonl")),
        &conversation(&in_main, "main", json!({})),
    );
    let in_linked = new_id();
    write_lines(
        &linked_dir.join(format!("{in_linked}.jsonl")),
        &conversation(&in_linked, "linked", json!({})),
    );
    let both = new_id();
    let stale = main_dir.join(format!("{both}.jsonl"));
    write_lines(&stale, &conversation(&both, "stale copy", json!({})));
    set_mtime(&stale, 100);
    write_lines(
        &linked_dir.join(format!("{both}.jsonl")),
        &conversation(&both, "fresh copy", json!({})),
    );

    let listed = list_sessions(ListSessionsOptions::new().directory(&main));
    let mut found = ids(&listed);
    found.sort();
    let mut expected = vec![in_main.clone(), in_linked.clone(), both.clone()];
    expected.sort();
    assert_eq!(found, expected);
    let deduplicated = listed
        .iter()
        .find(|session| session.session_id == both)
        .unwrap();
    assert_eq!(deduplicated.summary, "fresh copy");

    let mut own_only = ids(&list_sessions(
        ListSessionsOptions::new()
            .directory(&main)
            .include_worktrees(false),
    ));
    own_only.sort();
    let mut expected = vec![in_main, both];
    expected.sort();
    assert_eq!(own_only, expected);
    assert_eq!(
        get_session_info(&in_linked, Some(&main)).unwrap().summary,
        "linked"
    );
}

#[test]
fn a_directory_outside_git_lists_only_its_own_project() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "no git", json!({})));
    assert_eq!(
        ids(&list_sessions(
            ListSessionsOptions::new().directory(project.path())
        )),
        [id]
    );
}

#[test]
fn messages_follow_the_chain_and_skip_corrupt_and_hidden_entries() {
    let project = Project::new();
    let id = new_id();
    let mut meta = user("m1", Some("a1"), &id, Value::from("meta"));
    meta["isMeta"] = Value::Bool(true);
    let mut compact = user("c1", Some("m1"), &id, Value::from("summary of before"));
    compact["isCompactSummary"] = Value::Bool(true);
    let lines = vec![
        user("u1", None, &id, Value::from("hello")),
        assistant("a1", Some("u1"), &id, "hi"),
        json!({"type": "custom-title", "customTitle": "t"}),
        meta,
        json!({"type": "progress", "uuid": "p1", "parentUuid": "m1"}),
        compact,
        json!({"type": "user", "parentUuid": "c1", "message": {"content": "no uuid"}}),
        assistant("a2", Some("c1"), &id, "after"),
    ];
    let path = project.write(&id, &lines);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.insert_str(0, "{not json\n");
    std::fs::write(&path, text).unwrap();

    let messages =
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
    let uuids: Vec<&str> = messages
        .iter()
        .map(|message| message.uuid.as_str())
        .collect();
    assert_eq!(uuids, ["u1", "a1", "c1", "a2"]);
    assert_eq!(messages[0].r#type, SessionMessageType::User);
    assert_eq!(messages[1].r#type, SessionMessageType::Assistant);
    assert_eq!(messages[0].session_id, id);
    assert_eq!(messages[0].message["content"], "hello");
    assert!(
        messages
            .iter()
            .all(|message| message.parent_tool_use_id.is_none())
    );

    let page: Vec<String> = get_session_messages(
        &id,
        SessionMessagesOptions::new()
            .directory(project.path())
            .limit(2)
            .offset(1),
    )
    .into_iter()
    .map(|message| message.uuid)
    .collect();
    assert_eq!(page, ["a1", "c1"]);
    assert_eq!(
        get_session_messages(&id, SessionMessagesOptions::new()).len(),
        4
    );
}

#[test]
fn a_parent_cycle_returns_without_looping() {
    let project = Project::new();
    let id = new_id();
    project.write(
        &id,
        &[
            user("x", Some("y"), &id, Value::from("one")),
            assistant("y", Some("x"), &id, "two"),
            user("z", Some("y"), &id, Value::from("three")),
        ],
    );
    let messages =
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
    assert_eq!(messages.len(), 3);
}

#[test]
fn two_entries_that_are_each_other_s_parent_give_no_messages() {
    let project = Project::new();
    let id = new_id();
    project.write(
        &id,
        &[
            user("u1", Some("a1"), &id, Value::from("hi")),
            assistant("a1", Some("u1"), &id, "hello"),
        ],
    );
    assert!(
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()))
            .is_empty()
    );
}

#[test]
fn missing_or_invalid_sessions_give_empty_lists() {
    let project = Project::new();
    assert!(
        get_session_messages(
            &new_id(),
            SessionMessagesOptions::new().directory(project.path())
        )
        .is_empty()
    );
    assert!(
        get_session_messages(
            "bad",
            SessionMessagesOptions::new().directory(project.path())
        )
        .is_empty()
    );
    let empty = new_id();
    std::fs::write(project.transcript(&empty), "").unwrap();
    assert!(
        get_session_messages(
            &empty,
            SessionMessagesOptions::new().directory(project.path())
        )
        .is_empty()
    );
}

#[test]
fn a_subagent_chain_is_read_root_first_with_every_field() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "spawn", json!({})));
    let subagents = project.dir.join(&id).join("subagents");
    write_lines(
        &subagents.join("agent-abc.jsonl"),
        &[
            user("u1", None, &id, Value::from("task")),
            assistant("a1", Some("u1"), &id, "working"),
            user("u2", Some("a1"), &id, Value::from("continue")),
            assistant("a2", Some("u2"), &id, "done"),
        ],
    );
    let messages = get_subagent_messages(
        &id,
        "abc",
        SessionMessagesOptions::new().directory(project.path()),
    );
    assert_eq!(uuids(&messages), ["u1", "a1", "u2", "a2"]);
    assert_eq!(messages[0].r#type, SessionMessageType::User);
    assert_eq!(messages[0].session_id, id);
    assert_eq!(
        messages[0].message,
        json!({"role": "user", "content": "task"})
    );
    assert_eq!(messages[0].parent_tool_use_id, None);
    assert_eq!(messages[3].r#type, SessionMessageType::Assistant);
}

#[test]
fn the_sidecar_parent_ids_are_set_on_every_subagent_message() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "spawn", json!({})));
    let subagents = project.dir.join(&id).join("subagents");
    write_lines(
        &subagents.join("agent-abc.jsonl"),
        &[
            user("u1", None, &id, Value::from("task")),
            assistant("a1", Some("u1"), &id, "done"),
        ],
    );
    std::fs::write(
        subagents.join("agent-abc.meta.json"),
        json!({
            "agentType": "general-purpose",
            "toolUseId": "toolu_01ABC",
            "parentAgentId": "a-parent",
            "spawnDepth": 2,
        })
        .to_string(),
    )
    .unwrap();
    let messages = get_subagent_messages(
        &id,
        "abc",
        SessionMessagesOptions::new().directory(project.path()),
    );
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|message| {
        message.parent_tool_use_id.as_deref() == Some("toolu_01ABC")
            && message.parent_agent_id.as_deref() == Some("a-parent")
    }));
}

#[test]
fn subagents_are_listed_and_read_with_their_parent_ids() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "spawn agents", json!({})));
    let subagents = project.dir.join(&id).join("subagents");
    let agent_lines = |agent: &str| {
        vec![
            user(&format!("{agent}-u"), None, &id, Value::from("task")),
            assistant(
                &format!("{agent}-a"),
                Some(&format!("{agent}-u")),
                &id,
                "done",
            ),
        ]
    };
    write_lines(&subagents.join("agent-alpha.jsonl"), &agent_lines("alpha"));
    std::fs::write(
        subagents.join("agent-alpha.meta.json"),
        json!({"toolUseId": "toolu_1", "agentType": "general"}).to_string(),
    )
    .unwrap();
    let nested = subagents.join("workflows").join("run-1");
    write_lines(&nested.join("agent-beta.jsonl"), &agent_lines("beta"));
    std::fs::write(
        nested.join("agent-beta.meta.json"),
        json!({"toolUseId": "toolu_2", "parentAgentId": "alpha"}).to_string(),
    )
    .unwrap();
    write_lines(&subagents.join("agent-gamma.jsonl"), &agent_lines("gamma"));
    std::fs::write(subagents.join("agent-gamma.meta.json"), "[1, 2]").unwrap();
    std::fs::write(subagents.join("notes.txt"), "ignored").unwrap();
    std::fs::write(subagents.join("other.jsonl"), "{}\n").unwrap();
    std::fs::write(subagents.join("agent-noext"), "{}").unwrap();

    assert_eq!(
        list_subagents(&id, Some(project.path())),
        ["alpha", "gamma", "beta"]
    );
    assert_eq!(list_subagents(&id, None), ["alpha", "gamma", "beta"]);

    let alpha = get_subagent_messages(
        &id,
        "alpha",
        SessionMessagesOptions::new().directory(project.path()),
    );
    assert_eq!(alpha.len(), 2);
    assert_eq!(alpha[0].parent_tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(alpha[0].parent_agent_id, None);
    let beta = get_subagent_messages(
        &id,
        "beta",
        SessionMessagesOptions::new()
            .directory(project.path())
            .limit(1)
            .offset(1),
    );
    assert_eq!(beta.len(), 1);
    assert_eq!(beta[0].uuid, "beta-a");
    assert_eq!(beta[0].parent_agent_id.as_deref(), Some("alpha"));
    let gamma = get_subagent_messages(
        &id,
        "gamma",
        SessionMessagesOptions::new().directory(project.path()),
    );
    assert_eq!(gamma[0].parent_tool_use_id, None);

    assert!(
        get_subagent_messages(
            &id,
            "missing",
            SessionMessagesOptions::new().directory(project.path())
        )
        .is_empty()
    );
    assert!(
        get_subagent_messages(
            &id,
            "",
            SessionMessagesOptions::new().directory(project.path())
        )
        .is_empty()
    );
    assert!(list_subagents(&new_id(), Some(project.path())).is_empty());
    assert!(list_subagents("bad", Some(project.path())).is_empty());
}

const CONFIG_DIR_HOST_VARIABLE: &str = "CLAUDE_AGENT_RUST_SDK_TEST_CONFIG_DIR_HOST_SESSION";
const CONFIG_DIR_REPORT: &str = "config-dir-report=";

#[test]
fn host_process_for_the_config_dir_tests() {
    let Ok(session_id) = std::env::var(CONFIG_DIR_HOST_VARIABLE) else {
        return;
    };
    let report = json!({
        "listed": ids(&list_sessions(ListSessionsOptions::new())),
        "info": get_session_info(&session_id, None).is_some(),
        "messages": get_session_messages(&session_id, SessionMessagesOptions::new()).len(),
    });
    println!("{CONFIG_DIR_REPORT}{report}");
}

fn read_in_a_process_with_config_dir(config: &Path, session_id: &str) -> Value {
    let mut host = Command::new(std::env::current_exe().unwrap());
    host.args([
        "host_process_for_the_config_dir_tests",
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ]);
    host.env("CLAUDE_CONFIG_DIR", config);
    host.env(common::transcripts::KEEP_CONFIG_DIR_VARIABLE, "1");
    host.env(CONFIG_DIR_HOST_VARIABLE, session_id);
    let output = host.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let report = stdout
        .lines()
        .find_map(|line| line.split_once(CONFIG_DIR_REPORT).map(|(_, report)| report))
        .unwrap_or_else(|| panic!("the host did not report: {stdout}"));
    serde_json::from_str(report).unwrap()
}

#[test]
fn a_config_dir_that_does_not_exist_gives_empty_results() {
    let base = tempfile::tempdir().unwrap();
    let report = read_in_a_process_with_config_dir(&base.path().join("nonexistent"), &new_id());
    assert_eq!(report, json!({"listed": [], "info": false, "messages": 0}));
}

#[test]
fn a_projects_dir_without_projects_lists_nothing() {
    let base = tempfile::tempdir().unwrap();
    let projects = base.path().join("projects");
    std::fs::create_dir(&projects).unwrap();
    let id = new_id();
    assert_eq!(
        read_in_a_process_with_config_dir(base.path(), &id),
        json!({"listed": [], "info": false, "messages": 0})
    );

    write_lines(
        &projects.join("some-project").join(format!("{id}.jsonl")),
        &conversation(&id, "hello", json!({})),
    );
    assert_eq!(
        read_in_a_process_with_config_dir(base.path(), &id),
        json!({"listed": [id], "info": true, "messages": 2})
    );
}

#[test]
fn a_new_session_info_keeps_its_required_fields_and_leaves_the_rest_unset() {
    let info = SdkSessionInfo::new("abc", "test", 1000);
    assert_eq!(info.session_id, "abc");
    assert_eq!(info.summary, "test");
    assert_eq!(info.last_modified, 1000);
    assert_eq!(info.file_size, None);
    assert_eq!(info.custom_title, None);
    assert_eq!(info.first_prompt, None);
    assert_eq!(info.git_branch, None);
    assert_eq!(info.cwd, None);
    assert_eq!(info.tag, None);
    assert_eq!(info.created_at, None);
}

#[test]
fn a_progress_entry_inside_the_chain_is_walked_through_and_left_out() {
    let project = Project::new();
    let id = new_id();
    project.write(
        &id,
        &[
            user("u1", None, &id, Value::from("hello")),
            json!({"type": "progress", "uuid": "p1", "parentUuid": "u1", "sessionId": id}),
            assistant("a1", Some("p1"), &id, "hi"),
        ],
    );
    let messages =
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
    assert_eq!(uuids(&messages), ["u1", "a1"]);
}

#[test]
fn the_chain_ends_at_the_leaf_written_last_in_the_file() {
    let project = Project::new();
    let id = new_id();
    project.write(
        &id,
        &[
            user("root", None, &id, Value::from("root")),
            assistant("old-leaf", Some("root"), &id, "old"),
            assistant("new-leaf", Some("root"), &id, "new"),
        ],
    );
    let messages =
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
    assert_eq!(uuids(&messages), ["root", "new-leaf"]);
}

#[test]
fn a_transcript_ending_on_a_progress_entry_is_walked_back_to_its_last_message() {
    let project = Project::new();
    let id = new_id();
    project.write(
        &id,
        &[
            user("u1", None, &id, Value::from("hi")),
            assistant("a1", Some("u1"), &id, "hello"),
            json!({"type": "progress", "uuid": "p1", "parentUuid": "a1", "sessionId": id}),
        ],
    );
    let messages =
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
    assert_eq!(uuids(&messages), ["u1", "a1"]);
}

#[test]
fn a_single_user_entry_is_returned_alone() {
    let project = Project::new();
    let id = new_id();
    project.write(
        &id,
        &[json!({"type": "user", "uuid": "a", "parentUuid": null})],
    );
    let messages =
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
    assert_eq!(uuids(&messages), ["a"]);
    assert_eq!(messages[0].r#type, SessionMessageType::User);
}

fn parallel_tool_turn(
    session_id: &str,
    parent: &str,
    tools: usize,
    result_order: &[usize],
) -> Vec<Value> {
    let mut lines = Vec::new();
    for index in 0..tools {
        let previous = if index == 0 {
            parent.to_owned()
        } else {
            format!("call-{}", index - 1)
        };
        lines.push(json!({
            "type": "assistant", "uuid": format!("call-{index}"), "parentUuid": previous,
            "sessionId": session_id, "timestamp": format!("2026-01-01T00:00:01.{index:03}Z"),
            "message": {"id": "msg_parallel", "role": "assistant", "content": [
                {"type": "tool_use", "id": format!("toolu_{index}"), "name": "Read", "input": {}}
            ]},
        }));
    }
    for (position, index) in result_order.iter().enumerate() {
        lines.push(json!({
            "type": "user", "uuid": format!("result-{index}"), "parentUuid": format!("call-{index}"),
            "sessionId": session_id, "sourceToolAssistantUUID": format!("call-{index}"),
            "timestamp": format!("2026-01-01T00:00:02.{position:03}Z"),
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": format!("toolu_{index}"), "content": "ok"}
            ]},
        }));
    }
    lines
}

/// The ids of the `kind` blocks of the messages, read from `field`.
fn block_ids(messages: &[SessionMessage], kind: &str, field: &str) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| message.message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == kind)
        .map(|block| block[field].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn parallel_tool_calls_keep_every_call_and_result_whatever_the_result_order() {
    for result_order in [[0, 1, 2], [2, 1, 0], [1, 2, 0], [0, 2, 1]] {
        let project = Project::new();
        let id = new_id();
        let mut lines = vec![user("prompt", None, &id, Value::from("read them"))];
        lines.extend(parallel_tool_turn(&id, "prompt", 3, &result_order));
        let last_result = format!("result-{}", result_order[2]);
        lines.push(assistant("answer", Some(&last_result), &id, "done"));
        project.write(&id, &lines);

        let messages =
            get_session_messages(&id, SessionMessagesOptions::new().directory(project.path()));
        let mut result_ids = block_ids(&messages, "tool_result", "tool_use_id");
        result_ids.sort();
        let context = format!("result order {result_order:?}");
        assert_eq!(
            block_ids(&messages, "tool_use", "id"),
            ["toolu_0", "toolu_1", "toolu_2"],
            "{context}"
        );
        assert_eq!(result_ids, ["toolu_0", "toolu_1", "toolu_2"], "{context}");
        let distinct: std::collections::HashSet<&str> = uuids(&messages).into_iter().collect();
        assert_eq!(messages.len(), 8, "{context}");
        assert_eq!(distinct.len(), 8, "{context}");
        assert_eq!(messages[0].uuid, "prompt", "{context}");
        assert_eq!(
            messages[1..7]
                .iter()
                .map(|message| message.r#type)
                .collect::<Vec<_>>(),
            [
                SessionMessageType::Assistant,
                SessionMessageType::Assistant,
                SessionMessageType::Assistant,
                SessionMessageType::User,
                SessionMessageType::User,
                SessionMessageType::User,
            ],
            "{context}"
        );
        assert_eq!(messages[7].uuid, "answer", "{context}");
    }
}

#[test]
fn parallel_tool_calls_that_end_the_transcript_keep_every_result() {
    let project = Project::new();
    let id = new_id();
    let mut lines = vec![user("prompt", None, &id, Value::from("read them"))];
    lines.extend(parallel_tool_turn(&id, "prompt", 2, &[1, 0]));
    project.write(&id, &lines);
    assert_eq!(
        uuids(&get_session_messages(
            &id,
            SessionMessagesOptions::new().directory(project.path())
        )),
        ["prompt", "call-0", "call-1", "result-1", "result-0"]
    );
}

#[test]
fn pagination_counts_the_recovered_parallel_tool_calls_and_results() {
    let project = Project::new();
    let id = new_id();
    let mut lines = vec![user("prompt", None, &id, Value::from("read them"))];
    lines.extend(parallel_tool_turn(&id, "prompt", 3, &[0, 1, 2]));
    lines.push(assistant("answer", Some("result-2"), &id, "k"));
    project.write(&id, &lines);

    let page = get_session_messages(
        &id,
        SessionMessagesOptions::new()
            .directory(project.path())
            .limit(3)
            .offset(4),
    );
    assert_eq!(uuids(&page), ["result-0", "result-1", "result-2"]);
    assert_eq!(
        get_session_messages(&id, SessionMessagesOptions::new().directory(project.path())).len(),
        8
    );
}

#[test]
fn a_transcript_without_a_tag_entry_or_timestamps_has_no_tag_and_no_creation_time() {
    let project = Project::new();
    let id = plain_session(&project, "hello");
    let listed = list_sessions(
        ListSessionsOptions::new()
            .directory(project.path())
            .include_worktrees(false),
    );
    assert_eq!(ids(&listed), [id]);
    assert_eq!(listed[0].tag, None);
    assert_eq!(listed[0].created_at, None);
}

#[test]
fn a_tag_key_inside_a_tool_use_input_is_never_read_as_the_tag() {
    let project = Project::new();
    let tool_use = |tag: &str| {
        json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "mcp__docker__build", "input": {"tag": tag, "context": "."}}
        ]}})
    };
    let tagged = new_id();
    project.write(
        &tagged,
        &[
            json!({"type": "user", "message": {"content": "tag this v1.0"}}),
            json!({"type": "tag", "tag": "real-tag", "sessionId": tagged}),
            tool_use("myapp:v2"),
        ],
    );
    let untagged = new_id();
    project.write(
        &untagged,
        &[
            json!({"type": "user", "message": {"content": "build docker"}}),
            tool_use("prod"),
        ],
    );

    let listed = list_sessions(
        ListSessionsOptions::new()
            .directory(project.path())
            .include_worktrees(false),
    );
    assert_eq!(listed.len(), 2);
    let tag_of = |id: &str| {
        listed
            .iter()
            .find(|session| session.session_id == id)
            .unwrap()
            .tag
            .clone()
    };
    assert_eq!(tag_of(&tagged).as_deref(), Some("real-tag"));
    assert_eq!(tag_of(&untagged), None);
}

#[test]
fn created_at_is_read_in_milliseconds_and_does_not_exceed_a_later_modification_time() {
    let project = Project::new();
    let id = new_id();
    let path = project.write(
        &id,
        &[json!({
            "type": "user",
            "message": {"content": "hello"},
            "timestamp": "2026-01-01T00:00:00.000Z",
        })],
    );
    std::fs::File::options()
        .append(true)
        .open(&path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1_769_904_000))
        .unwrap();

    let listed = list_sessions(
        ListSessionsOptions::new()
            .directory(project.path())
            .include_worktrees(false),
    );
    assert_eq!(ids(&listed), [id]);
    assert_eq!(listed[0].created_at, Some(1_767_225_600_000));
    assert_eq!(listed[0].last_modified, 1_769_904_000_000);
    assert!(listed[0].created_at.unwrap() <= listed[0].last_modified);
}

#[test]
fn a_session_of_another_project_is_found_only_without_a_directory() {
    let first = Project::new();
    let second = Project::new();
    let id = plain_session(&first, "in A only");
    assert!(get_session_info(&id, Some(second.path())).is_none());
    assert!(get_session_info(&id, None).is_some());
}

fn session_with_subagents_dir(project: &Project) -> (String, PathBuf) {
    let id = plain_session(project, "Hello Claude");
    let subagents = project.dir.join(&id).join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    (id, subagents)
}

fn agent_transcript(session_id: &str) -> Vec<Value> {
    vec![
        user("u1", None, session_id, Value::from("task")),
        assistant("a1", Some("u1"), session_id, "done"),
    ]
}

fn have_no_parent_ids(messages: &[SessionMessage]) -> bool {
    messages
        .iter()
        .all(|message| message.parent_tool_use_id.is_none() && message.parent_agent_id.is_none())
}

#[test]
fn a_session_without_subagents_lists_none() {
    let project = Project::new();
    let without_directory = plain_session(&project, "Hello Claude");
    assert!(list_subagents(&without_directory, Some(project.path())).is_empty());

    let (with_empty_directory, subagents) = session_with_subagents_dir(&project);
    assert!(subagents.is_dir());
    assert!(list_subagents(&with_empty_directory, Some(project.path())).is_empty());
}

#[test]
fn subagent_messages_are_empty_for_an_invalid_or_unknown_session() {
    let project = Project::new();
    assert!(get_subagent_messages("not-a-uuid", "abc", SessionMessagesOptions::new()).is_empty());
    assert!(get_subagent_messages("", "abc", SessionMessagesOptions::new()).is_empty());
    assert!(get_subagent_messages(&new_id(), "abc", SessionMessagesOptions::new()).is_empty());
    assert!(
        get_subagent_messages(
            &new_id(),
            "abc",
            SessionMessagesOptions::new().directory(project.path())
        )
        .is_empty()
    );
}

#[test]
fn parent_ids_are_absent_when_the_sidecar_is_missing_or_unusable() {
    let project = Project::new();
    let (id, subagents) = session_with_subagents_dir(&project);
    let sidecars = [
        ("missing", None),
        ("invalid", Some("not json {".to_owned())),
        (
            "without-ids",
            Some(json!({"agentType": "general-purpose"}).to_string()),
        ),
        (
            "wrong-types",
            Some(json!({"toolUseId": 42, "parentAgentId": ["x"]}).to_string()),
        ),
    ];
    for (agent, sidecar) in sidecars {
        write_lines(
            &subagents.join(format!("agent-{agent}.jsonl")),
            &agent_transcript(&id),
        );
        if let Some(sidecar) = sidecar {
            std::fs::write(subagents.join(format!("agent-{agent}.meta.json")), sidecar).unwrap();
        }
        let messages = get_subagent_messages(
            &id,
            agent,
            SessionMessagesOptions::new().directory(project.path()),
        );
        assert_eq!(messages.len(), 2, "{agent}");
        assert!(have_no_parent_ids(&messages), "{agent}");
    }
}

#[test]
fn a_sidecar_that_is_a_directory_leaves_the_parent_ids_absent() {
    let project = Project::new();
    let (id, subagents) = session_with_subagents_dir(&project);
    write_lines(&subagents.join("agent-x.jsonl"), &agent_transcript(&id));
    std::fs::create_dir(subagents.join("agent-x.meta.json")).unwrap();

    let messages = get_subagent_messages(
        &id,
        "x",
        SessionMessagesOptions::new().directory(project.path()),
    );
    assert_eq!(messages.len(), 2);
    assert!(have_no_parent_ids(&messages));
}

#[test]
fn corrupt_and_blank_lines_of_a_subagent_transcript_are_skipped() {
    let project = Project::new();
    let (id, subagents) = session_with_subagents_dir(&project);
    let lines = [
        type_first(&user("u1", None, &id, Value::from("hi"))),
        "not valid json {".to_owned(),
        String::new(),
        type_first(&assistant("a1", Some("u1"), &id, "ok")),
    ];
    std::fs::write(subagents.join("agent-x.jsonl"), lines.join("\n") + "\n").unwrap();

    let messages = get_subagent_messages(
        &id,
        "x",
        SessionMessagesOptions::new().directory(project.path()),
    );
    assert_eq!(uuids(&messages), ["u1", "a1"]);
}

#[test]
fn an_empty_subagent_transcript_gives_no_messages() {
    let project = Project::new();
    let (id, subagents) = session_with_subagents_dir(&project);
    std::fs::write(subagents.join("agent-empty.jsonl"), "").unwrap();
    assert!(
        get_subagent_messages(
            &id,
            "empty",
            SessionMessagesOptions::new().directory(project.path())
        )
        .is_empty()
    );
}

/// Python: `list_sessions(directory=None, limit=None, offset=0,
/// include_worktrees=True)`.
#[test]
fn list_sessions_options_default_to_upstream_keyword_defaults() {
    let options = ListSessionsOptions::new();
    assert_eq!(options, ListSessionsOptions::default());
    assert_eq!(options.directory, None);
    assert_eq!(options.limit, None);
    assert_eq!(options.offset, 0);
    assert!(options.include_worktrees);
    let set = options
        .directory("/project")
        .limit(5)
        .offset(2)
        .include_worktrees(false);
    assert_eq!(set.directory.as_deref(), Some(Path::new("/project")));
    assert_eq!(
        (set.limit, set.offset, set.include_worktrees),
        (Some(5), 2, false)
    );
}

/// Python: `get_session_messages(session_id, directory=None, limit=None,
/// offset=0)`, and the same for the subagent and store readers.
#[test]
fn session_messages_options_default_to_upstream_keyword_defaults() {
    let options = SessionMessagesOptions::new();
    assert_eq!(options, SessionMessagesOptions::default());
    assert_eq!(options.directory, None);
    assert_eq!(options.limit, None);
    assert_eq!(options.offset, 0);
    let set = options.directory("/project").limit(5).offset(2);
    assert_eq!(set.directory.as_deref(), Some(Path::new("/project")));
    assert_eq!((set.limit, set.offset), (Some(5), 2));
}

/// Python: `fork_session(session_id, directory=None, up_to_message_id=None,
/// title=None)`.
#[test]
fn fork_session_options_default_to_upstream_keyword_defaults() {
    let options = ForkSessionOptions::new();
    assert_eq!(options, ForkSessionOptions::default());
    assert_eq!(options.directory, None);
    assert_eq!(options.up_to_message_id, None);
    assert_eq!(options.title, None);
    let set = options
        .directory("/project")
        .up_to_message_id("550e8400-e29b-41d4-a716-446655440000")
        .title("Branch");
    assert_eq!(set.directory.as_deref(), Some(Path::new("/project")));
    assert_eq!(
        set.up_to_message_id.as_deref(),
        Some("550e8400-e29b-41d4-a716-446655440000")
    );
    assert_eq!(set.title.as_deref(), Some("Branch"));
}

/// Python: `import_session_to_store(session_id, store, *, directory=None,
/// include_subagents=True, batch_size=MAX_PENDING_ENTRIES)`, where
/// `MAX_PENDING_ENTRIES` is 500.
#[test]
fn import_session_options_default_to_upstream_keyword_defaults() {
    let options = ImportSessionOptions::new();
    assert_eq!(options, ImportSessionOptions::default());
    assert_eq!(options.directory, None);
    assert!(options.include_subagents);
    assert_eq!(options.batch_size, 500);
    let set = options
        .directory("/project")
        .include_subagents(false)
        .batch_size(2);
    assert_eq!(set.directory.as_deref(), Some(Path::new("/project")));
    assert_eq!((set.include_subagents, set.batch_size), (false, 2));
}
