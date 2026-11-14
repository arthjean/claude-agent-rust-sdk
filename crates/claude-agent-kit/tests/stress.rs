#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use claude_agent_kit::{ClaudeAgentOptions, SubprocessTransport};
use futures_util::FutureExt;
use tokio::sync::Semaphore;

use common::{fake_claude, next_message, processes_matching};

const CYCLES: usize = 200;
const CONCURRENT_SESSIONS: usize = 16;
const SETTLE_LIMIT: Duration = Duration::from_secs(60);

fn uniquely_named_fake(directory: &std::path::Path) -> (PathBuf, String) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let token = format!("fcs-{:08x}", nanos ^ std::process::id());
    let binary = directory.join(format!("{token}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(fake_claude(), &binary).unwrap();
    (binary, token)
}

async fn cycle(index: usize, cli_path: PathBuf, spawned: Arc<AtomicUsize>) {
    let options = ClaudeAgentOptions::builder()
        .cli_path(cli_path)
        .build()
        .unwrap();
    match index % 4 {
        0 | 2 => {
            let transport = SubprocessTransport::new(options);
            transport.connect().await.unwrap();
            spawned.fetch_add(1, Ordering::SeqCst);
            let mut stream = transport.read_messages();
            assert_eq!(next_message(&mut stream).await["subtype"], "init");
            transport.close().await.unwrap();
        }
        1 => {
            let transport = SubprocessTransport::new(options);
            transport.connect().await.unwrap();
            spawned.fetch_add(1, Ordering::SeqCst);
            assert!(transport.close().now_or_never().is_none());
        }
        _ => {
            let session = async {
                let transport = SubprocessTransport::new(options);
                transport.connect().await.unwrap();
                spawned.fetch_add(1, Ordering::SeqCst);
                let mut stream = transport.read_messages();
                next_message(&mut stream).await;
                std::future::pending::<()>().await;
            };
            let cancelled = tokio::time::timeout(Duration::from_millis(150), session).await;
            assert!(cancelled.is_err());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_hundred_cycles_half_of_them_cancelled_leave_no_process() {
    let directory = tempfile::tempdir().unwrap();
    let (cli_path, token) = uniquely_named_fake(directory.path());
    assert!(processes_matching(&token).is_empty());

    let spawned = Arc::new(AtomicUsize::new(0));
    let slots = Arc::new(Semaphore::new(CONCURRENT_SESSIONS));
    let cycles: Vec<_> = (0..CYCLES)
        .map(|index| {
            let cli_path = cli_path.clone();
            let spawned = Arc::clone(&spawned);
            let slots = Arc::clone(&slots);
            tokio::spawn(async move {
                let _slot = slots.acquire().await.unwrap();
                cycle(index, cli_path, spawned).await;
            })
        })
        .collect();
    for cycle in cycles {
        cycle.await.unwrap();
    }
    assert_eq!(spawned.load(Ordering::SeqCst), CYCLES);

    let deadline = tokio::time::Instant::now() + SETTLE_LIMIT;
    loop {
        let remaining = processes_matching(&token);
        if remaining.is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} fake-claude processes remain: {remaining:?}",
            remaining.len()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
