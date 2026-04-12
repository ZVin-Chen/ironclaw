//! Integration test: drive `ironclaw` as a subprocess in NDJSON print mode and
//! assert that the event sequence matches the protocol.

#![cfg(feature = "libsql")]

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn ironclaw_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ironclaw"))
}

#[test]
#[ignore = "Requires LLM provider; run manually with IRONCLAW_STUB_LLM=1"]
fn print_mode_emits_init_assistant_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let output = Command::new(ironclaw_binary())
        .arg("--print")
        .arg("say hi and nothing else")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--no-db")
        .env("IRONCLAW_BASE_DIR", &home)
        .env("DATABASE_BACKEND", "libsql")
        .env("LIBSQL_PATH", home.join("test.db"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run ironclaw");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line must be valid JSON"))
        .collect();

    assert!(!lines.is_empty(), "should emit at least one NDJSON line");
    let first = &lines[0];
    assert_eq!(first["type"], "system");
    assert_eq!(first["subtype"], "init");
    assert_eq!(first["protocol_version"], "1");

    let last = lines.last().unwrap();
    assert_eq!(last["type"], "result");
    assert!(
        matches!(last["subtype"].as_str(), Some("success") | Some("error")),
        "last event should be a result; got {}",
        last
    );
}
