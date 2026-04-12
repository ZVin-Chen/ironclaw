"""E2E tests: NDJSON tool approval request/response flow.

Validates that tool calls requiring approval emit control_request events,
and that allow/deny responses are handled correctly.
"""


def test_approve_tool_allows_execution(open_streaming):
    """Approving a tool call should let execution proceed to result/success."""
    proc = open_streaming()
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


def test_deny_tool_stops_execution(open_streaming):
    """Denying a tool call should produce a result without tool output."""
    proc = open_streaming()
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
