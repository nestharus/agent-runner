//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator`

use oulipoly_state::mailbox::{AGENT_BASH_COMPLETE_KIND, MailboxDb, MailboxRow};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Deserialize)]
struct CompletionOwner {
    owner_session_id: String,
    owner_invocation_uuid: String,
}

enum CompletionOwnerError {
    Read(std::io::Error),
    Parse(serde_json::Error),
    EmptyBinding,
}

pub(super) fn reconcile_late_consumed_completions_on(
    db: &mut MailboxDb,
    session_id: &str,
) -> Result<(), String> {
    for row in db.list_pending(session_id)? {
        if !late_consumed_completion_candidate(&row) {
            continue;
        }
        let Some(owner) = completion_owner(&row) else {
            continue;
        };
        let Some(event_id) = db.acknowledge_consumed_completion_event_for_mailbox_seq(
            row.seq,
            &owner.owner_session_id,
            &owner.owner_invocation_uuid,
        )?
        else {
            continue;
        };
        emit_late_consumed_completion_acknowledgement(&row, &event_id);
    }
    Ok(())
}

fn late_consumed_completion_candidate(row: &MailboxRow) -> bool {
    row.kind == AGENT_BASH_COMPLETE_KIND && consumed_marker_is_regular_file(row)
}

fn emit_late_consumed_completion_acknowledgement(row: &MailboxRow, event_id: &str) {
    eprintln!(
        "{}",
        format_late_consumed_completion_acknowledgement(row, event_id)
    );
}

fn format_late_consumed_completion_acknowledgement(row: &MailboxRow, event_id: &str) -> String {
    format!(
        "late_consumed_completion_acknowledged session_id={} seq={} event_id={} handle={} owner_invocation_uuid={}",
        row.session_id,
        row.seq,
        event_id,
        row.handle,
        row.owner_invocation_uuid.as_deref().unwrap_or("unknown"),
    )
}

fn completion_owner(row: &MailboxRow) -> Option<CompletionOwner> {
    let result = read_completion_owner_metadata(row)
        .and_then(|metadata| parse_completion_owner(&metadata))
        .and_then(validate_completion_owner);
    match result {
        Ok(owner) => Some(owner),
        Err(err) => {
            warn_completion_owner_error(row, err);
            None
        }
    }
}

fn read_completion_owner_metadata(row: &MailboxRow) -> Result<Vec<u8>, CompletionOwnerError> {
    fs::read(&row.meta_path).map_err(CompletionOwnerError::Read)
}

fn parse_completion_owner(metadata: &[u8]) -> Result<CompletionOwner, CompletionOwnerError> {
    serde_json::from_slice(metadata).map_err(CompletionOwnerError::Parse)
}

fn validate_completion_owner(
    owner: CompletionOwner,
) -> Result<CompletionOwner, CompletionOwnerError> {
    if owner.owner_session_id.is_empty() || owner.owner_invocation_uuid.is_empty() {
        return Err(CompletionOwnerError::EmptyBinding);
    }
    Ok(owner)
}

fn warn_completion_owner_error(row: &MailboxRow, err: CompletionOwnerError) {
    match err {
        CompletionOwnerError::Read(err) => tracing::warn!(
            session_id = row.session_id,
            mailbox_seq = row.seq,
            meta = row.meta_path,
            "Failed to read consumed completion metadata: {err}"
        ),
        CompletionOwnerError::Parse(err) => tracing::warn!(
            session_id = row.session_id,
            mailbox_seq = row.seq,
            meta = row.meta_path,
            "Failed to parse consumed completion metadata: {err}"
        ),
        CompletionOwnerError::EmptyBinding => tracing::warn!(
            session_id = row.session_id,
            mailbox_seq = row.seq,
            meta = row.meta_path,
            "Consumed completion metadata has an empty owner binding"
        ),
    }
}

fn consumed_marker_is_regular_file(row: &MailboxRow) -> bool {
    let marker = consumed_marker_path(row);
    match marker_metadata(&marker) {
        Ok(metadata) => marker_metadata_is_regular_file(&metadata),
        Err(err) => {
            if !marker_is_missing(&err) {
                warn_consumed_marker_inspection(row, &marker, err);
            }
            false
        }
    }
}

fn consumed_marker_path(row: &MailboxRow) -> std::path::PathBuf {
    Path::new(&row.state_dir).join("consumed")
}

fn marker_metadata(marker: &Path) -> std::io::Result<std::fs::Metadata> {
    fs::symlink_metadata(marker)
}

fn marker_metadata_is_regular_file(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_file()
}

fn marker_is_missing(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

fn warn_consumed_marker_inspection(row: &MailboxRow, marker: &Path, err: std::io::Error) {
    tracing::warn!(
        session_id = row.session_id,
        mailbox_seq = row.seq,
        marker = %marker.display(),
        "Failed to inspect consumed completion marker: {err}"
    );
}

#[cfg(test)]
pub(super) struct ConsumedCompletionFixture {
    _dir: tempfile::TempDir,
    db_path: std::path::PathBuf,
    state_dir: std::path::PathBuf,
}

#[cfg(test)]
struct ConsumedCompletionFixturePaths {
    db_path: std::path::PathBuf,
    state_dir: std::path::PathBuf,
    state_dir_text: String,
    meta_path: String,
    log_path: String,
    rc_path: String,
}

#[cfg(test)]
impl ConsumedCompletionFixture {
    pub(super) const EVENT_ID: &'static str = "ab_late_consumed_fixture";
    pub(super) const INVOCATION_UUID: &'static str = "11111111-1111-4111-8111-111111111111";
    pub(super) const SESSION_ID: &'static str = "session-late-consumed-fixture";

    pub(super) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = consumed_completion_fixture_paths(&dir);
        stage_consumed_completion_fixture_files(&paths);
        seed_consumed_completion_fixture_mailbox(&paths);
        map_consumed_completion_fixture(dir, paths)
    }

    pub(super) fn mark_consumed(&self) {
        fs::write(self.state_dir.join("consumed"), []).unwrap();
    }

    pub(super) fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.db_path).unwrap()
    }
}

#[cfg(test)]
fn consumed_completion_fixture_paths(dir: &tempfile::TempDir) -> ConsumedCompletionFixturePaths {
    let db_path = dir.path().join("pid-identity.db");
    let state_dir = dir.path().join("agent-bash-state");
    ConsumedCompletionFixturePaths {
        db_path,
        state_dir_text: state_dir.to_string_lossy().to_string(),
        meta_path: state_dir.join("meta.json").to_string_lossy().to_string(),
        log_path: state_dir.join("log").to_string_lossy().to_string(),
        rc_path: state_dir.join("rc").to_string_lossy().to_string(),
        state_dir,
    }
}

#[cfg(test)]
fn stage_consumed_completion_fixture_files(paths: &ConsumedCompletionFixturePaths) {
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.meta_path, format_consumed_completion_fixture_owner()).unwrap();
}

#[cfg(test)]
fn format_consumed_completion_fixture_owner() -> String {
    serde_json::json!({
        "owner_session_id": ConsumedCompletionFixture::SESSION_ID,
        "owner_invocation_uuid": ConsumedCompletionFixture::INVOCATION_UUID,
    })
    .to_string()
}

#[cfg(test)]
fn seed_consumed_completion_fixture_mailbox(paths: &ConsumedCompletionFixturePaths) {
    use oulipoly_state::mailbox::{CompletionEventRegistrationInput, CompletionEventTriggerInput};

    let mut db = MailboxDb::open(&paths.db_path).unwrap();
    db.register_completion_event(CompletionEventRegistrationInput {
        event_id: ConsumedCompletionFixture::EVENT_ID,
        delivery_mode: "async",
        owner_session_id: Some(ConsumedCompletionFixture::SESSION_ID),
        owner_invocation_uuid: Some(ConsumedCompletionFixture::INVOCATION_UUID),
        state_dir: &paths.state_dir_text,
        meta_path: &paths.meta_path,
        log_path: &paths.log_path,
        rc_path: &paths.rc_path,
    })
    .unwrap();
    db.trigger_completion_event(CompletionEventTriggerInput {
        event_id: ConsumedCompletionFixture::EVENT_ID,
        payload_json: r#"{"schema_version":2,"handle":"ab_late_consumed_fixture"}"#,
        state_dir: &paths.state_dir_text,
        meta_path: &paths.meta_path,
        log_path: &paths.log_path,
        rc_path: &paths.rc_path,
        rc: 0,
        consumed: false,
    })
    .unwrap();
}

#[cfg(test)]
fn map_consumed_completion_fixture(
    dir: tempfile::TempDir,
    paths: ConsumedCompletionFixturePaths,
) -> ConsumedCompletionFixture {
    ConsumedCompletionFixture {
        _dir: dir,
        db_path: paths.db_path,
        state_dir: paths.state_dir,
    }
}
