//! S5-T1 independent intent: cycle6 success fence, atomic settlement and nonwaiting refusal.
//! Fixture input uses the real prompt-attestation promotion and exact mailbox selector;
//! it is not native-receipt or installed-provider evidence. No provider is executed.
use super::*;
use crate::mailbox_delivery::prepare_headless_resume_delivery_on;
use crate::migration_providers::ResumeExecutionEnvironment;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, CompletionEventRegistrationInput, MailboxDb,
};
use oulipoly_state::{InvocationMutationAuthority, InvocationStatus};
use std::collections::HashMap;
const SESSION: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN: &str = "730ba99f-8689-429e-8305-59d0b1e4f5a5";
struct DataDirGuard(Option<std::ffi::OsString>);
impl Drop for DataDirGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("OULIPOLY_DATA_DIR", v),
                None => std::env::remove_var("OULIPOLY_DATA_DIR"),
            }
        }
    }
}
fn registration(id: &str) -> CompletionEventRegistrationInput<'_> {
    CompletionEventRegistrationInput {
        event_id: "independent-outgoing",
        delivery_mode: "async",
        owner_session_id: Some(SESSION),
        owner_invocation_uuid: Some(id),
        state_dir: "/fixture/outgoing",
        meta_path: "/fixture/outgoing/meta",
        log_path: "/fixture/outgoing/log",
        rc_path: "/fixture/outgoing/rc",
    }
}
fn confirmed_resume_projection_gap(reject_projection: bool) {
    let _lock = crate::mailbox_delivery::DATA_DIR_ENV_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let _data = DataDirGuard(std::env::var_os("OULIPOLY_DATA_DIR"));
    unsafe {
        std::env::set_var("OULIPOLY_DATA_DIR", root.path());
    }
    let mut mailbox = MailboxDb::open(&root.path().join("pid-identity.db")).unwrap();
    mailbox
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: SESSION,
            handle: "older-incoming",
            payload_json: "{}",
            owner_invocation_uuid: Some("older-owner"),
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: None,
            state_dir: "/fixture/incoming",
            meta_path: "/fixture/incoming/meta",
            log_path: "/fixture/incoming/log",
            rc_path: "/fixture/incoming/rc",
            rc: 0,
        })
        .unwrap();
    let delivery =
        prepare_headless_resume_delivery_on(&mut mailbox, SESSION, CHAIN, None, None).unwrap();
    let nonce = delivery.delivery_nonce.as_deref().unwrap();
    std::fs::create_dir_all(root.path().join("config")).unwrap();
    std::fs::write(root.path().join("config/providers.toml"), "").unwrap();
    let services = crate::wiring::AgentRuntimeServices::production(crate::wiring::RuntimePaths {
        config_root: root.path().join("config"),
        models_dir: root.path().join("config/models"),
        agents_dir: root.path().join("config/agents"),
        data_root: root.path().into(),
        state_db_path: root.path().join("state.db"),
        lock_dir: root.path().join("locks"),
        working_dir: root.path().into(),
    })
    .unwrap();
    let env = ResumeExecutionEnvironment {
        state: oulipoly_state::StateDb::open(&root.path().join("state.db")).unwrap(),
        providers_cfg: oulipoly_config::ProvidersConfig {
            entries: HashMap::new(),
        },
        models: HashMap::new(),
        sessions_cfg: oulipoly_config::SessionsConfig {
            entries: HashMap::new(),
        },
        config_root: root.path().join("config"),
        models_dir: root.path().join("config/models"),
    };
    let mut resolved = oulipoly_state::ResolvedResume {
        chain_id: CHAIN.into(),
        active_session_id: SESSION.into(),
        active_provider: "fixture".into(),
        model_name: Some("fixture".into()),
        model: None,
    };
    let provider = oulipoly_config::ProviderConfig::model_provider("fixture", vec![]);
    let mut zero_turn = crate::zero_turn_orchestration::ZeroTurnConfirmationState::new();
    let mut accepted = false;
    let input = ResumeAttemptInput {
        agent_runtime_services: &services,
        env: &env,
        resolved: &mut resolved,
        answer: delivery.answer.as_deref(),
        mailbox_session_id: SESSION,
        mailbox_delivery_seqs: &delivery.seqs,
        mailbox_delivery_nonce: Some(nonce),
        mailbox_delivery_requires_turn_confirmation: true,
        manual_migrate: None,
        reservation: None,
        session_id: SESSION,
        working_dir: Some(root.path()),
        attempts: 1,
        max_attempts: 1,
        parent_invocation_id: None,
        effective_spawn_cwd: root.path(),
        zero_turn_confirmation: &mut zero_turn,
        provider_prompt_accepted: &mut accepted,
    };
    let mut bound =
        super::super::lifecycle::setup_bound_resume_attempt(&input, &provider, 0).unwrap();
    mailbox
        .bind_delivery_attempt_invocation(nonce, SESSION, &bound.attempt.invocation.id)
        .unwrap();
    mailbox
        .begin_headless_delivery_submission(nonce, SESSION, &bound.attempt.invocation.id, false)
        .unwrap();

    use oulipoly_runtime::executor::prompt_acceptance::*;
    let hash = wake::sha256_hex(delivery.answer.as_deref().unwrap().as_bytes());
    let attestation = oulipoly_provider::generated::PromptAcceptedMarkerValueV1 {
        protocol: oulipoly_provider::generated::PROMPT_ACCEPTANCE_V1.into(),
        provider_session_id: SESSION.into(),
        prompt_sha256: hash.clone(),
        delivery_nonce: Some(nonce.into()),
        source: None,
        message_id: None,
    };
    let acceptance = promote_prompt_acceptance_attestation(
        ExpectedPromptAcceptance {
            provider_session_id: SESSION,
            prompt_sha256: &hash,
            delivery_nonce: Some(nonce),
        },
        &attestation,
    )
    .unwrap();
    let evidence = independent_delivery_evidence(
        &mailbox,
        nonce,
        SESSION,
        Some(CHAIN),
        &delivery.seqs,
        delivery.answer.as_deref(),
        Some(&acceptance),
    )
    .unwrap()
    .unwrap();
    let id = bound.attempt.invocation.id.clone();
    let row = bound.attempt.invocation_row_id;
    let settlement = delivery_settlement(&input, &id, Some(&evidence)).unwrap();
    assert_ne!(nonce, "independent-outgoing");
    let fault = rusqlite::Connection::open(root.path().join("pid-identity.db")).unwrap();
    if reject_projection {
        fault.execute_batch("CREATE TRIGGER private_projection_failure BEFORE INSERT ON completion_event WHEN NEW.event_id='independent-outgoing' BEGIN SELECT RAISE(ABORT,'private projection failure'); END;").unwrap();
    }
    let mut writer = oulipoly_state::StateDb::open(env.state.path()).unwrap();
    let admission = writer.register_completion_event_with_authority(
        InvocationMutationAuthority::Standalone,
        &bound.attempt.completion_registration_authority,
        "outgoing-admission",
        registration(&id),
    );
    if reject_projection {
        assert!(
            admission
                .unwrap_err()
                .contains("private projection failure")
        );
        assert!(
            mailbox
                .completion_event("independent-outgoing")
                .unwrap()
                .is_none()
        );
    } else {
        admission.unwrap();
    }
    let sql = rusqlite::Connection::open(env.state.path()).unwrap();
    assert_eq!(
        sql.query_row(
            "SELECT count(*) FROM invocation_completion_obligations WHERE invocation_uuid=?1",
            [&id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let mut result = super::tests::clean_result();
    result.prompt_acceptance_attestation = Some(attestation);
    let completion = ResumeCompletionClassification {
        recovered_generic_nonzero: false,
        terminal_completion_confirmed: true,
    };
    let first = super::super::lifecycle::finalize_completed_attempt_control_for_resume(
        &input,
        &mut bound.attempt,
        &provider,
        SESSION,
        &result,
        settlement,
        &completion,
    );
    if reject_projection {
        assert!(
            first.is_err(),
            "confirmed incoming input authorized success with State-committed outgoing projection debt"
        );
        let error = first.err().unwrap();
        assert!(error.contains("process_integrity:"), "{error}");
        let old = std::mem::replace(
            &mut bound.attempt.guard,
            crate::invocation::finalize::FinalizerGuard::new(&env.state, row),
        );
        drop(old);
        assert_eq!(
            env.state.get_invocation_by_id(row).unwrap().unwrap().status,
            InvocationStatus::Running
        );
        assert_eq!(
            sql.query_row(
                "SELECT count(*) FROM session_delivery_acknowledgements",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            sql.query_row(
                "SELECT count(*) FROM invocation_returned_artifacts",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        // Revalidate the SAME genuine input, not replacement evidence or another provider turn.
        let retained = independent_delivery_evidence(
            &mailbox,
            nonce,
            SESSION,
            Some(CHAIN),
            &delivery.seqs,
            delivery.answer.as_deref(),
            Some(&acceptance),
        )
        .unwrap()
        .unwrap();
        assert_eq!(retained.confirmed, evidence.confirmed);
        fault
            .execute_batch("DROP TRIGGER private_projection_failure")
            .unwrap();
        writer
            .repair_admitted_completion_event(
                InvocationMutationAuthority::Standalone,
                "outgoing-admission",
                registration(&id),
            )
            .unwrap();
        super::super::lifecycle::finalize_completed_attempt_control_for_resume(
            &input,
            &mut bound.attempt,
            &provider,
            SESSION,
            &result,
            settlement,
            &completion,
        )
        .unwrap();
    } else {
        first.unwrap();
    }
    drop(bound);
    assert_eq!(
        env.state.get_invocation_by_id(row).unwrap().unwrap().status,
        InvocationStatus::Succeeded
    );
    let ack:(String,String,String)=sql.query_row("SELECT session_id,turn_generation_id,confirmed_evidence FROM session_delivery_acknowledgements WHERE delivery_id=?1",[nonce],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(ack, (SESSION.into(), id, evidence.confirmed));
}
#[test]
fn confirmed_resume_refuses_projection_gap_then_settles_same_retained_outcome() {
    confirmed_resume_projection_gap(true);
}
#[test]
fn confirmed_resume_coherent_outgoing_projection_succeeds() {
    confirmed_resume_projection_gap(false);
}
