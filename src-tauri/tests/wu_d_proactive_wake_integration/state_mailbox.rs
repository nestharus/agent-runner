//! ## Declared roles
//!
//! Roles: accessor.
//!
//! TEST: state-db, sidecar, mailbox, runtime, and process identity accessors
//! for proactive wake integration fixtures.

use crate::fixtures::Fixture;
use crate::model_config::path_string;
use crate::parse::ts;
use crate::{INVOCATION, MODEL, PROVIDER, SESSION};
use chrono::Utc;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, BindRuntimeGenerationRunning, CreateRuntimeGeneration, MailboxDb,
    RuntimeGenerationFence, RuntimeGenerationId, SessionRuntimeRunningUpdate, SessionRuntimeUpsert,
};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord, ProcessIdentity};
use oulipoly_state::{SessionTurnIngest, StateDb};
use rusqlite::Connection;
use std::fs;
use std::path::Path;
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
    pub(crate) fn pid_identity_session_id_for_provider(&self, provider_name: &str) -> String {
        self.sidecar_conn()
            .query_row(
                "SELECT session_id
                 FROM pid_identity
                 WHERE provider_name = ?1
                 ORDER BY recorded_at DESC, os_pid DESC
                 LIMIT 1",
                rusqlite::params![provider_name],
                |row| row.get(0),
            )
            .unwrap()
    }
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
        let row = rows
            .iter()
            .find(|row| row.delivered_by_invocation_uuid.is_some())
            .expect("delivered mailbox row");
        let owner_uuid = row
            .owner_invocation_uuid
            .as_deref()
            .expect("mailbox owner invocation");
        let delivery_uuid = row
            .delivered_by_invocation_uuid
            .as_deref()
            .expect("mailbox delivery invocation");
        let conn = Connection::open(self.state_path()).unwrap();
        let parent_uuid: Option<String> = conn
            .query_row(
                "SELECT parent.invocation_uuid
                 FROM invocations AS delivery
                 LEFT JOIN invocations AS parent ON parent.id = delivery.parent_invocation_id
                 WHERE delivery.invocation_uuid = ?1",
                rusqlite::params![delivery_uuid],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            parent_uuid.as_deref(),
            Some(owner_uuid),
            "auto-wake delivery invocation must remain a logical child of the invocation whose work caused the wake"
        );
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
        })
        .unwrap();
    }

    pub(crate) fn seed_live_pty_runtime(&self, control_path: &Path) -> ProcessIdentity {
        let identity = crate::wake_claim_setup::current_process_identity();
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        let models_dir = path_string(&self.models_dir);
        let control_path = path_string(control_path);
        let generation_id = RuntimeGenerationId::parse(INVOCATION).unwrap();
        db.create_runtime_generation(CreateRuntimeGeneration {
            generation_id: &generation_id,
            spawn_invocation_uuid: INVOCATION,
            session_id: Some(SESSION),
            runtime_mode: "pty_interactive",
            provider_name: PROVIDER,
            model_name: Some(MODEL),
            pty_control_path: Some(&control_path),
            models_dir: Some(&models_dir),
            effective_cwd: None,
        })
        .unwrap();
        db.bind_runtime_generation_running(BindRuntimeGenerationRunning {
            fence: RuntimeGenerationFence {
                generation_id: &generation_id,
                spawn_invocation_uuid: INVOCATION,
            },
            spawned_os_pid: identity.os_pid,
            exact_process_identity: Some(&identity),
            os_pgid: None,
        })
        .unwrap();
        db.mark_session_running(SessionRuntimeRunningUpdate {
            session_id: SESSION,
            mode: "pty_interactive",
            invocation_uuid: INVOCATION,
            provider_name: Some(PROVIDER),
            model_name: Some(MODEL),
            identity: &identity,
            pty_control_path: Some(&control_path),
            turn_start_max_mailbox_seq: None,
            models_dir: Some(&models_dir),
            effective_cwd: None,
        })
        .unwrap();
        identity
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

    pub(crate) fn record_identity(&self, identity: &ProcessIdentity) {
        self.record_identity_for(identity, SESSION, PROVIDER, MODEL);
    }

    pub(crate) fn record_identity_for(
        &self,
        identity: &ProcessIdentity,
        session_id: &str,
        provider_name: &str,
        model_name: &str,
    ) {
        let sidecar = PidIdentityDb::open(&self.sidecar_path()).unwrap();
        sidecar
            .record_identity(PidIdentityRecord {
                identity,
                os_pgid: None,
                invocation_uuid: INVOCATION,
                session_id: Some(session_id),
                provider_name: Some(provider_name),
                model_name: Some(model_name),
                recorded_at: "2026-06-04T12:00:00Z",
            })
            .unwrap();
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
