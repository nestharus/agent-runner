#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts")
}

/// `STATE_DIR` is part of the locator-script contract (parallel to
/// `turn_script` — scripts may use it for caching/state). The bundled
/// reference scripts don't use it today, but the contract says they're
/// allowed to, so the test injects a writable path so a future locator
/// implementation isn't surprised by its absence.
fn run_locator(script_name: &str, base_dir: &Path, session_id: &str) -> std::process::Output {
    let state_dir = tempfile::tempdir().unwrap();
    Command::new("bash")
        .arg(scripts_dir().join(script_name))
        .arg(base_dir)
        .env("SESSION_ID", session_id)
        .env("STATE_DIR", state_dir.path())
        .output()
        .unwrap()
}

#[test]
fn pr_c_locator_scripts_unchanged() {
    for script_name in ["claude-code-locate-transcript", "codex-locate-transcript"] {
        let path = scripts_dir().join(script_name);
        let metadata = fs::metadata(&path).unwrap();
        assert!(metadata.is_file(), "{path:?} should be a file");
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.starts_with("#!/usr/bin/env bash"), "{path:?}");
        assert!(body.contains("SESSION_ID"), "{path:?}");
    }
}

#[test]
fn claude_locator_script_prints_matching_transcript_path() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("project-a").join("session.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(
        &transcript,
        r#"{"type":"user","uuid":"turn-1","timestamp":"2026-04-17T08:00:00Z","sessionId":"5169694d-de0f-40d1-890c-6e28e55bab27"}"#,
    )
    .unwrap();

    let output = run_locator(
        "claude-code-locate-transcript",
        dir.path(),
        "5169694d-de0f-40d1-890c-6e28e55bab27",
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        transcript.canonicalize().unwrap().display().to_string()
    );
}

/// Claude Code names transcript files exactly `<session_id>.jsonl`.
/// The locator script's primary path matches by filename; this test
/// exercises that path (the existing test exercises the content-based
/// fallback when the filename doesn't match).
#[test]
fn claude_locator_script_matches_by_filename() {
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir
        .path()
        .join("project-a")
        .join(format!("{session_id}.jsonl"));
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    // Intentionally write content WITHOUT a matching sessionId field so
    // a content-based fallback would NOT match — only the filename match
    // should succeed.
    fs::write(
        &transcript,
        r#"{"type":"user","uuid":"turn-1","timestamp":"2026-04-17T08:00:00Z","sessionId":"some-other-id"}"#,
    )
    .unwrap();

    let output = run_locator("claude-code-locate-transcript", dir.path(), session_id);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        transcript.canonicalize().unwrap().display().to_string()
    );
}

/// Codex names rollout files `rollout-<timestamp>-<session_id>.jsonl`.
/// Primary path matches by filename suffix.
#[test]
fn codex_locator_script_matches_by_filename_suffix() {
    let session_id = "019d9d09-2de9-7902-b148-f9f3bed4fa41";
    let dir = tempfile::tempdir().unwrap();
    // Real Codex naming: `rollout-<ISO timestamp>-<session_id>.jsonl`.
    let transcript = dir
        .path()
        .join(format!("rollout-2026-04-17T08-00-00-{session_id}.jsonl"));
    // Intentionally NO matching session_meta inside, so a content
    // fallback would NOT match — only filename match should succeed.
    fs::write(
        &transcript,
        r#"{"type":"session_meta","payload":{"id":"some-other-id"}}"#,
    )
    .unwrap();

    let output = run_locator("codex-locate-transcript", dir.path(), session_id);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        transcript.canonicalize().unwrap().display().to_string()
    );
}

/// Content-fallback path: rollout file whose filename does NOT include
/// the session id but whose `session_meta.payload.id` matches.
#[test]
fn codex_locator_script_falls_back_to_session_meta_content() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("rollout-20260417.jsonl");
    fs::write(
        &transcript,
        concat!(
            r#"{"type":"session_meta","payload":{"id":"019d9d09-2de9-7902-b148-f9f3bed4fa41"}}"#,
            "\n",
            r#"{"type":"response_item","timestamp":"2026-04-17T08:00:00Z","payload":{"type":"message","role":"assistant"}}"#,
            "\n"
        ),
    )
    .unwrap();

    let output = run_locator(
        "codex-locate-transcript",
        dir.path(),
        "019d9d09-2de9-7902-b148-f9f3bed4fa41",
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        transcript.canonicalize().unwrap().display().to_string()
    );
}
