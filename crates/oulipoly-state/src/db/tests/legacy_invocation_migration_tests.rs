//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn migration_backfills_resolved_and_legacy_rows() {
    with_models_config(
        "mapped-model",
        r#"
[[providers]]
name = "fixture-provider"
"#,
        || {
            let dir = legacy_invocations_db(&[
                ("mapped-model", 0, 1, 0, None, "2026-04-17T08:00:00Z"),
                (
                    "missing-model",
                    0,
                    0,
                    7,
                    Some("rate_limit"),
                    "2026-04-17T08:05:00Z",
                ),
            ]);
            let db = StateDb::open(&dir.path().join("state.db")).unwrap();

            let rows: Vec<(String, Option<String>, String, String, String)> = db
                .conn
                .prepare(
                    "SELECT model_name, provider_name, status, invocation_uuid, finished_at
                         FROM invocations ORDER BY created_at",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();

            assert_eq!(rows[0].0, "mapped-model");
            assert_eq!(rows[0].1.as_deref(), Some("fixture-provider"));
            assert_eq!(rows[0].2, "succeeded");
            assert_eq!(rows[0].4, "2026-04-17T08:00:00Z");
            assert!(Uuid::parse_str(&rows[0].3).is_ok());

            assert_eq!(rows[1].0, "missing-model");
            assert_eq!(rows[1].1, None);
            assert_eq!(rows[1].2, "legacy");
            assert_eq!(rows[1].4, "2026-04-17T08:05:00Z");
            assert!(Uuid::parse_str(&rows[1].3).is_ok());
        },
    );
}

#[test]
fn migration_rolls_back_when_rebuild_fails() {
    let dir = legacy_invocations_db(&[("mapped-model", 0, 1, 0, None, "2026-04-17T08:00:00Z")]);
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE invocations_new (id INTEGER PRIMARY KEY);
             CREATE TABLE blocker (name TEXT);
             CREATE INDEX idx_invocations_uuid ON blocker(name);",
    )
    .unwrap();
    drop(conn);

    let err = match StateDb::open(&path) {
        Ok(_) => panic!("migration should fail"),
        Err(err) => err,
    };
    assert!(!err.is_empty());

    let conn = sqlite::Connection::open(&path).unwrap();
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(invocations)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        columns,
        vec![
            "id",
            "model_name",
            "provider_index",
            "success",
            "exit_code",
            "error_category",
            "created_at",
        ]
    );
    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(row_count, 1);
}

#[test]
fn migration_succeeds_with_corrupt_models_config_and_marks_rows_legacy() {
    let _guard = env_lock().lock().unwrap();
    let dir = legacy_invocations_db(&[
        ("any-model", 0, 1, 0, None, "2026-04-17T08:00:00Z"),
        (
            "other-model",
            1,
            0,
            1,
            Some("rate_limit"),
            "2026-04-17T08:05:00Z",
        ),
    ]);
    let path = dir.path().join("state.db");

    // Plant a corrupt models/ directory at XDG_CONFIG_HOME so the
    // load_models() call inside migration fails.
    let config_root = dir.path().join("oulipoly-agent-runner");
    let models_dir = config_root.join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(
        models_dir.join("broken.toml"),
        "this = is = not = valid = toml",
    )
    .unwrap();

    let old = std::env::var_os("XDG_CONFIG_HOME");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The DB open must succeed despite the corrupt config.
        let db = StateDb::open(&path).expect("DB open must not fail on corrupt models config");

        // Verify both legacy rows migrated cleanly with provider_name=NULL
        // and status='legacy' since the lookup couldn't resolve anything.
        let conn = sqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT model_name, provider_name, status, invocation_uuid, finished_at
                     FROM invocations ORDER BY created_at",
            )
            .unwrap();
        let rows: Vec<(String, Option<String>, String, String, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), 2);
        for r in &rows {
            assert!(
                r.1.is_none(),
                "provider_name must be NULL on corrupt config"
            );
            assert_eq!(r.2, "legacy", "status must be legacy on corrupt config");
            assert!(Uuid::parse_str(&r.3).is_ok());
            assert!(!r.4.is_empty(), "finished_at must be backfilled");
        }
        drop(db);
    }));
    match old {
        Some(value) => unsafe {
            std::env::set_var("XDG_CONFIG_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        },
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
