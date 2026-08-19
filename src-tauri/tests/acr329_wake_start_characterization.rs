#![cfg(unix)]
#![allow(dead_code)]

#[path = "../src/mailbox_delivery.rs"]
mod mailbox_delivery;
#[path = "../src/wake_coordinator/mod.rs"]
mod wake_coordinator;

use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, CreateRuntimeGeneration, EnqueueResult, MailboxDb,
    RuntimeGenerationId, SessionGenerationProjection, SessionMetadataUpsert,
    WakeClaimAcquireResult, WakeClaimRequest,
};
use std::fs;

const SESSION: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const INVOCATION: &str = "11111111-1111-4111-8111-111111111111";
const PROVIDER: &str = "acr329-poison-provider";
const MODEL: &str = "acr329-poison-model";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data_home = dir.path().join("xdg-data");
        let config_home = dir.path().join("xdg-config");
        let state_home = dir.path().join("xdg-state");
        let home = dir.path().join("home");
        let app_data = data_home.join("oulipoly-agent-runner");
        for path in [&data_home, &config_home, &state_home, &home] {
            fs::create_dir_all(path).unwrap();
        }
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("XDG_CONFIG_HOME", config_home);
            std::env::set_var("XDG_STATE_HOME", state_home);
            std::env::set_var("HOME", &home);
            std::env::set_var("OULIPOLY_DATA_DIR", app_data);
            std::env::remove_var("OULIPOLY_AUTO_WAKE");
            std::env::remove_var("OULIPOLY_AUTO_WAKE_SESSION_ID");
            std::env::remove_var("OULIPOLY_AUTO_WAKE_TOKEN");
            std::env::remove_var("OULIPOLY_AUTO_WAKE_COUNT");
            std::env::remove_var("OULIPOLY_AUTO_WAKE_MAX");
        }
        Self { dir }
    }

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open_default().unwrap()
    }

    fn seed_pending(&self, db: &mut MailboxDb, handle: &str) {
        let state_dir = self.dir.path().join(handle);
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("output.log");
        let rc = state_dir.join("rc");
        fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
        fs::write(&log, "provider poison canary\n").unwrap();
        fs::write(&rc, "0\n").unwrap();
        let result = db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle,
                payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
                owner_invocation_uuid: Some(INVOCATION),
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: state_dir.to_str().unwrap(),
                meta_path: meta.to_str().unwrap(),
                log_path: log.to_str().unwrap(),
                rc_path: rc.to_str().unwrap(),
                rc: 0,
            })
            .unwrap();
        assert!(matches!(result, EnqueueResult::Inserted(_)));
    }

    fn seed_runtime(&self, db: &mut MailboxDb, wake_max: i64, wake_count: i64) {
        db.wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: SESSION,
                mode: "headless",
                invocation_uuid: Some(INVOCATION),
                provider_name: Some(PROVIDER),
                model_name: Some(MODEL),
                models_dir: None,
                effective_cwd: None,
                selected_auto_wake_max: Some(wake_max),
            })
            .unwrap();
        rusqlite::Connection::open(MailboxDb::default_path().unwrap())
            .unwrap()
            .execute(
                "UPDATE session_runtime SET auto_wake_count = ?2 WHERE session_id = ?1",
                rusqlite::params![SESSION, wake_count],
            )
            .unwrap();
    }

    fn assert_pending_without_spawn_attempt(
        &self,
        db: &MailboxDb,
        diagnostic: &wake_coordinator::WakeDiagnostic,
    ) {
        assert!(!diagnostic.attempted);
        let pending = db.list_pending(SESSION).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].delivered_at.is_none());
        assert_eq!(pending[0].delivery_attempts, 0);
    }
}

#[test]
fn notify_wake_preserves_generation_cap_and_live_claim_authority() {
    let generation_fixture = Fixture::new();
    let mut generation_db = generation_fixture.mailbox();
    generation_fixture.seed_pending(&mut generation_db, "h-generation");
    let generation_id = RuntimeGenerationId::parse("22222222-2222-4222-8222-222222222222").unwrap();
    generation_db
        .runtime_lifecycle()
        .create_runtime_generation(CreateRuntimeGeneration {
            generation_id: &generation_id,
            spawn_invocation_uuid: INVOCATION,
            session_id: Some(SESSION),
            runtime_mode: "headless",
            provider_name: PROVIDER,
            model_name: Some(MODEL),
            pty_control_path: None,
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();

    let generation = wake_coordinator::trigger_notify_wake(SESSION);

    assert_eq!(generation.status, "busy");
    assert!(!generation.attempted);
    assert!(generation.claim_token.is_none());
    assert!(generation.wake_pid.is_none());
    assert!(
        generation_db
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        generation_db
            .runtime_lifecycle_reader()
            .session_generation_projection(SESSION)
            .unwrap(),
        SessionGenerationProjection::One(_)
    ));
    generation_fixture.assert_pending_without_spawn_attempt(&generation_db, &generation);

    let cap_fixture = Fixture::new();
    let mut cap_db = cap_fixture.mailbox();
    cap_fixture.seed_pending(&mut cap_db, "h-cap");
    cap_fixture.seed_runtime(&mut cap_db, 3, 3);

    let cap = wake_coordinator::trigger_notify_wake(SESSION);

    assert_eq!(cap.status, "auto_wake_cap_reached");
    assert!(!cap.attempted);
    assert_eq!(cap.auto_wake_count, Some(3));
    assert!(cap.claim_token.is_none());
    assert!(cap.wake_pid.is_none());
    assert!(
        cap_db
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .is_none()
    );
    let cap_runtime = cap_db
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(cap_runtime.selected_auto_wake_max, Some(3));
    assert_eq!(cap_runtime.auto_wake_count, 3);
    cap_fixture.assert_pending_without_spawn_attempt(&cap_db, &cap);

    let claim_fixture = Fixture::new();
    let mut claim_db = claim_fixture.mailbox();
    claim_fixture.seed_pending(&mut claim_db, "h-claim");
    claim_fixture.seed_runtime(&mut claim_db, 8, 0);
    let claim_token = "acr329-live-claim";
    let acquired = claim_db
        .wake_sessions()
        .try_acquire_wake_claim(WakeClaimRequest {
            session_id: SESSION,
            claim_token,
            reason: "notify_idle",
            auto_wake_count: 4,
            wake_invocation_uuid: None,
            stale_after_seconds: 600,
        })
        .unwrap();
    assert!(matches!(acquired, WakeClaimAcquireResult::Acquired(_)));
    claim_db
        .wake_sessions()
        .record_wake_claim_pid_identity(
            SESSION,
            claim_token,
            i64::from(std::process::id()),
            Some(PROVIDER),
            Some(MODEL),
        )
        .unwrap();
    let before = claim_db
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .unwrap();

    let in_flight = wake_coordinator::trigger_notify_wake(SESSION);

    assert_eq!(in_flight.status, "already_in_flight");
    assert!(!in_flight.attempted);
    assert_eq!(in_flight.claim_token.as_deref(), Some(claim_token));
    assert_eq!(in_flight.wake_pid, before.wake_pid);
    assert_eq!(in_flight.auto_wake_count, Some(4));
    let after = claim_db
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(after.claim_token, before.claim_token);
    assert_eq!(after.wake_pid, before.wake_pid);
    assert_eq!(after.auto_wake_count, before.auto_wake_count);
    let claim_runtime = claim_db
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(claim_runtime.selected_auto_wake_max, Some(8));
    assert_eq!(claim_runtime.auto_wake_count, 4);
    claim_fixture.assert_pending_without_spawn_attempt(&claim_db, &in_flight);
}
