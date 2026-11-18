//! The stderr callback on a clean run.
//!
//! A run is clean when nothing gives the CLI a reason to warn. A machine
//! that holds both a login of the CLI and an API key is not: the CLI
//! reports on stderr that the key takes precedence. So when the
//! credentials come from the environment, the session gets an empty
//! configuration directory, where there is no login to conflict with.

use claude_agent_rust_sdk::StderrCallback;

use crate::suite;

#[tokio::test]
#[ignore = "runs the real CLI; run with `cargo xtask e2e`"]
async fn the_stderr_callback_receives_nothing_on_a_clean_run() {
    let lines = suite::log::<String>();
    let seen = lines.clone();
    let (_directory, root) = suite::workspace();
    let cwd = root.join("project");
    std::fs::create_dir(&cwd).unwrap();
    let mut builder = suite::options()
        .stderr(StderrCallback::from_sync(move |line| {
            seen.lock().unwrap().push(line.to_owned());
        }))
        .cwd(&cwd);
    if suite::credentials_travel_with_the_environment() {
        let config_directory = root.join("config");
        std::fs::create_dir(&config_directory).unwrap();
        builder = builder.env_var("CLAUDE_CONFIG_DIR", config_directory.to_string_lossy());
    }

    let messages = suite::run("What is 1+1?", builder.build().unwrap()).await;
    suite::forget(&cwd);

    assert!(!suite::result(&messages).is_error);
    assert_eq!(suite::entries(&lines), Vec::<String>::new());
}
