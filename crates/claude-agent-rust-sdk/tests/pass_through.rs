#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use claude_agent_rust_sdk::{
    ClaudeAgentOptions, ContextUsageResponse, McpServerStatusConfig, McpStatusResponse, Message,
    ModelUsage, parse_message,
};
use indexmap::IndexMap;
use serde_json::{Value, json};

use common::mock::{cli_answering, connected, is_control_request, success};

const KEY: &str = "futureKey";
const VALUE: i64 = 7;

/// A place where the Python parser or client hands a dictionary over
/// untouched, with a fixture holding one object at each level listed.
struct Place {
    name: &'static str,
    python: &'static str,
    fixture: Value,
    levels: &'static [&'static str],
}

/// Levels where the key is deliberately not reachable, each with its
/// reason. An exemption whose key becomes reachable fails the test.
const EXEMPTIONS: &[(&str, &str, &str)] = &[(
    "result.modelUsage",
    "/modelUsage",
    "a key of `modelUsage` is a model name; the injected value 7 is not an \
     object, so it is not a model usage entry and is skipped",
)];

fn result_fixture() -> Value {
    json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 1,
        "duration_api_ms": 1,
        "is_error": false,
        "num_turns": 1,
        "session_id": "session",
        "usage": {"input_tokens": 1, "server_tool_use": {"web_search_requests": 0}},
        "structured_output": {"answer": {"value": 1}},
        "modelUsage": {"claude-x": {"inputTokens": 1, "costUSD": 0.5}},
        "permission_denials": [{"tool_name": "Bash", "tool_input": {"command": "ls"}}],
    })
}

fn places() -> Vec<Place> {
    vec![
        Place {
            name: "result.usage",
            python: "message_parser.py:322",
            fixture: result_fixture(),
            levels: &["/usage", "/usage/server_tool_use"],
        },
        Place {
            name: "result.structured_output",
            python: "message_parser.py:324",
            fixture: result_fixture(),
            levels: &["/structured_output", "/structured_output/answer"],
        },
        Place {
            name: "result.modelUsage",
            python: "message_parser.py:325",
            fixture: result_fixture(),
            levels: &["/modelUsage", "/modelUsage/claude-x"],
        },
        Place {
            name: "result.permission_denials",
            python: "message_parser.py:326",
            fixture: result_fixture(),
            levels: &["/permission_denials/0", "/permission_denials/0/tool_input"],
        },
        Place {
            name: "stream_event.event",
            python: "message_parser.py:352",
            fixture: json!({
                "type": "stream_event",
                "uuid": "u",
                "session_id": "session",
                "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "x"}},
            }),
            levels: &["/event", "/event/delta"],
        },
        Place {
            name: "rate_limit_event.raw",
            python: "message_parser.py:374",
            fixture: json!({
                "type": "rate_limit_event",
                "uuid": "u",
                "session_id": "session",
                "rate_limit_info": {"status": "allowed", "resetsAt": 1},
            }),
            levels: &["/rate_limit_info"],
        },
        Place {
            name: "get_mcp_status",
            python: "query.py:813-815, client.py:471-505",
            fixture: json!({"mcpServers": [
                {
                    "name": "files",
                    "status": "connected",
                    "serverInfo": {"name": "files", "version": "1.0.0"},
                    "config": {"type": "http", "url": "https://example.test/mcp"},
                    "scope": "project",
                    "tools": [{"name": "read", "description": "Read", "annotations": {"readOnly": true}}],
                },
                {"name": "calc", "status": "connected", "config": {"type": "sdk", "name": "calc"}},
                {"name": "proxy", "status": "pending", "config": {"type": "claudeai-proxy", "url": "https://p", "id": "i"}},
                {"name": "local", "status": "failed", "config": {"command": "server"}},
            ]}),
            levels: &[
                "",
                "/mcpServers/0",
                "/mcpServers/0/serverInfo",
                "/mcpServers/0/config",
                "/mcpServers/0/tools/0",
                "/mcpServers/0/tools/0/annotations",
                "/mcpServers/1/config",
                "/mcpServers/2/config",
                "/mcpServers/3/config",
            ],
        },
        Place {
            name: "get_context_usage",
            python: "query.py:817-819, client.py:507-538",
            fixture: json!({
                "categories": [{"name": "System prompt", "tokens": 1, "color": "blue"}],
                "totalTokens": 1,
                "maxTokens": 2,
                "rawMaxTokens": 2,
                "percentage": 50.0,
                "model": "claude-x",
                "isAutoCompactEnabled": true,
                "memoryFiles": [{"path": "CLAUDE.md", "type": "Project", "tokens": 1}],
                "mcpTools": [{"name": "read", "serverName": "files", "tokens": 1, "isLoaded": true}],
                "agents": [{"agentType": "a", "source": "user", "tokens": 1}],
                "gridRows": [[{"color": "blue", "isFilled": true}]],
                "systemTools": [{"name": "Bash", "tokens": 1}],
                "systemPromptSections": [{"name": "intro", "tokens": 1}],
                "slashCommands": {"totalCommands": 1},
                "skills": {"totalSkills": 1},
                "messageBreakdown": {"toolCallTokens": 1},
                "apiUsage": {"input_tokens": 1},
            }),
            levels: &[
                "",
                "/categories/0",
                "/memoryFiles/0",
                "/mcpTools/0",
                "/agents/0",
                "/gridRows/0/0",
                "/systemTools/0",
                "/systemPromptSections/0",
                "/slashCommands",
                "/skills",
                "/messageBreakdown",
                "/apiUsage",
            ],
        },
    ]
}

fn inject(fixture: &mut Value, levels: &[&str]) {
    for level in levels {
        let object = fixture
            .pointer_mut(level)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("the fixture has no object at {level:?}"));
        object.insert(KEY.to_owned(), json!(VALUE));
    }
}

/// The parsed value, seen as JSON through its public fields, with the
/// layout of the wire message so a level has the same pointer in both.
async fn parsed_view(name: &str, wire: Value) -> Value {
    match name {
        "get_mcp_status" | "get_context_usage" => {
            let transport = cli_answering(move |frame| {
                if is_control_request(frame, "mcp_status")
                    || is_control_request(frame, "get_context_usage")
                {
                    vec![success(frame, wire.clone())]
                } else {
                    Vec::new()
                }
            });
            let client = connected(&transport, ClaudeAgentOptions::default()).await;
            let view = if name == "get_mcp_status" {
                serde_json::to_value(client.get_mcp_status().await.unwrap()).unwrap()
            } else {
                serde_json::to_value(client.get_context_usage().await.unwrap()).unwrap()
            };
            client.disconnect().await.unwrap();
            view
        }
        _ => match parse_message(wire).unwrap() {
            Message::Result(result) => json!({
                "usage": result.usage,
                "structured_output": result.structured_output,
                "modelUsage": serde_json::to_value(&result.model_usage).unwrap(),
                "permission_denials": result.permission_denials,
            }),
            Message::StreamEvent(event) => json!({"event": event.event}),
            Message::RateLimit(event) => json!({"rate_limit_info": event.rate_limit_info.raw}),
            other => panic!("unexpected message {other:?}"),
        },
    }
}

#[tokio::test(start_paused = true)]
async fn every_key_python_passes_through_is_reachable_from_the_parsed_value() {
    let places = places();
    for (place, level, _) in EXEMPTIONS {
        assert!(
            places
                .iter()
                .any(|known| known.name == *place && known.levels.contains(level)),
            "the exemption {place} {level} names no level of the table"
        );
    }
    let mut missing = Vec::new();
    let mut stale = Vec::new();
    for place in places {
        let mut wire = place.fixture.clone();
        inject(&mut wire, place.levels);
        let view = parsed_view(place.name, wire).await;
        for level in place.levels {
            let reachable = view.pointer(&format!("{level}/{KEY}")) == Some(&json!(VALUE));
            let exempted = EXEMPTIONS
                .iter()
                .any(|(name, exempt, _)| *name == place.name && exempt == level);
            match (reachable, exempted) {
                (false, false) => {
                    missing.push(format!("{} ({}) at {level:?}", place.name, place.python))
                }
                (true, true) => stale.push(format!("{} at {level:?}", place.name)),
                _ => {}
            }
        }
    }
    assert!(missing.is_empty(), "unknown keys lost: {missing:#?}");
    assert!(stale.is_empty(), "exemptions no longer needed: {stale:#?}");
}

fn model_usage(wire: Value) -> Option<IndexMap<String, ModelUsage>> {
    match parse_message(wire).unwrap() {
        Message::Result(result) => result.model_usage,
        other => panic!("unexpected message {other:?}"),
    }
}

#[test]
fn model_usage_exposes_a_key_it_does_not_model() {
    let mut wire = result_fixture();
    wire["modelUsage"] = json!({"claude-x": {"inputTokens": 1, KEY: VALUE}});
    let usages = model_usage(wire).unwrap();
    let usage = &usages["claude-x"];
    assert_eq!(usage.input_tokens, 1);
    assert_eq!(usage.extra.get(KEY), Some(&json!(VALUE)));
    assert_eq!(usage.extra.len(), 1, "{:?}", usage.extra);
}

#[test]
fn model_usage_entry_that_is_not_an_object_is_skipped() {
    let mut wire = result_fixture();
    wire["modelUsage"] =
        json!({"claude-x": {"inputTokens": 3}, "broken": 7, "listed": [1], "none": null});
    let usages = model_usage(wire).unwrap();
    let models: Vec<&str> = usages.keys().map(String::as_str).collect();
    assert_eq!(models, ["claude-x"]);
    assert_eq!(usages["claude-x"].input_tokens, 3);
}

#[test]
fn values_parsed_with_unknown_keys_write_them_back() {
    let usage: ModelUsage =
        serde_json::from_value(json!({"inputTokens": 2, "costUSD": 0.25, KEY: VALUE})).unwrap();
    assert_eq!(usage.extra.get(KEY), Some(&json!(VALUE)));
    let written = serde_json::to_value(&usage).unwrap();
    assert_eq!(written["inputTokens"], 2);
    assert_eq!(written[KEY], VALUE);

    let status = json!({"mcpServers": [{
        "name": "files",
        "status": "connected",
        "serverInfo": {"name": "files", "version": "1", KEY: VALUE},
        "tools": [{"name": "read", "annotations": {"readOnly": true, KEY: VALUE}, KEY: VALUE}],
        KEY: VALUE,
    }], KEY: VALUE});
    let parsed: McpStatusResponse = serde_json::from_value(status.clone()).unwrap();
    assert_eq!(parsed.extra[KEY], VALUE);
    assert_eq!(parsed.mcp_servers[0].extra[KEY], VALUE);
    let server = &parsed.mcp_servers[0];
    assert_eq!(server.server_info.as_ref().unwrap().extra[KEY], VALUE);
    let tool = &server.tools.as_ref().unwrap()[0];
    assert_eq!(tool.extra[KEY], VALUE);
    assert_eq!(tool.annotations.as_ref().unwrap().extra[KEY], VALUE);
    assert_eq!(serde_json::to_value(&parsed).unwrap(), status);

    let usage =
        json!({"categories": [{"name": "c", "tokens": 1, "color": "x", KEY: VALUE}], KEY: VALUE});
    let parsed: ContextUsageResponse = serde_json::from_value(usage).unwrap();
    assert_eq!(parsed.extra[KEY], VALUE);
    assert_eq!(parsed.categories[0].extra[KEY], VALUE);
    let written = serde_json::to_value(&parsed).unwrap();
    assert_eq!(written[KEY], VALUE);
    assert_eq!(written["categories"][0][KEY], VALUE);
}

#[test]
fn a_known_server_config_with_a_key_it_does_not_model_keeps_it_in_extra() {
    let raw = json!({"type": "http", "url": "https://example.test/mcp", KEY: VALUE});
    let config: McpServerStatusConfig = serde_json::from_value(raw.clone()).unwrap();
    let McpServerStatusConfig::Http(http) = &config else {
        panic!("expected an http configuration, got {config:?}");
    };
    assert_eq!(http.url, "https://example.test/mcp");
    assert_eq!(
        http.extra.keys().collect::<Vec<_>>(),
        [KEY],
        "the type is not an extra key"
    );
    assert_eq!(config.type_name(), Some("http"));
    assert_eq!(
        serde_json::to_string(&config)
            .unwrap()
            .matches("\"type\"")
            .count(),
        1
    );
    assert_eq!(serde_json::to_value(&config).unwrap(), raw);

    let typed: McpServerStatusConfig =
        serde_json::from_value(json!({"type": "http", "url": "https://example.test/mcp"})).unwrap();
    assert!(matches!(typed, McpServerStatusConfig::Http(_)), "{typed:?}");
}
