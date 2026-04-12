"""Fixtures for NDJSON CLI mode E2E tests.

These tests drive ironclaw as a subprocess via stdin/stdout NDJSON protocol
using live LLM calls. No mocking.

Uses the user's main ironclaw database directly — LLM provider config and
API keys are read from the existing DB at startup. Test conversations are
written to the main DB but are harmless (just normal conversation records).
"""

import json
import os
import subprocess
import threading
import time
from pathlib import Path

import pytest

# Project root (five levels up: conftest.py -> ndjson/ -> scenarios/ -> e2e/ -> tests/ -> root)
ROOT = Path(__file__).resolve().parent.parent.parent.parent.parent

# ~/.ironclaw/.env holds DATABASE_URL and DATABASE_BACKEND
_IRONCLAW_ENV = Path.home() / ".ironclaw" / ".env"


def _read_ironclaw_env() -> dict[str, str]:
    """Read key=value pairs from ~/.ironclaw/.env."""
    result = {}
    if not _IRONCLAW_ENV.exists():
        pytest.fail(
            f"Cannot find {_IRONCLAW_ENV} — ironclaw is not configured. "
            f"Run 'ironclaw onboard' first."
        )
    for line in _IRONCLAW_ENV.read_text().splitlines():
        line = line.strip()
        if line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        result[key.strip()] = value.strip().strip('"').strip("'")
    return result


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
            self.proc.wait(timeout=60)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=10)
        return self.proc.returncode


@pytest.fixture(scope="session")
def ironclaw_binary():
    """Ensure ironclaw binary is built. Returns the binary path."""
    target_dir = _cargo_target_dir()
    binary = target_dir / "debug" / "ironclaw"
    if not binary.exists():
        print("Building ironclaw (this may take a while)...")
        subprocess.run(
            ["cargo", "build"],
            cwd=ROOT,
            check=True,
            timeout=600,
        )
    assert binary.exists(), f"Binary not found at {binary}"
    return str(binary)


@pytest.fixture(scope="session")
def ndjson_env():
    """Session-scoped environment dict for NDJSON subprocess tests.

    Inherits the current environment and injects DB config from ~/.ironclaw/.env
    so the subprocess connects to the user's main ironclaw database.
    """
    ironclaw_vars = _read_ironclaw_env()
    env = os.environ.copy()
    # Inject DB connection from ~/.ironclaw/.env
    if "DATABASE_BACKEND" in ironclaw_vars:
        env["DATABASE_BACKEND"] = ironclaw_vars["DATABASE_BACKEND"]
    if "DATABASE_URL" in ironclaw_vars:
        env["DATABASE_URL"] = ironclaw_vars["DATABASE_URL"]
    env.update({
        "ONBOARD_COMPLETED": "true",
        "SANDBOX_ENABLED": "false",
        "HEARTBEAT_ENABLED": "false",
        "ROUTINES_ENABLED": "false",
        "EMBEDDING_ENABLED": "false",
        "CLI_ENABLED": "false",
    })
    return env


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
            f"LLM health check failed — API key not configured or LLM unreachable. "
            f"Configure ironclaw with a valid LLM provider before running NDJSON "
            f"E2E tests.\n"
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
            cmd, env=env, capture_output=True, timeout=180,
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
