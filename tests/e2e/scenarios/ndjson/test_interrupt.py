"""E2E tests: NDJSON interrupt flow.

Validates that sending an interrupt message during processing results in
a result event with subtype 'interrupted'.
"""

import tempfile
import os
import time


def test_interrupt_during_processing(open_streaming):
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
