#![cfg(unix)]

mod fixtures;

use agent_runner_lib::state::{ReadOnlyOpenError, StateDb};
use fixtures::initiative_06_schema_probe::*;

/// Risk: T1 — Schema-probe success on current-schema DB returns full JSON with `compatible: true`, `safe_for_import_replace` per features.
/// Level: particular-integration.
/// Source: contract §7 row T1.
/// Observable: exit 0; stdout JSON has all required fields; compatibility shapes nested correctly.
/// Residual: future feature-flag flips can change `safe_for_import_replace`; this asserts the Phase 6 feature map.
#[test]
fn schema_probe_current_schema_db_emits_compatible_report() {
    let fixture = current_schema_db_fixture();

    let output = fixture.run_schema_probe(&[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(stdout.ends_with('\n'), "{stdout:?}");
    assert_eq!(stdout.lines().count(), 1, "{stdout:?}");
    assert!(!stdout.contains("\n  "), "{stdout:?}");

    let json = parse_stdout_json(&output);
    assert_eq!(json["binary"]["name"], "oulipoly-agent-runner");
    assert!(!json["binary"]["version"].as_str().unwrap().is_empty());
    assert!(json["binary"]["commit"].as_str().is_some());
    assert_eq!(
        json["state_db"]["path"],
        fixture.db_path().to_string_lossy().as_ref()
    );
    assert_eq!(json["state_db"]["exists"], true);
    assert_eq!(
        json["state_db"]["schema_version"].as_u64(),
        Some(CURRENT_SCHEMA_VERSION as u64)
    );
    assert_eq!(
        json["state_db"]["user_version"].as_u64(),
        Some(CURRENT_SCHEMA_VERSION as u64)
    );
    assert_eq!(
        json["state_db"]["current_schema_version"].as_u64(),
        Some(CURRENT_SCHEMA_VERSION as u64)
    );
    assert_eq!(
        json["state_db"]["minimum_supported_schema_version"].as_u64(),
        Some(MINIMUM_SUPPORTED_SCHEMA_VERSION as u64)
    );
    assert_eq!(json["state_db"]["compatible"], true);
    assert_structural_maps(&json, true);
    assert_no_dotted_compatibility_keys(&json);
    assert_eq!(json["features"]["session_locate"], false);
    assert_eq!(json["features"]["session_export"], false);
    assert_eq!(json["features"]["session_import_replace"], false);
    assert_eq!(json["features"]["session_pause_handshake"], false);
    assert_eq!(json["features"]["session_schema_probe"], true);
    assert_eq!(json["supported_storage_types"][0], "claude_code");
    assert_eq!(json["supported_storage_types"][1], "codex_session");
    assert_eq!(json["supported_storage_types"][2], "other");
    assert_eq!(json["safe_for_import_replace"], false);
}

/// Risk: T2 — Missing-DB case: probe returns exit `0` with JSON `state_db.exists: false`; structural booleans all `false`; `safe_for_import_replace: false`.
/// Level: particular-integration.
/// Source: contract §7 row T2.
/// Observable: exit 0; stdout JSON; `state_db.exists` false.
/// Residual: does not cover a race where the file disappears between `exists` and SQLite open; T6 covers the API-level `Missing` variant.
#[test]
fn schema_probe_missing_db_emits_non_mutating_success_report() {
    let fixture = missing_db_fixture();
    let data_app_dir = fixture.data_home().join("oulipoly-agent-runner");

    let output = fixture.run_schema_probe(&[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        !data_app_dir.exists(),
        "missing DB probe must not create the default state directory"
    );
    let json = parse_stdout_json(&output);
    assert_eq!(json["state_db"]["exists"], false);
    assert_eq!(json["state_db"]["schema_version"].as_u64(), Some(0));
    assert_eq!(json["state_db"]["user_version"].as_u64(), Some(0));
    assert_eq!(json["state_db"]["compatible"], false);
    assert_structural_maps(&json, false);
    assert_no_dotted_compatibility_keys(&json);
    assert_eq!(json["safe_for_import_replace"], false);
}

/// Risk: T3 — Incompatible-schema case: DB present, `user_version: 1` below `MINIMUM_SUPPORTED_SCHEMA_VERSION`; probe returns exit 14.
/// Level: particular-integration.
/// Source: contract §7 row T3.
/// Observable: exit 14; stderr JSON code `schema-incompatible`.
/// Residual: structural incompatibility at current version is covered indirectly by T1/T8 map checks, not exhaustively fuzzed here.
#[test]
fn schema_probe_old_user_version_exits_schema_incompatible() {
    let fixture = incompatible_schema_db_fixture();

    let output = fixture.run_schema_probe(&[]);

    assert_eq!(output.status.code(), Some(14), "{output:?}");
    let json = assert_json_error(&output, "schema-incompatible");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("schema"),
        "{json}"
    );
}

/// Risk: D6 — Newer schema versions are not safe for the current binary's public session surface.
/// Level: particular-integration.
/// Source: proposal §5 / §9.1 row D6.
/// Observable: exit 14; stderr JSON code `schema-incompatible`.
/// Residual: does not predict future compatibility policy after the current constant is bumped.
#[test]
fn schema_probe_future_user_version_exits_schema_incompatible() {
    let fixture = future_schema_db_fixture();

    let output = fixture.run_schema_probe(&[]);

    assert_eq!(output.status.code(), Some(14), "{output:?}");
    assert_json_error(&output, "schema-incompatible");
}

/// Risk: D6 — Required indexes with matching names but wrong definitions must not satisfy compatibility.
/// Level: particular-integration.
/// Source: proposal §9.1 row D6.
/// Observable: exit 14; stderr JSON code `schema-incompatible`.
/// Residual: validates column order for required indexes, not every SQLite index property.
#[test]
fn schema_probe_wrong_index_definition_exits_schema_incompatible() {
    let fixture = wrong_index_definition_db_fixture();

    let output = fixture.run_schema_probe(&[]);

    assert_eq!(output.status.code(), Some(14), "{output:?}");
    assert_json_error(&output, "schema-incompatible");
}

/// Risk: T4 — Operational error case: DB file unreadable returns exit 1.
/// Level: particular-integration.
/// Source: contract §7 row T4.
/// Observable: exit 1; stderr JSON code `operational-error`.
/// Residual: Unix permission semantics can vary under privileged runners; direct API variant tests cover the classifier.
#[test]
fn schema_probe_unreadable_db_exits_operational_error() {
    let fixture = unreadable_db_fixture();

    let output = fixture.run_schema_probe(&[]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = assert_json_error(&output, "operational-error");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("state.db"),
        "{json}"
    );
}

/// Risk: T5 — `StateDb::open_read_only` does not mutate DB or create files.
/// Level: component.
/// Source: contract §7 row T5.
/// Observable: mtime unchanged; no new files.
/// Residual: does not prove every SQLite PRAGMA is absent; it checks the physical side effects that caught prior mutating open paths.
#[test]
fn open_read_only_preserves_existing_db_physical_snapshot() {
    let fixture = current_schema_db_fixture();
    let before = fixture.physical_snapshot();

    let db = StateDb::open_read_only(&fixture.db_path()).unwrap();
    drop(db);

    let after = fixture.physical_snapshot();
    assert_eq!(after, before);
}

/// Risk: T5 — `StateDb::open_read_only` does not mutate DB or create files.
/// Level: component.
/// Source: contract §7 row T5.
/// Observable: parent directory is not created for a missing target.
/// Residual: race conditions between path existence and open are covered by the `Missing` variant contract.
#[test]
fn open_read_only_missing_path_does_not_create_parent_directory() {
    let fixture = missing_db_fixture();
    let path = fixture.missing_parent_db_path();
    let before = fixture.physical_snapshot_for(&path);

    let result = StateDb::open_read_only(&path);

    assert!(matches!(result, Err(ReadOnlyOpenError::Missing { .. })));
    let after = fixture.physical_snapshot_for(&path);
    assert_eq!(after, before);
    assert!(!path.parent().unwrap().exists());
}

/// Risk: T6 — `ReadOnlyOpenError` variants raise per spec: Missing, NotADatabase, PermissionDenied, WalSidecarError, Operational.
/// Level: component.
/// Source: contract §7 row T6.
/// Observable: `Missing` variant fires for absent file.
/// Residual: CLI missing-DB success mapping is covered by T2.
#[test]
fn open_read_only_classifies_missing_file() {
    let fixture = missing_db_fixture();
    let path = fixture.db_path();

    let err = match StateDb::open_read_only(&path) {
        Ok(_) => panic!("missing file unexpectedly opened"),
        Err(err) => err,
    };

    match err {
        ReadOnlyOpenError::Missing { path: actual } => assert_eq!(actual, path),
        other => panic!("expected Missing, got {other:?}"),
    }
}

/// Risk: T6 — `ReadOnlyOpenError` variants raise per spec: Missing, NotADatabase, PermissionDenied, WalSidecarError, Operational.
/// Level: component.
/// Source: contract §7 row T6.
/// Observable: `NotADatabase` variant fires for an invalid SQLite file.
/// Residual: does not attempt every SQLite corruption mode.
#[test]
fn open_read_only_classifies_not_a_database_file() {
    let fixture = invalid_database_fixture();
    let path = fixture.db_path();

    let err = match StateDb::open_read_only(&path) {
        Ok(_) => panic!("invalid database unexpectedly opened"),
        Err(err) => err,
    };

    match err {
        ReadOnlyOpenError::NotADatabase {
            path: actual,
            message,
        } => {
            assert_eq!(actual, path);
            assert!(!message.is_empty());
        }
        other => panic!("expected NotADatabase, got {other:?}"),
    }
}

/// Risk: T6 — `ReadOnlyOpenError` variants raise per spec: Missing, NotADatabase, PermissionDenied, WalSidecarError, Operational.
/// Level: component.
/// Source: contract §7 row T6.
/// Observable: `PermissionDenied` variant fires for an unreadable file.
/// Residual: Unix permission semantics can vary under privileged runners.
#[test]
fn open_read_only_classifies_permission_denied_file() {
    let fixture = unreadable_db_fixture();
    let path = fixture.db_path();

    let err = match StateDb::open_read_only(&path) {
        Ok(_) => panic!("unreadable database unexpectedly opened"),
        Err(err) => err,
    };

    match err {
        ReadOnlyOpenError::PermissionDenied { path: actual } => assert_eq!(actual, path),
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
}

/// Risk: T6 — `ReadOnlyOpenError` variants raise per spec: Missing, NotADatabase, PermissionDenied, WalSidecarError, Operational.
/// Level: component.
/// Source: contract §7 row T6.
/// Observable: `WalSidecarError` variant fires when WAL/SHM sidecar reads fail.
/// Residual: SQLite may surface sidecar failures differently across platforms; this fixture targets the required Unix behavior.
#[test]
fn open_read_only_classifies_wal_sidecar_error() {
    let fixture = wal_sidecar_error_fixture();
    let path = fixture.db_path();

    let err = match StateDb::open_read_only(&path) {
        Ok(_) => panic!("database with unreadable WAL sidecars unexpectedly opened"),
        Err(err) => err,
    };

    match err {
        ReadOnlyOpenError::WalSidecarError {
            path: actual,
            message,
        } => {
            assert_eq!(actual, path);
            assert!(!message.is_empty());
        }
        other => panic!("expected WalSidecarError, got {other:?}"),
    }
}

/// Risk: T6 — `ReadOnlyOpenError` variants raise per spec: Missing, NotADatabase, PermissionDenied, WalSidecarError, Operational.
/// Level: component.
/// Source: contract §7 row T6.
/// Observable: `Operational` variant fires for unexpected open errors.
/// Residual: exact SQLite OS error text is not asserted.
#[test]
fn open_read_only_classifies_other_operational_error() {
    let fixture = directory_database_fixture();
    let path = fixture.db_path();

    let err = match StateDb::open_read_only(&path) {
        Ok(_) => panic!("directory path unexpectedly opened as a database"),
        Err(err) => err,
    };

    match err {
        ReadOnlyOpenError::Operational { message } => assert!(!message.is_empty()),
        other => panic!("expected Operational, got {other:?}"),
    }
}

/// Risk: T7 — `safe_for_import_replace` predicate respects features map.
/// Level: unit.
/// Source: contract §7 row T7.
/// Observable: Boolean flips per condition.
/// Residual: future import-replace write implementation is outside this probe predicate test.
#[test]
fn schema_probe_report_safe_for_import_replace_predicate_follows_inputs() {
    let safe = report_for_predicate(true, true, true, true, supported_storage_types());
    assert!(safe.safe_for_import_replace);

    let import_replace_disabled =
        report_for_predicate(true, true, false, true, supported_storage_types());
    assert!(!import_replace_disabled.safe_for_import_replace);

    let pause_disabled = report_for_predicate(true, true, true, false, supported_storage_types());
    assert!(!pause_disabled.safe_for_import_replace);

    let missing_db = report_for_predicate(false, true, true, true, supported_storage_types());
    assert!(!missing_db.safe_for_import_replace);

    let incompatible_db = report_for_predicate(true, false, true, true, supported_storage_types());
    assert!(!incompatible_db.safe_for_import_replace);

    let incomplete_storage_vocab =
        report_for_predicate(true, true, true, true, vec!["claude_code".to_string()]);
    assert!(!incomplete_storage_vocab.safe_for_import_replace);
}

/// Risk: T8 — JSON shape stability: compatibility maps emit nested, not dotted, keys.
/// Level: unit.
/// Source: contract §7 row T8.
/// Observable: parse JSON; verify no dotted keys.
/// Residual: compact CLI formatting is covered by T1/T2 rather than this serialization unit.
#[test]
fn schema_probe_report_serializes_nested_compatibility_maps() {
    let report = report_for_json_shape();

    let json = serde_json::to_value(report).unwrap();

    assert_eq!(json["state_db"]["tables"]["session_turns"], true);
    assert_eq!(
        json["state_db"]["required_columns"]["session_turns"]["parent_turn_id"],
        true
    );
    assert_eq!(
        json["state_db"]["required_indexes"]["session_chain_segments"]["idx_segments_chain_active"],
        true
    );
    assert!(
        json["state_db"]["tables"]
            .get("session_turns.parent_turn_id")
            .is_none()
    );
    assert!(
        json["state_db"]["required_columns"]
            .get("session_turns.parent_turn_id")
            .is_none()
    );
    assert_no_dotted_compatibility_keys(&json);
}
