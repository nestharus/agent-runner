use oulipoly_state::{
    InvocationMutationAuthority, InvocationStart, LegacyProviderNames, StateDb,
    StateTerminalTimestampRepair, StateTimestampRecordFamily,
    schema::{self, SchemaCompatibility},
};
use rusqlite::{Connection, params};
mod fixtures;
mod timestamp_fixture;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, StateDb, i64, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let state = StateDb::open(&path).unwrap();
    let invocation_uuid = uuid::Uuid::new_v4().to_string();
    let row = state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.clone(),
            model_name: "age371-model".into(),
            provider_name: "age371-provider".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    (directory, path, state, row, invocation_uuid)
}

#[test]
fn invocation_transition_timestamp_is_atomic_and_terminal_time_is_immutable() {
    let (_directory, path, state, row, invocation_uuid) = fixture();
    state
        .finalize_invocation(
            InvocationMutationAuthority::Standalone,
            row,
            true,
            0,
            None,
            Some("completed"),
        )
        .unwrap();

    let connection = Connection::open(&path).unwrap();
    let timestamps: (String, String, String, String) = connection
        .query_row(
            "SELECT finished_at,lifecycle_updated_at,retention_eligible_at,retention_status
             FROM invocations WHERE invocation_uuid=?1",
            [&invocation_uuid],
            |record| {
                Ok((
                    record.get(0)?,
                    record.get(1)?,
                    record.get(2)?,
                    record.get(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(timestamps.0, timestamps.1);
    assert_eq!(timestamps.0, timestamps.2);
    assert_eq!(timestamps.3, "eligible");

    let replacement = "2030-01-01T00:00:00Z";
    let immutable = connection
        .execute(
            "UPDATE invocations SET finished_at=?1 WHERE invocation_uuid=?2",
            params![replacement, invocation_uuid],
        )
        .unwrap_err();
    assert!(
        immutable
            .to_string()
            .contains("terminal timestamp is immutable")
    );
    let reopen = connection
        .execute(
            "UPDATE invocations SET status='running' WHERE invocation_uuid=?1",
            [&invocation_uuid],
        )
        .unwrap_err();
    assert!(reopen.to_string().contains("terminal state cannot reopen"));
    assert_eq!(
        connection
            .query_row(
                "SELECT finished_at FROM invocations WHERE invocation_uuid=?1",
                [&invocation_uuid],
                |record| record.get::<_, String>(0),
            )
            .unwrap(),
        timestamps.0
    );
}

#[test]
fn explicit_repair_is_historical_only_atomic_and_append_only() {
    let (_directory, path, mut live, row, invocation_uuid) = fixture();
    live.finalize_invocation(
        InvocationMutationAuthority::Standalone,
        row,
        false,
        1,
        Some("fixture"),
        Some("failed"),
    )
    .unwrap();
    let original_timestamp: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT finished_at FROM invocations WHERE invocation_uuid=?1",
            [&invocation_uuid],
            |record| record.get(0),
        )
        .unwrap();
    let request = StateTerminalTimestampRepair {
        family: StateTimestampRecordFamily::Invocation,
        record_key: &invocation_uuid,
        new_timestamp: "2030-01-01T00:00:00Z",
        actor: "age371-test",
        reason: "repair verified fixture evidence",
    };
    let blocked = live.repair_terminal_timestamp(request.clone()).unwrap_err();
    assert!(blocked.contains("live_history_barrier"), "{blocked}");
    drop(live);

    let mut historical = StateDb::open_historical(&path).unwrap();
    let repair_id = historical.repair_terminal_timestamp(request).unwrap();
    drop(historical);

    let connection = Connection::open(&path).unwrap();
    let repaired: (String, String, String) = connection
        .query_row(
            "SELECT finished_at,retention_eligible_at,retention_status
             FROM invocations WHERE invocation_uuid=?1",
            [&invocation_uuid],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .unwrap();
    assert_eq!(repaired.0, "2030-01-01T00:00:00Z");
    assert_eq!(repaired.0, repaired.1);
    assert_eq!(repaired.2, "eligible");
    let audit: (String, String, String) = connection
        .query_row(
            "SELECT actor,reason,new_value FROM record_timestamp_repairs WHERE repair_id=?1",
            [&repair_id],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .unwrap();
    assert_eq!(audit.0, "age371-test");
    assert_eq!(audit.2, repaired.0);
    assert!(
        connection
            .execute(
                "DELETE FROM record_timestamp_repairs WHERE repair_id=?1",
                [&repair_id],
            )
            .is_err()
    );
    drop(connection);

    let mut historical = StateDb::open_historical(&path).unwrap();
    historical
        .repair_terminal_timestamp(StateTerminalTimestampRepair {
            family: StateTimestampRecordFamily::Invocation,
            record_key: &invocation_uuid,
            new_timestamp: &original_timestamp,
            actor: "age371-test",
            reason: "restore verified original fixture evidence",
        })
        .unwrap();
    drop(historical);

    // The first audit row is no longer the newest authorization and cannot be
    // reused to replay its old A->B repair without a new audit entry.
    let stale_authorization = Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE invocations SET finished_at='2030-01-01T00:00:00Z'
             WHERE invocation_uuid=?1",
            [&invocation_uuid],
        )
        .unwrap_err();
    assert!(
        stale_authorization
            .to_string()
            .contains("terminal timestamp is immutable")
    );
}

#[test]
fn legacy_unknown_and_wall_clock_regression_are_retention_ineligible() {
    let (_directory, path, state, _, _) = fixture();
    drop(state);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO invocations(
                invocation_uuid,model_name,provider_name,provider_index,status,
                created_at,finished_at
             ) VALUES(?1,'legacy','age371-provider',0,'legacy',?2,?2)",
            params![uuid::Uuid::new_v4().to_string(), "2020-01-01T00:00:00Z"],
        )
        .unwrap();
    let anomaly_uuid = uuid::Uuid::new_v4().to_string();
    connection
        .execute(
            "INSERT INTO invocations(
                invocation_uuid,model_name,provider_name,provider_index,status,created_at
             ) VALUES(?1,'clock','age371-provider',0,'running','2030-01-01T00:00:00Z')",
            [&anomaly_uuid],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE invocations
             SET status='failed',success=0,exit_code=1,finished_at='2029-12-31T23:59:59Z'
             WHERE invocation_uuid=?1",
            [&anomaly_uuid],
        )
        .unwrap();

    let counts: (i64, i64) = connection
        .query_row(
            "SELECT
               SUM(retention_status='legacy_unknown' AND retention_eligible_at IS NULL),
               SUM(retention_status='clock_anomaly' AND retention_eligible_at IS NULL)
             FROM invocations WHERE model_name IN ('legacy','clock')",
            [],
            |record| Ok((record.get(0)?, record.get(1)?)),
        )
        .unwrap();
    assert_eq!(counts, (1, 1));
}

#[test]
fn schema_26_upgrade_preserves_evidence_and_does_not_invent_legacy_age() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    drop(StateDb::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    timestamp_fixture::remove_v27_timestamp_contract(&connection);
    connection.pragma_update(None, "user_version", 26).unwrap();

    let authoritative_uuid = uuid::Uuid::new_v4().to_string();
    let ambiguous_uuid = uuid::Uuid::new_v4().to_string();
    connection
        .execute(
            "INSERT INTO invocations(
                invocation_uuid,model_name,provider_name,provider_index,status,
                success,exit_code,created_at,finished_at
             ) VALUES(?1,'migration','provider',0,'succeeded',1,0,?2,?3)",
            params![
                authoritative_uuid,
                "2026-01-01T00:00:00Z",
                "2026-01-01T00:01:00Z"
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO invocations(
                invocation_uuid,model_name,provider_name,provider_index,status,
                success,exit_code,created_at,finished_at
             ) VALUES(?1,'migration','provider',0,'succeeded',1,0,?2,?2)",
            params![ambiguous_uuid, "2020-01-01T00:00:00Z"],
        )
        .unwrap();
    drop(connection);

    drop(StateDb::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    let authoritative: (String, String, String) = connection
        .query_row(
            "SELECT finished_at,retention_eligible_at,retention_status
             FROM invocations WHERE invocation_uuid=?1",
            [&authoritative_uuid],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .unwrap();
    assert_eq!(authoritative.0, "2026-01-01T00:01:00Z");
    assert_eq!(authoritative.0, authoritative.1);
    assert_eq!(authoritative.2, "eligible");

    let ambiguous: (String, Option<String>, String) = connection
        .query_row(
            "SELECT finished_at,retention_eligible_at,retention_status
             FROM invocations WHERE invocation_uuid=?1",
            [&ambiguous_uuid],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .unwrap();
    assert_eq!(ambiguous.0, "2020-01-01T00:00:00Z");
    assert_eq!(ambiguous.1, None);
    assert_eq!(ambiguous.2, "legacy_unknown");
}

#[test]
fn current_drift_repair_reinstalls_the_complete_invocation_timestamp_contract() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let state = StateDb::open(&path).unwrap();
    let invocation_uuid = uuid::Uuid::new_v4().to_string();
    let row = state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.clone(),
            model_name: "current-drift".into(),
            provider_name: "provider".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .finalize_invocation(
            InvocationMutationAuthority::Standalone,
            row,
            true,
            0,
            None,
            Some("completed"),
        )
        .unwrap();
    drop(state);

    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER invocations_timestamp_after_insert;
             DROP TRIGGER invocations_timestamp_after_terminal;
             DROP TRIGGER invocations_created_at_immutable;
             DROP TRIGGER invocations_finished_at_immutable;
             DROP TRIGGER invocations_terminal_reopen_forbidden;
             DROP INDEX idx_invocations_retention_eligible;
             ALTER TABLE invocations DROP COLUMN lifecycle_updated_at;
             ALTER TABLE invocations DROP COLUMN retention_eligible_at;
             ALTER TABLE invocations DROP COLUMN retention_status;",
        )
        .unwrap();
    drop(connection);

    drop(StateDb::open(&path).unwrap());
    assert_invocation_contract_and_audited_repair(&path, &invocation_uuid, "2031-01-01T00:00:00Z");

    let before = invocation_contract_snapshot(&path, &invocation_uuid);
    drop(StateDb::open(&path).unwrap());
    assert_eq!(
        invocation_contract_snapshot(&path, &invocation_uuid),
        before
    );
}

#[test]
fn current_pre_uuid_rebuild_installs_guards_before_commit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    drop(StateDb::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE invocations;")
        .unwrap();
    create_pre_uuid_invocations(&connection);
    connection
        .execute(
            "INSERT INTO invocations(
                model_name,provider_index,success,exit_code,error_category,created_at
             ) VALUES('rebuild-model',0,1,0,NULL,'2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    drop(connection);

    let mut provider_names = LegacyProviderNames::new();
    provider_names.insert(
        ("rebuild-model".to_string(), 0),
        "rebuild-provider".to_string(),
    );
    drop(StateDb::open_with_legacy_provider_names(&path, &provider_names).unwrap());
    let connection = Connection::open(&path).unwrap();
    let invocation_uuid: String = connection
        .query_row(
            "SELECT invocation_uuid FROM invocations WHERE model_name='rebuild-model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let preserved: (String, Option<String>, String) = connection
        .query_row(
            "SELECT created_at,retention_eligible_at,retention_status
             FROM invocations WHERE invocation_uuid=?1",
            [&invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(preserved.0, "2026-01-01T00:00:00Z");
    assert_eq!(preserved.1, None);
    assert_eq!(preserved.2, "legacy_unknown");
    connection
        .execute(
            "UPDATE invocations SET finished_at='2026-01-01T00:01:00Z'
             WHERE invocation_uuid=?1",
            [&invocation_uuid],
        )
        .unwrap();
    drop(connection);

    assert_invocation_contract_and_audited_repair(&path, &invocation_uuid, "2026-01-01T00:02:00Z");
    let before = invocation_contract_snapshot(&path, &invocation_uuid);
    drop(StateDb::open_with_legacy_provider_names(&path, &provider_names).unwrap());
    assert_eq!(
        invocation_contract_snapshot(&path, &invocation_uuid),
        before
    );
}

#[test]
fn versionless_pre_uuid_full_runner_is_preserved_and_reopen_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let connection = Connection::open(&path).unwrap();
    fixtures::create_full_state_schema(&connection, 0);
    connection
        .execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE invocations;")
        .unwrap();
    create_pre_uuid_invocations(&connection);
    for (model, success, exit_code, error_category, created_at) in [
        ("versionless-success", 1, 0, None, "2020-01-01T00:00:00Z"),
        (
            "versionless-failure",
            0,
            7,
            Some("fixture_failure"),
            "2020-01-01T00:01:00Z",
        ),
    ] {
        connection
            .execute(
                "INSERT INTO invocations(
                    model_name,provider_index,success,exit_code,error_category,created_at
                 ) VALUES(?1,0,?2,?3,?4,?5)",
                params![model, success, exit_code, error_category, created_at],
            )
            .unwrap();
    }
    assert_eq!(fixtures::user_version(&connection), 0);
    assert_eq!(
        schema::classify(&connection).unwrap(),
        SchemaCompatibility::LegacyVersionless
    );
    drop(connection);

    drop(StateDb::open(&path).unwrap());
    let first = versionless_invocation_snapshot(&path);
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].0, "versionless-failure");
    assert_eq!(first[0].2, 0);
    assert_eq!(first[0].3, 0);
    assert_eq!(first[0].4, 7);
    assert_eq!(first[0].5.as_deref(), Some("fixture_failure"));
    assert_eq!(first[0].6, "2020-01-01T00:01:00Z");
    assert_eq!(first[1].0, "versionless-success");
    assert_eq!(first[1].2, 0);
    assert_eq!(first[1].3, 1);
    assert_eq!(first[1].4, 0);
    assert_eq!(first[1].5, None);
    assert_eq!(first[1].6, "2020-01-01T00:00:00Z");
    assert!(
        first
            .iter()
            .all(|row| { row.7.is_none() && row.8.is_none() && row.9 == "legacy_unknown" }),
        "versionless rows must retain unknown terminal age: {first:?}"
    );
    assert!(
        first
            .iter()
            .all(|row| uuid::Uuid::parse_str(&row.1).is_ok())
    );
    assert_ne!(first[0].1, first[1].1);

    drop(StateDb::open(&path).unwrap());
    assert_eq!(versionless_invocation_snapshot(&path), first);
}

#[test]
fn versionless_unrecognized_pre_uuid_full_runner_fails_closed_without_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let connection = Connection::open(&path).unwrap();
    fixtures::create_full_state_schema(&connection, 0);
    connection
        .execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE invocations;")
        .unwrap();
    create_pre_uuid_invocations(&connection);
    connection
        .execute_batch("ALTER TABLE invocations ADD COLUMN unexpected_hand_edit TEXT;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO invocations(
                model_name,provider_index,success,exit_code,error_category,created_at,
                unexpected_hand_edit
             ) VALUES('unsupported-shape',3,0,17,'unknown','2020-01-01T00:00:00Z','keep-me')",
            [],
        )
        .unwrap();
    assert_eq!(
        schema::classify(&connection).unwrap(),
        SchemaCompatibility::LegacyVersionless
    );
    let before_schema = fixtures::schema_fingerprint(&connection);
    let before_version = fixtures::user_version(&connection);
    let before_journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let before_row: (String, i64, i64, i64, String) = connection
        .query_row(
            "SELECT model_name,provider_index,success,exit_code,unexpected_hand_edit
             FROM invocations",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    drop(connection);

    let error = match StateDb::open(&path) {
        Ok(_) => panic!("unrecognized pre-UUID shape must fail closed"),
        Err(error) => error,
    };
    assert!(error.contains("unrecognized schema shape"), "{error}");

    let connection = Connection::open(&path).unwrap();
    assert_eq!(fixtures::user_version(&connection), before_version);
    assert_eq!(fixtures::schema_fingerprint(&connection), before_schema);
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        before_journal
    );
    let after_row: (String, i64, i64, i64, String) = connection
        .query_row(
            "SELECT model_name,provider_index,success,exit_code,unexpected_hand_edit
             FROM invocations",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(after_row, before_row);
}

fn create_pre_uuid_invocations(connection: &Connection) {
    connection
        .execute_batch(
            "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                success INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                error_category TEXT,
                created_at TEXT NOT NULL
             );",
        )
        .unwrap();
}

fn assert_invocation_contract_and_audited_repair(
    path: &std::path::Path,
    invocation_uuid: &str,
    repaired_at: &str,
) {
    let connection = Connection::open(path).unwrap();
    let original: String = connection
        .query_row(
            "SELECT finished_at FROM invocations WHERE invocation_uuid=?1",
            [invocation_uuid],
            |row| row.get(0),
        )
        .unwrap();
    let rewrite = connection
        .execute(
            "UPDATE invocations SET finished_at='2035-01-01T00:00:00Z'
             WHERE invocation_uuid=?1",
            [invocation_uuid],
        )
        .unwrap_err();
    assert!(
        rewrite
            .to_string()
            .contains("terminal timestamp is immutable")
    );
    let reopen = connection
        .execute(
            "UPDATE invocations SET status='running' WHERE invocation_uuid=?1",
            [invocation_uuid],
        )
        .unwrap_err();
    assert!(reopen.to_string().contains("terminal state cannot reopen"));
    drop(connection);

    let mut historical = StateDb::open_historical(path).unwrap();
    let repair_id = historical
        .repair_terminal_timestamp(StateTerminalTimestampRepair {
            family: StateTimestampRecordFamily::Invocation,
            record_key: invocation_uuid,
            new_timestamp: repaired_at,
            actor: "age371-current-repair-test",
            reason: "verified current-schema fixture evidence",
        })
        .unwrap();
    drop(historical);

    let connection = Connection::open(path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT finished_at FROM invocations WHERE invocation_uuid=?1",
                [invocation_uuid],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        repaired_at
    );
    let audit: (String, String) = connection
        .query_row(
            "SELECT old_value,new_value FROM record_timestamp_repairs WHERE repair_id=?1",
            [&repair_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(audit, (original, repaired_at.to_string()));
    assert!(
        connection
            .execute(
                "DELETE FROM record_timestamp_repairs WHERE repair_id=?1",
                [&repair_id],
            )
            .is_err()
    );
}

fn invocation_contract_snapshot(
    path: &std::path::Path,
    invocation_uuid: &str,
) -> (String, Option<String>, String, i64) {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT lifecycle_updated_at,retention_eligible_at,retention_status,
                    (SELECT COUNT(*) FROM record_timestamp_repairs
                     WHERE record_family='invocation' AND record_key=?1)
             FROM invocations WHERE invocation_uuid=?1",
            [invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

type VersionlessInvocationSnapshot = (
    String,
    String,
    i64,
    i64,
    i64,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
);

fn versionless_invocation_snapshot(path: &std::path::Path) -> Vec<VersionlessInvocationSnapshot> {
    let connection = Connection::open(path).unwrap();
    assert_eq!(
        fixtures::user_version(&connection),
        oulipoly_state::schema::CURRENT_SCHEMA_VERSION
    );
    let mut statement = connection
        .prepare(
            "SELECT model_name,invocation_uuid,provider_index,success,exit_code,error_category,
                    created_at,finished_at,
                    retention_eligible_at,retention_status
             FROM invocations ORDER BY model_name",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
