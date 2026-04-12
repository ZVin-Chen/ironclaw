"""E2E tests: NDJSON multi-turn streaming interaction.

Validates that multiple user messages in streaming mode each produce
independent result events, and that conversation context is maintained.
"""


def test_two_turns_each_produces_result(open_streaming):
    """Each user message in streaming mode should produce its own result event."""
    proc = open_streaming()
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


def test_second_turn_sees_prior_context(open_streaming):
    """Second turn should have access to first turn's conversation context."""
    proc = open_streaming()
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


def test_eof_closes_without_crash(open_streaming):
    """Closing stdin (EOF) should not crash the process."""
    proc = open_streaming()
    try:
        proc.read_until("system", "init")

        # Close stdin immediately after init (no user message needed).
        # The process should exit cleanly, not crash or panic.
        exit_code = proc.close()
        # Exit code 0 is ideal. SIGKILL (-9) or SIGTERM (-15) from our
        # cleanup timeout is also acceptable — it means the process was
        # still alive (not crashed) when we shut it down.
        assert exit_code in (0, -9, -15), (
            f"unexpected exit code {exit_code} after EOF"
        )
    except Exception:
        proc.close()
        raise
