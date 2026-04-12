"""E2E tests: NDJSON multi-turn streaming interaction.

Validates that multiple user messages in streaming mode each produce
independent result events, and that conversation context is maintained.
"""

import tempfile
import os


def test_two_turns_each_produces_result(open_streaming):
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


def test_second_turn_sees_prior_context(open_streaming):
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


def test_eof_closes_gracefully(open_streaming):
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
