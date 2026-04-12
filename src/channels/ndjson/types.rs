//! NDJSON wire protocol types.

use serde::{Deserialize, Serialize};

/// Output wire-format selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatMode {
    /// IronClaw native NDJSON format.
    Ironclaw,
    /// Claude Code compatible format.
    ClaudeCode,
}

/// Optional event categories included in the output stream.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub reasoning: bool,
    pub hooks: bool,
    pub cost: bool,
    pub partial_messages: bool,
}

impl EventFilter {
    pub fn from_names(names: &[String]) -> Self {
        let mut filter = Self::default();
        for name in names {
            match name.trim().to_ascii_lowercase().as_str() {
                "reasoning" => filter.reasoning = true,
                "hooks" => filter.hooks = true,
                "cost" => filter.cost = true,
                "partial_messages" | "partial" => filter.partial_messages = true,
                _ => {}
            }
        }
        filter
    }
}

/// Parsed line from stdin.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NdjsonInput {
    /// A user message to send into the agent loop.
    User {
        /// Plain string or structured content array (see spec).
        content: serde_json::Value,
    },
    /// Response to a pending `control_request`.
    ControlResponse {
        request_id: String,
        response: ControlResponsePayload,
    },
    /// Cancel the current turn.
    Interrupt,
}

/// Control-response payload discriminated by `behavior`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlResponsePayload {
    /// Approve this single tool call.
    Allow {
        #[serde(default)]
        #[allow(dead_code)]
        updated_input: Option<serde_json::Value>,
    },
    /// Approve and add to always-allow list for this session.
    Always,
    /// Reject the tool call with an optional user-facing reason.
    Deny {
        #[serde(default)]
        #[allow(dead_code)]
        message: Option<String>,
    },
}

/// Line written to stdout.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NdjsonOutput {
    System(SystemEvent),
    Assistant {
        session_id: String,
        message: AssistantMessage,
    },
    #[allow(dead_code)]
    User {
        session_id: String,
        message: UserMessage,
    },
    ToolStarted {
        session_id: String,
        tool_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<serde_json::Value>,
    },
    ToolCompleted {
        session_id: String,
        tool_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_preview: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    ControlRequest {
        request_id: String,
        session_id: String,
        request: ControlRequestPayload,
    },
    Result(ResultEvent),
    Error {
        session_id: String,
        message: String,
    },
}

/// Assistant message wrapper (matches design spec shape).
#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<AssistantToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// Tool call block inside an assistant message.
#[derive(Debug, Clone, Serialize)]
pub struct AssistantToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// User message wrapper used to ferry tool_results back to the LLM.
#[derive(Debug, Clone, Serialize)]
pub struct UserMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_results: Vec<UserToolResult>,
}

/// A tool result block inside a user message.
#[derive(Debug, Clone, Serialize)]
pub struct UserToolResult {
    pub tool_use_id: String,
    pub name: String,
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
}

/// System event family (compound discriminator uses `subtype`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum SystemEvent {
    Init {
        session_id: String,
        resumed: bool,
        model: String,
        tools: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        protocol_version: String,
    },
    Thinking { session_id: String, message: String },
    Status { session_id: String, status: String },
    CompactStarted {
        session_id: String,
        trigger: String,
        strategy: String,
        usage_percent: f32,
    },
    CompactCompleted {
        session_id: String,
        turns_removed: usize,
        tokens_before: usize,
        tokens_after: usize,
        summary_written: bool,
    },
    SkillActivated {
        session_id: String,
        skill_names: Vec<String>,
    },
    Reasoning {
        session_id: String,
        narrative: String,
        decisions: Vec<ToolDecisionDto>,
    },
}

/// Per-tool decision in a reasoning update.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDecisionDto {
    pub tool_name: String,
    pub rationale: String,
}

/// `control_request` payload variants.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlRequestPayload {
    /// Tool approval required.
    CanUseTool {
        tool_name: String,
        parameters: serde_json::Value,
        allow_always: bool,
    },
}

/// Final `result` event.
#[derive(Debug, Clone, Serialize)]
pub struct ResultEvent {
    pub subtype: ResultSubtype,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub duration_ms: u64,
    pub num_turns: u32,
    pub usage: UsageTotals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Final-result subtype discriminator.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSubtype {
    Success,
    #[allow(dead_code)]
    Error,
    #[allow(dead_code)]
    ErrorMaxTurns,
    #[allow(dead_code)]
    Interrupted,
}

/// Token usage totals reported in a `result` event.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Formatted decimal string per design spec — avoids float rounding on the wire.
    pub total_cost_usd: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_filter_parses_names_case_insensitive() {
        let filter =
            EventFilter::from_names(&["reasoning".into(), "Hooks".into(), "UNKNOWN".into()]);
        assert!(filter.reasoning);
        assert!(filter.hooks);
        assert!(!filter.cost);
    }

    #[test]
    fn compat_mode_equality() {
        assert_eq!(CompatMode::Ironclaw, CompatMode::Ironclaw);
        assert_ne!(CompatMode::Ironclaw, CompatMode::ClaudeCode);
    }

    #[test]
    fn system_init_serializes_with_type_and_subtype() {
        let ev = NdjsonOutput::System(SystemEvent::Init {
            session_id: "abc-123".into(),
            resumed: false,
            model: "claude-sonnet-4-6".into(),
            tools: vec!["shell".into(), "read_file".into()],
            cwd: Some("/repo".into()),
            protocol_version: "1".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "system");
        assert_eq!(json["subtype"], "init");
        assert_eq!(json["session_id"], "abc-123");
        assert_eq!(json["resumed"], false);
        assert_eq!(json["model"], "claude-sonnet-4-6");
        assert_eq!(json["tools"][0], "shell");
        assert_eq!(json["protocol_version"], "1");
    }

    #[test]
    fn assistant_event_serializes_with_message_envelope() {
        let ev = NdjsonOutput::Assistant {
            session_id: "abc".into(),
            message: AssistantMessage {
                role: "assistant".into(),
                content: Some("Let me read the file.".into()),
                tool_calls: vec![AssistantToolCall {
                    id: "t-1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                }],
                stop_reason: Some("tool_use".into()),
            },
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "assistant");
        assert_eq!(json["session_id"], "abc");
        assert_eq!(json["message"]["content"], "Let me read the file.");
        assert_eq!(json["message"]["tool_calls"][0]["name"], "read_file");
        assert_eq!(json["message"]["stop_reason"], "tool_use");
    }

    #[test]
    fn tool_started_serializes_with_parameters() {
        let ev = NdjsonOutput::ToolStarted {
            session_id: "abc".into(),
            tool_name: "read_file".into(),
            tool_use_id: Some("t-1".into()),
            parameters: Some(serde_json::json!({"path": "src/main.rs"})),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "tool_started");
        assert_eq!(json["tool_name"], "read_file");
        assert_eq!(json["parameters"]["path"], "src/main.rs");
    }

    #[test]
    fn tool_completed_serializes_success_flag() {
        let ev = NdjsonOutput::ToolCompleted {
            session_id: "abc".into(),
            tool_name: "shell".into(),
            tool_use_id: None,
            success: false,
            output_preview: None,
            error: Some("exit code 1".into()),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "tool_completed");
        assert_eq!(json["success"], false);
        assert_eq!(json["error"], "exit code 1");
    }

    #[test]
    fn result_success_serializes_usage() {
        let ev = NdjsonOutput::Result(ResultEvent {
            subtype: ResultSubtype::Success,
            session_id: "abc".into(),
            result: Some("Done.".into()),
            duration_ms: 3456,
            num_turns: 2,
            usage: UsageTotals {
                input_tokens: 1234,
                output_tokens: 567,
                total_cost_usd: "0.0042".into(),
            },
            error: None,
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "result");
        assert_eq!(json["subtype"], "success");
        assert_eq!(json["usage"]["input_tokens"], 1234);
        assert_eq!(json["usage"]["total_cost_usd"], "0.0042");
        assert_eq!(json["duration_ms"], 3456);
    }

    #[test]
    fn result_error_serializes_error_field() {
        let ev = NdjsonOutput::Result(ResultEvent {
            subtype: ResultSubtype::Error,
            session_id: "abc".into(),
            result: None,
            duration_ms: 10,
            num_turns: 0,
            usage: UsageTotals::default(),
            error: Some("LLM provider unavailable".into()),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["subtype"], "error");
        assert_eq!(json["error"], "LLM provider unavailable");
    }

    #[test]
    fn control_request_serializes_can_use_tool_payload() {
        let ev = NdjsonOutput::ControlRequest {
            request_id: "req-1".into(),
            session_id: "abc".into(),
            request: ControlRequestPayload::CanUseTool {
                tool_name: "shell".into(),
                parameters: serde_json::json!({"command": "rm -rf target/"}),
                allow_always: true,
            },
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "control_request");
        assert_eq!(json["request_id"], "req-1");
        assert_eq!(json["request"]["subtype"], "can_use_tool");
        assert_eq!(json["request"]["tool_name"], "shell");
        assert_eq!(json["request"]["allow_always"], true);
    }

    #[test]
    fn system_compact_completed_serializes_counts() {
        let ev = NdjsonOutput::System(SystemEvent::CompactCompleted {
            session_id: "abc".into(),
            turns_removed: 8,
            tokens_before: 85000,
            tokens_after: 32000,
            summary_written: true,
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "system");
        assert_eq!(json["subtype"], "compact_completed");
        assert_eq!(json["turns_removed"], 8);
        assert_eq!(json["tokens_before"], 85000);
        assert_eq!(json["summary_written"], true);
    }

    #[test]
    fn input_user_with_string_content_parses() {
        let line = r#"{"type":"user","content":"hello"}"#;
        let parsed: NdjsonInput = serde_json::from_str(line).unwrap();
        match parsed {
            NdjsonInput::User { content } => {
                assert_eq!(content.as_str(), Some("hello"));
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn input_control_response_allow_parses() {
        let line =
            r#"{"type":"control_response","request_id":"req-1","response":{"behavior":"allow"}}"#;
        let parsed: NdjsonInput = serde_json::from_str(line).unwrap();
        match parsed {
            NdjsonInput::ControlResponse {
                request_id,
                response,
            } => {
                assert_eq!(request_id, "req-1");
                match response {
                    ControlResponsePayload::Allow { .. } => {}
                    _ => panic!("expected Allow"),
                }
            }
            _ => panic!("expected ControlResponse"),
        }
    }

    #[test]
    fn input_control_response_deny_captures_message() {
        let line = r#"{"type":"control_response","request_id":"req-1","response":{"behavior":"deny","message":"nope"}}"#;
        let parsed: NdjsonInput = serde_json::from_str(line).unwrap();
        match parsed {
            NdjsonInput::ControlResponse { response, .. } => match response {
                ControlResponsePayload::Deny { message } => {
                    assert_eq!(message.as_deref(), Some("nope"))
                }
                _ => panic!("expected Deny"),
            },
            _ => panic!("expected ControlResponse"),
        }
    }

    #[test]
    fn input_interrupt_parses() {
        let line = r#"{"type":"interrupt"}"#;
        let parsed: NdjsonInput = serde_json::from_str(line).unwrap();
        assert!(matches!(parsed, NdjsonInput::Interrupt));
    }

    #[test]
    fn input_unknown_type_is_error() {
        let line = r#"{"type":"bogus"}"#;
        let result: Result<NdjsonInput, _> = serde_json::from_str(line);
        assert!(result.is_err(), "unknown type should fail to deserialize");
    }

    #[test]
    fn control_response_rejects_unknown_payload_fields() {
        let line = r#"{"type":"control_response","request_id":"x","response":{"behavior":"deny","messagee":"typo"}}"#;
        let result: Result<NdjsonInput, _> = serde_json::from_str(line);
        assert!(
            result.is_err(),
            "unknown field in control_response payload should fail to deserialize"
        );
    }
}
