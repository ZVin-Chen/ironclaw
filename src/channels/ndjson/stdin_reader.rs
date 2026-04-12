//! Read NDJSON from stdin (or any `AsyncBufRead`) and route parsed lines into
//! the channel's `IncomingMessage` stream and approval state.


use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{Mutex, mpsc};

use crate::channels::IncomingMessage;
use crate::channels::ndjson::approval::ApprovalState;
use crate::channels::ndjson::types::{ControlResponsePayload, NdjsonInput};

/// Shared handle to the currently-resolved session id.
pub type SessionIdSlot = Arc<Mutex<Option<String>>>;

/// What the reader should do when it sees a `user` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserLinePolicy {
    /// Accept `user` lines and forward them as `IncomingMessage`s.
    Accept,
    /// Drop `user` lines (used by `-p` print mode after the first prompt).
    Drop,
}

/// Outcome emitted when the reader parses a line.
#[derive(Debug)]
pub enum ReaderEvent {
    InjectMessage(IncomingMessage),
    ResolveApproval(String, ControlResponsePayload),
    Interrupt(IncomingMessage),
    ParseError { line: String, reason: String },
}

/// Parse a single stdin line into a `ReaderEvent` (if any).
pub fn parse_line(
    line: &str,
    user_id: &str,
    session_id: Option<&str>,
    user_policy: UserLinePolicy,
) -> Option<ReaderEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    match serde_json::from_str::<NdjsonInput>(trimmed) {
        Ok(NdjsonInput::User { content }) => {
            if user_policy == UserLinePolicy::Drop {
                return None;
            }
            let text = match &content {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let mut msg = IncomingMessage::new("ndjson", user_id, text);
            if let Some(scope) = session_id {
                msg = msg.with_conversation_scope(scope);
            }
            Some(ReaderEvent::InjectMessage(msg))
        }
        Ok(NdjsonInput::ControlResponse {
            request_id,
            response,
        }) => Some(ReaderEvent::ResolveApproval(request_id, response)),
        Ok(NdjsonInput::Interrupt) => {
            let mut msg = IncomingMessage::new("ndjson", user_id, "/interrupt");
            if let Some(scope) = session_id {
                msg = msg.with_conversation_scope(scope);
            }
            Some(ReaderEvent::Interrupt(msg))
        }
        Err(e) => Some(ReaderEvent::ParseError {
            line: trimmed.into(),
            reason: e.to_string(),
        }),
    }
}

/// Run the reader loop to completion on the given stream.
pub async fn run_reader<R>(
    reader: R,
    user_id: String,
    session_id: SessionIdSlot,
    approvals: ApprovalState,
    msg_tx: mpsc::Sender<IncomingMessage>,
    user_policy: UserLinePolicy,
    on_parse_error: impl Fn(String, String) + Send + Sync,
) where
    R: AsyncRead + Unpin + Send,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!("ndjson stdin read error: {}", e);
                break;
            }
        };
        let sid = session_id.lock().await.clone();
        let event = parse_line(&line, &user_id, sid.as_deref(), user_policy);
        match event {
            Some(ReaderEvent::InjectMessage(msg)) | Some(ReaderEvent::Interrupt(msg)) => {
                if msg_tx.send(msg).await.is_err() {
                    break;
                }
            }
            Some(ReaderEvent::ResolveApproval(request_id, response)) => {
                approvals.resolve(&request_id, response).await;
            }
            Some(ReaderEvent::ParseError { line, reason }) => {
                on_parse_error(line, reason);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lines_are_ignored() {
        let out = parse_line("   \n", "user", None, UserLinePolicy::Accept);
        assert!(out.is_none());
    }

    #[test]
    fn user_string_content_becomes_incoming_message() {
        let out = parse_line(
            r#"{"type":"user","content":"hello"}"#,
            "owner",
            Some("sess-123"),
            UserLinePolicy::Accept,
        )
        .expect("event expected");
        match out {
            ReaderEvent::InjectMessage(msg) => {
                assert_eq!(msg.channel, "ndjson");
                assert_eq!(msg.user_id, "owner");
                assert_eq!(msg.content, "hello");
                assert_eq!(msg.conversation_scope_id.as_deref(), Some("sess-123"));
            }
            _ => panic!("expected InjectMessage"),
        }
    }

    #[test]
    fn user_drop_policy_suppresses_subsequent_messages() {
        let out = parse_line(
            r#"{"type":"user","content":"hi"}"#,
            "owner",
            None,
            UserLinePolicy::Drop,
        );
        assert!(out.is_none());
    }

    #[test]
    fn interrupt_becomes_interrupt_slash_command() {
        let out = parse_line(
            r#"{"type":"interrupt"}"#,
            "owner",
            None,
            UserLinePolicy::Accept,
        )
        .expect("event expected");
        match out {
            ReaderEvent::Interrupt(msg) => {
                assert_eq!(msg.content, "/interrupt");
            }
            _ => panic!("expected Interrupt"),
        }
    }

    #[test]
    fn control_response_routes_to_approval_state() {
        let out = parse_line(
            r#"{"type":"control_response","request_id":"req-1","response":{"behavior":"allow"}}"#,
            "owner",
            None,
            UserLinePolicy::Accept,
        )
        .expect("event expected");
        match out {
            ReaderEvent::ResolveApproval(id, payload) => {
                assert_eq!(id, "req-1");
                assert!(matches!(payload, ControlResponsePayload::Allow { .. }));
            }
            _ => panic!("expected ResolveApproval"),
        }
    }

    #[test]
    fn malformed_json_becomes_parse_error() {
        let out = parse_line("not a json line", "owner", None, UserLinePolicy::Accept)
            .expect("event expected");
        match out {
            ReaderEvent::ParseError { line, reason } => {
                assert_eq!(line, "not a json line");
                assert!(!reason.is_empty());
            }
            _ => panic!("expected ParseError"),
        }
    }

    #[tokio::test]
    async fn run_reader_processes_lines_from_in_memory_buffer() {
        let input = b"{\"type\":\"user\",\"content\":\"hello\"}\n{\"type\":\"interrupt\"}\n";
        let cursor = std::io::Cursor::new(input.to_vec());
        let (tx, mut rx) = mpsc::channel(4);
        let slot: SessionIdSlot = Arc::new(Mutex::new(None));
        let approvals = ApprovalState::new();
        let errors = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let errors_clone = Arc::clone(&errors);

        run_reader(
            cursor,
            "owner".into(),
            slot,
            approvals,
            tx,
            UserLinePolicy::Accept,
            move |line, reason| {
                let errors = Arc::clone(&errors_clone);
                tokio::spawn(async move {
                    errors.lock().await.push((line, reason));
                });
            },
        )
        .await;

        let first = rx.recv().await.expect("first message");
        assert_eq!(first.content, "hello");
        let second = rx.recv().await.expect("second message");
        assert_eq!(second.content, "/interrupt");
        assert!(rx.recv().await.is_none(), "stream should close at EOF");
    }
}
