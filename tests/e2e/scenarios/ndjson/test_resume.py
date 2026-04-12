"""E2E tests: NDJSON session persistence and resume.

Validates that conversations persist to the database and can be resumed
across separate ironclaw invocations.
"""

import uuid as uuid_mod


def test_resume_by_session_id(run_print):
    """Resuming by session_id should carry forward conversation context."""
    # First invocation: establish a secret word
    exit1, lines1, _ = run_print(
        "Remember this secret word: pineapple. Just confirm you got it.",
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
    """--resume should resume a recent session (resumed=true in init)."""
    # First invocation: create a conversation
    exit1, lines1, _ = run_print("say ok")
    init1 = next(e for e in lines1 if e.get("subtype") == "init")
    assert init1["resumed"] is False

    # Second invocation: --resume (no explicit session_id)
    # With a shared DB, --resume picks the most recent conversation for
    # the current user, which may or may not be the one we just created
    # (other tests may have created conversations in between).
    _, lines2, _ = run_print(
        "say hi",
        extra_args=["--resume"],
    )
    init2 = next(e for e in lines2 if e.get("subtype") == "init")
    assert init2["resumed"] is True, "expected resumed=true with --resume flag"


def test_resume_unknown_uuid_starts_fresh(run_print):
    """Passing an unknown session_id should either error or start a fresh session."""
    fake_uuid = str(uuid_mod.uuid4())
    exit_code, lines, stderr = run_print(
        "say ok",
        extra_args=["--session-id", fake_uuid],
    )

    # The behavior may be: error out, OR treat as a new session with that UUID.
    # Both are acceptable — verify we get a coherent response either way.
    if exit_code != 0:
        # Errored out — that's fine
        return

    # If it succeeded, verify it produced a valid init + result sequence
    init = next((e for e in lines if e.get("subtype") == "init"), None)
    assert init is not None, f"no init event in {[e.get('type') for e in lines]}"
    result = next((e for e in lines if e.get("type") == "result"), None)
    assert result is not None, f"no result event in {[e.get('type') for e in lines]}"
