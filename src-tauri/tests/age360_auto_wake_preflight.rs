//! Invalid automatic resume requests must not bootstrap a sidecar or provider.
//! Markers select validation, never confer ownership. Native positive controls
//! live in age360_completion_continuation (real independent driver/custodian).
#![cfg(unix)]

use std::process::{Command, Stdio};

const SESSION: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";

#[test]
fn invalid_auto_wake_resume_routes_do_not_create_state() {
    for args in [
        vec!["resume", SESSION, "--prompt", "not executed"],
        vec![
            "resume",
            "--session-id",
            SESSION,
            "--prompt",
            "not executed",
        ],
        vec!["-m", "never-loaded-model", "--resume", SESSION],
        vec!["--resume", SESSION],
        vec!["repl", "--resume", SESSION],
    ] {
        for (marker, session, token) in [
            ("1", SESSION, "missing-claim"),
            ("0", SESSION, "missing-claim"),
            ("", SESSION, "missing-claim"),
            ("1", "wrong-session", "missing-claim"),
            ("1", SESSION, ""),
            ("1", "", "missing-claim"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let output = entry(root.path())
                .env("OULIPOLY_AUTO_WAKE", marker)
                .env("OULIPOLY_AUTO_WAKE_SESSION_ID", session)
                .env("OULIPOLY_AUTO_WAKE_TOKEN", token)
                .stdin(Stdio::null())
                .args(&args)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
            assert!(output.stdout.is_empty(), "{output:?}");
            assert!(output.stderr.is_empty(), "{output:?}");
            assert_eq!(
                std::fs::read_dir(root.path()).unwrap().count(),
                0,
                "{args:?}"
            );
        }
    }
}

fn entry(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("CODEX_HOME", root.join("codex"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("OULIPOLY_DATA_DIR", root.join("data"));
    command
}

#[test]
fn seeded_state_missing_sidecar_rejects_auto_wake_without_invocation() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let state = oulipoly_state::StateDb::open(&data.join("state.db")).unwrap();
    drop(state);
    let sidecar = data.join("pid-identity.db");
    assert!(!sidecar.exists());
    let output = entry(root.path())
        .env("OULIPOLY_AUTO_WAKE", "1")
        .env("OULIPOLY_AUTO_WAKE_SESSION_ID", SESSION)
        .env("OULIPOLY_AUTO_WAKE_TOKEN", "absent-claim")
        .args([
            "resume",
            "--session-id",
            SESSION,
            "--prompt",
            "never executed",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(!sidecar.exists());
    let conn = rusqlite::Connection::open_with_flags(
        data.join("state.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

// These fixtures contain a recoverable *previous-boot* runtime and an unspent
// claim. They are deliberately not native owner/launcher authority. Rejected
// entry must not let a bogus token trigger recovery, migration or admission.
fn seeded_sidecar(root: &std::path::Path, legacy: bool) {
    let data = root.join("data");
    drop(oulipoly_state::StateDb::open(&data.join("state.db")).unwrap());
    let path = data.join("pid-identity.db");
    drop(oulipoly_state::mailbox::MailboxDb::open(&path).unwrap());
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO runtime_generation (
            generation_uuid, lifecycle_state, spawn_invocation_uuid, session_id,
            runtime_mode, provider_name, spawned_os_pid, identity_os_pid,
            identity_os_boot_id, identity_os_pid_starttime_ticks, created_at, running_at,
            active_delivery_claim_uuid, active_delivery_claimed_at, active_delivery_seqs_json,
            creator_identity_os_pid, creator_identity_os_boot_id, creator_identity_os_pid_starttime_ticks
         ) VALUES (?1, 'running', 'ordering-prior-invocation', ?2,
            'headless', 'never-executed', 2147483647, 2147483647,
            'd9d2dbd3-b989-449f-8f60-51f995ce67b0', 1, '2026-01-01', '2026-01-01',
            'caef4f55-5838-4094-9f48-b6863114050e', '2026-01-01', '[1]',
            2147483647, 'd9d2dbd3-b989-449f-8f60-51f995ce67b0', 1)",
        ["7e87db38-1c8c-4398-874e-93fd0449f691", SESSION],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_wake_claim
         (session_id, claim_token, claimed_at, reason, auto_wake_count)
         VALUES (?1, 'fixture-claim', '2026-01-01', 'ordering-control', 1)",
        [SESSION],
    )
    .unwrap();
    if legacy {
        // Undo precisely the additive v19 schema, not a version-only mismatch.
        conn.execute_batch(
            "DROP TRIGGER completion_continuation_notification_ack;
             DROP TABLE completion_continuation_notification;
             PRAGMA user_version=18;",
        )
        .unwrap();
    }
}

fn logical_snapshot(path: &std::path::Path) -> (i64, Vec<(String, Vec<String>)>) {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let version = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let mut tables = vec!["sqlite_schema".to_string()];
    tables.extend(
        conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap),
    );
    let rows = tables
        .into_iter()
        .map(|table| {
            let sql = format!("SELECT * FROM \"{}\"", table.replace('"', "\"\""));
            let mut query = conn.prepare(&sql).unwrap();
            let columns = query.column_count();
            let mut rows = query
                .query_map([], |row| {
                    let values = (0..columns)
                        .map(|i| row.get::<_, rusqlite::types::Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(format!("{values:?}"))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>();
            rows.sort();
            (table, rows)
        })
        .collect();
    (version, rows)
}

fn file_snapshot(root: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.push((path.clone(), Vec::new()));
            files.extend(file_snapshot(&path));
        } else {
            files.push((path.clone(), std::fs::read(&path).unwrap()));
        }
    }
    files.sort();
    files
}

fn existing_sidecar_refusal(dead_endpoint: bool) {
    for legacy in [false, true] {
        for token in ["bogus-token", "fixture-claim"] {
            for args in [
                vec!["resume", SESSION, "--prompt", "must not execute"],
                vec![
                    "resume",
                    "--session-id",
                    SESSION,
                    "--prompt",
                    "must not execute",
                ],
                vec!["-m", "never-loaded-model", "--resume", SESSION],
                vec!["--resume", SESSION],
                vec!["repl", "--resume", SESSION],
            ] {
                let root = tempfile::tempdir().unwrap();
                seeded_sidecar(root.path(), legacy);
                let sidecar = root.path().join("data/pid-identity.db");
                let state = root.path().join("data/state.db");
                let before_mailbox = logical_snapshot(&sidecar);
                let before_state = logical_snapshot(&state);
                let before_files = file_snapshot(root.path());
                let mut command = entry(root.path());
                command
                    .env("OULIPOLY_AUTO_WAKE", "1")
                    .env("OULIPOLY_AUTO_WAKE_SESSION_ID", SESSION)
                    .env("OULIPOLY_AUTO_WAKE_TOKEN", token)
                    .stdin(Stdio::null())
                    .args(&args);
                if dead_endpoint {
                    command.env(
                        "OULIPOLY_COMPLETION_ENDPOINT",
                        root.path().join("absent.sock"),
                    );
                }
                let output = command.output().unwrap();
                if dead_endpoint {
                    assert!(!output.status.success(), "{args:?}: {output:?}");
                    // Existing bootstrap participates in live SQLite. It is not
                    // a physical read-only contract, unlike static rejection.
                } else {
                    assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
                    assert!(
                        output.stdout.is_empty() && output.stderr.is_empty(),
                        "{output:?}"
                    );
                    assert_eq!(file_snapshot(root.path()), before_files, "{args:?}");
                }
                // Includes unchanged user_version/schema, runtime state and
                // active delivery claim, wake identity, all attempts/admissions,
                // native contexts/owners, and the entire invocation table.
                assert_eq!(logical_snapshot(&sidecar), before_mailbox, "{args:?}");
                assert_eq!(logical_snapshot(&state), before_state, "{args:?}");
            }
        }
    }
}

#[test]
fn existing_sidecar_without_owner_cannot_migrate_reconcile_or_admit() {
    existing_sidecar_refusal(false);
}

#[test]
fn existing_sidecar_dead_endpoint_cannot_migrate_reconcile_or_admit() {
    existing_sidecar_refusal(true);
}

#[cfg(target_os = "linux")]
#[test]
fn writable_validator_control_really_migrates_and_recovers_fixture() {
    let root = tempfile::tempdir().unwrap();
    seeded_sidecar(root.path(), true);
    let path = root.path().join("data/pid-identity.db");
    assert_eq!(logical_snapshot(&path).0, 18);
    let mut db = oulipoly_state::mailbox::MailboxDb::open(&path).unwrap();
    let identity =
        oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
    assert!(
        !db.wake_sessions()
            .validate_wake_claim_for_child(SESSION, "bogus-token", &identity)
            .unwrap()
    );
    drop(db);
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        19
    );
    let recovered = conn.query_row(
        "SELECT lifecycle_state, terminal_reason, active_delivery_claim_uuid FROM runtime_generation",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
    ).unwrap();
    assert_eq!(recovered, ("exited".into(), "recovered_dead".into(), None));
}
