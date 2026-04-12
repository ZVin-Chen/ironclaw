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
