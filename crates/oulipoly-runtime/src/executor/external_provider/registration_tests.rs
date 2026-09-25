//! Private synthetic failures through the production registration and error carrier.
use super::*;

const INVOCATION: &str = "11111111-1111-4111-8111-111111111111";

fn context(path: &std::path::Path) -> SpawnIdentityContext {
    context_from_parent_invocation_env(
        Some(r#"{"source":"opencode3","id":"11111111-1111-4111-8111-111111111111"}"#),
        "SECRET_PROVIDER",
        None,
        Some("fixture-session"),
        SpawnRuntimeMode::Headless,
        None,
        None,
    )
    .unwrap()
    .with_mailbox_db_path(path.to_path_buf())
}

fn failure(context: &SpawnIdentityContext, cause: &str) {
    let error = register_standalone_generation(Some(context)).expect_err("registration must fail");
    let ServiceError::Dependency { message } = error.service_error else {
        panic!("classification changed");
    };
    assert_eq!(
        message,
        format!(
            "external provider protocol failed: runtime_generation_registration_failed; cause={cause}; invocation={INVOCATION}"
        )
    );
    assert!(matches!(
        *error.failure,
        ProviderLaunchFailure::Execution(_)
    ));
    assert!(!message.contains("SECRET"));
}

#[test]
fn registration_storage_open_failure_is_bounded_and_correlated() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("SECRET_CONFIG");
    std::fs::create_dir(&secret).unwrap();
    let path = secret.join("pid-identity.db");
    std::fs::create_dir(&path).unwrap();
    failure(&context(&path), "StorageOpen");
}

#[test]
fn registration_native_exclusion_and_binding_storage_are_distinct() {
    for broken_storage in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        drop(oulipoly_state::mailbox::MailboxDb::open(&path).unwrap());
        let conn = rusqlite::Connection::open(&path).unwrap();
        // Synthetic pre-existing reservation; no production owner or fake drain.
        conn.execute_batch("INSERT INTO completion_supervisor_authority(authority_id,domain_id,phase,created_by_generation,guardian_identity)
            SELECT 'fixture-authority',domain_id,'active','owner','{}' FROM completion_continuation_domain;
            INSERT INTO completion_continuation_owner(generation,domain_id,phase,guardian_identity,driver_identity,endpoint,supervisor_authority_id)
            SELECT 'owner',domain_id,'running','{}','{}','fixture','fixture-authority' FROM completion_continuation_domain;
            INSERT INTO completion_continuation_attempt(attempt_id,domain_id,owner_generation,operation,request_sha256,session_id,claim_token,phase,result_path,supervisor_authority_id)
            SELECT 'attempt',domain_id,'owner','activation','hash','fixture-session','claim','reserved','fixture','fixture-authority' FROM completion_continuation_domain;").unwrap();
        if broken_storage {
            let identity = oulipoly_state::pid_identity::read_live_process_identity(i64::from(
                std::process::id(),
            ))
            .unwrap()
            .unwrap();
            let launcher = serde_json::to_string(
                &oulipoly_state::completion_continuation::SourceProcessIdentity {
                    pid: identity.os_pid,
                    boot_id: identity.os_boot_id,
                    starttime_ticks: identity.os_pid_starttime_ticks,
                },
            )
            .unwrap();
            conn.execute("UPDATE completion_continuation_attempt SET launcher_identity=?1,revision=revision+1 WHERE attempt_id='attempt'", [&launcher]).unwrap();
            conn.execute_batch("CREATE TRIGGER secret_binding_failure BEFORE UPDATE ON completion_continuation_attempt BEGIN SELECT RAISE(ABORT,'SECRET_STORAGE_DETAIL'); END;").unwrap();
        }
        failure(
            &context(&path),
            if broken_storage {
                "Creation(NativeActivationBinding)"
            } else {
                "Creation(NativeActivationConflict)"
            },
        );
        let count: i64 = conn
            .query_row("SELECT count(*) FROM runtime_generation", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "failed registration must not create a generation");
        let phase: String = conn
            .query_row(
                "SELECT phase FROM completion_continuation_attempt WHERE attempt_id='attempt'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            phase, "reserved",
            "diagnostics must not discharge reservation"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn registration_custody_setup_failure_does_not_leak_path() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("SECRET_CONFIG");
    std::fs::create_dir(&secret).unwrap();
    let path = secret.join("pid-identity.db");
    drop(oulipoly_state::mailbox::MailboxDb::open(&path).unwrap());
    std::fs::write(
        path.with_extension("starting-custody-v1"),
        b"not a directory",
    )
    .unwrap();
    failure(&context(&path), "CustodyStart");
}
