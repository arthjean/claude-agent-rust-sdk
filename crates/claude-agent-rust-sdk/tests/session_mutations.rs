#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::HashSet;

use claude_agent_rust_sdk::{
    Error, delete_session, fork_session, get_session_info, get_session_messages, list_sessions,
    rename_session, tag_session,
};
use serde_json::{Value, json};

use common::transcripts::{Project, assistant, conversation, new_id, read_lines, user};

fn transcript(project: &Project, id: &str) -> Vec<Value> {
    let ids: Vec<String> = (0..4).map(|_| new_id()).collect();
    let lines = vec![
        user(&ids[0], None, id, Value::from("first prompt")),
        assistant(&ids[1], Some(&ids[0]), id, "answer"),
        json!({"type": "progress", "uuid": ids[2], "parentUuid": ids[1], "sessionId": id}),
        user(&ids[3], Some(&ids[2]), id, Value::from("second prompt")),
    ];
    project.write(id, &lines);
    lines
}

#[test]
fn rename_appends_a_trimmed_custom_title_and_the_last_wins() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    rename_session(&id, "  First  ", Some(project.path())).unwrap();
    rename_session(&id, "Second", Some(project.path())).unwrap();
    let text = std::fs::read_to_string(project.transcript(&id)).unwrap();
    let last = text.lines().last().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(last).unwrap(),
        json!({"type": "custom-title", "customTitle": "Second", "sessionId": id})
    );
    assert!(last.starts_with(r#"{"type":"custom-title","#));
    assert!(text.contains(r#""customTitle":"First""#));
    assert_eq!(
        get_session_info(&id, Some(project.path())).unwrap().summary,
        "Second"
    );
}

#[test]
fn rename_rejects_bad_input_and_missing_sessions() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    assert!(matches!(
        rename_session("bad", "t", None),
        Err(Error::Config { .. })
    ));
    assert!(matches!(
        rename_session(&id, "   ", Some(project.path())),
        Err(Error::Config { .. })
    ));
    assert!(matches!(
        rename_session(&new_id(), "t", Some(project.path())),
        Err(Error::SessionNotFound { .. })
    ));
}

#[test]
fn empty_transcripts_are_not_touched_and_count_as_missing() {
    let project = Project::new();
    let id = new_id();
    std::fs::write(project.transcript(&id), "").unwrap();
    assert!(matches!(
        rename_session(&id, "t", Some(project.path())),
        Err(Error::SessionNotFound { .. })
    ));
    assert!(matches!(
        tag_session(&id, Some("x"), Some(project.path())),
        Err(Error::SessionNotFound { .. })
    ));
    assert_eq!(std::fs::metadata(project.transcript(&id)).unwrap().len(), 0);
}

#[test]
fn appends_never_create_the_file() {
    let project = Project::new();
    let id = new_id();
    assert!(rename_session(&id, "t", Some(project.path())).is_err());
    assert!(!project.transcript(&id).exists());
}

#[test]
fn a_transcript_whose_parent_directory_is_missing_counts_as_session_not_found() {
    let project = Project::new();
    let id = new_id();
    std::fs::remove_dir(&project.dir).unwrap();
    assert!(!project.transcript(&id).parent().unwrap().exists());
    assert!(matches!(
        rename_session(&id, "t", Some(project.path())),
        Err(Error::SessionNotFound { .. })
    ));
    assert!(matches!(
        tag_session(&id, Some("x"), Some(project.path())),
        Err(Error::SessionNotFound { .. })
    ));
    assert!(!project.dir.exists());

    #[cfg(unix)]
    {
        let project = Project::new();
        let elsewhere = tempfile::tempdir().unwrap();
        let regular_file = elsewhere.path().join("regular-file");
        std::fs::write(&regular_file, "x").unwrap();
        for target in [
            elsewhere.path().join("missing").join("transcript.jsonl"),
            regular_file.join("transcript.jsonl"),
        ] {
            let id = new_id();
            std::os::unix::fs::symlink(&target, project.transcript(&id)).unwrap();
            assert!(
                matches!(
                    rename_session(&id, "t", Some(project.path())),
                    Err(Error::SessionNotFound { .. })
                ),
                "{}",
                target.display()
            );
            assert!(!target.exists());
        }
    }
}

const NO_PROJECTS_HOST_VARIABLE: &str = "CLAUDE_AGENT_RUST_SDK_TEST_NO_PROJECTS_HOST";

#[test]
fn host_process_for_the_missing_projects_directory_test() {
    if std::env::var_os(NO_PROJECTS_HOST_VARIABLE).is_none() {
        return;
    }
    let outcome = match rename_session(&new_id(), "title", None) {
        Err(error @ Error::SessionNotFound { .. }) => format!("session-not-found: {error}"),
        other => format!("{other:?}"),
    };
    println!("rename-outcome={outcome}");
}

#[test]
fn rename_without_a_projects_directory_reports_the_session_as_not_found() {
    let root = tempfile::tempdir().unwrap();
    let config_dir = root.path().join("nonexistent");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "host_process_for_the_missing_projects_directory_test",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(NO_PROJECTS_HOST_VARIABLE, "1")
        .env("CLAUDE_CONFIG_DIR", &config_dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("rename-outcome=session-not-found"),
        "{stdout}"
    );
    assert!(stdout.contains("(no projects directory)"), "{stdout}");
    assert!(!config_dir.exists());
}

#[test]
fn rename_without_a_directory_skips_empty_stubs() {
    let first = Project::new();
    let second = Project::new();
    for (stub, real) in [(&first, &second), (&second, &first)] {
        let id = new_id();
        std::fs::write(stub.transcript(&id), "").unwrap();
        real.write(&id, &conversation(&id, "hello", json!({})));
        rename_session(&id, "Found", None).unwrap();
        assert_eq!(
            get_session_info(&id, Some(real.path())).unwrap().summary,
            "Found"
        );
        assert_eq!(std::fs::metadata(stub.transcript(&id)).unwrap().len(), 0);
    }
}

#[test]
fn a_tag_is_appended_as_compact_json_in_the_field_order_of_the_cli() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));

    tag_session(&id, Some("mytag"), Some(project.path())).unwrap();

    let text = std::fs::read_to_string(project.transcript(&id)).unwrap();
    assert_eq!(
        text.trim_end().lines().last(),
        Some(format!(r#"{{"type":"tag","tag":"mytag","sessionId":"{id}"}}"#).as_str())
    );
}

#[test]
fn tags_are_sanitized_cleared_with_none_and_the_last_wins() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    tag_session(
        &id,
        Some("  exp\u{200b}eri\u{202e}ment\u{feff} "),
        Some(project.path()),
    )
    .unwrap();
    assert_eq!(
        get_session_info(&id, Some(project.path()))
            .unwrap()
            .tag
            .as_deref(),
        Some("experiment")
    );
    let last = read_lines(&project.transcript(&id)).pop().unwrap();
    assert_eq!(
        last,
        json!({"type": "tag", "tag": "experiment", "sessionId": id})
    );

    tag_session(&id, Some("\u{ff21}"), Some(project.path())).unwrap();
    assert_eq!(
        get_session_info(&id, Some(project.path()))
            .unwrap()
            .tag
            .as_deref(),
        Some("A")
    );
    tag_session(&id, None, Some(project.path())).unwrap();
    assert_eq!(
        read_lines(&project.transcript(&id)).pop().unwrap()["tag"],
        ""
    );
    assert_eq!(
        get_session_info(&id, Some(project.path())).unwrap().tag,
        None
    );

    assert!(matches!(
        tag_session(&id, Some("\u{200b}\u{200d}  "), Some(project.path())),
        Err(Error::Config { .. })
    ));
    assert!(matches!(
        tag_session("bad", Some("x"), None),
        Err(Error::Config { .. })
    ));
}

#[test]
fn concurrent_rename_and_tag_both_land() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    let path = project.path().to_path_buf();
    let threads: Vec<_> = (0..8)
        .map(|index| {
            let id = id.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                if index % 2 == 0 {
                    rename_session(&id, &format!("title {index}"), Some(&path)).unwrap();
                } else {
                    tag_session(&id, Some(&format!("tag{index}")), Some(&path)).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let lines = read_lines(&project.transcript(&id));
    assert_eq!(lines.len(), 2 + 8);
    assert_eq!(
        lines
            .iter()
            .filter(|line| line["type"] == "custom-title")
            .count(),
        4
    );
    assert_eq!(lines.iter().filter(|line| line["type"] == "tag").count(), 4);
}

#[test]
fn delete_removes_the_transcript_and_the_subagent_directory() {
    let project = Project::new();
    let id = new_id();
    project.write(&id, &conversation(&id, "hello", json!({})));
    let subagents = project.dir.join(&id).join("subagents");
    std::fs::create_dir_all(&subagents).unwrap();
    std::fs::write(subagents.join("agent-a.jsonl"), "{}\n").unwrap();
    delete_session(&id, Some(project.path())).unwrap();
    assert!(!project.transcript(&id).exists());
    assert!(!project.dir.join(&id).exists());
    assert!(list_sessions(Some(project.path()), None, 0, false).is_empty());
    assert!(matches!(
        delete_session(&id, Some(project.path())),
        Err(Error::SessionNotFound { .. })
    ));
    assert!(matches!(
        delete_session("bad", None),
        Err(Error::Config { .. })
    ));

    let other = new_id();
    project.write(&other, &conversation(&other, "hello", json!({})));
    delete_session(&other, None).unwrap();
    assert!(!project.transcript(&other).exists());
}

#[test]
fn fork_writes_a_relinked_copy_with_new_ids() {
    let project = Project::new();
    let id = new_id();
    let mut source = transcript(&project, &id);
    source[0]["slug"] = Value::from("leak");
    source[0]["teamName"] = Value::from("team");
    let mut side = user(&new_id(), None, &id, Value::from("side"));
    side["isSidechain"] = Value::Bool(true);
    source.push(side);
    source
        .push(json!({"type": "content-replacement", "sessionId": id, "replacements": [{"x": 1}]}));
    source.push(
        json!({"type": "content-replacement", "sessionId": "other", "replacements": [{"y": 2}]}),
    );
    project.write(&id, &source);

    let fork = fork_session(&id, Some(project.path()), None, None).unwrap();
    assert_ne!(fork.session_id, id);
    let path = project.transcript(&fork.session_id);
    let lines = read_lines(&path);
    assert_eq!(lines.len(), 3 + 2);
    let originals: HashSet<String> = source
        .iter()
        .filter_map(|line| line["uuid"].as_str().map(str::to_owned))
        .collect();
    for line in &lines[..3] {
        assert!(!originals.contains(line["uuid"].as_str().unwrap()));
        assert_eq!(line["sessionId"], fork.session_id.as_str());
        assert_eq!(line["isSidechain"], false);
        assert_eq!(line["forkedFrom"]["sessionId"], id.as_str());
        assert!(line.get("slug").is_none() && line.get("teamName").is_none());
    }
    assert_eq!(lines[1]["parentUuid"], lines[0]["uuid"]);
    assert_eq!(
        lines[2]["parentUuid"], lines[1]["uuid"],
        "relinked across the dropped progress entry"
    );
    assert_eq!(lines[0]["forkedFrom"]["messageUuid"], source[0]["uuid"]);
    assert_eq!(lines[3]["type"], "content-replacement");
    assert_eq!(lines[3]["replacements"], json!([{"x": 1}]));
    assert_eq!(lines[4]["type"], "custom-title");
    assert_eq!(lines[4]["customTitle"], "first prompt (fork)");
    assert_eq!(
        get_session_info(&fork.session_id, Some(project.path()))
            .unwrap()
            .summary,
        "first prompt (fork)"
    );
    assert_eq!(
        get_session_messages(&fork.session_id, Some(project.path()), None, 0).len(),
        3
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn fork_cuts_inclusively_and_honors_titles() {
    let project = Project::new();
    let id = new_id();
    let source = transcript(&project, &id);
    rename_session(&id, "Original", Some(project.path())).unwrap();
    let cut = source[1]["uuid"].as_str().unwrap();
    let fork = fork_session(&id, Some(project.path()), Some(cut), Some("  Branch ")).unwrap();
    let lines = read_lines(&project.transcript(&fork.session_id));
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1]["forkedFrom"]["messageUuid"], cut);
    assert_eq!(lines[2]["customTitle"], "Branch");

    let derived = fork_session(&id, None, None, None).unwrap();
    let lines = read_lines(&project.transcript(&derived.session_id));
    assert_eq!(lines.last().unwrap()["customTitle"], "Original (fork)");
}

#[test]
fn fork_errors_create_no_file() {
    let project = Project::new();
    let id = new_id();
    transcript(&project, &id);
    let before = std::fs::read_dir(&project.dir).unwrap().count();
    assert!(matches!(
        fork_session(&id, Some(project.path()), Some(&new_id()), None),
        Err(Error::Config { .. })
    ));
    assert!(matches!(
        fork_session(&id, Some(project.path()), Some("bad"), None),
        Err(Error::Config { .. })
    ));
    assert!(matches!(
        fork_session(&new_id(), Some(project.path()), None, None),
        Err(Error::SessionNotFound { .. })
    ));
    assert!(matches!(
        fork_session("bad", None, None, None),
        Err(Error::Config { .. })
    ));
    let only_meta = new_id();
    project.write(
        &only_meta,
        &[json!({"type": "custom-title", "customTitle": "t"})],
    );
    assert!(matches!(
        fork_session(&only_meta, Some(project.path()), None, None),
        Err(Error::Config { .. })
    ));
    assert_eq!(std::fs::read_dir(&project.dir).unwrap().count(), before + 1);
}
