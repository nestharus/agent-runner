use serde_json::Value;
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
fn claude_code_turns_emits_body_chunks_for_user_and_assistant_turns() {
    // risk: adapter regression; level: particular-integration; source: contract §4 T11 / proposal A5,A7.
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("project-a").join("session.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        concat!(
            r#"{"type":"user","uuid":"user-turn","timestamp":"2026-04-17T08:00:00Z","sessionId":"5169694d-de0f-40d1-890c-6e28e55bab27","message":"claude user body"}"#,
            "\n",
            r#"{"type":"assistant","uuid":"assistant-turn","timestamp":"2026-04-17T08:00:01Z","sessionId":"5169694d-de0f-40d1-890c-6e28e55bab27","message":{"content":[{"type":"text","text":"claude assistant body"}]}}"#,
            "\n"
        ),
    )
    .unwrap();

    let output = run_claude_code_turns(dir.path());

    assert!(output.status.success(), "{output:?}");
    let records: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let user = records
        .iter()
        .find(|record| record["turn_id"] == "user-turn")
        .expect("user turn must be emitted");
    let assistant = records
        .iter()
        .find(|record| record["turn_id"] == "assistant-turn")
        .expect("assistant turn must be emitted");
    assert_eq!(
        user["body"],
        serde_json::json!([{"type":"text","text":"claude user body"}])
    );
    assert_eq!(
        assistant["body"],
        serde_json::json!([{"type":"text","text":"claude assistant body"}])
    );
}

#[test]
fn claude_code_turns_skips_unextractable_dict_chunks_and_reads_top_level_content() {
    // risk: adapter regression; level: particular-integration; source: CodeRabbit R1-F04.
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("project-a").join("session.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        concat!(
            r#"{"type":"assistant","uuid":"assistant-turn","timestamp":"2026-04-17T08:00:01Z","sessionId":"5169694d-de0f-40d1-890c-6e28e55bab27","message":{"content":[{"type":"tool_use"},{"type":"text","content":"kept content"}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"user-turn","timestamp":"2026-04-17T08:00:00Z","sessionId":"5169694d-de0f-40d1-890c-6e28e55bab27","content":{"type":7,"content":"top level content"}}"#,
            "\n"
        ),
    )
    .unwrap();

    let output = run_claude_code_turns(dir.path());

    assert!(output.status.success(), "{output:?}");
    let records: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let assistant = records
        .iter()
        .find(|record| record["turn_id"] == "assistant-turn")
        .expect("assistant turn must be emitted");
    let user = records
        .iter()
        .find(|record| record["turn_id"] == "user-turn")
        .expect("user turn must be emitted");
    assert_eq!(
        assistant["body"],
        serde_json::json!([{"type":"text","text":"kept content"}])
    );
    assert_eq!(
        user["body"],
        serde_json::json!([{"type":"text","text":"top level content"}])
    );
}
