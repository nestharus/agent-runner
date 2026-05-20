//! AGE-159 W5 RCA recognizer tightening — unit-shaped predicate tests.
//!
//! These tests pin the strict semantics of `text_contains_terminal_envelope`
//! and `filesystem_artifact_recovers_terminal` per the AGE-159 Step 6a
//! contract: only exact `OULIPOLY_TERMINAL_SIGNAL=` / `OULIPOLY_RESULT=`
//! markers (with the strict AGE-153/154 key sets and matching invocation
//! UUID) — plus raw `<uuid>.result` JSON with the strict result-envelope key
//! set — count as terminal evidence. Start-time `.invocation` artifacts,
//! alias markers, suffix-only artifact names, and UUID-plus-broad-status
//! substrings are rejected.
//!
//! Declared roles: formatter, accessor, validator.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::{filesystem_artifact_recovers_terminal, text_contains_terminal_envelope};

const INVOCATION_UUID: &str = "39fa2c60-8ee1-4986-90d3-b081f72936b5";
const OTHER_INVOCATION_UUID: &str = "8c8e7d7d-83d4-49ac-b4ef-2c0a8655880b";

fn terminal_signal_json(invocation_uuid: &str) -> String {
    format!(
        r#"{{"evidence":{{"source":"test"}},"invocation_id":"{invocation_uuid}","kind":"external_kill","session_id":null}}"#
    )
}

fn result_json(invocation_uuid: &str) -> String {
    format!(
        r#"{{"error_category":null,"exit_code":137,"finished_at":"2026-05-19T00:00:00Z","id":"{invocation_uuid}","status":"failed","success":false,"terminal_reason":"external_kill"}}"#
    )
}

fn write_file(root: &Path, name: &str, contents: &str) {
    fs::write(root.join(name), contents).unwrap();
}

#[test]
fn text_terminal_signal_marker_with_matching_uuid_accepts() {
    let text = format!(
        "noise\nOULIPOLY_TERMINAL_SIGNAL={}\n",
        terminal_signal_json(INVOCATION_UUID)
    );

    assert!(text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_result_marker_with_matching_uuid_accepts() {
    let text = format!("OULIPOLY_RESULT={}\n", result_json(INVOCATION_UUID));

    assert!(text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_raw_result_json_line_with_matching_uuid_accepts() {
    let text = result_json(INVOCATION_UUID);

    assert!(text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_invocation_start_marker_rejected() {
    let text = format!(r#"OULIPOLY_INVOCATION={{"id":"{INVOCATION_UUID}","status":"running"}}"#);

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_uuid_plus_status_running_rejected() {
    let text = format!(r#"observed {INVOCATION_UUID} with "status":"running""#);

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_uuid_plus_terminal_reason_without_marker_rejected() {
    let text = format!("{INVOCATION_UUID} terminal_reason=external_kill");

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_alias_terminal_marker_rejected() {
    for marker in [
        "OULIPOLY_TERMINAL=",
        "OULIPOLY_FINAL=",
        "OULIPOLY_ENVELOPE=",
    ] {
        let text = format!("{marker}{}", result_json(INVOCATION_UUID));

        assert!(
            !text_contains_terminal_envelope(&text, INVOCATION_UUID),
            "alias marker must not count as terminal evidence: {marker}"
        );
    }
}

#[test]
fn text_terminal_signal_missing_required_key_rejected() {
    let text = format!(
        r#"OULIPOLY_TERMINAL_SIGNAL={{"evidence":{{"source":"test"}},"invocation_id":"{INVOCATION_UUID}","session_id":null}}"#
    );

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_terminal_signal_extra_unrecognized_key_rejected() {
    let text = format!(
        r#"OULIPOLY_TERMINAL_SIGNAL={{"evidence":{{"source":"test"}},"invocation_id":"{INVOCATION_UUID}","kind":"external_kill","session_id":null,"unexpected":true}}"#
    );

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_terminal_signal_invocation_id_mismatch_rejected() {
    let text = format!(
        "OULIPOLY_TERMINAL_SIGNAL={}",
        terminal_signal_json(OTHER_INVOCATION_UUID)
    );

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_result_marker_id_mismatch_rejected() {
    let text = format!("OULIPOLY_RESULT={}", result_json(OTHER_INVOCATION_UUID));

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_terminal_signal_followed_by_garbage_rejected() {
    let text = format!("OULIPOLY_TERMINAL_SIGNAL={INVOCATION_UUID} not-json");

    assert!(!text_contains_terminal_envelope(&text, INVOCATION_UUID));
}

#[test]
fn text_empty_input_rejected() {
    assert!(!text_contains_terminal_envelope("", INVOCATION_UUID));
}

#[test]
fn fs_result_artifact_with_raw_result_json_accepts() {
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        &format!("{INVOCATION_UUID}.result"),
        &result_json(INVOCATION_UUID),
    );

    assert!(filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_arbitrary_filename_with_terminal_signal_marker_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!(
        "noise\nOULIPOLY_TERMINAL_SIGNAL={}\n",
        terminal_signal_json(INVOCATION_UUID)
    );
    write_file(dir.path(), "arbitrary.log", &contents);

    assert!(filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_arbitrary_filename_with_result_marker_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("OULIPOLY_RESULT={}\n", result_json(INVOCATION_UUID));
    write_file(dir.path(), "captured-output.txt", &contents);

    assert!(filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_invocation_start_artifact_with_running_status_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!(r#"{{"id":"{INVOCATION_UUID}","status":"running"}}"#);
    write_file(
        dir.path(),
        &format!("{INVOCATION_UUID}.invocation"),
        &contents,
    );

    assert!(!filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_terminal_suffix_with_non_terminal_content_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        &format!("{INVOCATION_UUID}.terminal"),
        "terminal suffix alone is not strict evidence",
    );

    assert!(!filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_final_json_suffix_with_running_status_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!(r#"{{"id":"{INVOCATION_UUID}","status":"running"}}"#);
    write_file(
        dir.path(),
        &format!("{INVOCATION_UUID}.final.json"),
        &contents,
    );

    assert!(!filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_result_artifact_missing_required_key_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!(
        r#"{{"error_category":null,"exit_code":137,"finished_at":"2026-05-19T00:00:00Z","id":"{INVOCATION_UUID}","status":"failed","success":false}}"#
    );
    write_file(dir.path(), &format!("{INVOCATION_UUID}.result"), &contents);

    assert!(!filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_result_artifact_id_mismatch_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write_file(
        dir.path(),
        &format!("{INVOCATION_UUID}.result"),
        &result_json(OTHER_INVOCATION_UUID),
    );

    assert!(!filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_empty_root_rejected() {
    let dir = tempfile::tempdir().unwrap();

    assert!(!filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}

#[test]
fn fs_non_readable_file_does_not_panic_and_returns_false() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("non-readable.txt");
    fs::write(&path, "").unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&path, permissions).unwrap();

    assert!(!filesystem_artifact_recovers_terminal(
        dir.path(),
        INVOCATION_UUID
    ));
}
