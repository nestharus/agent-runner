//! ## Declared roles
//!
//! Roles: accessor, filter, formatter, mapper, orchestration, validator.
//!
//! TEST: state-db, sidecar, mailbox, runtime, and process identity accessors
//! for proactive wake integration fixtures.

use crate::fixtures::Fixture;
use crate::model_config::path_string;
use crate::parse::ts;
use crate::{INVOCATION, MODEL, PROVIDER, SESSION};
use chrono::Utc;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, CompletionEventRegistrationInput, MailboxDb, MailboxRow,
    SessionRuntimeRunningUpdate, SessionRuntimeUpsert,
};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, SessionTurnIngest, StateDb};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

struct SeedMailboxArtifacts {
    state_dir: PathBuf,
    meta: PathBuf,
    log: PathBuf,
    rc: PathBuf,
    state_dir_s: String,
    meta_s: String,
    log_s: String,
    rc_s: String,
}

impl Fixture {
    pub(crate) fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    pub(crate) fn sidecar_conn(&self) -> Connection {
        Connection::open(self.sidecar_path()).unwrap()
    }

    pub(crate) fn state(&self) -> StateDb {
        StateDb::open(&self.state_path()).unwrap()
    }

    pub(crate) fn assert_delivery_invocation_is_child_of_owner(&self, session_id: &str) {
        let rows = self.mailbox().list_mailbox(session_id, true).unwrap();
        let row = delivered_non_owner_row(&rows);
        let owner_uuid = row
            .owner_invocation_uuid
            .as_deref()
            .expect("mailbox owner invocation");
        let delivery_uuid = row
            .delivered_by_invocation_uuid
            .as_deref()
            .expect("mailbox delivery invocation");
        let parent_uuid = self.delivery_parent_uuid(delivery_uuid);
        assert_delivery_parent(&parent_uuid, owner_uuid);
    }

    fn delivery_parent_uuid(&self, delivery_uuid: &str) -> Option<String> {
        self.state()
            .connection()
            .query_row(
                "SELECT parent.invocation_uuid
                 FROM invocations AS delivery
                 LEFT JOIN invocations AS parent ON parent.id = delivery.parent_invocation_id
                 WHERE delivery.invocation_uuid = ?1",
                rusqlite::params![delivery_uuid],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub(crate) fn seed_outer_caller(
        &self,
        session_id: &str,
        invocation_uuid: &str,
        event_id: &str,
    ) {
        let state = self.state();
        let invocation_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: invocation_uuid.to_string(),
                model_name: MODEL.to_string(),
                provider_name: PROVIDER.to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .bind_invocation_provider_session_start(
                invocation_id,
                &ProviderSessionBinding {
                    provider_session_id: session_id.to_string(),
                    capture_method: "fixture",
                    resume_input_id: None,
                    provider_session_resolved_account: None,
                },
            )
            .unwrap();

        let identity = crate::wake_claim_setup::current_process_identity();
        PidIdentityDb::open(&self.sidecar_path())
            .unwrap()
            .record_identity(PidIdentityRecord {
                identity: &identity,
                os_pgid: None,
                invocation_uuid,
                session_id: Some(session_id),
                provider_name: Some(PROVIDER),
                model_name: Some(MODEL),
                recorded_at: "2026-06-04T12:00:00Z",
            })
            .unwrap();

        let mut mailbox = self.mailbox();
        let models_dir = path_string(&self.models_dir);
        mailbox
            .upsert_session_runtime(SessionRuntimeUpsert {
                session_id,
                mode: "pty_interactive",
                invocation_uuid: Some(invocation_uuid),
                provider_name: Some(PROVIDER),
                model_name: Some(MODEL),
                pty_control_path: None,
                models_dir: Some(&models_dir),
                effective_cwd: None,
                selected_auto_wake_max: None,
            })
            .unwrap();
        mailbox
            .mark_session_running(SessionRuntimeRunningUpdate {
                session_id,
                mode: "pty_interactive",
                invocation_uuid,
                provider_name: Some(PROVIDER),
                model_name: Some(MODEL),
                identity: &identity,
                pty_control_path: None,
                turn_start_max_mailbox_seq: None,
                models_dir: Some(&models_dir),
                effective_cwd: None,
            })
            .unwrap();

        let artifacts = self.seed_mailbox_artifacts(event_id);
        write_seed_mailbox_artifacts(&artifacts, event_id);
        MailboxDb::register_completion_event(
            mailbox.path(),
            CompletionEventRegistrationInput {
                event_id,
                delivery_mode: "async",
                owner_session_id: Some(session_id),
                owner_invocation_uuid: Some(invocation_uuid),
                state_dir: &artifacts.state_dir_s,
                meta_path: &artifacts.meta_s,
                log_path: &artifacts.log_s,
                rc_path: &artifacts.rc_s,
            },
        )
        .unwrap();
    }

    pub(crate) fn seed_session_turn(&self) {
        self.seed_session_turn_for(PROVIDER, SESSION, "turn-a");
    }

    pub(crate) fn seed_session_turn_for(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
    ) {
        let db = self.state();
        db.ingest_session_turns_batch(
            provider_name,
            &[SessionTurnIngest {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                timestamp: ts("2026-06-04T12:00:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .unwrap();
    }

    pub(crate) fn seed_consumed_notification_turn(&self, handle: &str) {
        self.state()
            .ingest_session_turns_batch(
                PROVIDER,
                &[SessionTurnIngest {
                    session_id: SESSION.to_string(),
                    turn_id: format!("turn-consumed-{handle}"),
                    timestamp: ts("2026-06-04T12:01:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some(format!("[OULIPOLY NOTIFICATIONS]\nhandle: {handle}\n")),
                }],
            )
            .unwrap();
    }

    pub(crate) fn seed_idle_runtime(&self) {
        self.seed_idle_runtime_for(SESSION, PROVIDER, MODEL);
    }

    pub(crate) fn seed_idle_runtime_for(
        &self,
        session_id: &str,
        provider_name: &str,
        model_name: &str,
    ) {
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        let models_dir = path_string(&self.models_dir);
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id,
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some(provider_name),
            model_name: Some(model_name),
            pty_control_path: None,
            models_dir: Some(&models_dir),
            effective_cwd: None,
            selected_auto_wake_max: None,
        })
        .unwrap();
    }

    pub(crate) fn seed_idle_runtime_without_models_dir(&self, session_id: &str) {
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id,
            mode: "headless",
            invocation_uuid: None,
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
            selected_auto_wake_max: None,
        })
        .unwrap();
    }

    pub(crate) fn seed_idle_runtime_with_wake_policy(
        &self,
        session_id: &str,
        selected_auto_wake_max: i64,
        auto_wake_count: i64,
    ) {
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        let models_dir = path_string(&self.models_dir);
        db.upsert_session_runtime(SessionRuntimeUpsert {
            session_id,
            mode: "headless",
            invocation_uuid: Some(INVOCATION),
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL),
            pty_control_path: None,
            models_dir: Some(&models_dir),
            effective_cwd: None,
            selected_auto_wake_max: Some(selected_auto_wake_max),
        })
        .unwrap();
        self.sidecar_conn()
            .execute(
                "UPDATE session_runtime SET auto_wake_count = ?2 WHERE session_id = ?1",
                rusqlite::params![session_id, auto_wake_count],
            )
            .unwrap();
    }

    pub(crate) fn seed_active_chain_for(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        model_name: &str,
    ) {
        let _ = StateDb::open(&self.state_path()).unwrap();
        let conn = Connection::open(self.state_path()).unwrap();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-06-04T12:00:00Z', '2026-06-04T12:00:00Z', ?2)",
            rusqlite::params![chain_id, model_name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-06-04T12:00:00Z', 'initial')",
            rusqlite::params![chain_id, provider_name, session_id],
        )
        .unwrap();
    }

    pub(crate) fn seed_mailbox(&self, session_id: &str, handle: &str) {
        self.seed_mailbox_for(session_id, handle, Some(INVOCATION));
    }

    pub(crate) fn seed_mailbox_for(
        &self,
        session_id: &str,
        handle: &str,
        owner_invocation_uuid: Option<&str>,
    ) {
        let artifacts = self.seed_mailbox_artifacts(handle);
        write_seed_mailbox_artifacts(&artifacts, handle);
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        db.enqueue_agent_bash_complete(&seed_mailbox_enqueue(
            session_id,
            handle,
            owner_invocation_uuid,
            &artifacts,
        ))
        .unwrap();
    }

    pub(crate) fn age_mailbox_for(&self, session_id: &str, seconds_old: i64) {
        let enqueued_at = (Utc::now() - chrono::Duration::seconds(seconds_old)).to_rfc3339();
        self.sidecar_conn()
            .execute(
                "UPDATE mailbox SET enqueued_at = ?2 WHERE session_id = ?1",
                rusqlite::params![session_id, enqueued_at],
            )
            .unwrap();
    }

    pub(crate) fn mark_mailbox_unconfirmed_twice(&self, session_id: &str, handle: &str) {
        let mut db = self.mailbox();
        let seq = mailbox_seq_for_handle(&db, session_id, handle);
        mark_delivery_failed_twice(&mut db, session_id, seq);
    }

    pub(crate) fn seed_dead_owner_backlog(&self) -> Vec<String> {
        let dead_sessions = dead_owner_session_ids();
        for (index, session_id) in dead_sessions.iter().enumerate() {
            self.seed_dead_owner_session(index, session_id);
        }
        dead_sessions
    }

    fn seed_mailbox_artifacts(&self, handle: &str) -> SeedMailboxArtifacts {
        let state_dir = self.work_dir.join(seed_mailbox_dir_name(handle));
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        SeedMailboxArtifacts {
            state_dir_s: path_string(&state_dir),
            meta_s: path_string(&meta),
            log_s: path_string(&log),
            rc_s: path_string(&rc),
            state_dir,
            meta,
            log,
            rc,
        }
    }

    fn seed_dead_owner_session(&self, index: usize, session_id: &str) {
        let handle = dead_owner_handle(index);
        self.seed_mailbox_for(session_id, &handle, None);
        self.age_mailbox_for(session_id, 86_400);
        crate::wake_claim_setup::seed_dead_wake_claim_for(
            self,
            session_id,
            &dead_owner_claim_token(index),
            601,
        );
    }

    pub(crate) fn seed_resumable_backlog_session(
        &self,
        chain_id: &str,
        session_id: &str,
        turn_id: &str,
        handle: &str,
        claim_token: &str,
        mailbox_age_seconds: Option<i64>,
    ) {
        self.seed_active_chain_for(chain_id, PROVIDER, session_id, MODEL);
        self.seed_session_turn_for(PROVIDER, session_id, turn_id);
        self.seed_idle_runtime_for(session_id, PROVIDER, MODEL);
        self.seed_mailbox(session_id, handle);
        if let Some(seconds_old) = mailbox_age_seconds {
            self.age_mailbox_for(session_id, seconds_old);
        }
        crate::wake_claim_setup::seed_dead_wake_claim_for(self, session_id, claim_token, 601);
    }
}

fn delivered_non_owner_row(rows: &[MailboxRow]) -> &MailboxRow {
    rows.iter()
        .find(|row| {
            row.delivered_by_invocation_uuid.is_some()
                && row.delivered_by_invocation_uuid != row.owner_invocation_uuid
        })
        .unwrap_or_else(|| {
            panic!("no mailbox row was delivered by a non-owner invocation: {rows:?}")
        })
}

fn assert_delivery_parent(parent_uuid: &Option<String>, owner_uuid: &str) {
    assert_eq!(parent_uuid.as_deref(), Some(owner_uuid));
}

fn seed_mailbox_dir_name(handle: &str) -> String {
    format!("seed-{handle}")
}

fn write_seed_mailbox_artifacts(artifacts: &SeedMailboxArtifacts, handle: &str) {
    fs::create_dir_all(&artifacts.state_dir).unwrap();
    fs::write(&artifacts.meta, r#"{"caller_chain":[]}"#).unwrap();
    fs::write(&artifacts.log, seed_mailbox_log(handle)).unwrap();
    fs::write(&artifacts.rc, "0\n").unwrap();
}

fn seed_mailbox_log(handle: &str) -> String {
    format!("log {handle}\n")
}

fn seed_mailbox_enqueue<'a>(
    session_id: &'a str,
    handle: &'a str,
    owner_invocation_uuid: Option<&'a str>,
    artifacts: &'a SeedMailboxArtifacts,
) -> AgentBashCompleteEnqueue<'a> {
    AgentBashCompleteEnqueue {
        session_id,
        handle,
        payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
        owner_invocation_uuid,
        matched_os_pid: Some(9_300),
        matched_os_boot_id: Some("boot-seeded"),
        matched_os_pid_starttime_ticks: Some(456),
        matched_chain_index: Some(0),
        state_dir: &artifacts.state_dir_s,
        meta_path: &artifacts.meta_s,
        log_path: &artifacts.log_s,
        rc_path: &artifacts.rc_s,
        rc: 0,
    }
}

fn mailbox_seq_for_handle(db: &MailboxDb, session_id: &str, handle: &str) -> i64 {
    db.list_mailbox(session_id, true)
        .unwrap()
        .into_iter()
        .find(|row| row.handle == handle)
        .unwrap_or_else(|| panic!("missing seeded mailbox row {handle}"))
        .seq
}

fn mark_delivery_failed_twice(db: &mut MailboxDb, session_id: &str, seq: i64) {
    mark_delivery_failed(db, session_id, seq);
    mark_delivery_failed(db, session_id, seq);
}

fn mark_delivery_failed(db: &mut MailboxDb, session_id: &str, seq: i64) {
    db.mark_delivery_failed(session_id, &[seq], "mailbox_delivery_unconfirmed")
        .unwrap();
}

fn dead_owner_session_ids() -> Vec<String> {
    (0..16).map(dead_owner_session_id).collect()
}

fn dead_owner_session_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012x}")
}

fn dead_owner_handle(index: usize) -> String {
    format!("h-dead-owner-{index}")
}

fn dead_owner_claim_token(index: usize) -> String {
    format!("dead0000-0000-4000-8000-{index:012x}")
}
