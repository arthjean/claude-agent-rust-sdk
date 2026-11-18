//! Agent definitions and setting sources.

use claude_agent_rust_sdk::{AgentDefinition, Message, SettingSource};

use crate::suite;

const LARGE_AGENT_COUNT: usize = 20;
const LARGE_PROMPT_BYTES: usize = 13 * 1024;

fn large_agents() -> Vec<(String, AgentDefinition)> {
    (0..LARGE_AGENT_COUNT)
        .map(|index| {
            let prompt = format!(
                "You are test agent #{index}. {}",
                "x".repeat(LARGE_PROMPT_BYTES)
            );
            (
                format!("large-agent-{index}"),
                AgentDefinition::new(
                    format!("Large test agent #{index} for stress testing"),
                    prompt,
                ),
            )
        })
        .collect()
}

fn assert_all_registered(agents: &[(String, AgentDefinition)], registered: &[String]) {
    for (name, _) in agents {
        assert!(
            registered.contains(name),
            "{name} should be registered, found {} agents: {registered:?}",
            registered.len()
        );
    }
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn an_agent_definition_is_listed_in_the_init_message() {
    let options = suite::options()
        .agent(
            "test-agent",
            AgentDefinition::new(
                "A test agent for verification",
                "You are a test agent. Always respond with 'Test agent activated'",
            )
            .tools(["Read"])
            .model("sonnet"),
        )
        .max_turns(1)
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "What is 2 + 2?").await;
    client.disconnect().await.unwrap();

    let agents = suite::init_list(&messages, "agents");
    assert!(agents.contains(&"test-agent".to_owned()), "{agents:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn an_agent_definition_works_with_the_query_function() {
    let options = suite::options()
        .agent(
            "test-agent-query",
            AgentDefinition::new(
                "A test agent for query function verification",
                "You are a test agent.",
            ),
        )
        .max_turns(1)
        .build()
        .unwrap();

    let messages = suite::run("What is 2 + 2?", options).await;

    let agents = suite::init_list(&messages, "agents");
    assert!(
        agents.contains(&"test-agent-query".to_owned()),
        "{agents:?}"
    );
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn large_agent_definitions_work_with_the_query_function() {
    let agents = large_agents();
    let options = suite::options()
        .agents(agents.clone())
        .max_turns(1)
        .build()
        .unwrap();

    let messages = suite::run("What is 2 + 2?", options).await;

    assert_all_registered(&agents, &suite::init_list(&messages, "agents"));
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn filesystem_agents_load_and_the_response_completes() {
    let (_directory, project) = suite::workspace();
    let agents = project.join(".claude").join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("fs-test-agent.md"),
        "---\nname: fs-test-agent\ndescription: A filesystem test agent for SDK testing\n\
         tools: Read\n---\n\nYou are a simple test agent. When asked a question, provide a \
         brief, helpful answer.\n",
    )
    .unwrap();
    let options = suite::options()
        .setting_sources([SettingSource::Project])
        .cwd(&project)
        .max_turns(1)
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "Say hello in exactly 3 words").await;
    client.disconnect().await.unwrap();
    suite::forget(&project);

    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Assistant(_))),
        "no assistant message after the init message: {messages:#?}"
    );
    suite::result(&messages);
    let agents = suite::init_list(&messages, "agents");
    assert!(agents.contains(&"fs-test-agent".to_owned()), "{agents:?}");
}

fn project_with_local_output_style() -> (tempfile::TempDir, std::path::PathBuf) {
    let (directory, project) = suite::workspace();
    let claude = project.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("settings.local.json"),
        r#"{"outputStyle": "local-test-style"}"#,
    )
    .unwrap();
    (directory, project)
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn default_setting_sources_load_the_local_settings() {
    let (_directory, project) = project_with_local_output_style();
    let options = suite::options().cwd(&project).max_turns(1).build().unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "What is 2 + 2?").await;
    client.disconnect().await.unwrap();
    suite::forget(&project);

    assert_eq!(suite::init(&messages)["output_style"], "local-test-style");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn user_only_setting_sources_exclude_project_commands() {
    let (_directory, project) = suite::workspace();
    let commands = project.join(".claude").join("commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::write(
        commands.join("testcmd.md"),
        "---\ndescription: Test command\n---\n\nThis is a test command.\n",
    )
    .unwrap();
    let options = suite::options()
        .setting_sources([SettingSource::User])
        .cwd(&project)
        .max_turns(1)
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "What is 2 + 2?").await;
    client.disconnect().await.unwrap();
    suite::forget(&project);

    let commands = suite::init_list(&messages, "slash_commands");
    assert!(!commands.contains(&"testcmd".to_owned()), "{commands:?}");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn listed_setting_sources_include_the_local_settings() {
    let (_directory, project) = project_with_local_output_style();
    let options = suite::options()
        .setting_sources([
            SettingSource::User,
            SettingSource::Project,
            SettingSource::Local,
        ])
        .cwd(&project)
        .max_turns(1)
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "What is 2 + 2?").await;
    client.disconnect().await.unwrap();
    suite::forget(&project);

    assert_eq!(suite::init(&messages)["output_style"], "local-test-style");
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn large_agent_definitions_are_sent_through_the_initialize_request() {
    let agents = large_agents();
    let total: usize = agents
        .iter()
        .map(|(_, agent)| serde_json::to_string(agent).unwrap().len())
        .sum();
    assert!(total > 250_000, "the agents weigh only {total} bytes");
    let options = suite::options()
        .agents(agents.clone())
        .max_turns(1)
        .build()
        .unwrap();

    let client = suite::connect(options).await;
    let messages = suite::turn(&client, "List available agents").await;
    client.disconnect().await.unwrap();

    let registered = suite::init_list(&messages, "agents");
    assert_all_registered(&agents, &registered);
    assert!(registered.len() >= agents.len());
}
