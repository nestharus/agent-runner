#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts")
}

#[test]
fn cwd_scripts_unchanged() {
    for script_name in ["claude-code-cwd", "codex-cwd", "opencode-cwd"] {
        let path = scripts_dir().join(script_name);
        let metadata = fs::metadata(&path).unwrap();
        assert!(metadata.is_file(), "{path:?} should be a file");
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "{path:?} should be executable"
        );
    }
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

#[test]
fn opencode_cwd_reads_directory_from_opencode_db() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session_id = "ses_1012bcfe8ffe7SLrwzf1UrYGtW";
    let db_path = dir.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "CREATE TABLE session (id text PRIMARY KEY, directory text NOT NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session (id, directory) VALUES (?1, ?2)",
        rusqlite::params![session_id, workspace.path().to_string_lossy().as_ref()],
    )
    .unwrap();
    drop(conn);

    let output = Command::new(scripts_dir().join("opencode-cwd"))
        .arg(dir.path())
        .arg(session_id)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["found"], true);
    assert_eq!(
        value["cwd"],
        workspace
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
}
