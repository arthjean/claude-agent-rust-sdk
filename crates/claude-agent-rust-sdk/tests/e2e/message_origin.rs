//! A host-stamped origin and message id come back on the result.

use claude_agent_rust_sdk::{MessageOriginKind, Prompt};
use futures_util::stream;
use serde_json::{Value, json};

use crate::suite;

fn user_message(text: &str, extra: Value) -> Value {
    let mut message = json!({
        "type": "user",
        "message": {"role": "user", "content": text},
        "parent_tool_use_id": null,
    });
    if let (Some(message), Some(extra)) = (message.as_object_mut(), extra.as_object()) {
        message.extend(extra.clone());
    }
    message
}

fn one(message: Value) -> Prompt {
    Prompt::stream(stream::iter([message]))
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_result_echoes_the_origin_and_the_id_stamped_by_the_host() {
    let prompt_uuid = uuid::Uuid::new_v4().to_string();
    let client = suite::connect(suite::options().max_turns(1).build().unwrap()).await;

    let stamped = suite::turn(
        &client,
        one(user_message(
            "Reply with exactly: one",
            json!({"origin": {"kind": "human"}, "uuid": prompt_uuid}),
        )),
    )
    .await;
    let unstamped = suite::turn(
        &client,
        one(user_message("Reply with exactly: two", json!({}))),
    )
    .await;
    client.disconnect().await.unwrap();

    let stamped = suite::result(&stamped);
    let unstamped = suite::result(&unstamped);
    assert_eq!(
        stamped.origin.as_ref().map(|origin| &origin.kind),
        Some(&MessageOriginKind::Human),
        "{stamped:#?}"
    );
    assert_eq!(unstamped.origin, None, "{unstamped:#?}");
    assert_eq!(
        stamped.user_message_uuid.as_deref(),
        Some(prompt_uuid.as_str())
    );
    assert_eq!(
        stamped.user_message_uuids.as_deref(),
        Some([prompt_uuid.clone()].as_slice())
    );
    assert_eq!(unstamped.user_message_uuid, None);
    assert_eq!(unstamped.user_message_uuids, None);
}
