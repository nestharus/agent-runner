#![cfg(unix)]

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

#[test]
fn claude_code_cwd_decodes_project_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let workspace = dir.path().join("workspace").join("rfq");
    fs::create_dir_all(&workspace).unwrap();
    let encoded = format!(
        "-{}",
        workspace
            .to_string_lossy()
            .trim_start_matches('/')
            .replace('/', "-")
    );
    let project_dir = dir.path().join(encoded);
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(project_dir.join(format!("{session_id}.jsonl")), "{}\n").unwrap();

    let output = Command::new(scripts_dir().join("claude-code-cwd"))
        .arg(dir.path())
        .arg(session_id)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["found"], true);
    assert_eq!(
        value["cwd"],
        workspace.canonicalize().unwrap().display().to_string()
    );
}

#[test]
fn codex_cwd_reads_payload_cwd_from_rollout_first_line() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let rollout_dir = dir.path().join("2026/05/10");
    fs::create_dir_all(&rollout_dir).unwrap();
    fs::write(
        rollout_dir.join(format!("rollout-2026-05-10T00-00-00-{session_id}.jsonl")),
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"cwd\":\"{}\"}}}}\n",
            workspace.path().display()
        ),
    )
    .unwrap();

    let output = Command::new(scripts_dir().join("codex-cwd"))
        .arg(dir.path())
        .arg(session_id)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["found"], true);
    assert_eq!(value["cwd"], workspace.path().to_string_lossy().as_ref());
}
