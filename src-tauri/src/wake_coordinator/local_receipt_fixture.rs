//! Test fixture: legacy local receipt markers have no recipient ACK authority.
use oulipoly_state::mailbox::MailboxDb;
use std::fs;
#[cfg(test)]
pub(super) struct LocalReceiptFixture {
    _dir: tempfile::TempDir,
    db_path: std::path::PathBuf,
    state_dir: std::path::PathBuf,
}

#[cfg(test)]
struct LocalReceiptFixturePaths {
    state_path: std::path::PathBuf,
    db_path: std::path::PathBuf,
    state_dir: std::path::PathBuf,
    state_dir_text: String,
    meta_path: String,
    log_path: String,
    rc_path: String,
}

#[cfg(test)]
impl LocalReceiptFixture {
    pub(super) const EVENT_ID: &'static str = "ab_late_consumed_fixture";
    pub(super) const INVOCATION_UUID: &'static str = "11111111-1111-4111-8111-111111111111";
    pub(super) const SESSION_ID: &'static str = "session-late-consumed-fixture";

    pub(super) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = local_receipt_fixture_paths(&dir);
        stage_local_receipt_fixture_files(&paths);
        seed_local_receipt_fixture_mailbox(&paths);
        map_local_receipt_fixture(dir, paths)
    }

    pub(super) fn mark_local_receipt(&self) {
        fs::write(self.state_dir.join("consumed"), []).unwrap();
    }

    pub(super) fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.db_path).unwrap()
    }
}

#[cfg(test)]
fn local_receipt_fixture_paths(dir: &tempfile::TempDir) -> LocalReceiptFixturePaths {
    let state_path = dir.path().join("state.db");
    let db_path = MailboxDb::path_for_state_db(&state_path);
    let state_dir = dir.path().join("agent-bash-state");
    LocalReceiptFixturePaths {
        state_path,
        db_path,
        state_dir_text: state_dir.to_string_lossy().to_string(),
        meta_path: state_dir.join("meta.json").to_string_lossy().to_string(),
        log_path: state_dir.join("log").to_string_lossy().to_string(),
        rc_path: state_dir.join("rc").to_string_lossy().to_string(),
        state_dir,
    }
}

#[cfg(test)]
fn stage_local_receipt_fixture_files(paths: &LocalReceiptFixturePaths) {
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.meta_path, format_local_receipt_fixture_owner()).unwrap();
}

#[cfg(test)]
fn format_local_receipt_fixture_owner() -> String {
    serde_json::json!({
        "owner_session_id": LocalReceiptFixture::SESSION_ID,
        "owner_invocation_uuid": LocalReceiptFixture::INVOCATION_UUID,
    })
    .to_string()
}

#[cfg(test)]
fn seed_local_receipt_fixture_mailbox(paths: &LocalReceiptFixturePaths) {
    use oulipoly_state::mailbox::{CompletionEventRegistrationInput, CompletionEventTriggerInput};
    use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};

    let mut state = StateDb::open(&paths.state_path).unwrap();
    let invocation_start = state
        .start_invocation_with_completion_registration_authority(&InvocationStart {
            invocation_uuid: LocalReceiptFixture::INVOCATION_UUID.to_string(),
            model_name: "consumed-completion-fixture".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            oulipoly_state::InvocationMutationAuthority::Standalone,
            invocation_start.invocation_row_id,
            &ProviderSessionBinding {
                provider_session_id: LocalReceiptFixture::SESSION_ID.to_string(),
                capture_method: "fixture",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    state
        .register_completion_event_with_authority(
            oulipoly_state::InvocationMutationAuthority::Standalone,
            &invocation_start.completion_registration_authority,
            "late-consumed-fixture-admission",
            CompletionEventRegistrationInput {
                event_id: LocalReceiptFixture::EVENT_ID,
                delivery_mode: "async",
                owner_session_id: Some(LocalReceiptFixture::SESSION_ID),
                owner_invocation_uuid: Some(LocalReceiptFixture::INVOCATION_UUID),
                state_dir: &paths.state_dir_text,
                meta_path: &paths.meta_path,
                log_path: &paths.log_path,
                rc_path: &paths.rc_path,
            },
        )
        .unwrap();
    let mut db = MailboxDb::open(&paths.db_path).unwrap();
    db.trigger_completion_event(CompletionEventTriggerInput {
        event_id: LocalReceiptFixture::EVENT_ID,
        payload_json: r#"{"schema_version":2,"handle":"ab_late_consumed_fixture"}"#,
        state_dir: &paths.state_dir_text,
        meta_path: &paths.meta_path,
        log_path: &paths.log_path,
        rc_path: &paths.rc_path,
        rc: 0,
    })
    .unwrap();
}

#[cfg(test)]
fn map_local_receipt_fixture(
    dir: tempfile::TempDir,
    paths: LocalReceiptFixturePaths,
) -> LocalReceiptFixture {
    LocalReceiptFixture {
        _dir: dir,
        db_path: paths.db_path,
        state_dir: paths.state_dir,
    }
}
