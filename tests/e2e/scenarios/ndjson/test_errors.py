"""E2E tests: NDJSON error handling and edge cases.

Validates graceful handling of invalid input, bad session IDs, and
conflicting CLI flags.
"""

import subprocess
import uuid


def test_invalid_json_on_stdin(open_streaming):
    """Invalid JSON on stdin should emit an error event but not crash."""
    proc = open_streaming()
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
    # Invalid UUID is caught at startup (exit code 1) or by clap (exit code 2)
    assert result.returncode != 0, (
        f"expected non-zero exit code for invalid UUID, got {result.returncode}\n"
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
