# NDJSON CLI Mode — Integration Guide

IronClaw can be driven as a subprocess over stdin/stdout using newline-delimited JSON (NDJSON). This is the interface for IDE extensions, scripts, and SDKs that need to interact with IronClaw programmatically.

## Quick Start

### Single-shot query

```bash
ironclaw -p "explain main.rs" --output-format stream-json --no-onboard 2>/dev/null
```

Output (one JSON object per line):

```
{"type":"system","subtype":"init","session_id":"a1b2...","model":"glm-5","tools":[...],"protocol_version":"1","resumed":false}
{"type":"system","subtype":"thinking","session_id":"a1b2...","message":"Processing..."}
{"type":"result","subtype":"success","session_id":"a1b2...","result":"...","duration_ms":3456,"num_turns":1,"usage":{"input_tokens":234,"output_tokens":567,"total_cost_usd":"0.004"}}
```

The process exits after emitting the `result` event.

### Multi-turn streaming

```bash
ironclaw --output-format stream-json --input-format stream-json --no-onboard 2>/dev/null
```

Then write JSON lines to stdin:

```json
{"type":"user","content":"hello"}
```

Wait for a `result` event, then send the next message. Close stdin (EOF) to end the session.

### Resume a prior conversation

```bash
# By session ID (from a previous init event)
ironclaw -p "continue" --output-format stream-json --session-id <uuid> --no-onboard 2>/dev/null

# Or resume the most recent conversation
ironclaw -p "continue" --output-format stream-json --resume --no-onboard 2>/dev/null
```

## CLI Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `-p, --print <PROMPT>` | string | — | Send one prompt, emit events, exit |
| `--output-format <FMT>` | enum | `text` | `text` / `stream-json` / `json` |
| `--input-format <FMT>` | enum | `text` | `text` / `stream-json` |
| `--session-id <UUID>` | string | — | Resume a specific session (conflicts with `--resume`) |
| `--resume` | bool | false | Resume the most recent session (conflicts with `--session-id`) |
| `--verbose` | bool | false | Include intermediate `assistant` and `user` events |
| `--include-events <TYPES>` | comma-sep | — | Extra events: `reasoning`, `hooks`, `cost`, `partial_messages` |
| `--compat <MODE>` | enum | `ironclaw` | `ironclaw` / `claude-code` |
| `--no-onboard` | bool | false | **Required for programmatic use.** Skips the interactive setup wizard |
| `--no-db` | bool | false | Skip database connection (testing only) |

**Validation rules:**
- `--input-format stream-json` requires `--output-format stream-json`
- `--compat claude-code` requires `--output-format stream-json`
- `--session-id` and `--resume` are mutually exclusive
- `-p` and `-m` are mutually exclusive

## Output Events (stdout)

Every line on stdout is a complete JSON object with a `type` field. The `result` event is always the last event in a turn.

### `system/init`

Emitted once at startup. Contains session context.

```json
{"type":"system","subtype":"init","session_id":"<uuid>","resumed":false,"model":"glm-5","tools":["shell","read_file",...],"cwd":"/repo","protocol_version":"1"}
```

### `system/thinking`

Narrative status during LLM reasoning.

```json
{"type":"system","subtype":"thinking","session_id":"...","message":"Processing..."}
```

### `system/status`

Generic status updates.

```json
{"type":"system","subtype":"status","session_id":"...","status":"compacting"}
```

### `tool_started`

Tool execution has begun.

```json
{"type":"tool_started","session_id":"...","tool_name":"shell","tool_use_id":"t-1","parameters":{"command":"ls"}}
```

### `tool_completed`

Tool execution finished.

```json
{"type":"tool_completed","session_id":"...","tool_name":"shell","tool_use_id":"t-1","success":true,"output_preview":"file1.rs\nfile2.rs"}
```

### `control_request`

A tool requires caller approval before execution. See [Tool Approval Flow](#tool-approval-flow).

```json
{"type":"control_request","request_id":"req-1","session_id":"...","request":{"subtype":"can_use_tool","tool_name":"shell","parameters":{"command":"rm -rf target/"},"allow_always":true}}
```

### `result`

End of a turn. **Always the last event.** The process exits after this in `-p` mode.

```json
{"type":"result","subtype":"success","session_id":"...","result":"...","duration_ms":3456,"num_turns":1,"usage":{"input_tokens":234,"output_tokens":567,"total_cost_usd":"0.004"}}
```

| `subtype` | Meaning |
|-----------|---------|
| `success` | Normal completion |
| `error` | Runtime error |
| `error_max_turns` | Exceeded maximum LLM iterations |
| `interrupted` | Cancelled by `interrupt` message |

### `error`

Non-fatal error (e.g., invalid stdin input). The session continues.

```json
{"type":"error","session_id":"...","message":"Failed to parse stdin line: invalid JSON"}
```

### Verbose-only events

These appear only when `--verbose` is set:

- **`assistant`** — Full LLM response with `message.content`, `message.tool_calls`, `message.stop_reason`
- **`user`** — Tool results sent back to LLM with `message.tool_results`

## Input Messages (stdin)

Each line on stdin must be a complete JSON object with a `type` field.

### `user`

Send a message to the agent.

```json
{"type":"user","content":"explain the auth module"}
```

Content can also be a multimodal array:

```json
{"type":"user","content":[{"type":"text","text":"review this"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"..."}}]}
```

### `control_response`

Reply to a `control_request`.

```json
{"type":"control_response","request_id":"req-1","response":{"behavior":"allow"}}
```

| `behavior` | Effect |
|------------|--------|
| `allow` | Approve this single tool call |
| `always` | Approve this tool for the rest of the session |
| `deny` | Reject (optional `"message"` field for reason) |

### `interrupt`

Cancel the current turn.

```json
{"type":"interrupt"}
```

## Tool Approval Flow

When a tool requires approval, the sequence is:

1. IronClaw emits `control_request` with a `request_id`
2. Agent execution pauses (no timeout)
3. Caller sends `control_response` with the matching `request_id`
4. Agent resumes with the approved/denied result

```
caller                          ironclaw
  │                                │
  │  {"type":"user","content":...} │
  │──────────────────────────────>│
  │                                │  LLM decides to use a tool
  │  {"type":"control_request",    │
  │   "request_id":"req-1",...}    │
  │<──────────────────────────────│
  │                                │  (paused, waiting)
  │  {"type":"control_response",   │
  │   "request_id":"req-1",        │
  │   "response":{"behavior":"allow"}} │
  │──────────────────────────────>│
  │                                │  tool executes, LLM continues
  │  {"type":"result",...}         │
  │<──────────────────────────────│
```

## Session Resume

Conversations are persisted to the database automatically. Resume across process restarts:

- **`--session-id <uuid>`** — Resume a specific conversation. The `session_id` is in every `system/init` event.
- **`--resume`** — Resume the most recent conversation for the current user.

When resumed, the `init` event has `"resumed": true` and the LLM has access to prior conversation context.

## Claude Code Compatibility Mode

`--compat claude-code` transforms output events to match the Claude Code NDJSON protocol. This allows existing Claude Agent SDK tooling to drive IronClaw without modification.

Key differences from native mode:
- `tool_started` and `tool_completed` events are **dropped** (folded into `assistant`/`user` messages)
- `system/thinking`, `system/reasoning`, `system/skill_activated` are **dropped**
- Input format is unchanged — both modes parse the same `user`, `control_response`, and `interrupt` messages

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Normal completion |
| 1 | Runtime error (LLM failure, DB error, config error) |
| 2 | CLI argument error (invalid UUID, conflicting flags) |
| 130 | Interrupted by signal (SIGINT / SIGTERM) |

## Important Notes

- **Always pass `--no-onboard`** when spawning ironclaw programmatically. Without it, the process may block waiting for interactive setup input.
- **Redirect stderr** (`2>/dev/null` or `2>logfile.txt`). Logging output goes to stderr and is not NDJSON-formatted.
- **Detect turn completion** by watching for `{"type":"result",...}`. It is always the last event in a turn. In `-p` mode, the process exits immediately after.
- **All `session_id` values are UUIDs** and are consistent across all events in a session.
- **`protocol_version`** is `"1"`. Check this field for forward compatibility.
