//! The contract every [`SessionStore`] adapter must satisfy, as a reusable
//! suite.

use std::fmt;
use std::future::Future;

use serde_json::{Value, json};

use crate::session_store::{
    SessionKey, SessionListSubkeysKey, SessionStore, SessionStoreCapabilities, SessionStoreEntry,
    SessionStoreError, is_unsupported,
};
use crate::sessions::fold_session_summary;

/// A contract of the conformance suite that a store broke.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConformanceFailure {
    /// Number of the contract, as in the upstream Python suite (1 to 14).
    pub contract: u8,
    /// What was expected and what the store did.
    pub message: String,
}

impl fmt::Display for ConformanceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "session store contract {} failed: {}",
            self.contract, self.message
        )
    }
}

impl std::error::Error for ConformanceFailure {}

type Outcome = Result<(), ConformanceFailure>;

fn fail(contract: u8, message: impl Into<String>) -> ConformanceFailure {
    ConformanceFailure {
        contract,
        message: message.into(),
    }
}

fn check(contract: u8, holds: bool, message: impl FnOnce() -> String) -> Outcome {
    if holds {
        Ok(())
    } else {
        Err(fail(contract, message()))
    }
}

fn call<T>(
    contract: u8,
    operation: &str,
    result: Result<T, SessionStoreError>,
) -> Result<T, ConformanceFailure> {
    result.map_err(|error| fail(contract, format!("{operation} returned an error: {error}")))
}

fn entry(fields: Value) -> SessionStoreEntry {
    let mut entry = serde_json::Map::new();
    entry.insert("type".to_owned(), Value::from("x"));
    if let Value::Object(fields) = fields {
        entry.extend(fields);
    }
    entry
}

fn key(project: &str, session: &str) -> SessionKey {
    SessionKey::new(project, session)
}

fn main_key() -> SessionKey {
    key("proj", "sess")
}

fn sub(base: &SessionKey, subpath: &str) -> SessionKey {
    base.clone().with_subpath(subpath)
}

fn subkeys_of(base: &SessionKey) -> SessionListSubkeysKey {
    SessionListSubkeysKey::new(base.project_key.clone(), base.session_id.clone())
}

const MTIME_FLOOR: i64 = 1_000_000_000_000;

/// The optional methods an empty store implements: each is called once,
/// and only [`UnsupportedStoreOperation`](crate::UnsupportedStoreOperation)
/// marks it absent.
async fn implemented_optional_methods(store: &impl SessionStore) -> SessionStoreCapabilities {
    fn present<T>(result: &Result<T, SessionStoreError>) -> bool {
        !matches!(result, Err(error) if is_unsupported(error))
    }
    let probe = key("probe-project", "probe-session");
    SessionStoreCapabilities {
        list_sessions: present(&store.list_sessions(&probe.project_key).await),
        list_session_summaries: present(&store.list_session_summaries(&probe.project_key).await),
        delete: present(&store.delete(&probe).await),
        list_subkeys: present(&store.list_subkeys(&subkeys_of(&probe)).await),
    }
}

/// Checks the 14 behavioral contracts of
/// `src/claude_agent_sdk/testing/session_store_conformance.py` of the
/// upstream Python SDK against a store.
///
/// `make_store` is called once per contract, so each starts from an empty
/// store. The contracts of an optional method run when the store declares
/// it in [`SessionStore::capabilities`] or implements it, that is when a
/// call on an empty store does not return the
/// [`UnsupportedStoreOperation`](crate::UnsupportedStoreOperation) of the
/// default implementation; a declared method left unimplemented breaks its
/// first contract. Returns the first contract broken. As upstream reads the
/// summaries into a dict keyed by session id, a summary listed twice counts
/// once, the last one.
///
/// ```
/// use claude_agent_rust_sdk::InMemorySessionStore;
/// use claude_agent_rust_sdk::testing::run_session_store_conformance;
///
/// # async fn demo() {
/// run_session_store_conformance(|| async { InMemorySessionStore::new() })
///     .await
///     .unwrap();
/// # }
/// ```
///
/// # Errors
///
/// Returns the first [`ConformanceFailure`].
pub async fn run_session_store_conformance<S, F, Fut>(
    make_store: F,
) -> Result<(), ConformanceFailure>
where
    S: SessionStore,
    F: FnMut() -> Fut,
    Fut: Future<Output = S>,
{
    run_session_store_conformance_skipping(make_store, SessionStoreCapabilities::none()).await
}

/// [`run_session_store_conformance`], also skipping the contracts of the
/// optional methods set in `skip_optional`.
///
/// # Errors
///
/// Returns the first [`ConformanceFailure`].
#[allow(
    clippy::too_many_lines,
    reason = "the contracts run in upstream's order, one after the other, so each one reads \
              against its upstream counterpart"
)]
pub async fn run_session_store_conformance_skipping<S, F, Fut>(
    mut make_store: F,
    skip_optional: SessionStoreCapabilities,
) -> Result<(), ConformanceFailure>
where
    S: SessionStore,
    F: FnMut() -> Fut,
    Fut: Future<Output = S>,
{
    let probe = make_store().await;
    let declared = probe.capabilities();
    let implemented = implemented_optional_methods(&probe).await;
    let has_list_sessions =
        (declared.list_sessions || implemented.list_sessions) && !skip_optional.list_sessions;
    let has_summaries = (declared.list_session_summaries || implemented.list_session_summaries)
        && !skip_optional.list_session_summaries;
    let has_delete = (declared.delete || implemented.delete) && !skip_optional.delete;
    let has_subkeys =
        (declared.list_subkeys || implemented.list_subkeys) && !skip_optional.list_subkeys;
    let main = main_key();

    let store = make_store().await;
    let expected = vec![
        entry(json!({"uuid": "b", "n": 1})),
        entry(json!({"uuid": "a", "n": 2})),
    ];
    call(1, "append", store.append(&main, &expected).await)?;
    let loaded = call(1, "load", store.load(&main).await)?;
    check(1, loaded.as_ref() == Some(&expected), || {
        format!("load after append returned {loaded:?}")
    })?;

    let store = make_store().await;
    let unknown = call(2, "load", store.load(&key("proj", "nope")).await)?;
    check(2, unknown.is_none(), || {
        format!("load of an unknown key returned {unknown:?}")
    })?;
    call(
        2,
        "append",
        store
            .append(&main, &[entry(json!({"uuid": "x", "n": 1}))])
            .await,
    )?;
    let unknown = call(2, "load", store.load(&sub(&main, "nope")).await)?;
    check(2, unknown.is_none(), || {
        format!("load of an unknown subpath returned {unknown:?}")
    })?;

    let store = make_store().await;
    let batches = [
        vec![entry(json!({"uuid": "z", "n": 1}))],
        vec![
            entry(json!({"uuid": "a", "n": 2})),
            entry(json!({"uuid": "m", "n": 3})),
        ],
        vec![entry(json!({"uuid": "b", "n": 4}))],
    ];
    for batch in &batches {
        call(3, "append", store.append(&main, batch).await)?;
    }
    let loaded = call(3, "load", store.load(&main).await)?;
    let all: Vec<SessionStoreEntry> = batches.concat();
    check(3, loaded.as_ref() == Some(&all), || {
        format!("appends were not kept in call order: {loaded:?}")
    })?;

    let store = make_store().await;
    let single = vec![entry(json!({"uuid": "a", "n": 1}))];
    call(4, "append", store.append(&main, &single).await)?;
    call(4, "append", store.append(&main, &[]).await)?;
    let loaded = call(4, "load", store.load(&main).await)?;
    check(4, loaded.as_ref() == Some(&single), || {
        format!("an empty append changed the content: {loaded:?}")
    })?;

    let store = make_store().await;
    let subagent = sub(&main, "subagents/agent-1");
    let main_entries = vec![entry(json!({"uuid": "m", "n": 1}))];
    let sub_entries = vec![entry(json!({"uuid": "s", "n": 1}))];
    call(5, "append", store.append(&main, &main_entries).await)?;
    call(5, "append", store.append(&subagent, &sub_entries).await)?;
    let loaded_main = call(5, "load", store.load(&main).await)?;
    let loaded_sub = call(5, "load", store.load(&subagent).await)?;
    check(
        5,
        loaded_main.as_ref() == Some(&main_entries) && loaded_sub.as_ref() == Some(&sub_entries),
        || format!("subpath and main were not independent: {loaded_main:?} / {loaded_sub:?}"),
    )?;

    let store = make_store().await;
    let from_a = vec![entry(json!({"from": "A"}))];
    let from_b = vec![entry(json!({"from": "B"}))];
    call(6, "append", store.append(&key("A", "s1"), &from_a).await)?;
    call(6, "append", store.append(&key("B", "s1"), &from_b).await)?;
    let loaded_a = call(6, "load", store.load(&key("A", "s1")).await)?;
    let loaded_b = call(6, "load", store.load(&key("B", "s1")).await)?;
    check(
        6,
        loaded_a.as_ref() == Some(&from_a) && loaded_b.as_ref() == Some(&from_b),
        || format!("projects were not isolated: {loaded_a:?} / {loaded_b:?}"),
    )?;
    if has_list_sessions {
        let listed_a = call(6, "list_sessions", store.list_sessions("A").await)?;
        let listed_b = call(6, "list_sessions", store.list_sessions("B").await)?;
        check(6, listed_a.len() == 1 && listed_b.len() == 1, || {
            format!("list_sessions mixed projects: {listed_a:?} / {listed_b:?}")
        })?;
    }

    if has_list_sessions {
        let store = make_store().await;
        for (project, session) in [("proj", "a"), ("proj", "b"), ("other", "c")] {
            call(
                7,
                "append",
                store
                    .append(&key(project, session), &[entry(json!({"n": 1}))])
                    .await,
            )?;
        }
        let sessions = call(7, "list_sessions", store.list_sessions("proj").await)?;
        let mut ids: Vec<&str> = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        ids.sort_unstable();
        check(7, ids == ["a", "b"], || {
            format!("list_sessions returned {ids:?}")
        })?;
        check(
            7,
            sessions.iter().all(|session| session.mtime > MTIME_FLOOR),
            || format!("mtime is not in epoch milliseconds: {sessions:?}"),
        )?;
        let none = call(
            7,
            "list_sessions",
            store.list_sessions("never-appended-project").await,
        )?;
        check(7, none.is_empty(), || {
            format!("an unknown project listed {none:?}")
        })?;

        let store = make_store().await;
        call(
            8,
            "append",
            store
                .append(&key("proj", "main"), &[entry(json!({"n": 1}))])
                .await,
        )?;
        call(
            8,
            "append",
            store
                .append(
                    &sub(&key("proj", "main"), "subagents/agent-1"),
                    &[entry(json!({"n": 1}))],
                )
                .await,
        )?;
        let sessions = call(8, "list_sessions", store.list_sessions("proj").await)?;
        let ids: Vec<&str> = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        check(8, ids == ["main"], || {
            format!("list_sessions included subpaths: {ids:?}")
        })?;
    }

    if has_summaries {
        let store = make_store().await;
        let summary_key = key("proj", "summ-sess");
        call(14, "append", store.append(&summary_key, &[
            entry(json!({"timestamp": "2024-01-01T00:00:00.000Z", "customTitle": "first"})),
            entry(json!({"timestamp": "2024-01-01T00:00:01.000Z"})),
        ]).await)?;
        call(
            14,
            "append",
            store
                .append(
                    &summary_key,
                    &[entry(
                        json!({"timestamp": "2024-01-01T00:00:02.000Z", "customTitle": "second"}),
                    )],
                )
                .await,
        )?;
        call(
            14,
            "append",
            store
                .append(
                    &key("other", "elsewhere"),
                    &[entry(json!({"timestamp": "2024-01-01T00:00:00.000Z"}))],
                )
                .await,
        )?;
        let summaries = call(
            14,
            "list_session_summaries",
            store.list_session_summaries("proj").await,
        )?;
        let mut ids: Vec<&str> = summaries
            .iter()
            .map(|summary| summary.session_id.as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        check(14, ids == ["summ-sess"], || {
            format!("list_session_summaries returned {ids:?}")
        })?;
        let Some(summary) = summaries
            .iter()
            .rev()
            .find(|summary| summary.session_id == "summ-sess")
            .cloned()
        else {
            return Err(fail(14, format!("list_session_summaries returned {ids:?}")));
        };
        check(14, summary.mtime > MTIME_FLOOR, || {
            format!(
                "summary mtime is not in epoch milliseconds: {}",
                summary.mtime
            )
        })?;
        if has_list_sessions {
            let listed = call(14, "list_sessions", store.list_sessions("proj").await)?;
            let listed_mtime = listed
                .iter()
                .rev()
                .find(|entry| entry.session_id == "summ-sess")
                .map(|entry| entry.mtime);
            check(
                14,
                listed_mtime.is_some_and(|mtime| summary.mtime >= mtime),
                || {
                    format!(
                        "summary mtime {} is older than the list_sessions mtime {listed_mtime:?}",
                        summary.mtime
                    )
                },
            )?;
        }
        let refolded = fold_session_summary(
            Some(&summary),
            &summary_key,
            &[entry(json!({"timestamp": "2024-01-01T00:00:03.000Z"}))],
        );
        check(
            14,
            refolded.session_id == "summ-sess" && refolded.mtime == summary.mtime,
            || format!("the summary did not fold again: {refolded:?}"),
        )?;
        call(
            14,
            "append",
            store
                .append(
                    &sub(&summary_key, "subagents/agent-1"),
                    &[entry(
                        json!({"timestamp": "2024-01-01T00:00:09.000Z", "customTitle": "subagent"}),
                    )],
                )
                .await,
        )?;
        let after = call(
            14,
            "list_session_summaries",
            store.list_session_summaries("proj").await,
        )?;
        let data_after = after
            .iter()
            .rev()
            .find(|entry| entry.session_id == "summ-sess")
            .map(|entry| &entry.data);
        check(14, data_after == Some(&summary.data), || {
            format!("a subagent append changed the summary: {data_after:?}")
        })?;
        let none = call(
            14,
            "list_session_summaries",
            store.list_session_summaries("never-appended-project").await,
        )?;
        check(14, none.is_empty(), || {
            format!("an unknown project had summaries: {none:?}")
        })?;
        if has_delete {
            call(14, "delete", store.delete(&summary_key).await)?;
            let left = call(
                14,
                "list_session_summaries",
                store.list_session_summaries("proj").await,
            )?;
            check(14, left.is_empty(), || {
                format!("delete left summaries: {left:?}")
            })?;
        }
    }

    if has_delete {
        let store = make_store().await;
        call(
            9,
            "delete",
            store.delete(&key("proj", "never-written")).await,
        )?;
        call(
            9,
            "append",
            store.append(&main, &[entry(json!({"n": 1}))]).await,
        )?;
        call(9, "delete", store.delete(&main).await)?;
        let loaded = call(9, "load", store.load(&main).await)?;
        check(9, loaded.is_none(), || {
            format!("load after delete returned {loaded:?}")
        })?;

        let store = make_store().await;
        let sub1 = sub(&main, "subagents/agent-1");
        let sub2 = sub(&main, "subagents/agent-2");
        let other = key("proj", "sess2");
        let other_project = key("other-proj", "sess");
        for target in [&main, &sub1, &sub2, &other, &other_project] {
            call(
                10,
                "append",
                store.append(target, &[entry(json!({"n": 1}))]).await,
            )?;
        }
        call(10, "delete", store.delete(&main).await)?;
        for gone in [&main, &sub1, &sub2] {
            let loaded = call(10, "load", store.load(gone).await)?;
            check(10, loaded.is_none(), || {
                format!("{gone:?} survived the delete of its session: {loaded:?}")
            })?;
        }
        for kept in [&other, &other_project] {
            let loaded = call(10, "load", store.load(kept).await)?;
            check(10, loaded.is_some_and(|entries| entries.len() == 1), || {
                format!("{kept:?} was deleted too")
            })?;
        }
        if has_subkeys {
            let left = call(
                10,
                "list_subkeys",
                store.list_subkeys(&subkeys_of(&main)).await,
            )?;
            check(10, left.is_empty(), || {
                format!("subkeys survived the delete: {left:?}")
            })?;
        }
        if has_list_sessions {
            let listed = call(10, "list_sessions", store.list_sessions("proj").await)?;
            check(
                10,
                listed.iter().all(|entry| entry.session_id != "sess"),
                || format!("the deleted session is still listed: {listed:?}"),
            )?;
        }

        let store = make_store().await;
        for target in [&main, &sub1, &sub2] {
            call(
                11,
                "append",
                store.append(target, &[entry(json!({"n": 1}))]).await,
            )?;
        }
        call(11, "delete", store.delete(&sub1).await)?;
        let loaded = call(11, "load", store.load(&sub1).await)?;
        check(11, loaded.is_none(), || {
            format!("the deleted subkey is still there: {loaded:?}")
        })?;
        for kept in [&sub2, &main] {
            let loaded = call(11, "load", store.load(kept).await)?;
            check(11, loaded.is_some_and(|entries| entries.len() == 1), || {
                format!("{kept:?} was deleted with a subkey")
            })?;
        }
        if has_subkeys {
            let left = call(
                11,
                "list_subkeys",
                store.list_subkeys(&subkeys_of(&main)).await,
            )?;
            check(11, left == ["subagents/agent-2"], || {
                format!("list_subkeys returned {left:?}")
            })?;
        }
    }

    if has_subkeys {
        let store = make_store().await;
        call(
            12,
            "append",
            store.append(&main, &[entry(json!({"n": 1}))]).await,
        )?;
        call(
            12,
            "append",
            store
                .append(&sub(&main, "subagents/agent-1"), &[entry(json!({"n": 1}))])
                .await,
        )?;
        call(
            12,
            "append",
            store
                .append(&sub(&main, "subagents/agent-2"), &[entry(json!({"n": 1}))])
                .await,
        )?;
        call(
            12,
            "append",
            store
                .append(
                    &sub(&key("proj", "other-sess"), "subagents/agent-x"),
                    &[entry(json!({"n": 1}))],
                )
                .await,
        )?;
        let mut subkeys = call(
            12,
            "list_subkeys",
            store.list_subkeys(&subkeys_of(&main)).await,
        )?;
        subkeys.sort_unstable();
        check(
            12,
            subkeys == ["subagents/agent-1", "subagents/agent-2"],
            || format!("list_subkeys returned {subkeys:?}"),
        )?;

        let store = make_store().await;
        call(
            13,
            "append",
            store.append(&main, &[entry(json!({"n": 1}))]).await,
        )?;
        let subkeys = call(
            13,
            "list_subkeys",
            store.list_subkeys(&subkeys_of(&main)).await,
        )?;
        check(13, subkeys.is_empty(), || {
            format!("list_subkeys included the main transcript: {subkeys:?}")
        })?;
        let none = call(
            13,
            "list_subkeys",
            store
                .list_subkeys(&SessionListSubkeysKey::new("proj", "never-appended"))
                .await,
        )?;
        check(13, none.is_empty(), || {
            format!("an unknown session had subkeys: {none:?}")
        })?;
    }
    Ok(())
}
