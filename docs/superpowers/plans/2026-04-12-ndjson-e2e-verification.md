# NDJSON E2E Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a comprehensive E2E test suite for the NDJSON CLI mode using live LLM calls, verifying the full protocol contract from an external caller's perspective.

**Architecture:** Python pytest scenarios in `tests/e2e/scenarios/ndjson/` that spawn ironclaw as a subprocess, communicate via stdin/stdout NDJSON protocol, and assert on event shape, sequencing, and content. All tests use real LLM (`zai_anthropic`), no mocking.

**Tech Stack:** Python 3.11+, pytest, subprocess (sync), `json` stdlib. No new pip dependencies needed beyond existing `tests/e2e/pyproject.toml`.

**Spec:** `docs/superpowers/specs/2026-04-12-ndjson-e2e-verification-design.md`

**Worktree:** All work happens in `/Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode`

---

## File Structure

```
tests/e2e/scenarios/ndjson/
├── __init__.py              # Empty package marker
├── conftest.py              # Fixtures: binary, env, NdjsonProcess, health check
├── test_protocol.py         # 5 tests: init shape, result shape, valid NDJSON, session_id consistency, exit code
├── test_multiturn.py        # 3 tests: two turns, context retention, EOF graceful close
├── test_approval.py         # 2 tests: allow tool, deny tool
├── test_interrupt.py        # 1 test: interrupt during processing
├── test_resume.py           # 3 tests: resume by session_id, --resume flag, unknown UUID
├── test_compat.py           # 3 tests: compat init shape, drops ironclaw events, compat result shape
└── test_errors.py           # 3 tests: invalid JSON, invalid session_id format, conflicting flags
```

---

### Task 1: Create `conftest.py` with NdjsonProcess and fixtures

**Files:**
- Create: `tests/e2e/scenarios/ndjson/__init__.py`
- Create: `tests/e2e/scenarios/ndjson/conftest.py`

- [ ] **Step 1: Create the empty package marker**

```python
# tests/e2e/scenarios/ndjson/__init__.py
# (empty file)
```

- [ ] **Step 2: Create conftest.py with all fixtures and the NdjsonProcess class**

Write `tests/e2e/scenarios/ndjson/conftest.py`:

```python
"""Fixtures for NDJSON CLI mode E2E tests.

These tests drive ironclaw as a subprocess via stdin/stdout NDJSON protocol
using live LLM calls (zai_anthropic). No mocking.
"""

import json
import os
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import pytest

# Project root (four levels up: scenarios/ndjson/ -> scenarios/ -> e2e/ -> tests/ -> root)
ROOT = Path(__file__).resolve().parent.parent.parent.parent


def _cargo_target_dir() -> Path:
    """Resolve the actual cargo target directory (mirrors parent conftest logic)."""
    env_target = os.environ.get("CARGO_TARGET_DIR")
    if env_target:
        return Path(env_target)
    cargo_config = Path.home() / ".cargo" / "config.toml"
    if cargo_config.exists():
        try:
            for line in cargo_config.read_text().splitlines():
                line = line.strip()
                if line.startswith("target-dir"):
                    _, _, value = line.partition("=")
                    value = value.strip().strip('"').strip("'")
                    if value:
                        return Path(value)
        except Exception:
            pass
    return ROOT / "target"


class NdjsonProcess:
    """Interactive NDJSON subprocess wrapper.

    Wraps a subprocess.Popen with piped stdin/stdout for NDJSON communication.
    Read operations use a background reader thread to support timeouts.
    """

    def __init__(self, proc: subprocess.Popen):
        self.proc = proc
        self._lines: list[str] = []
        self._lock = threading.Lock()
        self._event = threading.Event()
        self._reader_thread = threading.Thread(target=self._read_loop, daemon=True)
        self._reader_thread.start()

    def _read_loop(self):
        """Background thread that reads stdout lines into a buffer."""
        try:
            for raw_line in self.proc.stdout:
                line = raw_line.strip()
                if line:
                    with self._lock:
                        self._lines.append(line)
                    self._event.set()
        except (ValueError, OSError):
            pass  # pipe closed

    def send(self, obj: dict) -> None:
        """Write a JSON object as a single line to stdin."""
        line = json.dumps(obj) + "\n"
        self.proc.stdin.write(line.encode("utf-8"))
        self.proc.stdin.flush()

    def send_user(self, content: str) -> None:
        """Shorthand: send a user message."""
        self.send({"type": "user", "content": content})

    def send_control_response(
        self, request_id: str, behavior: str, message: str | None = None
    ) -> None:
        """Shorthand: send a control_response."""
        response: dict = {"behavior": behavior}
        if message is not None:
            response["message"] = message
        self.send(
            {"type": "control_response", "request_id": request_id, "response": response}
        )

    def send_interrupt(self) -> None:
        """Shorthand: send an interrupt."""
        self.send({"type": "interrupt"})

    def read_event(self, timeout: float = 120) -> dict:
        """Read one NDJSON line from stdout. Raises TimeoutError."""
        deadline = time.monotonic() + timeout
        while True:
            with self._lock:
                if self._lines:
                    line = self._lines.pop(0)
                    if not self._lines:
                        self._event.clear()
                    return json.loads(line)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"No NDJSON event received within {timeout}s"
                )
            self._event.wait(timeout=min(remaining, 0.5))

    def read_until(
        self, type: str, subtype: str | None = None, timeout: float = 120
    ) -> dict:
        """Read events until one matches (type, subtype). Returns the match."""
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"Event type={type!r} subtype={subtype!r} not found within {timeout}s"
                )
            event = self.read_event(timeout=remaining)
            if event.get("type") == type:
                if subtype is None or event.get("subtype") == subtype:
                    return event

    def collect_until_result(self, timeout: float = 120) -> list[dict]:
        """Read all events until a 'result' event. Returns the full list."""
        events = []
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"No 'result' event received within {timeout}s. "
                    f"Got {len(events)} events: {[e.get('type') for e in events]}"
                )
            event = self.read_event(timeout=remaining)
            events.append(event)
            if event.get("type") == "result":
                return events

    def close(self) -> int:
        """Close stdin (EOF), wait for process exit, return exit code."""
        try:
            self.proc.stdin.close()
        except (OSError, BrokenPipeError):
            pass
        try:
            self.proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=5)
        return self.proc.returncode


def _build_ndjson_env(db_path: str) -> dict[str, str]:
    """Build subprocess environment inheriting current env + NDJSON overrides."""
    env = os.environ.copy()
    env.update({
        "LLM_BACKEND": "zai_anthropic",
        "DATABASE_BACKEND": "libsql",
        "LIBSQL_PATH": db_path,
        "ONBOARD_COMPLETED": "true",
        "SANDBOX_ENABLED": "false",
        "HEARTBEAT_ENABLED": "false",
        "ROUTINES_ENABLED": "false",
        "EMBEDDING_ENABLED": "false",
        "CLI_ENABLED": "false",
    })
    return env


@pytest.fixture(scope="session")
def ironclaw_binary():
    """Ensure ironclaw binary is built. Returns the binary path."""
    target_dir = _cargo_target_dir()
    binary = target_dir / "debug" / "ironclaw"
    if not binary.exists():
        print("Building ironclaw (this may take a while)...")
        subprocess.run(
            ["cargo", "build", "--no-default-features", "--features", "libsql"],
            cwd=ROOT,
            check=True,
            timeout=600,
        )
    assert binary.exists(), f"Binary not found at {binary}"
    return str(binary)


@pytest.fixture(scope="session")
def ndjson_db_dir():
    """Session-scoped temp directory for the libSQL database."""
    with tempfile.TemporaryDirectory(prefix="ironclaw-ndjson-e2e-") as tmpdir:
        yield tmpdir


@pytest.fixture(scope="session")
def ndjson_env(ndjson_db_dir):
    """Session-scoped environment dict for NDJSON subprocess tests."""
    db_path = os.path.join(ndjson_db_dir, "ndjson-e2e.db")
    return _build_ndjson_env(db_path)


@pytest.fixture(scope="session", autouse=True)
def llm_health_check(ironclaw_binary, ndjson_env):
    """Verify LLM is reachable before any test runs. Fails loudly if not."""
    result = subprocess.run(
        [ironclaw_binary, "-p", "say ok", "--output-format", "stream-json",
         "--no-onboard"],
        env=ndjson_env,
        capture_output=True,
        timeout=120,
    )
    stdout = result.stdout.decode("utf-8", errors="replace")
    lines = []
    for line in stdout.strip().splitlines():
        line = line.strip()
        if line:
            try:
                lines.append(json.loads(line))
            except json.JSONDecodeError:
                pass

    has_result_success = any(
        e.get("type") == "result" and e.get("subtype") == "success"
        for e in lines
    )
    if not has_result_success:
        stderr = result.stderr.decode("utf-8", errors="replace")
        pytest.fail(
            f"LLM health check failed for zai_anthropic — API key not configured "
            f"or LLM unreachable. Configure ironclaw with a valid API key before "
            f"running NDJSON E2E tests.\n"
            f"exit_code={result.returncode}\n"
            f"stdout:\n{stdout[:2000]}\n"
            f"stderr:\n{stderr[:2000]}"
        )


@pytest.fixture(scope="session")
def run_print(ironclaw_binary, ndjson_env):
    """Factory fixture: run ironclaw -p <prompt> --output-format stream-json.

    Returns (exit_code, ndjson_lines, stderr).
    """

    def _run(prompt: str, extra_args: list[str] | None = None,
             env_override: dict[str, str] | None = None) -> tuple[int, list[dict], str]:
        cmd = [
            ironclaw_binary, "-p", prompt,
            "--output-format", "stream-json",
            "--no-onboard",
        ]
        if extra_args:
            cmd.extend(extra_args)

        env = ndjson_env.copy()
        if env_override:
            env.update(env_override)

        result = subprocess.run(
            cmd, env=env, capture_output=True, timeout=120,
        )
        stdout = result.stdout.decode("utf-8", errors="replace")
        stderr = result.stderr.decode("utf-8", errors="replace")
        lines = []
        for line in stdout.strip().splitlines():
            line = line.strip()
            if line:
                lines.append(json.loads(line))
        return result.returncode, lines, stderr

    return _run


@pytest.fixture(scope="session")
def open_streaming(ironclaw_binary, ndjson_env):
    """Factory fixture: open an interactive NDJSON streaming session.

    Returns an NdjsonProcess wrapper.
    """

    def _open(extra_args: list[str] | None = None,
              env_override: dict[str, str] | None = None) -> NdjsonProcess:
        cmd = [
            ironclaw_binary,
            "--output-format", "stream-json",
            "--input-format", "stream-json",
            "--no-onboard",
        ]
        if extra_args:
            cmd.extend(extra_args)

        env = ndjson_env.copy()
        if env_override:
            env.update(env_override)

        proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
        )
        return NdjsonProcess(proc)

    return _open
```

- [ ] **Step 3: Verify the fixture loads correctly**

Run from the worktree:

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
source .venv/bin/activate
pytest scenarios/ndjson/ --collect-only
```

Expected: pytest discovers the `scenarios/ndjson/` package with 0 tests (no test files yet), no import errors.

- [ ] **Step 4: Commit**

```bash
git add tests/e2e/scenarios/ndjson/__init__.py tests/e2e/scenarios/ndjson/conftest.py
git commit -m "test(ndjson-e2e): scaffold conftest with NdjsonProcess and fixtures"
```

---

### Task 2: Implement `test_protocol.py` — Basic protocol contract

**Files:**
- Create: `tests/e2e/scenarios/ndjson/test_protocol.py`

- [ ] **Step 1: Write all protocol tests**

Write `tests/e2e/scenarios/ndjson/test_protocol.py`:

```python
"""E2E tests: NDJSON basic protocol contract.

Validates event shape, sequencing, and structural correctness of the NDJSON
output from ironclaw in -p (print) mode with a live LLM.
"""

import json
import uuid


def test_init_event_shape(run_print):
    """First event must be system/init with required fields."""
    exit_code, lines, stderr = run_print("say hello")
    assert len(lines) >= 1, f"expected at least 1 event, got {len(lines)}"

    init = lines[0]
    assert init["type"] == "system", f"first event type: {init.get('type')}"
    assert init["subtype"] == "init", f"first event subtype: {init.get('subtype')}"

    # session_id must be a valid UUID
    sid = init["session_id"]
    uuid.UUID(sid)  # raises ValueError if invalid

    assert init["protocol_version"] == "1"
    assert isinstance(init["model"], str) and len(init["model"]) > 0
    assert isinstance(init["tools"], list) and len(init["tools"]) > 0
    assert init["resumed"] is False


def test_result_event_terminates_print_mode(run_print):
    """Last event must be a result with usage data."""
    exit_code, lines, stderr = run_print("say hi")
    assert len(lines) >= 2, f"expected at least 2 events (init + result), got {len(lines)}"

    result = lines[-1]
    assert result["type"] == "result"
    assert result["subtype"] in ("success", "error")
    assert isinstance(result["duration_ms"], int) and result["duration_ms"] >= 0
    assert isinstance(result["num_turns"], int) and result["num_turns"] >= 1

    usage = result["usage"]
    assert isinstance(usage["input_tokens"], int) and usage["input_tokens"] > 0
    assert isinstance(usage["output_tokens"], int) and usage["output_tokens"] > 0
    assert isinstance(usage["total_cost_usd"], str)


def test_all_lines_valid_ndjson(run_print):
    """Every stdout line must parse as valid JSON with a type field."""
    exit_code, lines, stderr = run_print("say ok")
    assert len(lines) >= 2
    for i, event in enumerate(lines):
        assert isinstance(event, dict), f"line {i} is not a dict: {event}"
        assert "type" in event, f"line {i} missing 'type' field: {event}"


def test_session_id_consistent(run_print):
    """All events with a session_id field must share the same value."""
    exit_code, lines, stderr = run_print("say something short")
    session_ids = set()
    for event in lines:
        if "session_id" in event:
            session_ids.add(event["session_id"])
    assert len(session_ids) == 1, f"expected 1 unique session_id, got {session_ids}"


def test_exit_code_zero_on_success(run_print):
    """Exit code should be 0 when result/success is emitted."""
    exit_code, lines, stderr = run_print("say ok")
    result = lines[-1]
    if result.get("subtype") == "success":
        assert exit_code == 0, f"expected exit code 0, got {exit_code}\nstderr: {stderr}"
```

- [ ] **Step 2: Run the tests**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/test_protocol.py -v
```

Expected: all 5 tests pass (health check runs first, then protocol tests).

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/scenarios/ndjson/test_protocol.py
git commit -m "test(ndjson-e2e): add protocol contract tests"
```

---

### Task 3: Implement `test_multiturn.py` — Multi-turn streaming

**Files:**
- Create: `tests/e2e/scenarios/ndjson/test_multiturn.py`

- [ ] **Step 1: Write multi-turn tests**

Write `tests/e2e/scenarios/ndjson/test_multiturn.py`:

```python
"""E2E tests: NDJSON multi-turn streaming interaction.

Validates that multiple user messages in streaming mode each produce
independent result events, and that conversation context is maintained.
"""

import tempfile
import os


def test_two_turns_each_produces_result(open_streaming, ndjson_env, ironclaw_binary):
    """Each user message in streaming mode should produce its own result event."""
    with tempfile.TemporaryDirectory(prefix="ndjson-multiturn-") as tmpdir:
        db_path = os.path.join(tmpdir, "multiturn.db")
        proc = open_streaming(env_override={"LIBSQL_PATH": db_path})
        try:
            # Wait for init
            init = proc.read_until("system", "init")
            assert init["subtype"] == "init"

            # Turn 1
            proc.send_user("say the word apple and nothing else")
            events1 = proc.collect_until_result()
            result1 = events1[-1]
            assert result1["type"] == "result"

            # Turn 2
            proc.send_user("say the word banana and nothing else")
            events2 = proc.collect_until_result()
            result2 = events2[-1]
            assert result2["type"] == "result"

            # Two distinct result events
            assert len([e for e in events1 if e["type"] == "result"]) == 1
            assert len([e for e in events2 if e["type"] == "result"]) == 1
        finally:
            proc.close()


def test_second_turn_sees_prior_context(open_streaming, ndjson_env, ironclaw_binary):
    """Second turn should have access to first turn's conversation context."""
    with tempfile.TemporaryDirectory(prefix="ndjson-context-") as tmpdir:
        db_path = os.path.join(tmpdir, "context.db")
        proc = open_streaming(env_override={"LIBSQL_PATH": db_path})
        try:
            proc.read_until("system", "init")

            # Turn 1: establish a secret code word
            proc.send_user("remember this code word: elephant. just confirm you noted it.")
            events1 = proc.collect_until_result()
            assert events1[-1]["type"] == "result"

            # Turn 2: ask for the code word
            proc.send_user("what is the code word I just told you?")
            events2 = proc.collect_until_result()
            result2 = events2[-1]
            assert result2["type"] == "result"

            # The LLM response should contain "elephant"
            result_text = (result2.get("result") or "").lower()
            # Also check assistant events in case result text is empty
            all_text = result_text
            for e in events2:
                if e.get("type") == "assistant":
                    msg = e.get("message", {})
                    all_text += " " + (msg.get("content") or "").lower()

            assert "elephant" in all_text, (
                f"Expected 'elephant' in LLM response, got:\n"
                f"result: {result2.get('result')}\n"
                f"all_text: {all_text}"
            )
        finally:
            proc.close()


def test_eof_closes_gracefully(open_streaming, ndjson_env, ironclaw_binary):
    """Closing stdin (EOF) after a completed turn should exit cleanly."""
    with tempfile.TemporaryDirectory(prefix="ndjson-eof-") as tmpdir:
        db_path = os.path.join(tmpdir, "eof.db")
        proc = open_streaming(env_override={"LIBSQL_PATH": db_path})
        try:
            proc.read_until("system", "init")

            proc.send_user("say ok")
            proc.collect_until_result()

            exit_code = proc.close()
            assert exit_code == 0, f"expected exit code 0 after EOF, got {exit_code}"
        except Exception:
            proc.close()
            raise
```

- [ ] **Step 2: Run the tests**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/test_multiturn.py -v
```

Expected: all 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/scenarios/ndjson/test_multiturn.py
git commit -m "test(ndjson-e2e): add multi-turn streaming tests"
```

---

### Task 4: Implement `test_approval.py` — Tool approval flow

**Files:**
- Create: `tests/e2e/scenarios/ndjson/test_approval.py`

- [ ] **Step 1: Write approval tests**

Write `tests/e2e/scenarios/ndjson/test_approval.py`:

```python
"""E2E tests: NDJSON tool approval request/response flow.

Validates that tool calls requiring approval emit control_request events,
and that allow/deny responses are handled correctly.
"""

import tempfile
import os


def test_approve_tool_allows_execution(open_streaming, ironclaw_binary, ndjson_env):
    """Approving a tool call should let execution proceed to result/success."""
    with tempfile.TemporaryDirectory(prefix="ndjson-approve-") as tmpdir:
        db_path = os.path.join(tmpdir, "approve.db")
        proc = open_streaming(env_override={"LIBSQL_PATH": db_path})
        try:
            proc.read_until("system", "init")

            # Ask the LLM to use a shell tool — this should trigger approval
            proc.send_user(
                "Use the shell tool to run this exact command: echo hello_from_ironclaw"
            )

            # Read events until we see a control_request
            control_req = proc.read_until("control_request", timeout=120)
            assert control_req["request"]["subtype"] == "can_use_tool"
            request_id = control_req["request_id"]

            # Approve the tool call
            proc.send_control_response(request_id, "allow")

            # Collect remaining events until result
            events = proc.collect_until_result()
            result = events[-1]
            assert result["type"] == "result"
            assert result["subtype"] == "success", (
                f"expected success after approval, got {result.get('subtype')}: "
                f"{result.get('error')}"
            )
        finally:
            proc.close()


def test_deny_tool_stops_execution(open_streaming, ironclaw_binary, ndjson_env):
    """Denying a tool call should produce a result without tool output."""
    with tempfile.TemporaryDirectory(prefix="ndjson-deny-") as tmpdir:
        db_path = os.path.join(tmpdir, "deny.db")
        proc = open_streaming(env_override={"LIBSQL_PATH": db_path})
        try:
            proc.read_until("system", "init")

            # Ask the LLM to use a shell tool
            proc.send_user(
                "Use the shell tool to run this exact command: echo denied_test"
            )

            # Read events until we see a control_request
            control_req = proc.read_until("control_request", timeout=120)
            request_id = control_req["request_id"]

            # Deny the tool call
            proc.send_control_response(request_id, "deny", message="User rejected")

            # The agent should handle denial and eventually produce a result
            events = proc.collect_until_result()
            result = events[-1]
            assert result["type"] == "result"

            # The output should NOT contain the shell command's output
            result_text = (result.get("result") or "").lower()
            assert "denied_test" not in result_text, (
                "Tool output appeared despite denial"
            )
        finally:
            proc.close()
```

- [ ] **Step 2: Run the tests**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/test_approval.py -v
```

Expected: both tests pass. Note: the LLM must be prompted strongly enough to invoke the shell tool. If the LLM refuses or uses a different tool, the `read_until("control_request")` will timeout. If this happens, adjust the prompt wording to be more directive.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/scenarios/ndjson/test_approval.py
git commit -m "test(ndjson-e2e): add tool approval flow tests"
```

---

### Task 5: Implement `test_interrupt.py` — Interrupt flow

**Files:**
- Create: `tests/e2e/scenarios/ndjson/test_interrupt.py`

- [ ] **Step 1: Write interrupt test**

Write `tests/e2e/scenarios/ndjson/test_interrupt.py`:

```python
"""E2E tests: NDJSON interrupt flow.

Validates that sending an interrupt message during processing results in
a result event with subtype 'interrupted'.
"""

import tempfile
import os
import time


def test_interrupt_during_processing(open_streaming, ironclaw_binary, ndjson_env):
    """Sending interrupt during LLM processing should produce result/interrupted."""
    with tempfile.TemporaryDirectory(prefix="ndjson-interrupt-") as tmpdir:
        db_path = os.path.join(tmpdir, "interrupt.db")
        proc = open_streaming(env_override={"LIBSQL_PATH": db_path})
        try:
            proc.read_until("system", "init")

            # Send a complex prompt that will take a while to process
            proc.send_user(
                "Write a very detailed 3000-word essay about the history of "
                "distributed computing systems, covering all major milestones "
                "from the 1960s to present day."
            )

            # Give the LLM a moment to start processing, then interrupt
            time.sleep(2)
            proc.send_interrupt()

            # We should get a result event — either interrupted or success
            # (if the LLM finished before the interrupt was processed)
            result = proc.read_until("result", timeout=120)
            assert result["type"] == "result"
            assert result["subtype"] in ("interrupted", "success"), (
                f"expected 'interrupted' or 'success', got {result.get('subtype')}"
            )
        finally:
            proc.close()
```

- [ ] **Step 2: Run the test**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/test_interrupt.py -v
```

Expected: test passes. The `subtype` may be `"interrupted"` or `"success"` depending on LLM timing — both are acceptable since the interrupt is a race with the response.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/scenarios/ndjson/test_interrupt.py
git commit -m "test(ndjson-e2e): add interrupt flow test"
```

---

### Task 6: Implement `test_resume.py` — Session persistence and resume

**Files:**
- Create: `tests/e2e/scenarios/ndjson/test_resume.py`

- [ ] **Step 1: Write resume tests**

Write `tests/e2e/scenarios/ndjson/test_resume.py`:

```python
"""E2E tests: NDJSON session persistence and resume.

Validates that conversations persist to the database and can be resumed
across separate ironclaw invocations.
"""

import tempfile
import os
import uuid as uuid_mod


def test_resume_by_session_id(run_print):
    """Resuming by session_id should carry forward conversation context."""
    with tempfile.TemporaryDirectory(prefix="ndjson-resume-") as tmpdir:
        db_path = os.path.join(tmpdir, "resume.db")
        env = {"LIBSQL_PATH": db_path}

        # First invocation: establish a secret word
        exit1, lines1, _ = run_print(
            "Remember this secret word: pineapple. Just confirm you got it.",
            env_override=env,
        )
        assert exit1 == 0 or any(
            e.get("subtype") == "success" for e in lines1
        ), f"first run failed: {lines1}"

        # Extract session_id from init event
        init1 = next(e for e in lines1 if e.get("subtype") == "init")
        session_id = init1["session_id"]
        assert init1["resumed"] is False

        # Second invocation: resume with session_id and ask for the word
        exit2, lines2, stderr2 = run_print(
            "What was the secret word I told you?",
            extra_args=["--session-id", session_id],
            env_override=env,
        )

        # Verify resumed flag
        init2 = next(e for e in lines2 if e.get("subtype") == "init")
        assert init2["resumed"] is True
        assert init2["session_id"] == session_id

        # The LLM should recall "pineapple" from the resumed context
        result2 = next(e for e in lines2 if e.get("type") == "result")
        result_text = (result2.get("result") or "").lower()
        all_text = result_text
        for e in lines2:
            if e.get("type") == "assistant":
                msg = e.get("message", {})
                all_text += " " + (msg.get("content") or "").lower()

        assert "pineapple" in all_text, (
            f"Expected 'pineapple' in resumed conversation response.\n"
            f"result: {result2.get('result')}\n"
            f"stderr: {stderr2[:500]}"
        )


def test_resume_flag_picks_latest(run_print):
    """--resume should resume the most recent session."""
    with tempfile.TemporaryDirectory(prefix="ndjson-resume-flag-") as tmpdir:
        db_path = os.path.join(tmpdir, "resume-flag.db")
        env = {"LIBSQL_PATH": db_path}

        # First invocation: create a conversation
        exit1, lines1, _ = run_print("say ok", env_override=env)
        init1 = next(e for e in lines1 if e.get("subtype") == "init")
        session_id = init1["session_id"]

        # Second invocation: --resume (no explicit session_id)
        _, lines2, _ = run_print(
            "say hi",
            extra_args=["--resume"],
            env_override=env,
        )
        init2 = next(e for e in lines2 if e.get("subtype") == "init")
        assert init2["resumed"] is True
        assert init2["session_id"] == session_id


def test_resume_unknown_uuid_fails(run_print):
    """Passing an unknown session_id should produce an error."""
    with tempfile.TemporaryDirectory(prefix="ndjson-resume-bad-") as tmpdir:
        db_path = os.path.join(tmpdir, "resume-bad.db")
        fake_uuid = str(uuid_mod.uuid4())
        exit_code, lines, stderr = run_print(
            "say ok",
            extra_args=["--session-id", fake_uuid],
            env_override={"LIBSQL_PATH": db_path},
        )

        # Should fail: either non-zero exit code or result/error
        has_error = (
            exit_code != 0
            or any(
                e.get("type") == "result" and e.get("subtype") == "error"
                for e in lines
            )
            or any(e.get("type") == "error" for e in lines)
        )
        assert has_error, (
            f"Expected error for unknown UUID {fake_uuid}, "
            f"got exit_code={exit_code}, events={[e.get('type') for e in lines]}"
        )
```

- [ ] **Step 2: Run the tests**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/test_resume.py -v
```

Expected: all 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/scenarios/ndjson/test_resume.py
git commit -m "test(ndjson-e2e): add session resume tests"
```

---

### Task 7: Implement `test_compat.py` — Claude Code compatibility mode

**Files:**
- Create: `tests/e2e/scenarios/ndjson/test_compat.py`

- [ ] **Step 1: Write compat mode tests**

Write `tests/e2e/scenarios/ndjson/test_compat.py`:

```python
"""E2E tests: NDJSON Claude Code compatibility mode.

Validates that --compat claude-code transforms output events to match
the Claude Code NDJSON schema and drops IronClaw-specific events.
"""


# IronClaw-specific event types/subtypes that should NOT appear in compat mode
IRONCLAW_ONLY_TYPES = {"tool_started", "tool_completed"}
IRONCLAW_ONLY_SUBTYPES = {"thinking", "skill_activated", "reasoning"}


def test_compat_init_event_shape(run_print):
    """system/init should be present and well-formed in compat mode."""
    exit_code, lines, stderr = run_print(
        "say hello",
        extra_args=["--compat", "claude-code"],
    )
    assert len(lines) >= 2, f"expected at least 2 events, got {len(lines)}"

    init = lines[0]
    assert init["type"] == "system"
    assert init["subtype"] == "init"
    assert "session_id" in init


def test_compat_drops_ironclaw_events(run_print):
    """Compat mode should not emit IronClaw-specific event types."""
    exit_code, lines, stderr = run_print(
        "say hello",
        extra_args=["--compat", "claude-code"],
    )

    for event in lines:
        event_type = event.get("type")
        event_subtype = event.get("subtype")
        assert event_type not in IRONCLAW_ONLY_TYPES, (
            f"IronClaw-only event type {event_type!r} found in compat mode: {event}"
        )
        assert event_subtype not in IRONCLAW_ONLY_SUBTYPES, (
            f"IronClaw-only subtype {event_subtype!r} found in compat mode: {event}"
        )


def test_compat_result_event_shape(run_print):
    """result event in compat mode should match Claude Code schema."""
    exit_code, lines, stderr = run_print(
        "say hi",
        extra_args=["--compat", "claude-code"],
    )

    result = next((e for e in lines if e.get("type") == "result"), None)
    assert result is not None, f"no result event found in {[e.get('type') for e in lines]}"
    assert "session_id" in result
    assert "duration_ms" in result
    assert "num_turns" in result
    assert "usage" in result
    assert isinstance(result["usage"], dict)
```

- [ ] **Step 2: Run the tests**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/test_compat.py -v
```

Expected: all 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/scenarios/ndjson/test_compat.py
git commit -m "test(ndjson-e2e): add Claude Code compatibility mode tests"
```

---

### Task 8: Implement `test_errors.py` — Error handling

**Files:**
- Create: `tests/e2e/scenarios/ndjson/test_errors.py`

- [ ] **Step 1: Write error handling tests**

Write `tests/e2e/scenarios/ndjson/test_errors.py`:

```python
"""E2E tests: NDJSON error handling and edge cases.

Validates graceful handling of invalid input, bad session IDs, and
conflicting CLI flags.
"""

import subprocess
import tempfile
import os
import uuid


def test_invalid_json_on_stdin(open_streaming, ironclaw_binary, ndjson_env):
    """Invalid JSON on stdin should emit an error event but not crash."""
    with tempfile.TemporaryDirectory(prefix="ndjson-badjson-") as tmpdir:
        db_path = os.path.join(tmpdir, "badjson.db")
        proc = open_streaming(env_override={"LIBSQL_PATH": db_path})
        try:
            proc.read_until("system", "init")

            # Send invalid JSON
            proc.proc.stdin.write(b"this is not json\n")
            proc.proc.stdin.flush()

            # Should get an error event (not a crash)
            error_event = proc.read_until("error", timeout=30)
            assert error_event["type"] == "error"
            assert "message" in error_event

            # Process should still be alive — send a valid message
            proc.send_user("say ok")
            events = proc.collect_until_result()
            result = events[-1]
            assert result["type"] == "result"
        finally:
            proc.close()


def test_invalid_session_id_format(ironclaw_binary, ndjson_env):
    """--session-id with a non-UUID should exit with code 2."""
    result = subprocess.run(
        [
            ironclaw_binary, "-p", "say ok",
            "--output-format", "stream-json",
            "--session-id", "not-a-uuid",
            "--no-onboard",
        ],
        env=ndjson_env,
        capture_output=True,
        timeout=30,
    )
    assert result.returncode == 2, (
        f"expected exit code 2 for invalid UUID, got {result.returncode}\n"
        f"stderr: {result.stderr.decode('utf-8', errors='replace')[:500]}"
    )


def test_conflicting_flags(ironclaw_binary, ndjson_env):
    """--session-id and --resume together should exit with code 2."""
    fake_uuid = str(uuid.uuid4())
    result = subprocess.run(
        [
            ironclaw_binary, "-p", "say ok",
            "--output-format", "stream-json",
            "--session-id", fake_uuid,
            "--resume",
            "--no-onboard",
        ],
        env=ndjson_env,
        capture_output=True,
        timeout=30,
    )
    assert result.returncode == 2, (
        f"expected exit code 2 for conflicting flags, got {result.returncode}\n"
        f"stderr: {result.stderr.decode('utf-8', errors='replace')[:500]}"
    )
```

- [ ] **Step 2: Run the tests**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/test_errors.py -v
```

Expected: all 3 tests pass. The `test_invalid_session_id_format` and `test_conflicting_flags` tests should be fast (no LLM call needed — they fail at CLI argument validation).

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/scenarios/ndjson/test_errors.py
git commit -m "test(ndjson-e2e): add error handling tests"
```

---

### Task 9: Run full suite and fix any failures

**Files:**
- Modify: `tests/e2e/scenarios/ndjson/conftest.py` (if needed)
- Modify: any test file that needs adjustment

- [ ] **Step 1: Run the complete NDJSON E2E suite**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/ -v --timeout=180
```

Expected: all 20 tests pass (5 protocol + 3 multiturn + 2 approval + 1 interrupt + 3 resume + 3 compat + 3 errors).

- [ ] **Step 2: Fix any failures**

If tests fail:
- Check stderr output for LLM errors, configuration issues, or missing flags
- Adjust prompts if the LLM doesn't trigger the expected behavior (e.g., tool approval tests may need more directive prompts)
- Adjust timeouts if LLM responses are slow
- Check if `exit code 2` tests match the actual clap error code

- [ ] **Step 3: Commit fixes (if any)**

```bash
git add tests/e2e/scenarios/ndjson/
git commit -m "test(ndjson-e2e): fix failures from full suite run"
```

- [ ] **Step 4: Final clean run to confirm all green**

```bash
cd /Users/vincentchen/source_code/ironclaw/.worktrees/ndjson-cli-mode/tests/e2e
pytest scenarios/ndjson/ -v
```

Expected: all tests pass with no errors or warnings.
