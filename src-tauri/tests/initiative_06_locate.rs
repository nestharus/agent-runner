#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06::*;

/// Risk: T1 — resolver pass-through, single chain, single segment.
/// Level: particular-integration.
/// Source: contract §6 row T1; A1, A3.
/// Observable: exit 0; stdout JSON parses with all 8 required fields.
/// Residual: does not validate provider-native transcript content.
#[test]
fn locate_succeeds_for_single_chain_single_segment_session() {
    let prepared = cli_claude_success_fixture();

    let output = prepared.fixture.run_locate(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let json = parse_stdout_json(&output);
    for field in required_success_fields() {
        assert!(json.get(field).is_some(), "missing {field} in {json}");
    }
    assert_eq!(
        json.as_object().unwrap().len(),
        required_success_fields().len()
    );
    assert_eq!(json["session_id"], prepared.session_id);
    assert_eq!(json["chain_id"], prepared.chain_id);
    assert_eq!(json["provider_name"], prepared.provider_name);
    assert_eq!(json["storage_type"], "claude_code");
    assert_eq!(json["transcript_state"], "available");
    assert_eq!(json["mutable"], true);
}

/// Risk: T4 — D2 no-storage fails closed at CLI level.
/// Level: particular-integration.
/// Source: contract §6 row T4; A3, A5.
/// Observable: exit 12; stderr JSON code unsupported-storage; stdout empty.
/// Residual: not all third-party locator failures classified ideally.
#[test]
fn locate_no_storage_provider_with_present_locator_exits_unsupported_storage() {
    let prepared = cli_no_storage_fixture(true);

    let output = prepared.fixture.run_locate(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(12), "{output:?}");
    let json = assert_json_error(&output, "unsupported-storage");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("\"storage_type\":\"other\""),
        "{output:?}"
    );
    assert!(json["error"]["message"].as_str().unwrap().contains("other"));
}

/// Risk: T4 — D2 no-storage fails closed even without a locator.
/// Level: particular-integration.
/// Source: contract §6 row T4; A3, A5.
/// Observable: exit 12; stderr JSON code unsupported-storage; stdout empty.
/// Residual: not all third-party locator failures classified ideally.
#[test]
fn locate_no_storage_provider_without_locator_exits_unsupported_storage() {
    let prepared = cli_no_storage_fixture(false);

    let output = prepared.fixture.run_locate(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(12), "{output:?}");
    assert_json_error(&output, "unsupported-storage");
}

/// Risk: T7 — D5 default DB only, no --state-db override.
/// Level: particular-integration.
/// Source: contract §6 row T7; A6.
/// Observable: clap rejects --state-db for session locate.
/// Residual: GUI state DB integration out of scope.
#[test]
fn locate_rejects_state_db_override_flag() {
    let prepared = cli_claude_success_fixture();

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--state-db", "/tmp/ignored.db"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--state-db"), "{stderr}");
}

/// Risk: T8 — missing well-formed UUID maps to session-not-found.
/// Level: particular-integration.
/// Source: contract §6 row T8; A1.
/// Observable: exit 10; stderr JSON code session-not-found.
/// Residual: none.
#[test]
fn locate_unknown_well_formed_uuid_exits_session_not_found() {
    let fixture = cli_missing_uuid_fixture();

    let output = fixture.run_locate("99999999-9999-4999-8999-999999999999", &[]);

    assert_eq!(output.status.code(), Some(10), "{output:?}");
    assert_json_error(&output, "session-not-found");
}

/// Risk: T9 — invalid UUID fails before DB open.
/// Level: end-to-end.
/// Source: contract §6 row T9; A1.
/// Observable: exit 2; stderr JSON code invalid-session-id; no state dir created.
/// Residual: clap structural usage errors may use clap formatting.
#[test]
fn locate_invalid_uuid_exits_two_before_state_db_open() {
    let fixture = cli_invalid_uuid_fixture();

    let output = fixture.run_locate("not-a-uuid", &[]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_json_error(&output, "invalid-session-id");
    assert!(
        !fixture.data_home().join("oulipoly-agent-runner").exists(),
        "invalid UUID should not open or initialize default state DB"
    );
}

/// Risk: T10 — D6 locator errors are unsupported-storage at CLI level.
/// Level: particular-integration.
/// Source: contract §6 row T10; A3.
/// Observable: exit 12; stderr JSON code unsupported-storage; stdout empty.
/// Residual: 600s timeout behavior covered by existing locate_transcript tests.
#[test]
fn locate_locator_error_exits_unsupported_storage_without_stdout_json() {
    let prepared = cli_locator_failure_fixture();

    let output = prepared.fixture.run_locate(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(12), "{output:?}");
    let json = assert_json_error(&output, "unsupported-storage");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("locator"),
        "{json}"
    );
}

/// Risk: T10 — D6 --json flag does not change compact error formatting.
/// Level: particular-integration.
/// Source: contract §6 row T10; A3.
/// Observable: exit 12; stderr JSON code unsupported-storage; stdout empty.
/// Residual: 600s timeout behavior covered by existing locate_transcript tests.
#[test]
fn locate_json_flag_preserves_unsupported_storage_error_shape() {
    let prepared = cli_locator_failure_fixture();

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(output.status.code(), Some(12), "{output:?}");
    assert_json_error(&output, "unsupported-storage");
    assert!(String::from_utf8_lossy(&output.stderr).ends_with('\n'));
}

/// Risk: T15 — read-only behavior after default DB open.
/// Level: particular-integration.
/// Source: contract §6 row T15; A6.
/// Observable: row counts and transcript mtime unchanged after locate.
/// Residual: physical read-only open deferred to 06-schema-probe.
#[test]
fn locate_does_not_mutate_state_rows_or_transcript_file_after_open() {
    let prepared = cli_read_only_fixture();
    let before = prepared
        .fixture
        .snapshot_read_only_state(&prepared.jsonl_path);

    let output = prepared.fixture.run_locate(&prepared.session_id, &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let after = prepared
        .fixture
        .snapshot_read_only_state(&prepared.jsonl_path);
    assert_eq!(after, before);
}

/// Risk: T16 — CLI JSON shape stability.
/// Level: particular-integration.
/// Source: contract §6 row T16; A3, A4.
/// Observable: compact one-line JSON with stable required field set.
/// Residual: non-UTF-8 OS paths intentionally unsupported, not fuzzed.
#[test]
fn locate_success_stdout_is_single_compact_json_line_with_required_fields() {
    let prepared = cli_claude_success_fixture();

    let output = prepared
        .fixture
        .run_locate(&prepared.session_id, &["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(stdout.ends_with('\n'), "{stdout:?}");
    assert_eq!(stdout.lines().count(), 1, "{stdout:?}");
    assert!(!stdout.contains("\n  "), "{stdout:?}");
    let json = parse_stdout_json(&output);
    for field in required_success_fields() {
        assert!(json.get(field).is_some(), "missing {field} in {json}");
    }
    assert_eq!(
        json.as_object().unwrap().len(),
        required_success_fields().len()
    );
}
