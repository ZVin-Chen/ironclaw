//! Transform `NdjsonOutput` into the Claude Code NDJSON shape.
//!
//! This is a pure function — no I/O, no locks. The channel calls it before
//! writing a line when `compat_mode == CompatMode::ClaudeCode`.
//!
//! # Lossy mappings
//!
//! - `ResultSubtype::Interrupted` is mapped to `"error_during_execution"`
//!   because the Claude Code SDK has no dedicated subtype for user
//!   interrupts. `is_error` is still set to `true`.
//! - `SystemEvent::Thinking`, `Reasoning`, and `SkillActivated` have no
//!   Claude Code equivalent and are dropped (return `Value::Null`). This
//!   is unconditional — even if the user enables `--include-events
//!   reasoning`, these events are suppressed in Claude Code compat mode.

use serde_json::{json, Value};

use crate::channels::ndjson::types::{
    ControlRequestPayload, NdjsonOutput, ResultSubtype, SystemEvent,
};

/// Convert an `NdjsonOutput` event to its Claude Code wire shape. Returns
/// `Value::Null` when the event has no Claude Code equivalent; the caller
/// should skip writing null values.
pub fn to_claude_code(event: &NdjsonOutput) -> Value {
    match event {
        NdjsonOutput::System(SystemEvent::Init {
            session_id,
            resumed,
            model,
            tools,
            cwd,
            protocol_version,
        }) => {
            let _ = resumed;
            let _ = protocol_version;
            json!({
                "type": "system",
                "subtype": "init",
                "session_id": session_id,
                "cwd": cwd,
                "model": model,
                "tools": tools,
                "permissionMode": "default",
                "apiKeySource": "user",
            })
        }
        NdjsonOutput::System(SystemEvent::Status { session_id, status }) => json!({
            "type": "system",
            "subtype": "status",
            "status": status,
            "session_id": session_id,
        }),
        NdjsonOutput::System(SystemEvent::CompactStarted { session_id, .. }) => json!({
            "type": "system",
            "subtype": "status",
            "status": "compacting",
            "session_id": session_id,
        }),
        NdjsonOutput::System(SystemEvent::CompactCompleted {
            session_id,
            tokens_before,
            ..
        }) => json!({
            "type": "system",
            "subtype": "compact_boundary",
            "session_id": session_id,
            "compact_metadata": {
                "trigger": "auto",
                "pre_tokens": tokens_before,
            }
        }),
        NdjsonOutput::System(SystemEvent::Thinking { .. }) => Value::Null,
        NdjsonOutput::System(SystemEvent::SkillActivated { .. }) => Value::Null,
        NdjsonOutput::System(SystemEvent::Reasoning { .. }) => Value::Null,
        NdjsonOutput::Assistant { session_id, message } => {
            let mut content_blocks: Vec<Value> = Vec::new();
            if let Some(text) = &message.content {
                content_blocks.push(json!({ "type": "text", "text": text }));
            }
            for call in &message.tool_calls {
                content_blocks.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            json!({
                "type": "assistant",
                "session_id": session_id,
                "message": {
                    "role": message.role,
                    "content": content_blocks,
                    "stop_reason": message.stop_reason,
                }
            })
        }
        NdjsonOutput::User { session_id, message } => {
            let content_blocks: Vec<Value> = message
                .tool_results
                .iter()
                .map(|r| {
                    json!({
                        "type": "tool_result",
                        "tool_use_id": r.tool_use_id,
                        "content": r.content,
                        "is_error": r.is_error,
                    })
                })
                .collect();
            json!({
                "type": "user",
                "session_id": session_id,
                "message": {
                    "role": message.role,
                    "content": content_blocks,
                }
            })
        }
        NdjsonOutput::ToolStarted { .. } => Value::Null,
        NdjsonOutput::ToolCompleted { .. } => Value::Null,
        NdjsonOutput::ControlRequest {
            request_id,
            session_id,
            request,
        } => match request {
            ControlRequestPayload::CanUseTool {
                tool_name,
                parameters,
                allow_always: _,
            } => json!({
                "type": "control_request",
                "request_id": request_id,
                "session_id": session_id,
                "request": {
                    "subtype": "can_use_tool",
                    "tool_name": tool_name,
                    "input": parameters,
                }
            }),
        },
        NdjsonOutput::Result(r) => {
            let subtype_str = match r.subtype {
                ResultSubtype::Success => "success",
                ResultSubtype::Error => "error_during_execution",
                ResultSubtype::ErrorMaxTurns => "error_max_turns",
                ResultSubtype::Interrupted => "error_during_execution",
            };
            let is_error = !matches!(r.subtype, ResultSubtype::Success);
            json!({
                "type": "result",
                "subtype": subtype_str,
                "session_id": r.session_id,
                "is_error": is_error,
                "result": r.result,
                "duration_ms": r.duration_ms,
                "num_turns": r.num_turns,
                "usage": {
                    "input_tokens": r.usage.input_tokens,
                    "output_tokens": r.usage.output_tokens,
                },
                "total_cost_usd": r.usage.total_cost_usd,
            })
        }
        NdjsonOutput::Error { message, .. } => json!({
            "type": "error",
            "message": message,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::ndjson::types::{
        AssistantMessage, AssistantToolCall, ControlRequestPayload, NdjsonOutput, ResultEvent,
        ResultSubtype, SystemEvent, UsageTotals,
    };

    #[test]
    fn init_emits_claude_code_fields() {
        let ev = NdjsonOutput::System(SystemEvent::Init {
            session_id: "sess".into(),
            resumed: true,
            model: "claude-sonnet-4-6".into(),
            tools: vec!["shell".into()],
            cwd: Some("/repo".into()),
            protocol_version: "1".into(),
        });
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "system");
        assert_eq!(v["subtype"], "init");
        assert_eq!(v["permissionMode"], "default");
        assert_eq!(v["apiKeySource"], "user");
        assert_eq!(v["model"], "claude-sonnet-4-6");
        assert_eq!(v["tools"][0], "shell");
    }

    #[test]
    fn thinking_is_dropped() {
        let ev = NdjsonOutput::System(SystemEvent::Thinking {
            session_id: "sess".into(),
            message: "Analyzing".into(),
        });
        assert!(to_claude_code(&ev).is_null());
    }

    #[test]
    fn tool_started_is_dropped() {
        let ev = NdjsonOutput::ToolStarted {
            session_id: "sess".into(),
            tool_name: "read_file".into(),
            tool_use_id: Some("t-1".into()),
            parameters: None,
        };
        assert!(to_claude_code(&ev).is_null());
    }

    #[test]
    fn assistant_collapses_to_content_blocks() {
        let ev = NdjsonOutput::Assistant {
            session_id: "sess".into(),
            message: AssistantMessage {
                role: "assistant".into(),
                content: Some("Let me check.".into()),
                tool_calls: vec![AssistantToolCall {
                    id: "t-1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "main.rs"}),
                }],
                stop_reason: Some("tool_use".into()),
            },
        };
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "assistant");
        let content = v["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Let me check.");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "t-1");
        assert_eq!(content[1]["input"]["path"], "main.rs");
    }

    #[test]
    fn compact_completed_maps_to_boundary_with_pre_tokens() {
        let ev = NdjsonOutput::System(SystemEvent::CompactCompleted {
            session_id: "sess".into(),
            turns_removed: 5,
            tokens_before: 85000,
            tokens_after: 32000,
            summary_written: true,
        });
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "system");
        assert_eq!(v["subtype"], "compact_boundary");
        assert_eq!(v["compact_metadata"]["trigger"], "auto");
        assert_eq!(v["compact_metadata"]["pre_tokens"], 85000);
    }

    #[test]
    fn control_request_preserves_subtype_and_tool_name() {
        let ev = NdjsonOutput::ControlRequest {
            request_id: "req-1".into(),
            session_id: "sess".into(),
            request: ControlRequestPayload::CanUseTool {
                tool_name: "shell".into(),
                parameters: serde_json::json!({"command": "ls"}),
                allow_always: true,
            },
        };
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "control_request");
        assert_eq!(v["request"]["subtype"], "can_use_tool");
        assert_eq!(v["request"]["tool_name"], "shell");
        assert_eq!(v["request"]["input"]["command"], "ls");
    }

    #[test]
    fn result_success_maps_cost_totals() {
        let ev = NdjsonOutput::Result(ResultEvent {
            subtype: ResultSubtype::Success,
            session_id: "sess".into(),
            result: Some("done".into()),
            duration_ms: 1234,
            num_turns: 3,
            usage: UsageTotals {
                input_tokens: 100,
                output_tokens: 50,
                total_cost_usd: "0.001".into(),
            },
            error: None,
        });
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "result");
        assert_eq!(v["subtype"], "success");
        assert_eq!(v["is_error"], false);
        assert_eq!(v["usage"]["input_tokens"], 100);
        assert_eq!(v["total_cost_usd"], "0.001");
    }

    #[test]
    fn user_message_collapses_tool_results_to_content_blocks() {
        use crate::channels::ndjson::types::{UserMessage, UserToolResult};
        let ev = NdjsonOutput::User {
            session_id: "sess".into(),
            message: UserMessage {
                role: "user".into(),
                tool_results: vec![
                    UserToolResult {
                        tool_use_id: "t-1".into(),
                        name: "read_file".into(),
                        content: "file body".into(),
                        is_error: false,
                    },
                    UserToolResult {
                        tool_use_id: "t-2".into(),
                        name: "shell".into(),
                        content: "command failed".into(),
                        is_error: true,
                    },
                ],
            },
        };
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "user");
        assert_eq!(v["session_id"], "sess");
        let content = v["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "t-1");
        assert_eq!(content[0]["content"], "file body");
        assert_eq!(content[0]["is_error"], false);
        assert_eq!(content[1]["tool_use_id"], "t-2");
        assert_eq!(content[1]["is_error"], true);
    }

    #[test]
    fn result_interrupted_maps_to_error_during_execution() {
        let ev = NdjsonOutput::Result(ResultEvent {
            subtype: ResultSubtype::Interrupted,
            session_id: "sess".into(),
            result: None,
            duration_ms: 50,
            num_turns: 1,
            usage: UsageTotals::default(),
            error: Some("user interrupted".into()),
        });
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "result");
        assert_eq!(v["subtype"], "error_during_execution");
        assert_eq!(v["is_error"], true);
    }

    #[test]
    fn error_event_preserves_message_field() {
        let ev = NdjsonOutput::Error {
            session_id: "sess".into(),
            message: "LLM provider unavailable".into(),
        };
        let v = to_claude_code(&ev);
        assert_eq!(v["type"], "error");
        assert_eq!(v["message"], "LLM provider unavailable");
    }
}
