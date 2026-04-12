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
