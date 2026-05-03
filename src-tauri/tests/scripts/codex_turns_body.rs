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

fn run_codex_turns(base_dir: &Path) -> std::process::Output {
    let state_dir = tempfile::tempdir().unwrap();
    Command::new("python3")
        .arg(scripts_dir().join("codex-turns"))
        .arg(base_dir)
        .env("STATE_DIR", state_dir.path())
        .output()
        .unwrap()
}

#[test]
fn codex_turns_emits_body_chunks_for_user_and_assistant_messages() {
    // risk: adapter regression; level: particular-integration; source: contract §4 T12 / proposal A5,A7.
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir
        .path()
        .join("rollout-2026-04-17T08-00-00-5169694d-de0f-40d1-890c-6e28e55bab27.jsonl");
    fs::write(
        &transcript,
        concat!(
            r#"{"type":"session_meta","payload":{"id":"5169694d-de0f-40d1-890c-6e28e55bab27"}}"#,
            "\n",
            r#"{"type":"response_item","timestamp":"2026-04-17T08:00:00Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"codex user body"}]}}"#,
            "\n",
            r#"{"type":"response_item","timestamp":"2026-04-17T08:00:01Z","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"codex assistant body"}]}}"#,
            "\n"
        ),
    )
    .unwrap();

    let output = run_codex_turns(dir.path());

    assert!(output.status.success(), "{output:?}");
    let records: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let user = records
        .iter()
        .find(|record| record["role"] == "user")
        .expect("user message turn must be emitted");
    let assistant = records
        .iter()
        .find(|record| record["role"] == "assistant")
        .expect("assistant message turn must be emitted");
    assert_eq!(
        user["body"],
        serde_json::json!([{"type":"text","text":"codex user body"}])
    );
    assert_eq!(
        assistant["body"],
        serde_json::json!([{"type":"text","text":"codex assistant body"}])
    );
}

#[test]
fn codex_turns_skips_unextractable_dict_chunks_and_reads_top_level_content() {
    // risk: adapter regression; level: particular-integration; source: CodeRabbit R1-F03.
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir
        .path()
        .join("rollout-2026-04-17T08-00-00-5169694d-de0f-40d1-890c-6e28e55bab27.jsonl");
    fs::write(
        &transcript,
        concat!(
            r#"{"type":"session_meta","payload":{"id":"5169694d-de0f-40d1-890c-6e28e55bab27"}}"#,
            "\n",
            r#"{"type":"response_item","timestamp":"2026-04-17T08:00:00Z","payload":{"type":"message","role":"user","content":{"type":7,"content":"codex top level content"}}}"#,
            "\n",
            r#"{"type":"response_item","timestamp":"2026-04-17T08:00:01Z","payload":{"type":"message","role":"assistant","content":[{"type":"tool_call"},{"type":"output_text","content":"codex kept content"}]}}"#,
            "\n"
        ),
    )
    .unwrap();

    let output = run_codex_turns(dir.path());

    assert!(output.status.success(), "{output:?}");
    let records: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let user = records
        .iter()
        .find(|record| record["role"] == "user")
        .expect("user message turn must be emitted");
    let assistant = records
        .iter()
        .find(|record| record["role"] == "assistant")
        .expect("assistant message turn must be emitted");
    assert_eq!(
        user["body"],
        serde_json::json!([{"type":"text","text":"codex top level content"}])
    );
    assert_eq!(
        assistant["body"],
        serde_json::json!([{"type":"text","text":"codex kept content"}])
    );
}
