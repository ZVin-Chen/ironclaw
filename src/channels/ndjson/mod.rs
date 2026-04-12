//! NDJSON CLI channel — subprocess-friendly stdin/stdout protocol.
//!
//! See the design spec at `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`.

mod approval;
mod compat;
mod session;
mod stdin_reader;
mod types;

// Public API for external consumers (main.rs, integration tests).
pub use session::{SessionResolveArgs, resolve_session_id};
pub use types::{CompatMode, EventFilter};

// Crate-internal helpers.
pub(crate) use approval::ApprovalState;
pub(crate) use compat::to_claude_code;
pub(crate) use stdin_reader::{SessionIdSlot, UserLinePolicy, run_reader};
pub(crate) use types::{
    AssistantMessage, ControlRequestPayload, ControlResponsePayload, NdjsonOutput, ResultEvent,
    ResultSubtype, SystemEvent, ToolDecisionDto, UsageTotals,
};

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::io::stdin;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::channels::{
    Channel, IncomingMessage, MessageStream, OutgoingResponse, StatusUpdate,
};
use crate::db::{ConversationStore, Database};
use crate::error::ChannelError;

/// Configuration for a `NdjsonChannel`.
#[derive(Debug, Clone)]
pub struct NdjsonChannelConfig {
    pub user_id: String,
    pub initial_prompt: Option<String>,
    pub streaming_input: bool,
    pub compat_mode: CompatMode,
    pub verbose: bool,
    pub include_events: EventFilter,
    pub tools: Vec<String>,
    pub model_name: String,
    pub session_id_arg: Option<String>,
    pub resume_latest: bool,
}

/// Running NDJSON channel state.
pub struct NdjsonChannel {
    config: NdjsonChannelConfig,
    session_id: SessionIdSlot,
    pending_approvals: ApprovalState,
    msg_tx: Arc<Mutex<Option<mpsc::Sender<IncomingMessage>>>>,
    /// Stored as the full `Database` supertrait so we can reuse the same
    /// `Arc` that `AppComponents::db` holds without needing to downcast to a
    /// more specific sub-trait. We only need `ConversationStore` methods.
    database: Option<Arc<dyn Database>>,
    stdout_lock: Arc<std::sync::Mutex<()>>,
    turn_usage: Arc<Mutex<TurnUsage>>,
    turn_start: Arc<Mutex<Option<Instant>>>,
}

/// Per-turn accumulation used to build the final `result` event.
#[derive(Debug, Default, Clone)]
struct TurnUsage {
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: String,
    num_turns: u32,
}

/// Write a single NDJSON line to stdout. Thread-safe via the provided lock.
fn write_ndjson_line(
    lock: &std::sync::Mutex<()>,
    compat_mode: CompatMode,
    event: &NdjsonOutput,
) {
    let value = match compat_mode {
        CompatMode::Ironclaw => match serde_json::to_value(event) {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!("ndjson serialize error: {}", e);
                return;
            }
        },
        CompatMode::ClaudeCode => to_claude_code(event),
    };

    if value.is_null() {
        return;
    }

    let line = match serde_json::to_string(&value) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("ndjson serialize error: {}", e);
            return;
        }
    };

    let Ok(_guard) = lock.lock() else {
        return;
    };
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if writeln!(handle, "{line}").is_ok() {
        let _ = handle.flush();
    }
}

impl NdjsonChannel {
    pub fn new(config: NdjsonChannelConfig, database: Option<Arc<dyn Database>>) -> Self {
        Self {
            config,
            session_id: Arc::new(Mutex::new(None)),
            pending_approvals: ApprovalState::new(),
            msg_tx: Arc::new(Mutex::new(None)),
            database,
            stdout_lock: Arc::new(std::sync::Mutex::new(())),
            turn_usage: Arc::new(Mutex::new(TurnUsage::default())),
            turn_start: Arc::new(Mutex::new(None)),
        }
    }

    /// Borrow the database as a `ConversationStore` (via supertrait
    /// upcasting). Returns `None` when no database was configured.
    fn conversation_store(&self) -> Option<&dyn ConversationStore> {
        self.database
            .as_deref()
            .map(|db| db as &dyn ConversationStore)
    }

    /// Write one NDJSON line to stdout, delegating to [`write_ndjson_line`].
    fn emit(&self, event: &NdjsonOutput) {
        write_ndjson_line(&self.stdout_lock, self.config.compat_mode, event);
    }

    async fn current_session_id(&self) -> String {
        self.session_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| "pending".to_string())
    }
}

#[async_trait]
impl Channel for NdjsonChannel {
    fn name(&self) -> &str {
        "ndjson"
    }

    async fn start(&self) -> Result<MessageStream, ChannelError> {
        let (tx, rx) = mpsc::channel::<IncomingMessage>(32);
        *self.msg_tx.lock().await = Some(tx.clone());

        // Resolve session id if flags request it.
        let resolve_args = SessionResolveArgs {
            session_id: self.config.session_id_arg.clone(),
            resume_latest: self.config.resume_latest,
        };
        let resolved = resolve_session_id(
            &resolve_args,
            &self.config.user_id,
            self.conversation_store(),
        )
        .await?;
        let resumed = resolved.is_some();
        let session_id_str = resolved
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        *self.session_id.lock().await = Some(session_id_str.clone());

        // Emit init event.
        let cwd = std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string());
        self.emit(&NdjsonOutput::System(SystemEvent::Init {
            session_id: session_id_str.clone(),
            resumed,
            model: self.config.model_name.clone(),
            tools: self.config.tools.clone(),
            cwd,
            protocol_version: "1".into(),
        }));

        // Spawn the stdin reader (streaming input or interrupt-only).
        let user_policy = if self.config.initial_prompt.is_some() && !self.config.streaming_input {
            UserLinePolicy::Drop
        } else {
            UserLinePolicy::Accept
        };
        if self.config.streaming_input || self.config.initial_prompt.is_some() {
            let user_id = self.config.user_id.clone();
            let session_slot = Arc::clone(&self.session_id);
            let approvals = self.pending_approvals.clone();
            let tx_for_reader = tx.clone();
            let stdout_lock = Arc::clone(&self.stdout_lock);
            let compat_mode = self.config.compat_mode;

            tokio::spawn(async move {
                run_reader(
                    stdin(),
                    user_id,
                    session_slot,
                    approvals,
                    tx_for_reader,
                    user_policy,
                    move |line, reason| {
                        let err = NdjsonOutput::Error {
                            session_id: "pending".into(),
                            message: format!("parse error: {} ({})", reason, line),
                        };
                        write_ndjson_line(&stdout_lock, compat_mode, &err);
                    },
                )
                .await;
            });
        }

        // Inject the initial prompt after the reader spawns so that ordering
        // is deterministic.
        if let Some(ref prompt) = self.config.initial_prompt {
            let mut msg = IncomingMessage::new("ndjson", &self.config.user_id, prompt)
                .with_metadata(serde_json::json!({
                    "single_message_mode": !self.config.streaming_input,
                }));
            if let Some(ref sid) = *self.session_id.lock().await {
                msg = msg.with_conversation_scope(sid);
            }
            let _ = tx.send(msg).await;
        }

        *self.turn_start.lock().await = Some(Instant::now());
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn respond(
        &self,
        _msg: &IncomingMessage,
        response: OutgoingResponse,
    ) -> Result<(), ChannelError> {
        let session_id = self.current_session_id().await;

        // Emit the final assistant message (always; `--verbose` also emits
        // intermediate ones but those are sent via status updates when
        // supported — not implemented in v1).
        if self.config.verbose {
            self.emit(&NdjsonOutput::Assistant {
                session_id: session_id.clone(),
                message: AssistantMessage {
                    role: "assistant".into(),
                    content: Some(response.content.clone()),
                    tool_calls: Vec::new(),
                    stop_reason: Some("end_turn".into()),
                },
            });
        }

        let usage = self.turn_usage.lock().await.clone();
        let duration_ms = self
            .turn_start
            .lock()
            .await
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);

        self.emit(&NdjsonOutput::Result(ResultEvent {
            subtype: ResultSubtype::Success,
            session_id,
            result: Some(response.content),
            duration_ms,
            num_turns: usage.num_turns.max(1),
            usage: UsageTotals {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                total_cost_usd: if usage.cost_usd.is_empty() {
                    "0.0000".into()
                } else {
                    usage.cost_usd.clone()
                },
            },
            error: None,
        }));

        // Reset turn state for the next message.
        *self.turn_start.lock().await = Some(Instant::now());
        *self.turn_usage.lock().await = TurnUsage::default();

        // In -p mode with non-streaming input, send /quit to shut down.
        if self.config.initial_prompt.is_some()
            && !self.config.streaming_input
            && let Some(tx) = self.msg_tx.lock().await.as_ref()
        {
            let quit = IncomingMessage::new("ndjson", &self.config.user_id, "/quit");
            let _ = tx.send(quit).await;
        }
        Ok(())
    }

    async fn send_status(
        &self,
        status: StatusUpdate,
        _metadata: &serde_json::Value,
    ) -> Result<(), ChannelError> {
        let session_id = self.current_session_id().await;

        match status {
            StatusUpdate::Thinking(msg) => {
                self.emit(&NdjsonOutput::System(SystemEvent::Thinking {
                    session_id,
                    message: msg,
                }));
            }
            StatusUpdate::Status(msg) => {
                self.emit(&NdjsonOutput::System(SystemEvent::Status {
                    session_id,
                    status: msg,
                }));
            }
            StatusUpdate::ToolStarted { name } => {
                self.emit(&NdjsonOutput::ToolStarted {
                    session_id,
                    tool_name: name,
                    tool_use_id: None,
                    parameters: None,
                });
            }
            StatusUpdate::ToolCompleted {
                name,
                success,
                error,
                parameters,
            } => {
                self.emit(&NdjsonOutput::ToolCompleted {
                    session_id,
                    tool_name: name,
                    tool_use_id: None,
                    success,
                    output_preview: None,
                    error,
                });
                let _ = parameters; // already redacted in StatusUpdate::tool_completed
            }
            StatusUpdate::ToolResult { name, preview } => {
                self.emit(&NdjsonOutput::ToolCompleted {
                    session_id,
                    tool_name: name,
                    tool_use_id: None,
                    success: true,
                    output_preview: Some(preview),
                    error: None,
                });
            }
            StatusUpdate::StreamChunk(_) => {
                // Partial-message streaming is not implemented in v1.
            }
            StatusUpdate::ApprovalNeeded {
                request_id,
                tool_name,
                description: _,
                parameters,
                allow_always,
            } => {
                let rx = self.pending_approvals.register(request_id.clone()).await;
                self.emit(&NdjsonOutput::ControlRequest {
                    request_id: request_id.clone(),
                    session_id: session_id.clone(),
                    request: ControlRequestPayload::CanUseTool {
                        tool_name,
                        parameters,
                        allow_always,
                    },
                });

                // Wait for the caller's control_response.
                match rx.await {
                    Ok(response) => {
                        let text = match response {
                            ControlResponsePayload::Allow { .. } => "y",
                            ControlResponsePayload::Always => "a",
                            ControlResponsePayload::Deny { .. } => "n",
                        };
                        if let Some(tx) = self.msg_tx.lock().await.as_ref() {
                            let mut msg =
                                IncomingMessage::new("ndjson", &self.config.user_id, text);
                            if let Some(ref sid) = *self.session_id.lock().await {
                                msg = msg.with_conversation_scope(sid);
                            }
                            let _ = tx.send(msg).await;
                        }
                    }
                    Err(_) => {
                        tracing::debug!("approval request {} was dropped", request_id);
                    }
                }
            }
            StatusUpdate::CompactionStarted {
                strategy,
                trigger,
                usage_percent,
            } => {
                self.emit(&NdjsonOutput::System(SystemEvent::CompactStarted {
                    session_id,
                    trigger,
                    strategy,
                    usage_percent,
                }));
            }
            StatusUpdate::CompactionCompleted {
                turns_removed,
                tokens_before,
                tokens_after,
                summary_written,
            } => {
                self.emit(&NdjsonOutput::System(SystemEvent::CompactCompleted {
                    session_id,
                    turns_removed,
                    tokens_before,
                    tokens_after,
                    summary_written,
                }));
            }
            StatusUpdate::ReasoningUpdate {
                narrative,
                decisions,
            } => {
                if self.config.include_events.reasoning || self.config.verbose {
                    let decisions = decisions
                        .into_iter()
                        .map(|d| ToolDecisionDto {
                            tool_name: d.tool_name,
                            rationale: d.rationale,
                        })
                        .collect();
                    self.emit(&NdjsonOutput::System(SystemEvent::Reasoning {
                        session_id,
                        narrative,
                        decisions,
                    }));
                }
            }
            StatusUpdate::SkillActivated { skill_names } => {
                self.emit(&NdjsonOutput::System(SystemEvent::SkillActivated {
                    session_id,
                    skill_names,
                }));
            }
            // TurnCost carries *cumulative* token counts from the agent loop,
            // so we overwrite (the last event has the session total). `num_turns`
            // is counted locally — one increment per TurnCost event.
            StatusUpdate::TurnCost {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                let mut usage = self.turn_usage.lock().await;
                usage.input_tokens = input_tokens;
                usage.output_tokens = output_tokens;
                usage.cost_usd = cost_usd.trim_start_matches('$').to_string();
                usage.num_turns = usage.num_turns.saturating_add(1);
            }
            // Variants without NDJSON representation.
            StatusUpdate::JobStarted { .. }
            | StatusUpdate::AuthRequired { .. }
            | StatusUpdate::AuthCompleted { .. }
            | StatusUpdate::ImageGenerated { .. }
            | StatusUpdate::Suggestions { .. } => {}
        }
        Ok(())
    }

    fn conversation_context(&self, _metadata: &serde_json::Value) -> HashMap<String, String> {
        HashMap::new()
    }

    async fn health_check(&self) -> Result<(), ChannelError> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ChannelError> {
        self.pending_approvals.clear().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> NdjsonChannelConfig {
        NdjsonChannelConfig {
            user_id: "owner".into(),
            initial_prompt: None,
            streaming_input: false,
            compat_mode: CompatMode::Ironclaw,
            verbose: false,
            include_events: EventFilter::default(),
            tools: vec!["shell".into()],
            model_name: "test-model".into(),
            session_id_arg: None,
            resume_latest: false,
        }
    }

    #[tokio::test]
    async fn channel_name_is_ndjson() {
        let ch = NdjsonChannel::new(minimal_config(), None);
        assert_eq!(ch.name(), "ndjson");
    }

    #[tokio::test]
    async fn health_check_is_ok() {
        let ch = NdjsonChannel::new(minimal_config(), None);
        ch.health_check().await.unwrap();
    }

    #[tokio::test]
    async fn turn_usage_records_cost() {
        let ch = NdjsonChannel::new(minimal_config(), None);
        let metadata = serde_json::json!({});
        ch.send_status(
            StatusUpdate::TurnCost {
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: "$0.0012".into(),
            },
            &metadata,
        )
        .await
        .unwrap();
        let usage = ch.turn_usage.lock().await.clone();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cost_usd, "0.0012");
        assert_eq!(usage.num_turns, 1);
    }
}
