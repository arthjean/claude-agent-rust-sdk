#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;
use std::process::Command;

use claude_agent_kit::{
    SessionMessageType, get_session_info, get_session_messages, get_subagent_messages,
    list_sessions, list_subagents, project_key_for_directory,
};
use serde_json::{Value, json};

use common::transcripts::{
    Project, assistant, config_dir, conversation, new_id, projects_dir, set_mtime, user,
    write_lines,
};

fn ids(sessions: &[claude_agent_kit::SdkSessionInfo]) -> Vec<String> {
    sessions
        .iter()
        .map(|session| session.session_id.clone())
        .collect()
}

#[test]
fn config_dir_comes_from_the_environment() {
    let project = Project::new();
    assert!(project.dir.starts_with(config_dir()));
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    let listed = list_sessions(Some(project.path()), None, 0, false);
    assert_eq!(ids(&listed), [id]);
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
        ids(&list_sessions(Some(project.path()), None, 0, false)),
        expected
    );
    assert_eq!(
        ids(&list_sessions(Some(project.path()), Some(2), 0, false)),
        expected[..2]
    );
    assert_eq!(
        ids(&list_sessions(Some(project.path()), Some(2), 1, false)),
        expected[1..3]
    );
    assert_eq!(
        ids(&list_sessions(Some(project.path()), Some(0), 3, false)),
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
    let empty = new_id();
    std::fs::write(project.transcript(&empty), "").unwrap();
    std::fs::write(project.dir.join("not-a-uuid.jsonl"), "{}\n").unwrap();
    std::fs::write(project.dir.join(format!("{}.txt", new_id())), "{}\n").unwrap();

    assert_eq!(
        ids(&list_sessions(Some(project.path()), None, 0, true)),
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

    let all = list_sessions(None, None, 0, true);
    let matching: Vec<_> = all
        .iter()
        .filter(|session| session.session_id == shared)
        .collect();
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].summary, "newer");
    assert!(all.iter().any(|session| session.session_id == alone));
    assert_eq!(get_session_info(&alone, None).unwrap().summary, "alone");
}

#[test]
fn missing_project_directory_lists_nothing() {
    let cwd = tempfile::tempdir().unwrap();
    assert!(list_sessions(Some(cwd.path()), None, 0, true).is_empty());
    assert!(get_session_messages(&new_id(), Some(cwd.path()), None, 0).is_empty());
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
        ids(&list_sessions(Some(&long), None, 0, false)),
        std::slice::from_ref(&id)
    );
    assert_eq!(get_session_messages(&id, Some(&long), None, 0).len(), 2);
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

    let listed = list_sessions(Some(&main), None, 0, true);
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

    let mut own_only = ids(&list_sessions(Some(&main), None, 0, false));
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
        ids(&list_sessions(Some(project.path()), None, 0, true)),
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

    let messages = get_session_messages(&id, Some(project.path()), None, 0);
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

    let page: Vec<String> = get_session_messages(&id, Some(project.path()), Some(2), 1)
        .into_iter()
        .map(|message| message.uuid)
        .collect();
    assert_eq!(page, ["a1", "c1"]);
    assert_eq!(get_session_messages(&id, None, None, 0).len(), 4);
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
    let messages = get_session_messages(&id, Some(project.path()), None, 0);
    assert_eq!(messages.len(), 3);
}

#[test]
fn missing_or_invalid_sessions_give_empty_lists() {
    let project = Project::new();
    assert!(get_session_messages(&new_id(), Some(project.path()), None, 0).is_empty());
    assert!(get_session_messages("bad", Some(project.path()), None, 0).is_empty());
    let empty = new_id();
    std::fs::write(project.transcript(&empty), "").unwrap();
    assert!(get_session_messages(&empty, Some(project.path()), None, 0).is_empty());
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

    assert_eq!(
        list_subagents(&id, Some(project.path())),
        ["alpha", "gamma", "beta"]
    );
    assert_eq!(list_subagents(&id, None), ["alpha", "gamma", "beta"]);

    let alpha = get_subagent_messages(&id, "alpha", Some(project.path()), None, 0);
    assert_eq!(alpha.len(), 2);
    assert_eq!(alpha[0].parent_tool_use_id.as_deref(), Some("toolu_1"));
    assert_eq!(alpha[0].parent_agent_id, None);
    let beta = get_subagent_messages(&id, "beta", Some(project.path()), Some(1), 1);
    assert_eq!(beta.len(), 1);
    assert_eq!(beta[0].uuid, "beta-a");
    assert_eq!(beta[0].parent_agent_id.as_deref(), Some("alpha"));
    let gamma = get_subagent_messages(&id, "gamma", Some(project.path()), None, 0);
    assert_eq!(gamma[0].parent_tool_use_id, None);

    assert!(get_subagent_messages(&id, "missing", Some(project.path()), None, 0).is_empty());
    assert!(get_subagent_messages(&id, "", Some(project.path()), None, 0).is_empty());
    assert!(list_subagents(&new_id(), Some(project.path())).is_empty());
    assert!(list_subagents("bad", Some(project.path())).is_empty());
}
