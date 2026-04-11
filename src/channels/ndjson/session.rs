//! Resolve a session id from CLI flags.
//!
//! - `--session-id <uuid>` is validated as a UUID and passed through unchanged.
//! - `--resume` queries the most recent conversation for the user. If there is
//!   no prior conversation, returns `None` (a fresh session is acceptable).
//! - Neither flag → `None`.

use uuid::Uuid;

use crate::db::ConversationStore;
use crate::error::ChannelError;

/// Arguments describing which session (if any) to resume.
#[derive(Debug, Clone, Default)]
pub struct SessionResolveArgs {
    pub session_id: Option<String>,
    pub resume_latest: bool,
}

/// Resolve a session id from arguments. Returns `Ok(Some(uuid_str))` when a
/// session should be loaded, `Ok(None)` when the caller should start fresh.
///
/// The store is passed as a plain reference — callers can supply any type
/// implementing `ConversationStore`.
pub async fn resolve_session_id(
    args: &SessionResolveArgs,
    user_id: &str,
    store: Option<&dyn ConversationStore>,
) -> Result<Option<String>, ChannelError> {
    if let Some(ref id) = args.session_id {
        Uuid::parse_str(id)
            .map_err(|e| ChannelError::Other(format!("invalid session-id UUID: {}", e)))?;
        return Ok(Some(id.clone()));
    }

    if args.resume_latest
        && let Some(store) = store
    {
        let recent = store
            .list_conversations_all_channels(user_id, 1)
            .await
            .map_err(|e| ChannelError::Other(format!("failed to query conversations: {}", e)))?;
        if let Some(first) = recent.first() {
            return Ok(Some(first.id.to_string()));
        }
        // No history — fall through to new session.
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn neither_flag_returns_none() {
        let args = SessionResolveArgs::default();
        let result = resolve_session_id(&args, "user", None).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn valid_session_id_passes_through() {
        let uuid = Uuid::new_v4().to_string();
        let args = SessionResolveArgs {
            session_id: Some(uuid.clone()),
            resume_latest: false,
        };
        let result = resolve_session_id(&args, "user", None).await.unwrap();
        assert_eq!(result, Some(uuid));
    }

    #[tokio::test]
    async fn invalid_session_id_returns_channel_error() {
        let args = SessionResolveArgs {
            session_id: Some("not-a-uuid".into()),
            resume_latest: false,
        };
        let result = resolve_session_id(&args, "user", None).await;
        assert!(matches!(result, Err(ChannelError::Other(_))));
    }

    #[tokio::test]
    async fn resume_without_store_returns_none() {
        let args = SessionResolveArgs {
            session_id: None,
            resume_latest: true,
        };
        let result = resolve_session_id(&args, "user", None).await.unwrap();
        assert!(result.is_none());
    }
}
