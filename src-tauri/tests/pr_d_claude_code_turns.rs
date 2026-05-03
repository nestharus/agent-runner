#![cfg(unix)]

use agent_runner_session::ScriptTurn;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts")
}

fn run_claude_code_turns(base_dir: &Path) -> std::process::Output {
    let state_dir = tempfile::tempdir().unwrap();
    Command::new("python3")
        .arg(scripts_dir().join("claude-code-turns"))
        .arg(base_dir)
        .env("STATE_DIR", state_dir.path())
        .output()
        .unwrap()
}

#[test]
fn claude_code_turns_emits_parent_uuid_and_is_sidechain_fields() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("project-a").join("session.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        concat!(
            r#"{"type":"user","uuid":"root-turn","timestamp":"2026-04-17T08:00:00Z","sessionId":"5169694d-de0f-40d1-890c-6e28e55bab27","isSidechain":false}"#,
            "\n",
            r#"{"type":"assistant","uuid":"child-turn","timestamp":"2026-04-17T08:00:01Z","sessionId":"5169694d-de0f-40d1-890c-6e28e55bab27","parentUuid":"root-turn","isSidechain":true}"#,
            "\n"
        ),
    )
    .unwrap();

    let output = run_claude_code_turns(dir.path());

    assert!(output.status.success(), "{output:?}");
    let turns: Vec<ScriptTurn> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let child = turns
        .iter()
        .find(|turn| turn.turn_id == "child-turn")
        .expect("assistant child turn should be emitted");

    assert_eq!(
        child.parent_turn_id.as_deref(),
        Some("root-turn"),
        "adapter must preserve Claude parentUuid"
    );
    assert_eq!(
        child.is_sidechain,
        Some(true),
        "adapter must preserve Claude isSidechain"
    );
}
