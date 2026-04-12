# NDJSON CLI Mode — E2E Verification Design

**Date**: 2026-04-12
**Status**: Approved
**Depends on**: `2026-04-11-ndjson-cli-mode-design.md`

## Summary

End-to-end verification suite for the NDJSON CLI mode feature (`feature/ndjson-cli-mode`
branch). Tests run as Python pytest scenarios under `tests/e2e/scenarios/ndjson/`, driving
the ironclaw binary as a subprocess via stdin/stdout — the exact interface external callers
(IDE extensions, SDKs, scripts) use in production.

All tests use **live LLM calls** (`LLM_BACKEND=zai_anthropic`) with real API keys resolved
through ironclaw's normal startup flow (settings/secrets store/OS keychain). No mocking.

## Goals

1. Verify the complete NDJSON protocol contract from an external caller's perspective
2. Cover all seven functional domains: protocol basics, multi-turn streaming, tool approval,
   interrupt, session resume, Claude Code compat mode, error handling
3. Use real LLM responses for high-fidelity validation
4. Fail loudly when prerequisites (API key, binary) are missing — never skip silently

## Non-Goals

- Token-by-token streaming validation (deferred — `LlmProvider` streaming is a separate effort)
- Performance benchmarking or latency assertions
- Testing ironclaw internals (unit/integration tests cover those)

## Directory Structure

```
tests/e2e/scenarios/ndjson/
├── __init__.py
├── conftest.py          # Fixtures: binary, env, subprocess helpers, LLM health check
├── test_protocol.py     # Basic protocol contract
├── test_multiturn.py    # Multi-turn streaming interaction
├── test_approval.py     # Tool approval request/response flow
├── test_interrupt.py    # Interrupt flow
├── test_resume.py       # Session persistence and resume
├── test_compat.py       # Claude Code compatibility mode
└── test_errors.py       # Error handling and edge cases
```

## Test Infrastructure

### conftest.py Fixtures

#### `ironclaw_binary` (session scope)

Reuses the parent `conftest.py` fixture. Resolves `target/debug/ironclaw`; triggers
`cargo build --no-default-features --features libsql` if the binary is absent.

#### `ndjson_env` (session scope)

Builds the subprocess environment dictionary. Inherits the current process environment
(so ironclaw can resolve API keys from `~/.ironclaw/` settings, encrypted secrets store,
or OS keychain), then overlays:

```
LLM_BACKEND=zai_anthropic
DATABASE_BACKEND=libsql
LIBSQL_PATH=<session-tempdir>/ndjson-e2e.db
ONBOARD_COMPLETED=true
SANDBOX_ENABLED=false
HEARTBEAT_ENABLED=false
ROUTINES_ENABLED=false
EMBEDDING_ENABLED=false
CLI_ENABLED=false
```

API key environment variables are **not** set by the test — ironclaw resolves them through
its normal `inject_llm_keys_from_secrets()` and OS credential store flow at startup.

#### `llm_health_check` (session scope, autouse)

Spawns `ironclaw -p "say ok" --output-format stream-json` with the `ndjson_env`
environment as a one-shot health check before any test runs. If the process fails or the
output contains no `result/success` event, calls `pytest.fail()` with a clear message:

```
pytest.fail(
    "LLM health check failed for zai_anthropic — API key not configured or LLM "
    "unreachable. Configure ironclaw with a valid API key before running NDJSON E2E tests."
)
```

This is **not** `pytest.skip()` — missing prerequisites are a hard failure.

#### `run_print` (session scope)

Helper function for single-shot `-p` mode tests:

```python
def run_print(prompt: str, extra_args: list[str] = None) -> tuple[int, list[dict], str]:
    """Spawn ironclaw -p <prompt> --output-format stream-json, wait for exit.
    
    Returns (exit_code, ndjson_lines, stderr).
    ndjson_lines is a list of parsed JSON dicts, one per stdout line.
    """
```

Timeout: 120 seconds. Raises on timeout.

#### `open_streaming` (session scope)

Helper function for multi-turn streaming tests:

```python
def open_streaming(extra_args: list[str] = None) -> NdjsonProcess:
    """Spawn ironclaw --output-format stream-json --input-format stream-json.
    
    Returns an NdjsonProcess wrapper for interactive stdin/stdout communication.
    """
```

### NdjsonProcess Class

Wraps a `subprocess.Popen` with piped stdin/stdout for interactive NDJSON communication.

```python
class NdjsonProcess:
    """Interactive NDJSON subprocess wrapper."""
    
    proc: subprocess.Popen
    
    def send(self, obj: dict) -> None:
        """Write a JSON object as a single line to stdin."""
    
    def send_user(self, content: str) -> None:
        """Shorthand: send {"type": "user", "content": content}."""
    
    def send_control_response(
        self, request_id: str, behavior: str, message: str | None = None
    ) -> None:
        """Shorthand: send {"type": "control_response", ...}."""
    
    def send_interrupt(self) -> None:
        """Shorthand: send {"type": "interrupt"}."""
    
    def read_event(self, timeout: float = 120) -> dict:
        """Read one NDJSON line from stdout. Raises TimeoutError after timeout."""
    
    def read_until(
        self, type: str, subtype: str | None = None, timeout: float = 120
    ) -> dict:
        """Read events until one matches (type, subtype). Returns the match.
        Raises TimeoutError if not found within timeout."""
    
    def collect_until_result(self, timeout: float = 120) -> list[dict]:
        """Read all events until a 'result' event. Returns the full list
        including the result event."""
    
    def close(self) -> int:
        """Close stdin (EOF), wait for process exit, return exit code."""
```

All read operations use a **120-second timeout** to accommodate real LLM latency.

### Test Isolation

Each test file gets its own temporary directory for `LIBSQL_PATH`. The `ndjson_env`
fixture creates a session-scoped tempdir, and tests that need cross-invocation isolation
(e.g., `test_resume.py`) create function-scoped tempdirs to avoid data pollution.

## Test Scenarios

### `test_protocol.py` — Basic Protocol Contract

| Test | Assertion |
|------|-----------|
| `test_init_event_shape` | First event is `system/init` with fields: `session_id` (valid UUID), `protocol_version` = `"1"`, `model` (non-empty string), `tools` (non-empty list), `resumed` = `false` |
| `test_result_event_terminates_print_mode` | Last event is `result` with `subtype` in `{"success", "error"}`, has `duration_ms` (int >= 0), `num_turns` (int >= 1), `usage` object with `input_tokens`, `output_tokens`, `total_cost_usd` |
| `test_all_lines_valid_ndjson` | Every stdout line parses as valid JSON with a `type` field |
| `test_session_id_consistent` | All events with a `session_id` field share the same value |
| `test_exit_code_zero_on_success` | Exit code is 0 when `result/success` is emitted |

### `test_multiturn.py` — Multi-Turn Streaming

| Test | Assertion |
|------|-----------|
| `test_two_turns_each_produces_result` | In streaming mode, send two `user` messages sequentially (waiting for `result` after each). Both turns produce their own `result` event. |
| `test_second_turn_sees_prior_context` | First message: "remember the code word: elephant". Second message: "what is the code word?". Assert the second `result` or preceding `assistant` events contain "elephant". |
| `test_eof_closes_gracefully` | After one completed turn, close stdin (EOF). Process exits with code 0. |

### `test_approval.py` — Tool Approval Flow

| Test | Assertion |
|------|-----------|
| `test_approve_tool_allows_execution` | Prompt LLM to run a shell command (e.g., "use the shell tool to run `echo hello_from_ironclaw`"). Wait for `control_request` event with `subtype: "can_use_tool"`. Reply with `{"behavior": "allow"}`. Collect remaining events. Assert `result/success` is reached and output references the command result. |
| `test_deny_tool_stops_execution` | Same prompt, but reply with `{"behavior": "deny"}`. Assert `result` is emitted (the agent handles denial gracefully) and no tool execution output appears in the result text. |

### `test_interrupt.py` — Interrupt Flow

| Test | Assertion |
|------|-----------|
| `test_interrupt_during_processing` | In streaming mode, send a complex prompt (e.g., "write a detailed 2000-word essay about distributed systems"). Immediately send `interrupt`. Assert a `result` event is received with `subtype: "interrupted"`. |

### `test_resume.py` — Session Persistence and Resume

| Test | Assertion |
|------|-----------|
| `test_resume_by_session_id` | First invocation: `-p "remember the secret word: pineapple"` — extract `session_id` from `system/init`. Second invocation: `-p "what was the secret word I told you?" --session-id <id>` — assert the response text contains "pineapple" and `init.resumed` = `true`. Both invocations share the same `LIBSQL_PATH`. |
| `test_resume_flag_picks_latest` | First invocation creates a conversation. Second invocation with `--resume` (no explicit session-id). Assert `init.resumed` = `true` and `init.session_id` matches the first invocation's session_id. |
| `test_resume_unknown_uuid_fails` | `--session-id <random-valid-uuid>` with a UUID that has no corresponding conversation. Assert non-zero exit code or `result/error` event. |

### `test_compat.py` — Claude Code Compatibility Mode

All tests pass `--compat claude-code` as an extra argument.

| Test | Assertion |
|------|-----------|
| `test_compat_init_event_shape` | `system/init` event is present and well-formed under compat mode. |
| `test_compat_drops_ironclaw_events` | Collect all events from a `-p` run. Assert none have `type` in `{"tool_started", "tool_completed"}` or `subtype` in `{"thinking", "skill_activated", "reasoning"}`. Only Claude Code-compatible event types appear. |
| `test_compat_result_event_shape` | `result` event contains `session_id`, `duration_ms`, `num_turns`, `usage` — matching Claude Code's expected schema. |

### `test_errors.py` — Error Handling

| Test | Assertion |
|------|-----------|
| `test_invalid_json_on_stdin` | In streaming mode, write a line of plain text (not JSON). Assert an `error` event is emitted. Then send a valid `user` message and assert the process still responds with a `result` — the invalid line does not crash the process. |
| `test_invalid_session_id_format` | Pass `--session-id not-a-uuid`. Assert exit code = 2 (CLI argument error). |
| `test_conflicting_flags` | Pass `--session-id <uuid> --resume` together. Assert exit code = 2 (mutually exclusive flags). |

## Design Constraints

- **Timeout**: 120 seconds per read operation (real LLM latency)
- **Isolation**: Each test file uses an independent `LIBSQL_PATH` tempdir; `test_resume.py`
  shares a tempdir across related invocations within a single test function
- **No mocking**: All LLM calls are real, using `zai_anthropic` backend
- **Fail loudly**: API key not configured → `pytest.fail()`, not `pytest.skip()`
- **LLM backend**: Fixed to `zai_anthropic`; API key resolved by ironclaw's normal startup
  flow (settings DB → secrets store → OS keychain → env vars)
- **No browser**: Pure subprocess tests, no Playwright dependency

## Running the Tests

```bash
cd tests/e2e
source .venv/bin/activate

# Run all NDJSON E2E tests
pytest scenarios/ndjson/ -v

# Run a specific test file
pytest scenarios/ndjson/test_protocol.py -v

# Run with extended timeout (per-test level, default 120s from pyproject.toml)
pytest scenarios/ndjson/ -v --timeout=180
```

Prerequisites:
- ironclaw configured with a valid `zai_anthropic` API key (via onboarding wizard,
  `~/.ironclaw/settings.json`, secrets store, or `ANTHROPIC_API_KEY` env var)
- Python >= 3.11 with `pytest`, `pytest-asyncio`, `pytest-timeout` installed
