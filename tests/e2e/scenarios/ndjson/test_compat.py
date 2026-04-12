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
