# NDJSON CLI Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a subprocess-friendly CLI mode that communicates over stdin/stdout via newline-delimited JSON (NDJSON), with session persistence across invocations and optional Claude Code compatibility.

**Architecture:** Introduce a new `NdjsonChannel` that implements the existing `Channel` trait. Reuse IronClaw's database-backed conversation persistence (`--session-id` → `conversation_scope_id` → `maybe_hydrate_thread`), the shared agentic loop, tool approval, interrupt, and automatic compaction machinery — no changes to agent/DB/LLM layers. Add two new structured `StatusUpdate` variants for compaction events and emit them in `thread_ops.rs`. Drive two output formats (IronClaw native + Claude Code compat) via a pure-function adapter layer.

**Tech Stack:** Rust, tokio async, serde/serde_json, async_trait, clap for CLI, existing `Channel`/`ConversationStore` traits, `tracing` for structured logs.

**Spec reference:** `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`

---

## File Structure

### New files

| Path | Responsibility |
|------|----------------|
| `src/channels/ndjson/mod.rs` | `NdjsonChannel` struct, builder, `Channel` trait impl |
| `src/channels/ndjson/types.rs` | `NdjsonInput` / `NdjsonOutput` / `SystemEvent` / nested types + serde |
| `src/channels/ndjson/compat.rs` | `to_claude_code(event)` output format transformer |
| `src/channels/ndjson/stdin_reader.rs` | stdin NDJSON parsing loop, input routing to channel sender |
| `src/channels/ndjson/approval.rs` | `ApprovalState` — control_request ↔ control_response correlation |
| `src/channels/ndjson/session.rs` | `resolve_session_id` — parse `--session-id` / `--resume` |
| `tests/ndjson_types.rs` | Integration test: round-trip and fixture tests for types |
| `tests/ndjson_session.rs` | Integration test: session_id resolution (valid, invalid, resume) |
| `tests/ndjson_channel.rs` | Integration test: end-to-end channel behavior via the trait |

### Modified files

| Path | Change |
|------|--------|
| `src/channels/channel.rs` | Add `StatusUpdate::CompactionStarted`, `StatusUpdate::CompactionCompleted` |
| `src/channels/mod.rs` | `mod ndjson; pub use ndjson::NdjsonChannel;` |
| `src/channels/repl.rs` | Match new `StatusUpdate` variants in `send_status` (dim status lines) |
| `src/agent/thread_ops.rs` | Emit `CompactionStarted`/`CompactionCompleted` in `handle_auto_compaction` block and `process_compact` |
| `src/cli/mod.rs` | Add `-p/--print`, `--output-format`, `--input-format`, `--session-id`, `--resume`, `--verbose`, `--include-events`, `--compat` global flags plus `OutputFormat`, `InputFormat`, `CompatFormat` enums |
| `src/main.rs` | Wire `NdjsonChannel` when `output_format != Text`; skip REPL/web/signal in that case |

### Out-of-scope for Phase 1 (tracked in spec)

- LLM token-by-token streaming (`stream_event` events)
- `AppEvent` variants for the web gateway (compaction events remain REPL-only in Phase 1)
- Multimodal content support in stdin `user` messages
- Shell completion updates

---

## Conventions Used Throughout

- **Imports:** `crate::` for cross-module; `super::` only in tests or intra-module refs.
- **Error handling:** No `.unwrap()` / `.expect()` in production code. Use `?` with `.map_err(|e| ChannelError::Other(e.to_string()))` for channel-layer errors.
- **stdout writes:** Only through `NdjsonChannel::emit(&NdjsonOutput)` which acquires `stdout_lock`. Never call `println!` / `print!` from `ndjson/**`.
- **Logging in NDJSON mode:** Use `tracing::debug!` or `tracing::trace!` only. Never `info!` / `warn!` in hot paths — they corrupt stdout only if misconfigured, but are noisy on stderr. `error!` is OK for unrecoverable failures.
- **Tests:** Unit tests in `mod tests {}` at the bottom of each `ndjson/*.rs` file. Integration tests in `tests/ndjson_*.rs`. Use `tokio::test` for async tests.
- **Commit frequency:** Commit after each step that leaves the tree in a compiling, test-passing state. Use `cargo test --lib -p ironclaw <module_name>` for targeted runs during TDD.
- **Zero clippy warnings:** Run `cargo clippy --all --benches --tests --examples --all-features` before committing any task.

---

## Task 1: Scaffold the new channel module

**Files:**
- Create: `src/channels/ndjson/mod.rs`
- Create: `src/channels/ndjson/types.rs`
- Modify: `src/channels/mod.rs`

- [ ] **Step 1: Create the module directory with an empty `mod.rs` stub**

```rust
// src/channels/ndjson/mod.rs
//! NDJSON CLI channel — subprocess-friendly stdin/stdout protocol.
//!
//! See the design spec at `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`.

mod types;

pub use types::{CompatMode, EventFilter};

#[cfg(test)]
mod tests {
    #[test]
    fn module_compiles() {}
}
```

- [ ] **Step 2: Create a minimal `types.rs` with the mode and filter types**

```rust
// src/channels/ndjson/types.rs
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

/// Placeholder — populated in Task 2.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NdjsonInput {}

/// Placeholder — populated in Task 2.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum NdjsonOutput {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_filter_parses_names_case_insensitive() {
        let filter = EventFilter::from_names(&[
            "reasoning".into(),
            "Hooks".into(),
            "UNKNOWN".into(),
        ]);
        assert!(filter.reasoning);
        assert!(filter.hooks);
        assert!(!filter.cost);
    }

    #[test]
    fn compat_mode_equality() {
        assert_eq!(CompatMode::Ironclaw, CompatMode::Ironclaw);
        assert_ne!(CompatMode::Ironclaw, CompatMode::ClaudeCode);
    }
}
```

Note: serde's tagged-enum macro requires at least one variant for an `#[derive(Serialize)]` without fields, but an empty enum is fine with `#[serde(tag = "type")]` since the tag is never emitted. If rustc complains about unused imports on the empty enum, remove `Serialize`/`Deserialize` for this step and reintroduce in Task 2.

- [ ] **Step 3: Wire the module into `src/channels/mod.rs`**

Edit `src/channels/mod.rs`. Add the following lines in the existing `mod` declarations block (alphabetical ordering; insert after `mod http;`). Note: the module is declared `pub mod` (not `mod`) so that integration tests in `tests/` can refer to types inside it.

```rust
pub mod ndjson;
```

and in the `pub use` block (alphabetical ordering; insert after `pub use http::...;`):

```rust
pub use ndjson::{CompatMode as NdjsonCompatMode, EventFilter as NdjsonEventFilter};
```

- [ ] **Step 4: Run the unit tests to verify scaffolding compiles**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson
```
Expected: three tests pass — `module_compiles`, `event_filter_parses_names_case_insensitive`, `compat_mode_equality`.

- [ ] **Step 5: Run clippy with zero warnings**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean exit.

- [ ] **Step 6: Commit**

```bash
git add src/channels/ndjson/mod.rs src/channels/ndjson/types.rs src/channels/mod.rs
git commit -m "feat(ndjson): scaffold ndjson channel module skeleton"
```

---

## Task 2: Define the NDJSON wire protocol types

**Files:**
- Modify: `src/channels/ndjson/types.rs`

This task flushes out all NdjsonOutput / NdjsonInput variants and their serde derivations. All serialization is byte-exact per the spec; later tasks rely on these shapes.

- [ ] **Step 1: Write failing tests for output serialization (append to existing `tests` module)**

```rust
// Append to src/channels/ndjson/types.rs `mod tests` block.

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
    let line = r#"{"type":"control_response","request_id":"req-1","response":{"behavior":"allow"}}"#;
    let parsed: NdjsonInput = serde_json::from_str(line).unwrap();
    match parsed {
        NdjsonInput::ControlResponse { request_id, response } => {
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
        NdjsonInput::ControlResponse { response, .. } => {
            match response {
                ControlResponsePayload::Deny { message } => assert_eq!(message.as_deref(), Some("nope")),
                _ => panic!("expected Deny"),
            }
        }
        _ => panic!("expected ControlResponse"),
    }
}

#[test]
fn input_interrupt_parses() {
    let line = r#"{"type":"interrupt"}"#;
    let parsed: NdjsonInput = serde_json::from_str(line).unwrap();
    matches!(parsed, NdjsonInput::Interrupt);
}

#[test]
fn input_unknown_type_is_error() {
    let line = r#"{"type":"bogus"}"#;
    let result: Result<NdjsonInput, _> = serde_json::from_str(line);
    assert!(result.is_err(), "unknown type should fail to deserialize");
}
```

- [ ] **Step 2: Run tests to confirm failures**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson::types
```
Expected: many compilation errors — types not yet defined.

- [ ] **Step 3: Implement all the types (replace the placeholder enums)**

Replace the empty `NdjsonInput` / `NdjsonOutput` placeholders in `src/channels/ndjson/types.rs` with the full definitions:

```rust
// Replace the placeholder enums with these.

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
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum ControlResponsePayload {
    /// Approve this single tool call.
    Allow {
        #[serde(default)]
        updated_input: Option<serde_json::Value>,
    },
    /// Approve and add to always-allow list for this session.
    Always,
    /// Reject the tool call with an optional user-facing reason.
    Deny {
        #[serde(default)]
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
    Thinking {
        session_id: String,
        message: String,
    },
    Status {
        session_id: String,
        status: String,
    },
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
    Error,
    ErrorMaxTurns,
    Interrupted,
}

/// Token usage totals reported in a `result` event.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: String,
}
```

- [ ] **Step 4: Run tests to verify pass**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson::types
```
Expected: all tests pass (~13 new + 2 existing = 15).

- [ ] **Step 5: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean exit. If unused-variant warnings appear, add `#[allow(dead_code)]` on `ControlResponsePayload::Always` only until Task 5 consumes it.

- [ ] **Step 6: Commit**

```bash
git add src/channels/ndjson/
git commit -m "feat(ndjson): define wire protocol types"
```

---

## Task 3: Add `StatusUpdate::CompactionStarted` / `CompactionCompleted`

**Files:**
- Modify: `src/channels/channel.rs`

The existing `Status(String)` variant is used today for the compaction notice. We replace that with two structured variants without removing `Status` (other callers still use it).

- [ ] **Step 1: Write a failing test for the new variants**

Append to `mod tests` in `src/channels/channel.rs`:

```rust
#[test]
fn compaction_started_carries_trigger_and_strategy() {
    let status = StatusUpdate::CompactionStarted {
        strategy: "summarize".into(),
        trigger: "auto".into(),
        usage_percent: 85.2,
    };
    match status {
        StatusUpdate::CompactionStarted { strategy, trigger, usage_percent } => {
            assert_eq!(strategy, "summarize");
            assert_eq!(trigger, "auto");
            assert!((usage_percent - 85.2).abs() < 0.01);
        }
        _ => panic!("expected CompactionStarted"),
    }
}

#[test]
fn compaction_completed_carries_token_counts() {
    let status = StatusUpdate::CompactionCompleted {
        turns_removed: 8,
        tokens_before: 85000,
        tokens_after: 32000,
        summary_written: true,
    };
    match status {
        StatusUpdate::CompactionCompleted {
            turns_removed,
            tokens_before,
            tokens_after,
            summary_written,
        } => {
            assert_eq!(turns_removed, 8);
            assert_eq!(tokens_before, 85000);
            assert_eq!(tokens_after, 32000);
            assert!(summary_written);
        }
        _ => panic!("expected CompactionCompleted"),
    }
}
```

- [ ] **Step 2: Run tests to confirm failures**

Run:
```bash
cargo test --lib -p ironclaw channels::channel
```
Expected: compile error — variants unknown.

- [ ] **Step 3: Add the two variants to `StatusUpdate`**

In `src/channels/channel.rs`, locate the `pub enum StatusUpdate` (around line 279) and append two new variants at the end, just before the closing brace (immediately after `SkillActivated { skill_names: Vec<String> }`):

```rust
    /// Context-window compaction has begun.
    CompactionStarted {
        /// One of "summarize", "truncate", or "workspace".
        strategy: String,
        /// One of "auto" or "manual".
        trigger: String,
        /// Usage percentage at the moment compaction was triggered.
        usage_percent: f32,
    },
    /// Context-window compaction has finished.
    CompactionCompleted {
        turns_removed: usize,
        tokens_before: usize,
        tokens_after: usize,
        summary_written: bool,
    },
```

- [ ] **Step 4: Handle the new variants in `ReplChannel::send_status`**

This is needed immediately because the match in `repl.rs` is exhaustive — otherwise the whole crate fails to compile.

In `src/channels/repl.rs`, locate the `match status` block inside `send_status` (around line 711). Add two new arms after the existing `SkillActivated` arm:

```rust
            StatusUpdate::CompactionStarted {
                strategy,
                trigger,
                usage_percent,
            } => {
                self.clear_transient();
                eprintln!(
                    "  {}\u{25C7} compacting ({trigger}, {strategy}) at {usage_percent:.0}%{}",
                    fmt::dim(),
                    fmt::reset()
                );
            }
            StatusUpdate::CompactionCompleted {
                turns_removed,
                tokens_before,
                tokens_after,
                summary_written,
            } => {
                self.clear_transient();
                let suffix = if summary_written {
                    " (summary written)"
                } else {
                    ""
                };
                eprintln!(
                    "  {}\u{25C6} compacted: {turns_removed} turns removed, {tokens_before} \u{2192} {tokens_after} tokens{suffix}{}",
                    fmt::dim(),
                    fmt::reset()
                );
            }
```

- [ ] **Step 5: Handle the new variants in every other `impl Channel` that matches `StatusUpdate` exhaustively**

Run:
```bash
cargo build --lib -p ironclaw 2>&1 | grep -E "error\[E0004\]|non-exhaustive"
```
Expected output: zero matches. If the compiler points to additional channels (web gateway, signal, relay, wasm, http, http_webhook, sse, etc.), add a no-op arm for each:

```rust
            StatusUpdate::CompactionStarted { .. }
            | StatusUpdate::CompactionCompleted { .. } => {
                // Handled structurally in NDJSON / REPL; other channels fall back to
                // the existing Status() rendering path (not used for compaction).
            }
```

Place these arms just before the final catch-all match arm (if any) or alongside the other ignored variants. Do NOT use a wildcard `_ =>` pattern — we want future `StatusUpdate` additions to be forced through the compiler.

- [ ] **Step 6: Verify the tree builds**

Run:
```bash
cargo build --lib -p ironclaw
```
Expected: clean build.

- [ ] **Step 7: Run channel tests**

Run:
```bash
cargo test --lib -p ironclaw channels
```
Expected: all pre-existing tests plus the two new tests pass.

- [ ] **Step 8: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean exit.

- [ ] **Step 9: Commit**

```bash
git add src/channels/channel.rs src/channels/repl.rs
# add any other channel files you had to update in Step 5
git commit -m "feat(status): add CompactionStarted and CompactionCompleted variants"
```

---

## Task 4: Emit compaction events from `thread_ops.rs`

**Files:**
- Modify: `src/agent/thread_ops.rs`

- [ ] **Step 1: Read the current `handle_auto_compaction` block and `process_compact`**

Run:
```bash
cargo check --lib -p ironclaw 2>&1 | head -5
```
Then locate both sites:

```bash
grep -n "context_monitor.suggest_compaction\|ContextCompactor::new" src/agent/thread_ops.rs
```

The auto block lives around lines 400–434; `process_compact` lives around lines 950–987.

- [ ] **Step 2: Replace the auto-compaction `Status(...)` emission with structured events**

In `src/agent/thread_ops.rs`, find this code (around line 409):

```rust
            if let Some(strategy) = self.context_monitor.suggest_compaction(&messages) {
                let pct = self.context_monitor.usage_percent(&messages);
                tracing::info!("Context at {:.1}% capacity, auto-compacting", pct);

                // Notify the user that compaction is happening
                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::Status(format!(
                            "Context at {:.0}% capacity, compacting...",
                            pct
                        )),
                        &message.metadata,
                    )
                    .await;

                let compactor = ContextCompactor::new(self.llm().clone());
                if let Err(e) = compactor
                    .compact(thread, strategy, self.workspace().map(|w| w.as_ref()))
                    .await
                {
                    tracing::warn!("Auto-compaction failed: {}", e);
                }
            }
```

Replace it with:

```rust
            if let Some(strategy) = self.context_monitor.suggest_compaction(&messages) {
                let pct = self.context_monitor.usage_percent(&messages);
                tracing::info!("Context at {:.1}% capacity, auto-compacting", pct);

                let strategy_name = match strategy {
                    crate::agent::context_monitor::CompactionStrategy::Summarize { .. } => {
                        "summarize"
                    }
                    crate::agent::context_monitor::CompactionStrategy::Truncate { .. } => {
                        "truncate"
                    }
                    crate::agent::context_monitor::CompactionStrategy::MoveToWorkspace => {
                        "workspace"
                    }
                };

                let _ = self
                    .channels
                    .send_status(
                        &message.channel,
                        StatusUpdate::CompactionStarted {
                            strategy: strategy_name.into(),
                            trigger: "auto".into(),
                            usage_percent: pct as f32,
                        },
                        &message.metadata,
                    )
                    .await;

                let compactor = ContextCompactor::new(self.llm().clone());
                match compactor
                    .compact(thread, strategy, self.workspace().map(|w| w.as_ref()))
                    .await
                {
                    Ok(result) => {
                        let _ = self
                            .channels
                            .send_status(
                                &message.channel,
                                StatusUpdate::CompactionCompleted {
                                    turns_removed: result.turns_removed,
                                    tokens_before: result.tokens_before,
                                    tokens_after: result.tokens_after,
                                    summary_written: result.summary_written,
                                },
                                &message.metadata,
                            )
                            .await;
                    }
                    Err(e) => {
                        tracing::warn!("Auto-compaction failed: {}", e);
                        let _ = self
                            .channels
                            .send_status(
                                &message.channel,
                                StatusUpdate::Status(format!(
                                    "Compaction failed: {}",
                                    e
                                )),
                                &message.metadata,
                            )
                            .await;
                    }
                }
            }
```

- [ ] **Step 3: Update `process_compact` (manual `/compact`) to emit the same structured events**

Find `process_compact` (around line 950). Read the current body:

```bash
sed -n '950,987p' src/agent/thread_ops.rs
```

Locate the `ContextCompactor::new(self.llm().clone())` invocation and the surrounding `usage_percent` / `suggest_compaction` calls. Before calling `compact()`, add a `CompactionStarted` emission with `trigger: "manual"`. After a successful `compact()`, add a `CompactionCompleted` emission. Mirror the auto path above exactly, except for the `trigger` value. The `message.channel` reference in `process_compact` will come from the `IncomingMessage` / `channels` context available in that function — reuse the pattern from the existing `Status(...)` call in that method.

- [ ] **Step 4: Build the crate**

Run:
```bash
cargo build --lib -p ironclaw
```
Expected: clean build.

- [ ] **Step 5: Run the agent tests**

Run:
```bash
cargo test --lib -p ironclaw agent
```
Expected: all pre-existing tests pass. (We are not adding unit tests here; the integration test in Task 10 will validate event emission end-to-end.)

- [ ] **Step 6: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean exit.

- [ ] **Step 7: Commit**

```bash
git add src/agent/thread_ops.rs
git commit -m "feat(agent): emit structured compaction events"
```

---

## Task 5: ApprovalState — control_request correlation

**Files:**
- Create: `src/channels/ndjson/approval.rs`
- Modify: `src/channels/ndjson/mod.rs`

- [ ] **Step 1: Create `approval.rs` with failing tests**

```rust
// src/channels/ndjson/approval.rs
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

use tokio::sync::{oneshot, Mutex};

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
            .resolve("req-1", ControlResponsePayload::Allow { updated_input: None })
            .await;
        assert!(delivered);
        let response = rx.await.expect("response should arrive");
        matches!(response, ControlResponsePayload::Allow { .. });
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
        assert!(result.is_err(), "dropped sender should surface as RecvError");
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
        matches!(b, ControlResponsePayload::Deny { .. });

        state
            .resolve("req-a", ControlResponsePayload::Allow { updated_input: None })
            .await;
        let a = rx_a.await.unwrap();
        matches!(a, ControlResponsePayload::Allow { .. });
    }
}
```

- [ ] **Step 2: Wire `approval` into `src/channels/ndjson/mod.rs`**

Add after `mod types;`:

```rust
mod approval;

pub(crate) use approval::ApprovalState;
```

Remove the `#[allow(dead_code)]` from `ControlResponsePayload::Always` if it was added in Task 2 — the tests in Step 1 now use it.

- [ ] **Step 3: Run tests**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson::approval
```
Expected: 4 tests pass.

- [ ] **Step 4: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/channels/ndjson/approval.rs src/channels/ndjson/mod.rs
git commit -m "feat(ndjson): add ApprovalState for control_request correlation"
```

---

## Task 6: Session resolution (`--session-id` / `--resume`)

**Files:**
- Create: `src/channels/ndjson/session.rs`
- Modify: `src/channels/ndjson/mod.rs`

- [ ] **Step 1: Read the `ConversationStore` API**

Run:
```bash
grep -n "fn list_conversations" src/db/mod.rs
```

Confirm the relevant methods:
- `list_conversations_all_channels(user_id, limit) -> Vec<ConversationSummary>`
- `conversation_belongs_to_user(id, user_id) -> bool`

We use `list_conversations_all_channels` for `--resume`. The `ConversationSummary` struct has an `id: Uuid` field.

- [ ] **Step 2: Create `session.rs` with failing tests**

```rust
// src/channels/ndjson/session.rs
//! Resolve a session id from CLI flags.
//!
//! - `--session-id <uuid>` is validated as a UUID and passed through unchanged.
//! - `--resume` queries the most recent conversation for the user. If there is
//!   no prior conversation, returns `None` (a fresh session is acceptable).
//! - Neither flag → `None`.

use std::sync::Arc;

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
/// implementing `ConversationStore` (including `&dyn Database` via supertrait
/// upcasting).
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
```

(We defer the "resume with real store returns latest uuid" test to Task 10 integration tests, where a libSQL in-memory DB is available.)

- [ ] **Step 3: Wire `session` into `src/channels/ndjson/mod.rs`**

Add:

```rust
mod session;

pub(crate) use session::{resolve_session_id, SessionResolveArgs};
```

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson::session
```
Expected: 4 tests pass.

- [ ] **Step 5: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/channels/ndjson/session.rs src/channels/ndjson/mod.rs
git commit -m "feat(ndjson): implement session id resolution from CLI flags"
```

---

## Task 7: Stdin reader — NDJSON parsing & input routing

**Files:**
- Create: `src/channels/ndjson/stdin_reader.rs`
- Modify: `src/channels/ndjson/mod.rs`

- [ ] **Step 1: Create `stdin_reader.rs` with failing tests**

```rust
// src/channels/ndjson/stdin_reader.rs
//! Read NDJSON from stdin (or any `AsyncBufRead`) and route parsed lines into
//! the channel's `IncomingMessage` stream and approval state.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::{mpsc, Mutex};

use crate::channels::ndjson::approval::ApprovalState;
use crate::channels::ndjson::types::{ControlResponsePayload, NdjsonInput};
use crate::channels::IncomingMessage;

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
///
/// Exposed for testing — the real reader pushes these into the channel's
/// internal sinks directly.
#[derive(Debug)]
pub enum ReaderEvent {
    /// A user message to inject into the agent loop.
    InjectMessage(IncomingMessage),
    /// A control response that should resolve a pending approval.
    ResolveApproval(String, ControlResponsePayload),
    /// An interrupt (`/interrupt`) message to inject.
    Interrupt(IncomingMessage),
    /// A parse error that should be emitted to stdout as an `error` event.
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
        Ok(NdjsonInput::ControlResponse { request_id, response }) => {
            Some(ReaderEvent::ResolveApproval(request_id, response))
        }
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
///
/// Each parsed event is dispatched: `InjectMessage` / `Interrupt` are pushed to
/// `msg_tx`, `ResolveApproval` is delivered to `approvals`, and `ParseError` is
/// forwarded to `on_parse_error`. The loop exits when the stream reaches EOF.
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
                matches!(payload, ControlResponsePayload::Allow { .. });
            }
            _ => panic!("expected ResolveApproval"),
        }
    }

    #[test]
    fn malformed_json_becomes_parse_error() {
        let out = parse_line(
            "not a json line",
            "owner",
            None,
            UserLinePolicy::Accept,
        )
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
        use tokio::io::AsyncWriteExt;

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

        // Poll the two injected messages.
        let first = rx.recv().await.expect("first message");
        assert_eq!(first.content, "hello");
        let second = rx.recv().await.expect("second message");
        assert_eq!(second.content, "/interrupt");
        assert!(rx.recv().await.is_none(), "stream should close at EOF");
    }
}
```

- [ ] **Step 2: Wire `stdin_reader` into `src/channels/ndjson/mod.rs`**

Add:

```rust
mod stdin_reader;
```

(No re-export needed yet — consumed inside the channel only.)

- [ ] **Step 3: Run tests**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson::stdin_reader
```
Expected: 7 tests pass.

- [ ] **Step 4: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw --tests -- -D warnings
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/channels/ndjson/stdin_reader.rs src/channels/ndjson/mod.rs
git commit -m "feat(ndjson): add stdin reader with line parser"
```

---

## Task 8: Claude Code compatibility transformer

**Files:**
- Create: `src/channels/ndjson/compat.rs`
- Modify: `src/channels/ndjson/mod.rs`

- [ ] **Step 1: Create `compat.rs` with failing tests**

```rust
// src/channels/ndjson/compat.rs
//! Transform `NdjsonOutput` into the Claude Code NDJSON shape.
//!
//! This is a pure function — no I/O, no locks. The channel calls it before
//! writing a line when `compat_mode == CompatMode::ClaudeCode`.

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
}
```

- [ ] **Step 2: Wire `compat` into `src/channels/ndjson/mod.rs`**

Add:

```rust
mod compat;

pub(crate) use compat::to_claude_code;
```

- [ ] **Step 3: Run tests**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson::compat
```
Expected: 7 tests pass.

- [ ] **Step 4: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/channels/ndjson/compat.rs src/channels/ndjson/mod.rs
git commit -m "feat(ndjson): add claude-code compatibility output transformer"
```

---

## Task 9: NdjsonChannel — Channel trait implementation

**Files:**
- Modify: `src/channels/ndjson/mod.rs`

This is the largest task. It ties the types, stdin reader, approvals, and session resolution together into a concrete `Channel` implementation.

- [ ] **Step 1: Define the `NdjsonChannel` struct and builder**

Replace the current `mod.rs` contents with:

```rust
// src/channels/ndjson/mod.rs
//! NDJSON CLI channel — subprocess-friendly stdin/stdout protocol.
//!
//! See the design spec at `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`.

mod approval;
mod compat;
mod session;
mod stdin_reader;
mod types;

// Public API for external consumers (main.rs, integration tests).
pub use session::{resolve_session_id, SessionResolveArgs};
pub use types::{CompatMode, EventFilter};

// Crate-internal helpers.
pub(crate) use approval::ApprovalState;
pub(crate) use compat::to_claude_code;
pub(crate) use stdin_reader::{run_reader, SessionIdSlot, UserLinePolicy};
pub(crate) use types::{
    AssistantMessage, AssistantToolCall, ControlRequestPayload, ControlResponsePayload,
    NdjsonInput, NdjsonOutput, ResultEvent, ResultSubtype, SystemEvent, ToolDecisionDto,
    UsageTotals, UserMessage, UserToolResult,
};

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::io::stdin;
use tokio::sync::{mpsc, Mutex};
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

impl NdjsonChannel {
    pub fn new(
        config: NdjsonChannelConfig,
        database: Option<Arc<dyn Database>>,
    ) -> Self {
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

    /// Write one NDJSON line to stdout. Serializes with a locked guard to
    /// prevent interleaved writes when multiple async tasks emit concurrently.
    fn emit(&self, event: &NdjsonOutput) {
        let value = match self.config.compat_mode {
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

        let Ok(_guard) = self.stdout_lock.lock() else {
            return;
        };
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        if writeln!(handle, "{line}").is_ok() {
            let _ = handle.flush();
        }
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
                        // Emit error event on stdout.
                        let err = NdjsonOutput::Error {
                            session_id: "pending".into(),
                            message: format!("parse error: {} ({})", reason, line),
                        };
                        let val = match compat_mode {
                            CompatMode::Ironclaw => serde_json::to_value(&err).unwrap_or(serde_json::Value::Null),
                            CompatMode::ClaudeCode => to_claude_code(&err),
                        };
                        if val.is_null() {
                            return;
                        }
                        let line_out = match serde_json::to_string(&val) {
                            Ok(s) => s,
                            Err(_) => return,
                        };
                        let Ok(_guard) = stdout_lock.lock() else {
                            return;
                        };
                        let stdout = std::io::stdout();
                        let mut handle = stdout.lock();
                        let _ = writeln!(handle, "{line_out}");
                        let _ = handle.flush();
                    },
                )
                .await;
            });
        }

        // Inject the initial prompt after the reader spawns so that ordering
        // is deterministic.
        if let Some(ref prompt) = self.config.initial_prompt {
            let mut msg = IncomingMessage::new("ndjson", &self.config.user_id, prompt)
                .with_metadata(serde_json::json!({"single_message_mode": !self.config.streaming_input}));
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
        if self.config.initial_prompt.is_some() && !self.config.streaming_input {
            if let Some(tx) = self.msg_tx.lock().await.as_ref() {
                let quit = IncomingMessage::new("ndjson", &self.config.user_id, "/quit");
                let _ = tx.send(quit).await;
            }
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
            StatusUpdate::ToolCompleted { name, success, error, parameters } => {
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
                let rx = self
                    .pending_approvals
                    .register(request_id.clone())
                    .await;
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
                            let mut msg = IncomingMessage::new("ndjson", &self.config.user_id, text);
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
            StatusUpdate::ReasoningUpdate { narrative, decisions } => {
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

```

- [ ] **Step 2: Build and fix compiler errors**

Run:
```bash
cargo build --lib -p ironclaw
```

Fix any type errors revealed (e.g. unused imports, missing trait bounds). Remove `_touch_unused_utc` once the build succeeds — it exists only to make the `chrono::Utc` import self-contained if the rest of the task ends up not using it.

- [ ] **Step 3: Write a minimal unit test for `emit()` in the `mod tests` block**

Append at the bottom of `mod.rs`:

```rust
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
```

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test --lib -p ironclaw channels::ndjson
```
Expected: all prior ndjson tests pass plus the three new ones.

- [ ] **Step 5: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/channels/ndjson/mod.rs
git commit -m "feat(ndjson): implement NdjsonChannel Channel trait impl"
```

---

## Task 10: Integration test — resume and channel E2E

**Files:**
- Create: `tests/ndjson_session.rs`

We test session resolution against a real libSQL in-memory database (per the project's testing rules — no mocks). The integration harness for a full agent loop is substantial, so this task focuses on: (1) `resolve_session_id` with a real store, and (2) the channel's `start()` / `emit` round-trip by swapping stdout with a pipe. A full end-to-end test with tools/LLM is covered by the later `tests/ndjson_channel.rs`.

- [ ] **Step 1: Create `tests/ndjson_session.rs`**

```rust
// tests/ndjson_session.rs
//! Integration test: NDJSON session resolution against a real libSQL store.

use std::sync::Arc;

use ironclaw::channels::ndjson::{resolve_session_id, SessionResolveArgs};
use ironclaw::db::{ConversationStore, Database, LibSqlBackend};

async fn new_backend() -> Arc<LibSqlBackend> {
    let backend = LibSqlBackend::new_memory()
        .await
        .expect("new in-memory libSQL backend");
    backend.run_migrations().await.expect("run migrations");
    Arc::new(backend)
}

#[tokio::test]
async fn resume_picks_latest_conversation_for_user() {
    let backend = new_backend().await;
    let store: &dyn ConversationStore = &*backend;

    let user = "alice";
    let id_old = store
        .create_conversation("ndjson", user, None)
        .await
        .expect("create old");
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
    let backend = new_backend().await;
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
```

- [ ] **Step 2: Make `resolve_session_id` / `SessionResolveArgs` publicly exported**

Integration tests in `tests/` use the crate as a library consumer, so the symbols must be `pub` (not `pub(crate)`).

Edit `src/channels/ndjson/mod.rs`: change

```rust
pub(crate) use session::{resolve_session_id, SessionResolveArgs};
```

to

```rust
pub use session::{resolve_session_id, SessionResolveArgs};
```

The parent `src/channels/mod.rs` already uses `pub mod ndjson;` (Task 1), so the symbols are reachable as `ironclaw::channels::ndjson::resolve_session_id` / `::SessionResolveArgs`.

- [ ] **Step 3: Run the integration test**

Run:
```bash
cargo test --test ndjson_session
```

Expected: 4 tests pass. The libSQL backend runs purely in-memory, so no `--features integration` is required. If `LibSqlBackend::new_memory` is feature-gated, gate the whole test file:

```rust
#![cfg(feature = "libsql")]
```

- [ ] **Step 4: Run clippy on the test target**

Run:
```bash
cargo clippy --test ndjson_session -- -D warnings
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add tests/ndjson_session.rs src/channels/ndjson/mod.rs src/channels/mod.rs
git commit -m "test(ndjson): integration test for session resolution"
```

---

## Task 11: CLI flags

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Add the `OutputFormat`, `InputFormat`, `CompatFormat` enums**

At the top of `src/cli/mod.rs`, just above `pub struct Cli`, add:

```rust
/// NDJSON / text output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Default human-facing REPL output.
    Text,
    /// Single JSON object at end of run (non-streaming).
    Json,
    /// Streaming NDJSON (one JSON object per line).
    StreamJson,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Text
    }
}

/// NDJSON / text input format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum InputFormat {
    /// Interactive rustyline REPL.
    Text,
    /// Streaming NDJSON on stdin.
    StreamJson,
}

impl Default for InputFormat {
    fn default() -> Self {
        Self::Text
    }
}

/// NDJSON output compatibility mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum CompatFormat {
    /// IronClaw native event shape.
    Ironclaw,
    /// Claude Code compatible event shape.
    ClaudeCode,
}

impl Default for CompatFormat {
    fn default() -> Self {
        Self::Ironclaw
    }
}
```

- [ ] **Step 2: Add the new global flags to `Cli`**

In `src/cli/mod.rs`, inside `pub struct Cli`, add these fields at the end (just before the closing brace):

```rust
    /// Non-interactive mode: send a prompt and exit.
    #[arg(short = 'p', long = "print", global = true, conflicts_with = "message")]
    pub print: Option<String>,

    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub output_format: OutputFormat,

    /// Input format.
    #[arg(long, global = true, value_enum, default_value_t = InputFormat::Text)]
    pub input_format: InputFormat,

    /// Resume a specific session by UUID (conflicts with --resume).
    #[arg(long, global = true, conflicts_with = "resume")]
    pub session_id: Option<String>,

    /// Resume the most recent session for the current user.
    #[arg(long, global = true)]
    pub resume: bool,

    /// Include intermediate assistant/user messages in output.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Additional event types to include in NDJSON output.
    #[arg(long, global = true, value_delimiter = ',')]
    pub include_events: Vec<String>,

    /// NDJSON output compatibility mode.
    #[arg(long, global = true, value_enum, default_value_t = CompatFormat::Ironclaw)]
    pub compat: CompatFormat,
```

- [ ] **Step 3: Add a validate() method for cross-flag rules**

At the bottom of the `impl Cli` block (create one if none exists), add:

```rust
impl Cli {
    /// Validate cross-flag invariants. Call after `clap` parsing.
    pub fn validate(&self) -> Result<(), String> {
        if self.input_format == InputFormat::StreamJson
            && self.output_format == OutputFormat::Text
        {
            return Err(
                "--input-format stream-json requires --output-format stream-json".into(),
            );
        }
        if self.compat == CompatFormat::ClaudeCode && self.output_format == OutputFormat::Text {
            return Err(
                "--compat claude-code requires --output-format stream-json".into(),
            );
        }
        Ok(())
    }

    /// Whether the process should run in NDJSON (non-text) mode.
    pub fn is_ndjson_mode(&self) -> bool {
        !matches!(self.output_format, OutputFormat::Text)
    }
}
```

If `impl Cli { ... }` already exists, append the methods inside it instead of creating a new block.

- [ ] **Step 4: Add unit tests for flag validation**

At the bottom of `src/cli/mod.rs`, inside a `#[cfg(test)] mod tests` block (create if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse_args(args: &[&str]) -> Cli {
        let mut full: Vec<String> = vec!["ironclaw".into()];
        full.extend(args.iter().map(|s| s.to_string()));
        Cli::parse_from(full)
    }

    #[test]
    fn text_is_default_output_format() {
        let cli = parse_args(&[]);
        assert_eq!(cli.output_format, OutputFormat::Text);
        assert!(!cli.is_ndjson_mode());
    }

    #[test]
    fn stream_json_flag_is_parsed() {
        let cli = parse_args(&["--output-format", "stream-json"]);
        assert_eq!(cli.output_format, OutputFormat::StreamJson);
        assert!(cli.is_ndjson_mode());
    }

    #[test]
    fn session_id_and_resume_conflict() {
        let result = Cli::try_parse_from([
            "ironclaw",
            "--session-id",
            "00000000-0000-0000-0000-000000000000",
            "--resume",
        ]);
        assert!(result.is_err(), "session-id and resume should conflict");
    }

    #[test]
    fn stream_json_input_without_stream_json_output_fails_validation() {
        let cli = parse_args(&["--input-format", "stream-json"]);
        assert!(cli.validate().is_err());
    }

    #[test]
    fn claude_code_compat_without_stream_json_fails_validation() {
        let cli = parse_args(&["--compat", "claude-code"]);
        assert!(cli.validate().is_err());
    }

    #[test]
    fn claude_code_compat_with_stream_json_is_valid() {
        let cli = parse_args(&[
            "--output-format",
            "stream-json",
            "--compat",
            "claude-code",
        ]);
        assert!(cli.validate().is_ok());
    }

    #[test]
    fn print_and_message_conflict() {
        let result = Cli::try_parse_from([
            "ironclaw",
            "--print",
            "hi",
            "--message",
            "world",
        ]);
        assert!(result.is_err(), "print and message should conflict");
    }

    #[test]
    fn include_events_comma_separated() {
        let cli = parse_args(&["--include-events", "reasoning,cost"]);
        assert_eq!(cli.include_events, vec!["reasoning", "cost"]);
    }
}
```

- [ ] **Step 5: Run tests**

Run:
```bash
cargo test --lib -p ironclaw cli::tests
```
Expected: 8 tests pass.

- [ ] **Step 6: Run clippy**

Run:
```bash
cargo clippy --lib -p ironclaw -- -D warnings
```
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add NDJSON mode flags and validation"
```

---

## Task 12: Wire NdjsonChannel into `main.rs`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Run `cli.validate()` after clap parsing**

Locate the `fn main()` / startup in `src/main.rs`. Find the line that parses CLI args (e.g. `let cli = Cli::parse();`). Immediately after it, add:

```rust
    if let Err(e) = cli.validate() {
        eprintln!("ironclaw: {}", e);
        std::process::exit(2);
    }
```

- [ ] **Step 2: Wire NdjsonChannel into the channel setup block**

Locate the existing channel setup (around line 396 — starts with `// Create CLI channel`). Replace the block:

```rust
    // Create CLI channel
    let repl_channel = if let Some(ref msg) = cli.message {
        Some(ReplChannel::with_message_for_user(
            config.owner_id.clone(),
            msg.clone(),
        ))
    } else if config.channels.cli.enabled {
        let repl = ReplChannel::with_user_id(config.owner_id.clone());
        repl.suppress_banner();
        Some(repl)
    } else {
        None
    };

    if let Some(repl) = repl_channel {
        channels.add(Box::new(repl)).await;
        if cli.message.is_some() {
            tracing::debug!("Single message mode");
        } else {
            channel_names.push("repl".to_string());
            tracing::debug!("REPL mode enabled");
        }
    }
```

with:

```rust
    // Channel setup: NDJSON mode replaces all other channels.
    if cli.is_ndjson_mode() {
        use ironclaw::channels::ndjson::{
            CompatMode, EventFilter, NdjsonChannel, NdjsonChannelConfig,
        };
        use ironclaw::cli::{CompatFormat, OutputFormat};

        let compat_mode = match cli.compat {
            CompatFormat::Ironclaw => CompatMode::Ironclaw,
            CompatFormat::ClaudeCode => CompatMode::ClaudeCode,
        };
        // `ToolRegistry::list()` already returns owned names.
        let tool_names: Vec<String> = components.tools.list().await;
        let model_name = components.llm.model_name().to_string();
        let config = NdjsonChannelConfig {
            user_id: config.owner_id.clone(),
            initial_prompt: cli.print.clone(),
            streaming_input: matches!(
                cli.input_format,
                ironclaw::cli::InputFormat::StreamJson
            ),
            compat_mode,
            verbose: cli.verbose,
            include_events: EventFilter::from_names(&cli.include_events),
            tools: tool_names,
            model_name,
            session_id_arg: cli.session_id.clone(),
            resume_latest: cli.resume,
        };
        let ndjson = NdjsonChannel::new(config, components.db.clone());
        channels.add(Box::new(ndjson)).await;
        channel_names.push("ndjson".to_string());
        tracing::debug!("NDJSON mode enabled; other channels suppressed");
        let _ = matches!(cli.output_format, OutputFormat::StreamJson);
    } else {
        // Create CLI channel (legacy text path)
        let repl_channel = if let Some(ref msg) = cli.message {
            Some(ReplChannel::with_message_for_user(
                config.owner_id.clone(),
                msg.clone(),
            ))
        } else if config.channels.cli.enabled {
            let repl = ReplChannel::with_user_id(config.owner_id.clone());
            repl.suppress_banner();
            Some(repl)
        } else {
            None
        };

        if let Some(repl) = repl_channel {
            channels.add(Box::new(repl)).await;
            if cli.message.is_some() {
                tracing::debug!("Single message mode");
            } else {
                channel_names.push("repl".to_string());
                tracing::debug!("REPL mode enabled");
            }
        }
    }
```

Confirmed against the current tree:
- `components.tools.list().await` returns `Vec<String>` (owned tool names) — used as-is above.
- `components.llm.model_name()` — from the `LlmProvider` trait.
- `components.db` is `Option<Arc<dyn Database>>`, which matches `NdjsonChannel::new`'s second parameter. Supertrait upcasting handles the conversion to `&dyn ConversationStore` internally.

- [ ] **Step 3: Suppress the web, signal, and wasm channels when NDJSON is active**

Grep the rest of `src/main.rs` for channel setup blocks:

```bash
grep -n "channels.add\|channel_names.push" src/main.rs
```

Wrap each subsequent setup block (web gateway, signal, wasm, webhook server) in:

```rust
    if !cli.is_ndjson_mode() {
        // ... existing block
    }
```

Take care not to break borrow checking — put the `if` at the outermost level of each block so that inner variables do not leak.

- [ ] **Step 4: Build**

Run:
```bash
cargo build --bin ironclaw
```
Expected: clean build. Fix compiler errors inline (usually name mismatches — use the exact field/method names that compile).

- [ ] **Step 5: Smoke test the flags**

Run:
```bash
cargo run --bin ironclaw -- --help 2>&1 | grep -E "print|output-format|input-format|session-id|resume|verbose|include-events|compat"
```
Expected: every new flag appears in the help output.

Run:
```bash
cargo run --bin ironclaw -- --input-format stream-json --output-format text 2>&1 | head -5
```
Expected: exit code 2 with message `ironclaw: --input-format stream-json requires --output-format stream-json`.

- [ ] **Step 6: Run all tests**

Run:
```bash
cargo test --lib -p ironclaw
```
Expected: clean.

- [ ] **Step 7: Run clippy**

Run:
```bash
cargo clippy --all --benches --tests --examples --all-features -- -D warnings
```
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): wire NdjsonChannel into startup based on CLI flags"
```

---

## Task 13: Integration test — full channel round trip

**Files:**
- Create: `tests/ndjson_channel.rs`

Validate that spawning `ironclaw` as a subprocess with `--output-format stream-json -p "..."` produces a well-formed NDJSON event sequence. This is a black-box test — it runs the binary and parses its stdout.

- [ ] **Step 1: Create `tests/ndjson_channel.rs`**

```rust
// tests/ndjson_channel.rs
//! Integration test: drive `ironclaw` as a subprocess in NDJSON print mode and
//! assert that the event sequence matches the protocol.

#![cfg(feature = "libsql")]

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn ironclaw_binary() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_ironclaw for integration tests.
    PathBuf::from(env!("CARGO_BIN_EXE_ironclaw"))
}

#[test]
#[ignore = "Requires LLM provider; run manually with IRONCLAW_STUB_LLM=1"]
fn print_mode_emits_init_assistant_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let output = Command::new(ironclaw_binary())
        .arg("--print")
        .arg("say hi and nothing else")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--no-db")
        .env("IRONCLAW_BASE_DIR", &home)
        .env("DATABASE_BACKEND", "libsql")
        .env("LIBSQL_PATH", home.join("test.db"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run ironclaw");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line must be valid JSON"))
        .collect();

    assert!(!lines.is_empty(), "should emit at least one NDJSON line");
    let first = &lines[0];
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "init");
    assert_eq!(first["protocol_version"], "1");

    let last = lines.last().unwrap();
    assert_eq!(last["type"], "result");
    assert!(
        matches!(last["subtype"].as_str(), Some("success") | Some("error")),
        "last event should be a result; got {}",
        last
    );
}
```

This test is `#[ignore]` by default because it requires either a real LLM or a trace replay. Add a follow-up ticket for recording a trace fixture (see `src/llm/recording.rs`) and removing the ignore.

- [ ] **Step 2: Verify the test compiles**

Run:
```bash
cargo test --test ndjson_channel --no-run
```
Expected: clean build.

- [ ] **Step 3: Run clippy**

Run:
```bash
cargo clippy --test ndjson_channel -- -D warnings
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add tests/ndjson_channel.rs
git commit -m "test(ndjson): add ignored subprocess integration smoke test"
```

---

## Task 14: Stdout contamination audit

**Files:**
- Search across the repo; modify any offenders discovered under code paths reachable in NDJSON mode.

- [ ] **Step 1: Grep for direct stdout writes in channel-relevant code paths**

Run:
```bash
grep -rn "println!" src/channels/ src/app.rs src/main.rs src/agent/
grep -rn "print!" src/channels/ src/app.rs src/main.rs src/agent/
```

Ignore hits inside `src/channels/repl.rs` (REPL is inactive in NDJSON mode) and `src/channels/ndjson/mod.rs` (our own).

- [ ] **Step 2: Convert any remaining hits to `eprintln!` or `tracing::debug!`**

For each grep hit outside of the above allowlist:
1. Read the function context to verify it runs on a code path reachable when `NdjsonChannel` is active.
2. If yes, replace `println!(...)` with `eprintln!(...)` or `tracing::debug!(...)`.
3. If no, leave it (REPL-only UI output is fine).

Likely offenders:
- Banner / welcome messages in `src/main.rs` — convert to `eprintln!`
- CLI subcommand output (`src/cli/*.rs`) — these only run on explicit subcommands like `ironclaw config list`, not during `ironclaw -p`, so they are out of scope.

- [ ] **Step 3: Verify `tracing_fmt.rs` writes to stderr**

Run:
```bash
grep -n "stderr\|stdout" src/tracing_fmt.rs
```

Confirm that the formatter uses `std::io::stderr`. If it uses stdout, gate the writer behind `cli.is_ndjson_mode()` or switch to stderr unconditionally.

- [ ] **Step 4: Build and run smoke test**

Run:
```bash
cargo build --bin ironclaw
cargo run --bin ironclaw -- --output-format stream-json --print "hello" --no-db 2>/dev/null | head -3
```

Expected: every line on stdout is a valid JSON object. Validate with:
```bash
cargo run --bin ironclaw -- --output-format stream-json --print "hello" --no-db 2>/dev/null | while read -r line; do echo "$line" | python3 -m json.tool >/dev/null && echo "ok" || echo "bad: $line"; done
```
Expected: every line prints `ok`.

- [ ] **Step 5: Run clippy**

Run:
```bash
cargo clippy --all --tests -- -D warnings
```
Expected: clean.

- [ ] **Step 6: Commit any changes**

If any files were modified:

```bash
git add <modified files>
git commit -m "chore(ndjson): redirect stdout chatter to stderr for clean NDJSON"
```

If no changes were needed, skip the commit and continue.

---

## Task 15: Documentation and spec cross-links

**Files:**
- Modify: `CLAUDE.md` (project root) — add NDJSON mode to the "Adding a New Channel" list
- Modify: `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md` — change Status to "Implemented"

- [ ] **Step 1: Add a one-line bullet in `CLAUDE.md` under "Adding a New Channel"**

Read `CLAUDE.md`:

```bash
grep -n "Adding a New Channel" CLAUDE.md
```

Locate the section and append a line referencing NDJSON mode:

```markdown
For programmatic access, IronClaw supports a subprocess-friendly NDJSON CLI mode; see `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`.
```

- [ ] **Step 2: Update spec status**

In `docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md`, change:

```markdown
**Status**: Draft — pending implementation
```

to:

```markdown
**Status**: Implemented (Phase 1)
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-04-11-ndjson-cli-mode-design.md
git commit -m "docs(ndjson): cross-link NDJSON CLI mode and mark spec implemented"
```

---

## Final Verification

- [ ] **Run the full test suite**

```bash
cargo test --workspace
```
Expected: all tests pass (new NDJSON tests green; no pre-existing tests regressed).

- [ ] **Run the full clippy lint**

```bash
cargo clippy --all --benches --tests --examples --all-features -- -D warnings
```
Expected: zero warnings.

- [ ] **Run cargo fmt**

```bash
cargo fmt --all
git diff --quiet || { git add -A && git commit -m "chore: cargo fmt"; }
```

- [ ] **Manual smoke test (three flag variants)**

```bash
# Native NDJSON, single-shot.
cargo run --bin ironclaw -- --output-format stream-json --print "hello" --no-db 2>/dev/null | head -5

# Claude Code compat, single-shot.
cargo run --bin ironclaw -- --output-format stream-json --compat claude-code --print "hello" --no-db 2>/dev/null | head -5

# Conflict validation.
cargo run --bin ironclaw -- --input-format stream-json 2>&1 | head -2
```

Expected outputs:
- The first two commands emit valid NDJSON starting with an `init` event and ending with a `result` event.
- The third command exits with code 2 and prints a validation error to stderr.

- [ ] **Push (optional)**

If the user requests it:

```bash
git push -u origin feature/ndjson-cli-mode
```

---

## Spec Coverage Cross-Check

Quick mapping from spec sections to plan tasks:

| Spec section | Task(s) |
|--------------|---------|
| Module layout | 1, 2, 5, 6, 7, 8, 9 |
| `NdjsonChannel` struct and data flow | 9 |
| CLI flags & validation rules | 11 |
| Protocol output events (native) | 2, 9 |
| Protocol input messages | 2, 7 |
| Claude Code compatibility mode | 2, 8, 9 |
| `StatusUpdate` new variants | 3 |
| Compaction event emission in `thread_ops.rs` | 4 |
| Session resume flow | 6, 10 |
| Tool approval flow (control_request/response) | 5, 7, 9 |
| Interrupt flow | 7, 9 (no agent change needed) |
| Stdout contamination protection | 9 (emit lock), 14 (audit) |
| Error handling (invalid JSON, invalid UUID) | 6, 7, 11 |
| Testing strategy (unit + integration) | 2, 5, 6, 7, 8, 10, 11, 13 |
| Out-of-scope (LLM streaming, multimodal) | intentionally deferred |
| Documentation & status update | 15 |
