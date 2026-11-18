//! Settings changed while a session is running.

use claude_agent_kit::PermissionMode;

use crate::suite;

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_permission_mode_changes_during_a_session() {
    let options = suite::options()
        .permission_mode(PermissionMode::Default)
        .build()
        .unwrap();
    let client = suite::connect(options).await;

    client
        .set_permission_mode(PermissionMode::AcceptEdits)
        .await
        .unwrap();
    let first = suite::turn(&client, "What is 2+2? Just respond with the number.").await;
    client
        .set_permission_mode(PermissionMode::Default)
        .await
        .unwrap();
    let second = suite::turn(&client, "What is 3+3? Just respond with the number.").await;
    client.disconnect().await.unwrap();

    assert!(!suite::result(&first).is_error);
    assert!(!suite::result(&second).is_error);
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_model_changes_during_a_session() {
    let client = suite::connect(suite::options().build().unwrap()).await;

    let default_model = suite::turn(&client, "What is 1+1? Just the number.").await;
    client.set_model(Some("haiku")).await.unwrap();
    let alias = suite::turn(&client, "What is 2+2? Just the number.").await;
    client.set_model(None).await.unwrap();
    let back = suite::turn(&client, "What is 3+3? Just the number.").await;
    client.disconnect().await.unwrap();

    for response in [&default_model, &alias, &back] {
        assert!(!suite::result(response).is_error, "{response:#?}");
    }
}

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn an_interrupt_is_accepted_during_a_session() {
    let client = suite::connect(suite::options().build().unwrap()).await;

    client.query("Count from 1 to 100 slowly.").await.unwrap();
    client.interrupt().await.unwrap();
    let messages = suite::response(&client).await;
    client.disconnect().await.unwrap();

    suite::result(&messages);
}
