//! Correlation between outgoing `control_request` events and incoming
//! `control_response` replies.
//!
//! `ApprovalState` is a small map of `request_id → oneshot::Sender`. When the
//! channel emits a `control_request`, it first calls [`register`] to obtain a
//! `oneshot::Receiver`. The stdin reader calls [`resolve`] when a matching
//! `control_response` line arrives. Unknown request ids are silently dropped
//! (the request may have been resolved by an interrupt or timed out).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};

use crate::channels::ndjson::types::ControlResponsePayload;

/// Shared state for tracking pending approvals.
#[derive(Debug, Default, Clone)]
pub struct ApprovalState {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<ControlResponsePayload>>>>,
}

impl ApprovalState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending approval. Returns the receiver the caller should
    /// `await` for the response.
    pub async fn register(&self, request_id: String) -> oneshot::Receiver<ControlResponsePayload> {
        let (tx, rx) = oneshot::channel();
        let mut guard = self.inner.lock().await;
        guard.insert(request_id, tx);
        rx
    }

    /// Deliver a response for a previously registered request. Returns `true`
    /// if the request was found and the response delivered.
    pub async fn resolve(&self, request_id: &str, response: ControlResponsePayload) -> bool {
        let mut guard = self.inner.lock().await;
        if let Some(tx) = guard.remove(request_id) {
            tx.send(response).is_ok()
        } else {
            false
        }
    }

    /// Cancel all pending approvals (e.g. on interrupt).
    pub async fn clear(&self) {
        let mut guard = self.inner.lock().await;
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_then_resolve_delivers_response() {
        let state = ApprovalState::new();
        let rx = state.register("req-1".into()).await;
        let delivered = state
            .resolve(
                "req-1",
                ControlResponsePayload::Allow {
                    updated_input: None,
                },
            )
            .await;
        assert!(delivered);
        let response = rx.await.expect("response should arrive");
        assert!(matches!(response, ControlResponsePayload::Allow { .. }));
    }

    #[tokio::test]
    async fn resolve_unknown_request_returns_false() {
        let state = ApprovalState::new();
        let delivered = state
            .resolve("missing", ControlResponsePayload::Always)
            .await;
        assert!(!delivered);
    }

    #[tokio::test]
    async fn clear_drops_pending_requests() {
        let state = ApprovalState::new();
        let rx = state.register("req-1".into()).await;
        state.clear().await;
        // The sender is dropped, so the receiver gets a cancellation error.
        let result = rx.await;
        assert!(
            result.is_err(),
            "dropped sender should surface as RecvError"
        );
    }

    #[tokio::test]
    async fn two_concurrent_registrations_are_independent() {
        let state = ApprovalState::new();
        let rx_a = state.register("req-a".into()).await;
        let rx_b = state.register("req-b".into()).await;

        state
            .resolve(
                "req-b",
                ControlResponsePayload::Deny {
                    message: Some("nope".into()),
                },
            )
            .await;
        let b = rx_b.await.unwrap();
        assert!(matches!(b, ControlResponsePayload::Deny { .. }));

        state
            .resolve(
                "req-a",
                ControlResponsePayload::Allow {
                    updated_input: None,
                },
            )
            .await;
        let a = rx_a.await.unwrap();
        assert!(matches!(a, ControlResponsePayload::Allow { .. }));
    }
}
