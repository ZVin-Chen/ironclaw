//! Integration test: NDJSON session resolution against a real libSQL store.

#![cfg(feature = "libsql")]

use std::sync::Arc;
use std::time::Duration;

use ironclaw::channels::ndjson::{SessionResolveArgs, resolve_session_id};
use ironclaw::db::libsql::LibSqlBackend;
use ironclaw::db::{ConversationStore, Database};
use tempfile::TempDir;

/// Build a file-backed libSQL backend with migrations applied.
///
/// File-backed (not `:memory:`) is required because `LibSqlBackend::connect()`
/// creates a fresh connection per operation, and in-memory SQLite databases do
/// not share state between connections. See `src/db/CLAUDE.md`.
async fn new_backend() -> (Arc<LibSqlBackend>, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("ndjson_session_test.db");
    let backend = LibSqlBackend::new_local(&db_path)
        .await
        .expect("new local libSQL backend");
    backend.run_migrations().await.expect("run migrations");
    (Arc::new(backend), dir)
}

#[tokio::test]
async fn resume_picks_latest_conversation_for_user() {
    let (backend, _dir) = new_backend().await;
    let store: &dyn ConversationStore = &*backend;

    let user = "alice";
    let id_old = store
        .create_conversation("ndjson", user, None)
        .await
        .expect("create old");
    // SQLite's `datetime(...)` used in the ORDER BY drops sub-second
    // precision, so we need at least a 1-second gap to make the
    // "most recent conversation" ordering deterministic.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let id_new = store
        .create_conversation("ndjson", user, None)
        .await
        .expect("create new");
    store.touch_conversation(id_new).await.expect("touch new");

    let args = SessionResolveArgs {
        session_id: None,
        resume_latest: true,
    };
    let resolved = resolve_session_id(&args, user, Some(store))
        .await
        .expect("resolve ok");
    assert_eq!(
        resolved.as_deref(),
        Some(id_new.to_string().as_str()),
        "most recent conversation (id_new) should be resumed, not id_old={}",
        id_old
    );
}

#[tokio::test]
async fn resume_with_no_history_returns_none() {
    let (backend, _dir) = new_backend().await;
    let store: &dyn ConversationStore = &*backend;

    let args = SessionResolveArgs {
        session_id: None,
        resume_latest: true,
    };
    let resolved = resolve_session_id(&args, "ghost", Some(store))
        .await
        .expect("resolve ok");
    assert!(resolved.is_none());
}

#[tokio::test]
async fn invalid_session_id_returns_channel_error() {
    let args = SessionResolveArgs {
        session_id: Some("not-a-uuid".into()),
        resume_latest: false,
    };
    let result = resolve_session_id(&args, "user", None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn valid_session_id_passes_through_even_without_store() {
    let uuid = uuid::Uuid::new_v4().to_string();
    let args = SessionResolveArgs {
        session_id: Some(uuid.clone()),
        resume_latest: false,
    };
    let resolved = resolve_session_id(&args, "user", None)
        .await
        .expect("resolve ok");
    assert_eq!(resolved, Some(uuid));
}
