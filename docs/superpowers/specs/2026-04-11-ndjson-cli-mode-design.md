# NDJSON CLI Mode — Design Spec

**Date**: 2026-04-11
**Status**: Implemented (Phase 1)
**Owner**: TBD

## Summary

Add a subprocess-friendly CLI mode to IronClaw that communicates over stdin/stdout using
newline-delimited JSON (NDJSON). External tools (IDE extensions, scripts, SDKs) can spawn
IronClaw as a child process and drive the agent programmatically, the same way Claude Code
works with `--output-format stream-json`.

Sessions persist across invocations: a caller can continue a prior conversation by passing
`--session-id <uuid>` or `--resume`. Conversation history is loaded from IronClaw's existing
database-backed persistence (no protocol-level history replay required).

Two output formats are supported:
- **IronClaw native** (default) — structured events aligned with IronClaw's concepts
  (`tool_started`, `plan_update`, `skill_activated`, etc.)
- **Claude Code compatible** (`--compat claude-code`) — event shape matches Claude Code's
  NDJSON protocol so existing Claude Agent SDK tooling can drive IronClaw unchanged

## Goals

1. IDE extensions and scripts can drive IronClaw via stdin/stdout without any HTTP server
2. Conversations persist across process restarts via `--session-id` / `--resume`
3. Tool approvals flow through a request/response protocol (no interactive UI)
4. Interrupt support — caller can cancel in-flight work
5. Context window compaction is transparent — callers see notification events but do not
   trigger compaction themselves (matches Claude Code semantics)
6. Optional Claude Code compatibility for existing SDK integrations
7. Minimal changes to existing subsystems: no behavioral changes to the agent loop, session
   manager, tool execution, or database layers. The only agent-side edit is emitting two
   new structured status events around existing compaction calls.

## Non-Goals

- Token-by-token LLM streaming (deferred — `LlmProvider` streaming is a separate effort)
- Programmatic compaction trigger via NDJSON (Claude Code does not expose this either —
  keeps protocol simple)
- Protocol-level history transfer (history stays in the database)
- Cross-process mutual exclusion (multiple ironclaw processes can run concurrently and
  share the database)

## Background

IronClaw already has the foundations needed for an NDJSON CLI mode:

- **Channel abstraction** (`src/channels/channel.rs`) — `Channel` trait with
  `IncomingMessage` / `OutgoingResponse` / `StatusUpdate` already supports the
  request/response/event pattern
- **Database-backed conversations** (`src/db/`) — all conversation messages persist via
  the `ConversationStore` trait; no in-memory-only state for message history
- **Thread hydration** (`src/agent/thread_ops.rs`) — when an `IncomingMessage` carries a
  `conversation_scope_id`, `maybe_hydrate_thread()` loads full message history from DB and
  rebuilds the LLM context via `rebuild_chat_messages_from_db()`
- **Shared agentic loop** (`src/agent/agentic_loop.rs`) — `LoopDelegate` pattern works
  identically regardless of which channel drove the call
- **Interrupt mechanism** — `/interrupt` submission → `process_interrupt()` →
  `ThreadState::Interrupted` → `ChatDelegate::check_signals() → LoopSignal::Stop`
- **Auto-compaction** — `ContextMonitor.suggest_compaction()` runs in
  `process_user_input()` before each new turn, independent of channel
- **StatusUpdate enum** — already covers 20+ event types (Thinking, ToolStarted,
  ToolCompleted, StreamChunk, ApprovalNeeded, TurnCost, ReasoningUpdate, SkillActivated, …)
- **Claude Code NDJSON parsing** — `src/worker/claude_bridge.rs` already parses Claude
  Code NDJSON for container workers (consumer side, not producer)

The implementation is therefore mostly a new `Channel` implementation plus two small
additions to the `StatusUpdate` enum for structured compaction events.

## Usage Overview

### Four Operation Modes

| Mode | Command | Behavior |
|------|---------|----------|
| Single query | `ironclaw -p "explain main.rs" --output-format stream-json` | Send one message, emit events, exit |
| Single query + resume | `ironclaw -p "then auth?" --output-format stream-json --session-id <uuid>` | Hydrate history from DB, append new turn, emit events, exit |
| Multi-turn streaming | `ironclaw --output-format stream-json --input-format stream-json` | Process does not exit; reads stdin, writes stdout until EOF |
| Multi-turn + resume | `ironclaw --output-format stream-json --input-format stream-json --session-id <uuid>` | Hydrate history, then continuous stdin/stdout |

### New CLI Flags (all global)

```
-p, --print <PROMPT>             Non-interactive: send a prompt then process exits
--output-format <FORMAT>         text (default) | stream-json | json
--input-format <FORMAT>          text (default) | stream-json
--session-id <UUID>              Resume a specific session by conversation UUID
--resume                         Resume the most recent session for the current user
--verbose                        Include intermediate assistant/user messages in output
--include-events <TYPES>         Comma-separated: reasoning,hooks,cost,partial_messages
--compat <FORMAT>                ironclaw (default) | claude-code
```

### Flag Validation Rules

- `--input-format stream-json` implies `--output-format stream-json`
  (streaming input requires streaming output)
- `--session-id` and `--resume` are mutually exclusive (clap auto-enforced)
- `--compat claude-code` has effect only when `--output-format stream-json`
- `-p` and `-m` are mutually exclusive — `-p` is the NDJSON counterpart of text-mode `-m`
- When `--output-format` is `stream-json` or `json`, the current process disables all
  non-NDJSON channels (REPL, Web, Signal, …). This is a **per-process** restriction; it
  does not affect other concurrent ironclaw instances.

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Normal completion (`result: success` or stdin EOF in streaming mode) |
| 1 | Runtime error (LLM failure, DB error, unrecoverable agent error) |
| 2 | CLI argument error (invalid UUID, conflicting flags, etc.) |
| 130 | Interrupted by signal (SIGINT / SIGTERM) |

## Protocol — IronClaw Native Format

All messages are single-line JSON objects. Each line is a complete, valid JSON object.
The `type` field is the primary discriminator; `subtype` provides secondary discrimination
for compound event families like `system/*`.

### Output Events (ironclaw → stdout)

#### `system/init`

Emitted once immediately after the process finishes initialization and before accepting
input. Carries session context needed by the caller.

```json
{
  "type": "system",
  "subtype": "init",
  "session_id": "a1b2c3d4-5678-9abc-def0-123456789abc",
  "resumed": false,
  "model": "claude-sonnet-4-6",
  "tools": ["shell", "read_file", "write_file", "edit_file", "memory_search", "http"],
  "cwd": "/repo",
  "protocol_version": "1"
}
```

- `session_id` — conversation UUID; stable across restarts. When `resumed: true`, this is
  the existing conversation's UUID; when `false`, it is freshly generated at startup.
- `resumed` — `true` if `--session-id` or `--resume` resolved to existing history
- `protocol_version` — integer-as-string; bump on breaking protocol changes

#### `system/thinking`

Narrative status during LLM reasoning.

```json
{"type": "system", "subtype": "thinking", "session_id": "...", "message": "Analyzing request..."}
```

#### `system/status`

Generic status change (compaction in progress, awaiting approval, etc.).

```json
{"type": "system", "subtype": "status", "session_id": "...", "status": "compacting"}
```

#### `system/compact_started` and `system/compact_completed`

Emitted when context-window compaction runs (auto or manual).

```json
{"type": "system", "subtype": "compact_started", "session_id": "...", "trigger": "auto", "strategy": "summarize", "usage_percent": 85.2}
```

```json
{"type": "system", "subtype": "compact_completed", "session_id": "...", "turns_removed": 8, "tokens_before": 85000, "tokens_after": 32000, "summary_written": true}
```

- `trigger` — `"auto"` (ContextMonitor threshold crossed) or `"manual"` (future)
- `strategy` — `"summarize"` | `"truncate"` | `"workspace"` (see `src/agent/compaction.rs`)

#### `system/skill_activated`

```json
{"type": "system", "subtype": "skill_activated", "session_id": "...", "skill_names": ["code-review"]}
```

#### `system/reasoning`

Emitted when `--include-events reasoning` or `--verbose` is set.

```json
{"type": "system", "subtype": "reasoning", "session_id": "...", "narrative": "Need to read file first", "decisions": [{"tool_name": "read_file", "rationale": "User asked to explain main.rs"}]}
```

#### `tool_started`

```json
{"type": "tool_started", "session_id": "...", "tool_name": "read_file", "tool_use_id": "t-1", "parameters": {"path": "src/main.rs"}}
```

- `tool_use_id` — stable identifier for correlating with `tool_completed` and for
  `control_request` matching; generated by `NdjsonChannel` if the agent does not supply one
- `parameters` — redacted according to the tool's `sensitive_params()` before emission

#### `tool_completed`

```json
{"type": "tool_completed", "session_id": "...", "tool_name": "read_file", "tool_use_id": "t-1", "success": true, "output_preview": "fn main() { ... }", "error": null}
```

- `output_preview` — truncated to avoid stdout flooding; full output is in the
  corresponding `user` message body when `--verbose` is set
- `error` — populated when `success: false`

#### `assistant`

Emitted when `--verbose` is set. Complete assistant turn as the LLM returned it.

```json
{
  "type": "assistant",
  "session_id": "...",
  "message": {
    "role": "assistant",
    "content": "Let me read main.rs first.",
    "tool_calls": [
      {"id": "t-1", "name": "read_file", "arguments": {"path": "src/main.rs"}}
    ],
    "stop_reason": "tool_use"
  }
}
```

#### `user`

Emitted when `--verbose` is set. Represents tool results fed back into the LLM.

```json
{
  "type": "user",
  "session_id": "...",
  "message": {
    "role": "user",
    "tool_results": [
      {"tool_use_id": "t-1", "name": "read_file", "content": "fn main() { ... }", "is_error": false}
    ]
  }
}
```

#### `control_request`

Emitted when a tool requires caller approval. The caller must respond with a
`control_response` on stdin using the same `request_id`.

```json
{
  "type": "control_request",
  "request_id": "req-1",
  "session_id": "...",
  "request": {
    "subtype": "can_use_tool",
    "tool_name": "shell",
    "parameters": {"command": "rm -rf target/"}
  }
}
```

Agent execution pauses until the matching `control_response` arrives on stdin. No timeout
is enforced (matches Claude Code behavior).

#### `result`

Emitted at the end of each turn. Carries the aggregated response plus usage totals.

```json
{
  "type": "result",
  "subtype": "success",
  "session_id": "...",
  "result": "This main.rs is the program entry point...",
  "duration_ms": 3456,
  "num_turns": 2,
  "usage": {"input_tokens": 1234, "output_tokens": 567, "total_cost_usd": 0.004}
}
```

- `subtype` values: `success` | `error` | `error_max_turns` | `interrupted`
- `num_turns` — number of LLM round-trips in this turn (not total turns in session)
- `usage` — aggregated across all LLM calls in this turn, from `cost_guard.model_usage()`

In `-p` mode, the process exits immediately after emitting `result`. In streaming-input
mode, the process continues waiting for stdin.

#### `error`

Emitted for errors that do not abort the session (parse errors on stdin, transient LLM
errors, etc.).

```json
{"type": "error", "session_id": "...", "message": "LLM provider returned 429: rate limited"}
```

### Input Messages (caller → stdin)

Each line on stdin is parsed as a `NdjsonInput` value.

#### `user`

```json
{"type": "user", "content": "Explain the auth module"}
```

`content` can also be a multimodal array (matches Claude API content parts):

```json
{
  "type": "user",
  "content": [
    {"type": "text", "text": "Review this screenshot"},
    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "..."}}
  ]
}
```

#### `control_response`

```json
{"type": "control_response", "request_id": "req-1", "response": {"behavior": "allow"}}
```

```json
{"type": "control_response", "request_id": "req-1", "response": {"behavior": "deny", "message": "User rejected this action"}}
```

`behavior` values:
- `allow` — one-time approval for this call
- `always` — approve this tool for the rest of the session
- `deny` — reject; optionally include a user-facing message

#### `interrupt`

```json
{"type": "interrupt"}
```

Causes the NdjsonChannel to inject a `/interrupt` `IncomingMessage`, triggering the
existing `process_interrupt()` → `ThreadState::Interrupted` → `check_signals → Stop` path.

## Protocol — Claude Code Compatibility Mode

Activated by `--compat claude-code`. The **input** format is unchanged (both modes parse
`NdjsonInput` the same way, because the `user` message structure is functionally
equivalent). Only **output** events are transformed.

The transformation lives in `src/channels/ndjson/compat.rs` as a pure function
`to_claude_code(event: &NdjsonOutput) -> serde_json::Value` (or `Null` for events that have
no Claude Code equivalent). Key mappings:

| IronClaw Native | Claude Code |
|-----------------|-------------|
| `system/init` | `{type: "system", subtype: "init", permissionMode, apiKeySource, …}` |
| `system/thinking` | (dropped) |
| `system/status (compacting)` | `{type: "system", subtype: "status", status: "compacting"}` |
| `system/compact_completed` | `{type: "system", subtype: "compact_boundary", compact_metadata: {trigger, pre_tokens}}` |
| `system/skill_activated` | (dropped) |
| `system/reasoning` | (dropped) |
| `tool_started` | (dropped — embedded in `assistant.content[].tool_use`) |
| `tool_completed` | (dropped — embedded in `user.content[].tool_result`) |
| `assistant` | `{type: "assistant", message: {role, content: [{type: "text"|"tool_use", …}], stop_reason}}` |
| `user` (tool_results) | `{type: "user", message: {role: "user", content: [{type: "tool_result", tool_use_id, content, is_error}]}}` |
| `control_request` | `{type: "control_request", request_id, request: {subtype: "can_use_tool", tool_name, input}}` |
| `result` | `{type: "result", subtype, session_id, result, duration_ms, num_turns, usage, …}` |
| `error` | `{type: "error", message}` |

Events that have no Claude Code equivalent are suppressed (return `Value::Null`; the emit
function skips `Null`). This ensures the compat stream is a clean subset that Claude Agent
SDK consumers can parse without errors.

Dropped IronClaw-specific events in compat mode: `tool_started`, `tool_completed`,
`system/thinking`, `system/reasoning`, `system/skill_activated`, and any
plan/mission/child-thread events (if present in `StatusUpdate`).

## Architecture

### Module Layout

```
src/channels/ndjson/
├── mod.rs            NdjsonChannel impl Channel trait + builder
├── types.rs          NdjsonInput / NdjsonOutput / SystemEvent / nested types
├── compat.rs         IronClaw → Claude Code output conversion
├── stdin_reader.rs   stdin NDJSON parsing + input routing
├── approval.rs       ApprovalState: control_request / control_response correlation
└── session.rs        resolve_session_id (--session-id / --resume)
```

Approximate sizes: 250 + 200 + 150 + 120 + 80 + 60 = ~860 lines of new code.

Each module has a single responsibility and is independently testable.

### NdjsonChannel Data Flow

```
┌──────────────────┐
│ External caller  │
│ (IDE/SDK/script) │
└─┬──────────────▲─┘
  │ stdin         │ stdout
  │ NDJSON        │ NDJSON
  ▼               │
┌────────────────────────────────────────────┐
│ NdjsonChannel                               │
│                                             │
│ start():                                    │
│   spawn StdinReader task:                   │
│     parse NdjsonInput line-by-line          │
│     ├─ User → IncomingMessage (with scope) │
│     ├─ ControlResponse → ApprovalState     │
│     └─ Interrupt → "/interrupt" message    │
│   emit system/init                          │
│   if initial_prompt: send first message    │
│                                             │
│ respond(msg, OutgoingResponse):             │
│   emit assistant event                      │
│   emit result event with usage totals       │
│                                             │
│ send_status(StatusUpdate):                  │
│   map to NdjsonOutput                       │
│   if compat_mode = ClaudeCode:              │
│     apply compat.rs transformation          │
│   acquire stdout_lock, write line           │
│   on ApprovalNeeded: register pending,      │
│     emit control_request, await response    │
└──┬───────────────────────────▲─────────────┘
   │ IncomingMessage            │ StatusUpdate
   │ (with conversation_scope)  │ OutgoingResponse
   ▼                            │
┌────────────────────────────────────────────┐
│ Agent loop (unchanged)                      │
│  - SessionManager.resolve_thread            │
│  - maybe_hydrate_thread (DB → chat history) │
│  - ContextMonitor.suggest_compaction        │
│  - ChatDelegate / run_agentic_loop          │
│  - Tool execution with approval pauses      │
└────────────────────────────────────────────┘
```

### Key Types

```rust
// src/channels/ndjson/mod.rs

pub struct NdjsonChannel {
    user_id: String,
    session_id: Arc<Mutex<Option<String>>>,
    initial_prompt: Option<String>,
    streaming_input: bool,
    compat_mode: CompatMode,
    verbose: bool,
    include_events: EventFilter,
    tools: Vec<String>,
    model_name: String,
    pending_approvals: ApprovalState,
    msg_tx: Arc<Mutex<Option<mpsc::Sender<IncomingMessage>>>>,
    conversation_store: Option<Arc<dyn ConversationStore>>,
    stdout_lock: Arc<Mutex<()>>,
    turn_usage: Arc<Mutex<TurnUsageAccumulator>>,
}

pub enum CompatMode { Ironclaw, ClaudeCode }

pub struct EventFilter {
    pub reasoning: bool,
    pub hooks: bool,
    pub cost: bool,
    pub partial_messages: bool,
}
```

### StatusUpdate → NdjsonOutput Mapping

| StatusUpdate variant | IronClaw Native NdjsonOutput | Claude Code compat |
|----------------------|------------------------------|--------------------|
| `Thinking` | `system/thinking` | (dropped) |
| `ToolStarted` | `tool_started` | (dropped — folded into `assistant`) |
| `ToolCompleted` | `tool_completed` | (dropped — folded into `user`) |
| `ToolResult` | (folded into `tool_completed`) | `user.tool_result` |
| `StreamChunk` | (deferred — streaming support) | `stream_event` (deferred) |
| `Status("compacting")` | `system/status` or `system/compact_started` | `system/status` |
| `CompactionStarted` (new) | `system/compact_started` | `system/status (compacting)` |
| `CompactionCompleted` (new) | `system/compact_completed` | `system/compact_boundary` |
| `ApprovalNeeded` | `control_request` | `control_request` |
| `TurnCost` | accumulated into `result.usage` | accumulated into `result.usage` |
| `ReasoningUpdate` | `system/reasoning` (if filter) | (dropped) |
| `SkillActivated` | `system/skill_activated` | (dropped) |
| `JobStarted` / job events | (dropped in v1) | (dropped in v1) |
| `ImageGenerated` | `system/image_generated` | (dropped) |
| `AuthRequired` | `system/auth_required` | (dropped) |
| `Suggestions` | `system/suggestions` (if filter) | (dropped) |
| Other | logged at debug level | logged at debug level |

## Integration with Existing System

### Zero-Change Subsystems

- Agent loop (`src/agent/agentic_loop.rs`)
- Session manager (`src/agent/session_manager.rs`)
- Context monitor (`src/agent/context_monitor.rs`)
- Compaction logic (`src/agent/compaction.rs`) — internal logic unchanged
- Tool execution (`src/tools/execute.rs`)
- Database (`src/db/`)
- LLM providers (`src/llm/`)
- Safety / skills / hooks

### Changed Subsystems

**`src/channels/channel.rs`** — add two new `StatusUpdate` variants:

```rust
pub enum StatusUpdate {
    // ... existing variants ...

    /// Context-window compaction has begun.
    CompactionStarted {
        strategy: String,      // "summarize" | "truncate" | "workspace"
        trigger: String,       // "auto" | "manual"
        usage_percent: f32,
    },

    /// Context-window compaction finished.
    CompactionCompleted {
        turns_removed: usize,
        tokens_before: usize,
        tokens_after: usize,
        summary_written: bool,
    },
}
```

**`src/agent/thread_ops.rs`** — emit the new variants in `handle_auto_compaction()` and
`process_compact()` before and after calling `ContextCompactor.compact()`.

**`src/channels/repl.rs`** — add `match` arms for the two new variants, printing a dim
status line (preserves backward compatibility with existing REPL UX).

**`src/channels/web/mod.rs`** and `crates/ironclaw_common/src/event.rs` — optionally add
corresponding `AppEvent` variants and map the new `StatusUpdate` values to them so the web
frontend also sees structured compaction events. Optional; not required for NDJSON to work.

**`src/cli/mod.rs`** — new global flags (listed in Usage Overview above). Each flag only
mutates the `Cli` struct; no new subcommands.

**`src/app.rs`** — channel selection logic: when `cli.output_format != Text`, construct
and register an `NdjsonChannel`; skip REPL and all other channel registrations.

### Process Scope

The restriction "only NdjsonChannel runs when `--output-format stream-json`" is strictly
**per-process**. Other concurrently running ironclaw instances are unaffected. Cross-process
coordination happens through the shared database: one process can write to a conversation
that another process later resumes via `--session-id`.

Concurrent writes to the same conversation from multiple processes are not specifically
protected by NDJSON mode — they fall under the same semantics as existing REPL + web
gateway coexistence, which relies on the `SessionManager` lock within a process and
database transactions across processes. Callers who spawn multiple ironclaw processes
against the same conversation UUID are responsible for serializing their writes.

## Session Persistence & Resume

IronClaw already persists every conversation to the database:

- `conversations` table — one row per conversation (UUID, channel, user_id, metadata,
  timestamps)
- `conversation_messages` table — one row per message (role: user|assistant|tool_calls,
  content)

`maybe_hydrate_thread()` in `src/agent/thread_ops.rs` already handles loading by UUID,
ownership verification, and context reconstruction via `rebuild_chat_messages_from_db()`.

### Resume Flow

1. `NdjsonChannel::resolve_session_id()` converts CLI flags to an optional UUID:
   - `--session-id <uuid>` → parse and validate, return `Some(uuid)`
   - `--resume` → query `ConversationStore::list_conversations(user_id, limit=1)` and
     return the most recent UUID; if none exists, return `None` and start fresh (not an
     error)
   - neither → `None` (new session)
2. Each `IncomingMessage` injected from stdin has its `conversation_scope_id` set to the
   resolved UUID
3. On first message, `Agent::handle_message` calls `maybe_hydrate_thread()` which:
   - Verifies `conversation_belongs_to_user()`
   - Calls `store.list_conversation_messages(uuid)`
   - Calls `rebuild_chat_messages_from_db()` to reconstruct the full `ChatMessage` list
     (user, assistant, tool_calls, tool_results)
   - Inserts the thread into the in-memory session
4. `ContextMonitor` runs its usual check before the new turn. If the loaded history
   exceeds the 80% threshold, auto-compaction runs transparently before the LLM call.
5. The new turn proceeds with the full prior context available to the LLM.

### Resume Edge Cases

- **Unknown UUID** — hydration returns rejection → emit `error` event + `result/error` →
  process exits (code 1)
- **UUID not owned by current user** — same as unknown UUID (fail-closed)
- **Empty conversation** — treat as new session (no error)
- **`--resume` with no prior conversations** — start fresh session (no error)
- **Invalid UUID format on CLI** — exit before channel starts, code 2

## Context Window Compaction

Matches Claude Code semantics: compaction is fully automatic, callers observe but do not
trigger. IronClaw's existing `ContextMonitor` already runs before each new turn
(`process_user_input()`) and calls `ContextCompactor.compact()` when usage crosses the 80%
threshold. No behavioral change here.

The only change is structured event emission:

1. Before `ContextCompactor.compact()` starts → emit
   `StatusUpdate::CompactionStarted { strategy, trigger, usage_percent }`
2. After compaction returns successfully → emit
   `StatusUpdate::CompactionCompleted { turns_removed, tokens_before, tokens_after, summary_written }`
3. On compaction failure → emit `StatusUpdate::Status("compaction failed")` (existing path),
   keep turns intact, proceed with the unmodified context

NdjsonChannel maps these to `system/compact_started` and `system/compact_completed` events
(or `system/status` + `system/compact_boundary` in claude-code compat mode).

No new NDJSON input messages are required — callers cannot trigger compaction
programmatically in v1. This intentionally matches the Claude Code surface.

## Tool Approval Flow

1. Agent loop reaches a tool with `ApprovalRequirement::Always` or
   `UnlessAutoApproved` (without auto-approve)
2. `ChatDelegate` returns `LoopOutcome::NeedApproval(pending)`
3. Dispatcher emits `StatusUpdate::ApprovalNeeded { request_id, tool_name, parameters, allow_always, description }`
4. `NdjsonChannel::send_status()` registers the `request_id` with `ApprovalState` and
   emits a `control_request` event
5. Agent loop is paused; the thread is in `AwaitingApproval` state (existing mechanism)
6. External caller writes `{"type": "control_response", "request_id": "...", "response": {...}}` to stdin
7. `StdinReader` routes the response via `ApprovalState.resolve()` which sends it through
   the oneshot channel
8. The resolver translates the response into a short-form text reply (`y` / `a` / `n`)
   and injects it as an `IncomingMessage`, matching how `ReplChannel` handles approval
   replies today
9. Agent loop resumes and proceeds with allowed / denied result

No timeout. The process waits indefinitely until the caller responds or exits (EOF).

## Interrupt Flow

1. External caller writes `{"type": "interrupt"}` to stdin
2. `StdinReader` injects an `IncomingMessage` with content `/interrupt`
3. `SubmissionParser::parse()` recognizes `/interrupt` → `Submission::Interrupt`
4. `Agent::handle_message` dispatches to `process_interrupt()`
5. `process_interrupt()` sets `ThreadState::Interrupted` on the current thread
6. `ChatDelegate::check_signals()` sees `Interrupted` on the next loop iteration, returns
   `LoopSignal::Stop`
7. `run_agentic_loop()` exits with `LoopOutcome::Stopped`
8. `NdjsonChannel` emits `result` with `subtype: "interrupted"`
9. In `-p` mode, process exits (code 130). In streaming-input mode, thread is reset to
   `Idle` and ready for the next stdin message.

### Interrupt Granularity

Interrupts take effect **between** agent loop iterations, not in the middle of an LLM call
or tool execution. Any in-flight LLM call or tool must complete before the interrupt is
honored. This matches Claude Code semantics.

## Stdout Contamination Protection

In NDJSON mode, the process's stdout is reserved exclusively for NDJSON events. Three
layers of protection:

1. **tracing output on stderr** — IronClaw's `tracing_fmt.rs` already writes to stderr
   (verified; no change needed)
2. **disable other channels per-process** — REPL / Web / Signal / Telegram / etc. are not
   registered when `--output-format` is `stream-json` or `json`
3. **stdout write lock** — `NdjsonChannel::emit()` acquires `stdout_lock` before writing,
   serializing output across all async tasks within the process

Audit tasks in the implementation plan:
- Grep for `println!` / `print!` in code paths reachable during NDJSON mode; convert to
  `eprintln!` / `tracing::debug!` or gate behind a `!ndjson_mode` check
- Verify no third-party crate writes to stdout during normal operation (rustyline, etc.)
- Verify `rustyline` is not initialized when NdjsonChannel is active

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Invalid JSON on stdin | Emit `error` event; continue reading next line |
| Unknown `type` on stdin | Emit `error` event; continue |
| Invalid `--session-id` UUID format | Exit before channel starts, code 2 |
| `--session-id` not found in DB | Emit `error` event + `result/error`; exit code 1 |
| `--session-id` not owned by user | Same as not found (fail-closed) |
| LLM call failure | Emit `error` event + `result/error`; exit in `-p` mode, continue in streaming mode |
| Tool execution failure | Emit `tool_completed` with `success: false, error: ...`; continue |
| Tool approval timeout | No timeout (matches Claude Code); wait for caller response |
| Stdin EOF while streaming | Graceful exit (code 0) |
| Compaction failure | Emit `system/status` event; keep turns; continue |
| Signal SIGINT / SIGTERM | Attempt graceful drain; exit code 130 |

## Testing Strategy

### Unit Tests

Per-module in `src/channels/ndjson/`:

- `types.rs` — round-trip serialization of each `NdjsonInput` / `NdjsonOutput` variant;
  unknown field handling (forward compatibility)
- `compat.rs` — for each `NdjsonOutput` variant, assert the claude-code translation
  matches a fixture JSON
- `stdin_reader.rs` — malformed JSON handling, partial lines, empty lines, unknown
  message types
- `approval.rs` — register → resolve happy path; unknown `request_id` (ignore); multiple
  concurrent pending approvals; resolve with both allow and deny payloads
- `session.rs` — `--session-id` valid UUID, invalid UUID, `--resume` with/without prior
  history, both flags unset

### Integration Tests (`tests/ndjson_*.rs`)

Drive `NdjsonChannel` end-to-end with a mock agent / real agent loop:

1. `test_single_prompt_mode` — `-p` flag sends one message, process exits after `result`
2. `test_streaming_input_multi_turn` — two user messages over stdin; assert second message
   sees context from first
3. `test_session_resume_via_session_id` — first invocation stores a conversation, second
   invocation with `--session-id` continues it; assert LLM context includes prior messages
4. `test_session_resume_via_resume_flag` — `--resume` picks up most recent conversation
5. `test_tool_approval_allow_flow` — emit `control_request`, write `control_response` with
   `allow`, assert tool executes
6. `test_tool_approval_deny_flow` — same but with `deny`, assert tool is blocked
7. `test_interrupt_mid_session` — send a long-running turn, write `interrupt`, assert
   `result/interrupted` is emitted
8. `test_compaction_events_emitted` — force context above threshold, assert
   `compact_started` and `compact_completed` events appear
9. `test_claude_code_compat_output` — run with `--compat claude-code`, assert output
   matches fixture files in Claude Code format
10. `test_invalid_session_id_uuid` — exit code 2
11. `test_unknown_session_id` — exit code 1 with `error` event
12. `test_malformed_stdin_json` — emit `error`, continue reading

### E2E Tests (`tests/e2e/`)

Python-based scripts in the existing Playwright/E2E framework:

- Spawn `ironclaw -p "..." --output-format stream-json` as a subprocess, parse stdout
  NDJSON, assert expected event sequence
- Spawn with `--input-format stream-json`, drive a multi-turn conversation from Python,
  verify session persistence across a simulated process restart

### Regression Test

Per the project's `review-discipline.md`, any bug fix during implementation must include a
regression test. The most likely regression points:

- stdout contamination (a newly introduced `println!` leaks into NDJSON output)
- UUID parsing edge cases (whitespace, case)
- ApprovalState leak (pending approvals not cleaned up on interrupt)

## Observability

- All NDJSON events are already tagged with `session_id`; structured logging from
  `NdjsonChannel` uses `tracing::debug!` (not `info!` — would contaminate stdout via log
  formatter if misconfigured)
- The turn count, duration, and usage totals flow into the `result` event from the
  existing `cost_guard.model_usage()` tracking
- For debugging protocol issues, `RUST_LOG=ironclaw::channels::ndjson=trace` will log
  every event emitted and every stdin line received

## Open Questions

1. **Claude Code `tool_use_id` format** — Claude Code uses `toolu_<hash>` style IDs.
   IronClaw's agentic loop uses synthetic `turn{N}_{M}` IDs. For compat mode, should we
   generate Claude-style IDs, or pass through IronClaw IDs? **Proposed default**: pass
   through IronClaw IDs; document that they may not match Claude Code conventions.

2. **Multimodal input handling** — when the caller sends a `user` message with an image
   content part, where does the image end up? `IncomingMessage` has an `attachments` field
   that accepts images. Proposed: detect `content` array with `image` parts and convert to
   `IncomingAttachment`. Scope for v1: text only; multimodal support deferred.

3. **`--include-events partial_messages`** — this flag corresponds to Claude Code's
   `--include-partial-messages`, which streams token-by-token. IronClaw does not yet have
   LLM-level streaming. Proposed: accept the flag but treat it as a no-op in v1; actual
   streaming is a future enhancement when `LlmProvider::stream_with_tools()` lands.

4. **Should `json` output format (single object, not stream) be implemented in v1?**
   Proposed: yes — trivial wrapper that collects events and emits a single JSON object at
   the end. Matches Claude Code's `--output-format json` flag. Deferred only if
   implementation complexity escalates.

## Out of Scope for v1

- LLM token-by-token streaming (`stream_event` events) — emit `assistant` whole-message
  events only
- Programmatic compaction trigger via NDJSON
- Multimodal input support (images/audio in `user` content parts)
- Hook lifecycle events (`--include-events hooks`) — flag accepted but inert
- MCP server status events over NDJSON
- Shell completion for new flags (trivial; can be added later)
- Per-tool permission policies (`--allowedTools` equivalent) — deferred to a future phase

## Implementation Phasing

**Phase 1 — Core NDJSON channel (MVP)**

- New module `src/channels/ndjson/` with all six files
- New `StatusUpdate::CompactionStarted` / `CompactionCompleted` variants
- Emit new variants in `thread_ops.rs` around compactor calls
- Handle new variants in `ReplChannel::send_status` (dim status line)
- New CLI flags in `src/cli/mod.rs`
- Channel selection logic in `src/app.rs`
- Unit tests for all modules
- Integration tests for single-prompt, streaming-input, resume, approval, interrupt,
  compaction, compat mode, and error cases

**Phase 2 — Polish**

- `AppEvent` variants for compaction events (web gateway UX)
- `--output-format json` (single-object collected mode)
- Stdout contamination audit and fixes
- Shell completion for new flags
- E2E scripts in `tests/e2e/`

**Phase 3 — Future enhancements (separate specs)**

- LLM-level streaming (`stream_event` events) — requires `LlmProvider` trait change
- Multimodal input support
- Programmatic permission policies
- Hook lifecycle events
