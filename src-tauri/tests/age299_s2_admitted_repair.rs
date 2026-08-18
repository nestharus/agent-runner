#![cfg(target_os = "linux")]

//! Exact CLI repair coverage for admitted completion continuity suffixes.
//!
//! ## Declared roles
//! `orchestration`, `validator`

use oulipoly_state::mailbox::{CompletionEventRegistrationInput, MailboxDb};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const INVOCATION_UUID: &str = "11111111-1111-4111-8111-111111111111";
const SESSION_ID: &str = "age299-s2-admitted-repair-session";

#[test]
fn cli_repairs_each_exact_missing_suffix_row_without_expired_owner_authority() {
    let directory = tempfile::tempdir().unwrap();
    let data_dir = directory.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    let state_path = data_dir.join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let mut state = StateDb::open(&state_path).unwrap();
    let started = state
        .start_invocation_with_completion_registration_authority(&InvocationStart {
            invocation_uuid: INVOCATION_UUID.to_string(),
            model_name: "age299-s2-repair".to_string(),
            provider_name: "test-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            started.invocation_row_id,
            &ProviderSessionBinding {
                provider_session_id: SESSION_ID.to_string(),
                capture_method: "age299_s2_repair_fixture",
                resume_input_id: None,
                provider_session_resolved_account: Some("test-provider".to_string()),
            },
        )
        .unwrap();
    drop(MailboxDb::open(&sidecar_path).unwrap());
    let pre_admission_sidecar = fs::read(&sidecar_path).unwrap();

    let first = CompletionFixture::new(directory.path(), "ab_age299_s2_repair_first");
    let second = CompletionFixture::new(directory.path(), "ab_age299_s2_repair_second");
    for fixture in [&first, &second] {
        state
            .register_completion_event_with_authority(
                &started.completion_registration_authority,
                &fixture.caller_admission_id(),
                fixture.registration(),
            )
            .unwrap();
    }
    state
        .finalize_invocation(
            started.invocation_row_id,
            false,
            74,
            Some("fixture"),
            Some("spawn_error"),
        )
        .unwrap();

    fs::remove_file(&sidecar_path).unwrap();
    fs::write(&sidecar_path, pre_admission_sidecar).unwrap();

    let changed_log = directory.path().join("changed-log");
    fs::write(&changed_log, b"different registration identity\n").unwrap();
    let changed_identity = first.run_repair_with_log(&data_dir, &changed_log);
    assert_eq!(changed_identity.status.code(), Some(74));
    assert!(
        String::from_utf8_lossy(&changed_identity.stdout)
            .contains("requires an exact admitted replay")
    );

    let out_of_order = second.run_repair(&data_dir);
    assert_eq!(out_of_order.status.code(), Some(74));
    assert!(
        String::from_utf8_lossy(&out_of_order.stdout).contains("continuity heads do not match"),
        "{}",
        String::from_utf8_lossy(&out_of_order.stdout)
    );

    for fixture in [&first, &second] {
        let output = fixture.run_repair(&data_dir);
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["status"], "repaired");
    }

    let repeated = first.run_repair(&data_dir);
    assert!(repeated.status.success(), "{repeated:?}");
    let response: serde_json::Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(response["status"], "already_repaired");

    let unadmitted = CompletionFixture::new(directory.path(), "ab_age299_s2_repair_unadmitted");
    let rejected = unadmitted.run_repair(&data_dir);
    assert_eq!(rejected.status.code(), Some(74));
    assert!(
        String::from_utf8_lossy(&rejected.stdout).contains("requires an exact admitted replay")
    );

    let state_head: (i64, String) = state
        .connection()
        .query_row(
            "SELECT authority_ordinal, continuity_digest
             FROM invocation_completion_continuity
             ORDER BY authority_ordinal DESC
             LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let sidecar = rusqlite::Connection::open(&sidecar_path).unwrap();
    let sidecar_head: (i64, String) = sidecar
        .query_row(
            "SELECT authority_ordinal, continuity_digest
             FROM completion_authority_continuity
             ORDER BY authority_ordinal DESC
             LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state_head, sidecar_head);
    assert_eq!(sidecar_head.0, 2);
}

struct CompletionFixture {
    handle: String,
    state_dir: std::path::PathBuf,
    meta: std::path::PathBuf,
    log: std::path::PathBuf,
    rc: std::path::PathBuf,
}

impl CompletionFixture {
    fn new(root: &Path, handle: &str) -> Self {
        let state_dir = root.join(handle);
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        fs::write(
            &meta,
            serde_json::to_vec(&serde_json::json!({
                "owner_session_id": SESSION_ID,
                "owner_invocation_uuid": INVOCATION_UUID,
                "caller_chain": [],
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(&log, b"retained completion output\n").unwrap();
        fs::write(&rc, b"0\n").unwrap();
        Self {
            handle: handle.to_string(),
            state_dir,
            meta,
            log,
            rc,
        }
    }

    fn caller_admission_id(&self) -> String {
        format!(
            "completion:{}:{}:owner:{}:{}",
            self.handle.len(),
            self.handle,
            INVOCATION_UUID.len(),
            INVOCATION_UUID
        )
    }

    fn registration(&self) -> CompletionEventRegistrationInput<'_> {
        CompletionEventRegistrationInput {
            event_id: &self.handle,
            delivery_mode: "async",
            owner_session_id: Some(SESSION_ID),
            owner_invocation_uuid: Some(INVOCATION_UUID),
            state_dir: self.state_dir.to_str().unwrap(),
            meta_path: self.meta.to_str().unwrap(),
            log_path: self.log.to_str().unwrap(),
            rc_path: self.rc.to_str().unwrap(),
        }
    }

    fn run_repair(&self, data_dir: &Path) -> Output {
        self.run_repair_with_log(data_dir, &self.log)
    }

    fn run_repair_with_log(&self, data_dir: &Path, log: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
            .args([
                "notify",
                "agent-bash-register",
                "--handle",
                &self.handle,
                "--delivery-mode",
                "async",
                "--state-dir",
                self.state_dir.to_str().unwrap(),
                "--meta",
                self.meta.to_str().unwrap(),
                "--log",
                log.to_str().unwrap(),
                "--rc",
                self.rc.to_str().unwrap(),
                "--repair-admitted",
                "--json",
            ])
            .env("OULIPOLY_DATA_DIR", data_dir)
            .env_remove("OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY")
            .output()
            .unwrap()
    }
}
