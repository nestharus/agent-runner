//! Exercises the common host fence through standalone, allocated and legacy entrypoints.
use super::*;
use rusqlite::Connection;

const INVOCATION: &str = "77777777-7777-4777-8777-777777777777";
fn context(path: &Path) -> SpawnIdentityContext {
    context_from_parent_invocation_env(
        Some(r#"{"source":"fixture","id":"77777777-7777-4777-8777-777777777777"}"#),
        "fixture",
        None,
        Some("session"),
        SpawnRuntimeMode::Headless,
        None,
        None,
    )
    .unwrap()
    .with_mailbox_db_path(path.to_path_buf())
}
fn prepare(sql: &Connection) {
    sql.execute("INSERT INTO mailbox_delivery_attempts(attempt_id,session_id,delivery_invocation_uuid,created_at,prepared_remaining_count,headless_submission_state)
        VALUES('delivery','session',?1,'fixture',1,'prepared')",[INVOCATION]).unwrap();
}
fn submission(sql: &Connection) -> (String, Option<String>) {
    sql.query_row("SELECT headless_submission_state,submission_started_at FROM mailbox_delivery_attempts WHERE attempt_id='delivery'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap()
}
fn register(c: &SpawnIdentityContext, path: usize) -> Result<(), String> {
    match path {
        0 => register_runtime_generation_starting_diagnostic(Some(c)).map_err(|e| e.to_string()),
        1 => register_allocated_runtime_generation_starting(c),
        _ => register_runtime_generation_starting(Some(c)),
    }
}
#[test]
fn age360_registration_rejection_is_unsubmitted_in_every_headless_entry() {
    for path in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar).unwrap());
        let sql = Connection::open(&sidecar).unwrap();
        prepare(&sql);
        sql.execute_batch("INSERT INTO completion_supervisor_authority(authority_id,domain_id,phase,created_by_generation,guardian_identity)
            SELECT 'fixture-authority',domain_id,'active','owner','{}' FROM completion_continuation_domain;
            INSERT INTO completion_continuation_owner(generation,domain_id,phase,guardian_identity,driver_identity,endpoint,supervisor_authority_id)
            SELECT 'owner',domain_id,'running','{}','{}','fixture','fixture-authority' FROM completion_continuation_domain;
            INSERT INTO completion_continuation_attempt(attempt_id,domain_id,owner_generation,operation,request_sha256,session_id,claim_token,phase,result_path,supervisor_authority_id)
            SELECT 'attempt',domain_id,'owner','activation','hash','session','claim','reserved','fixture','fixture-authority' FROM completion_continuation_domain;").unwrap();
        let c = context(&sidecar);
        let error = register(&c, path).unwrap_err();
        assert!(
            error.contains("current session activation reservation"),
            "{error}"
        );
        assert_eq!(submission(&sql), ("prepared".into(), None));
        let count: i64 = sql
            .query_row("SELECT count(*) FROM runtime_generation", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
#[test]
fn age360_registered_submission_is_possible_and_cannot_cross_twice() {
    for path in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("pid-identity.db");
        drop(MailboxDb::open(&sidecar).unwrap());
        let sql = Connection::open(&sidecar).unwrap();
        prepare(&sql);
        let c = context(&sidecar);
        register(&c, path).unwrap();
        let first = submission(&sql);
        assert_eq!(first.0, "possible");
        assert!(first.1.is_some());
        assert!(
            register(&c, path).is_err(),
            "entry {path} crossed a second time"
        );
        assert_eq!(
            submission(&sql),
            first,
            "historic possible must survive rejection"
        );
        let phase: String = sql
            .query_row("SELECT lifecycle_state FROM runtime_generation", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            phase, "starting",
            "second crossing must not finalize the original registration"
        );
        // Registration/fence only: no provider is spawned by this unit witness.
        let _ = finalize_or_retain_starting_failure(Some(&c));
    }
}
#[test]
fn age360_submission_storage_failure_is_not_permission_to_launch() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = dir.path().join("pid-identity.db");
    drop(MailboxDb::open(&sidecar).unwrap());
    let sql = Connection::open(&sidecar).unwrap();
    prepare(&sql);
    sql.execute_batch("CREATE TRIGGER stop_submission BEFORE UPDATE OF submission_started_at ON mailbox_delivery_attempts BEGIN SELECT RAISE(ABORT,'submission fault'); END;").unwrap();
    let c = context(&sidecar);
    let failure = register_runtime_generation_starting_diagnostic(Some(&c)).unwrap_err();
    assert_eq!(failure.cause, RegistrationCause::SubmissionFence);
    assert_eq!(submission(&sql), ("prepared".into(), None));
}
