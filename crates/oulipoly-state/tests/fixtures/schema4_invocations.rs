use super::{create_full_state_schema, open};
use rusqlite::{Connection, params};
use std::path::Path;

pub const SCHEMA4_ROOT_UUID: &str = "54000000-0000-4000-8000-000000000001";
pub const SCHEMA4_CHILD_UUID: &str = "54000000-0000-4000-8000-000000000002";
pub const SCHEMA4_RESUMED_UUID: &str = "54000000-0000-4000-8000-000000000003";
pub const SCHEMA4_NULL_SESSION_UUID: &str = "54000000-0000-4000-8000-000000000004";
pub const SCHEMA4_RUNNING_UUID: &str = "54000000-0000-4000-8000-000000000005";
pub const SCHEMA4_FAILED_UUID: &str = "54000000-0000-4000-8000-000000000006";
pub const SCHEMA4_LEGACY_UUID: &str = "54000000-0000-4000-8000-000000000007";
pub const SCHEMA4_SECOND_PROVIDER_UUID: &str = "54000000-0000-4000-8000-000000000008";
pub const PROVIDER_SESSION_A: &str = "provider-session-a";
pub const PROVIDER_SESSION_B: &str = "provider-session-b";
pub const RESUME_INPUT_A: &str = "resume-input-a";

pub fn build_schema4_invocation_fixture(path: &Path) {
    let conn = open(path);
    create_full_state_schema(&conn, 4);
    seed_schema4_invocation_rows(&conn);
}

pub fn seed_schema4_invocation_rows(conn: &Connection) {
    conn.execute("DELETE FROM invocations", []).unwrap();
    conn.execute_batch(
        "
        INSERT INTO providers
            (model_name, provider_name, invocation_count, error_count, last_invoked_at)
        VALUES
            ('fixture-model', 'fixture-provider', 7, 1, '2026-05-04T00:07:00Z'),
            ('fixture-model', 'other-provider', 1, 0, '2026-05-04T00:08:00Z')
        ON CONFLICT(model_name, provider_name) DO UPDATE SET
            invocation_count = excluded.invocation_count,
            error_count = excluded.error_count,
            last_invoked_at = excluded.last_invoked_at;
        ",
    )
    .unwrap();

    let rows = [
        (
            1,
            SCHEMA4_ROOT_UUID,
            "fixture-provider",
            0,
            None,
            "succeeded",
            Some(1),
            Some(0),
            None,
            Some("exit_zero"),
            Some(PROVIDER_SESSION_A),
            Some("stdout"),
            Some("accepted"),
            "2026-05-04T00:00:00Z",
            Some("2026-05-04T00:00:01Z"),
        ),
        (
            2,
            SCHEMA4_CHILD_UUID,
            "fixture-provider",
            0,
            Some(1),
            "succeeded",
            Some(1),
            Some(0),
            None,
            Some("exit_zero"),
            Some(PROVIDER_SESSION_A),
            Some("stdout_json_event"),
            Some("accepted"),
            "2026-05-04T00:01:00Z",
            Some("2026-05-04T00:01:01Z"),
        ),
        (
            3,
            SCHEMA4_RESUMED_UUID,
            "fixture-provider",
            0,
            None,
            "succeeded",
            Some(1),
            Some(0),
            None,
            Some("exit_zero"),
            Some(RESUME_INPUT_A),
            Some("resumed"),
            Some("accepted"),
            "2026-05-04T00:02:00Z",
            Some("2026-05-04T00:02:01Z"),
        ),
        (
            4,
            SCHEMA4_NULL_SESSION_UUID,
            "fixture-provider",
            0,
            None,
            "succeeded",
            Some(1),
            Some(0),
            None,
            Some("exit_zero"),
            None,
            Some("none"),
            None,
            "2026-05-04T00:03:00Z",
            Some("2026-05-04T00:03:01Z"),
        ),
        (
            5,
            SCHEMA4_RUNNING_UUID,
            "fixture-provider",
            0,
            None,
            "running",
            None,
            None,
            None,
            None,
            Some(PROVIDER_SESSION_B),
            Some("forced_flag_verified"),
            None,
            "2026-05-04T00:04:00Z",
            None,
        ),
        (
            6,
            SCHEMA4_FAILED_UUID,
            "fixture-provider",
            0,
            None,
            "failed",
            Some(0),
            Some(2),
            Some("rate_limit"),
            Some("exit_nonzero"),
            Some("failed-session"),
            Some("stdout"),
            Some("rejected"),
            "2026-05-04T00:05:00Z",
            Some("2026-05-04T00:05:01Z"),
        ),
        (
            7,
            SCHEMA4_LEGACY_UUID,
            "fixture-provider",
            0,
            None,
            "legacy",
            Some(0),
            Some(7),
            Some("unknown"),
            None,
            Some("legacy-session"),
            None,
            None,
            "2026-05-04T00:06:00Z",
            Some("2026-05-04T00:06:01Z"),
        ),
        (
            8,
            SCHEMA4_SECOND_PROVIDER_UUID,
            "other-provider",
            1,
            None,
            "succeeded",
            Some(1),
            Some(0),
            None,
            Some("exit_zero"),
            Some("other-session"),
            Some("stdout"),
            Some("accepted"),
            "2026-05-04T00:07:00Z",
            Some("2026-05-04T00:07:01Z"),
        ),
    ];

    for row in rows {
        conn.execute(
            "INSERT INTO invocations
                (id, invocation_uuid, model_name, provider_name, provider_index,
                 parent_invocation_id, status, success, exit_code, error_category,
                 terminal_reason, session_id, session_capture_method,
                 resume_acceptance_status, created_at, finished_at)
             VALUES
                (?1, ?2, 'fixture-model', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, ?15)",
            params![
                row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9, row.10,
                row.11, row.12, row.13, row.14
            ],
        )
        .unwrap();
    }
}
