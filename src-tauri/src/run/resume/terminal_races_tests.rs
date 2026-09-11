//! Deterministic terminal-handler interleavings; no timing sleeps or provider calls.
//! ACK uses a second real sidecar connection at the production checkpoints.
use super::*;
use crate::mailbox_delivery::prepare_headless_resume_delivery_on;
use crate::migration_providers::ResumeExecutionEnvironment;
use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, EnqueueResult, MailboxDb};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

thread_local! {
    static CHECKPOINT: std::cell::RefCell<Option<Box<dyn FnMut(&str)>>> = Default::default();
}
pub(in crate::run::resume) fn checkpoint(point: &str) {
    CHECKPOINT.with_borrow_mut(|hook| {
        if let Some(hook) = hook {
            hook(point);
        }
    });
}
struct HookGuard;
impl Drop for HookGuard {
    fn drop(&mut self) {
        CHECKPOINT.with_borrow_mut(|hook| *hook = None);
    }
}
struct DataDirGuard(Option<std::ffi::OsString>);
impl Drop for DataDirGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(value) => std::env::set_var("OULIPOLY_DATA_DIR", value),
                None => std::env::remove_var("OULIPOLY_DATA_DIR"),
            }
        }
    }
}
const SESSION: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN: &str = "730ba99f-8689-429e-8305-59d0b1e4f5a5";

fn terminal_ack_race(point: &'static str, partial: bool, independent: &str, exit_code: i32) {
    let _lock = crate::mailbox_delivery::DATA_DIR_ENV_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let _data = DataDirGuard(std::env::var_os("OULIPOLY_DATA_DIR"));
    unsafe {
        std::env::set_var("OULIPOLY_DATA_DIR", root.path());
    }
    let path = root.path().join("pid-identity.db");
    let mut mailbox = MailboxDb::open(&path).unwrap();
    for handle in if partial {
        vec!["first", "last"]
    } else {
        vec!["only"]
    } {
        assert!(matches!(
            mailbox
                .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                    session_id: SESSION,
                    handle,
                    payload_json: "{}",
                    owner_invocation_uuid: Some("owner"),
                    matched_os_pid: None,
                    matched_os_boot_id: None,
                    matched_os_pid_starttime_ticks: None,
                    matched_chain_index: None,
                    state_dir: "/fixture",
                    meta_path: "/fixture/meta",
                    log_path: "/fixture/log",
                    rc_path: "/fixture/rc",
                    rc: 0,
                })
                .unwrap(),
            EnqueueResult::Inserted(_)
        ));
    }
    let delivery =
        prepare_headless_resume_delivery_on(&mut mailbox, SESSION, CHAIN, None, None).unwrap();
    let nonce = delivery.delivery_nonce.as_deref().unwrap();
    if point == "before_failure_reconcile" {
        // Exercise both decisions together: the final partial ACK is older than
        // the global keep window, while this exact prepared finalizer is live.
        let mut newer = Vec::new();
        for n in 0..oulipoly_state::mailbox::TERMINAL_HISTORY_KEEP_ROWS {
            let EnqueueResult::Inserted(row) = mailbox
                .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                    session_id: "other-recipient",
                    handle: &format!("newer-{n}"),
                    payload_json: "{}",
                    owner_invocation_uuid: Some("owner"),
                    matched_os_pid: None,
                    matched_os_boot_id: None,
                    matched_os_pid_starttime_ticks: None,
                    matched_chain_index: None,
                    state_dir: "/fixture",
                    meta_path: "/fixture/meta",
                    log_path: "/fixture/log",
                    rc_path: "/fixture/rc",
                    rc: 0,
                })
                .unwrap()
            else {
                panic!()
            };
            newer.push(row.seq);
        }
        mailbox
            .mark_delivered("other-recipient", None, &newer, "other-consumer")
            .unwrap();
    }

    if partial {
        mailbox
            .acknowledge_range(SESSION, delivery.seqs[0], delivery.seqs[0], "consumer")
            .unwrap();
        assert!(
            !mailbox
                .delivery_attempt_fully_settled(nonce, SESSION, Some(CHAIN), &delivery.seqs)
                .unwrap()
        );
    }
    if independent == "observation" {
        mailbox
            .record_delivery_observation_anchor(
                nonce,
                SESSION,
                &oulipoly_state::mailbox::MailboxDeliveryObservationAnchor {
                    provider_name: "fixture".into(),
                    provider_instance_id: "instance".into(),
                    settings_id: "settings".into(),
                    provider_session_id: SESSION.into(),
                    resume_token: Some("tail".into()),
                    expected_sha256: wake::sha256_hex(
                        delivery.answer.as_deref().unwrap().trim().as_bytes(),
                    ),
                },
            )
            .unwrap();
        mailbox
            .record_delivery_observation_confirmation(nonce, "observed-user-turn")
            .unwrap();
    }
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
    let acceptance = if independent == "acceptance" {
        use oulipoly_runtime::executor::prompt_acceptance::*;
        let hash = wake::sha256_hex(delivery.answer.as_deref().unwrap().as_bytes());
        Some(
            promote_prompt_acceptance_attestation(
                ExpectedPromptAcceptance {
                    provider_session_id: SESSION,
                    prompt_sha256: &hash,
                    delivery_nonce: Some(nonce),
                },
                &oulipoly_provider::generated::PromptAcceptedMarkerValueV1 {
                    protocol: oulipoly_provider::generated::PROMPT_ACCEPTANCE_V1.into(),
                    provider_session_id: SESSION.into(),
                    prompt_sha256: hash.clone(),
                    delivery_nonce: Some(nonce.into()),
                    source: None,
                    message_id: None,
                },
            )
            .unwrap(),
        )
    } else {
        None
    };
    let mut result = super::tests::clean_result();
    result.exit_code = exit_code;
    let (go_tx, go_rx) = mpsc::sync_channel(0);
    let (done_tx, done_rx) = mpsc::sync_channel(0);
    let seq = *delivery.seqs.last().unwrap();
    let consumer = (point != "no_ack").then(|| {
        std::thread::spawn(move || {
            go_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let mut db = MailboxDb::open(&path).unwrap();
            assert_eq!(
                db.acknowledge_range(SESSION, seq, seq, "consumer").unwrap(),
                1
            );
            done_tx.send(()).unwrap();
        })
    });
    let mut fired = false;
    CHECKPOINT.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move |observed| {
            if observed == point && !fired {
                fired = true;
                go_tx.send(()).unwrap();
                done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }
        }))
    });
    let _hook = HookGuard;
    let outcome = handle_ordinary_resume_attempt_terminal_signal(
        &input,
        &mut bound.attempt,
        &provider,
        SESSION,
        &result,
        wake::ResumeCompletionEvidence {
            zero_turn_action: ZeroTurnAction::Continue,
            recovered_generic_nonzero: false,
            prompt_acceptance_confirmation: acceptance.as_ref(),
        },
        true,
        None,
    );
    if let Some(consumer) = consumer {
        consumer.join().unwrap(); // requires the selected checkpoint actually fired
    }
    let expected_exit = if point == "no_ack" { 1 } else { exit_code };
    assert!(
        matches!(outcome.unwrap(), ResumeAttemptLoopControl::Return(code) if code == expected_exit)
    );
    let invocation = env
        .state
        .get_invocation_by_uuid(&bound.attempt.invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(invocation.success, Some(expected_exit == 0));
    assert_eq!(invocation.exit_code, Some(expected_exit));
    assert_ne!(invocation.terminal_reason.as_deref(), Some("guard_drop"));
    assert_eq!(
        mailbox
            .delivery_attempt_fully_settled(nonce, SESSION, Some(CHAIN), &delivery.seqs)
            .unwrap(),
        point != "no_ack"
    );
    let state = rusqlite::Connection::open(root.path().join("state.db")).unwrap();
    let evidence = state
        .prepare("SELECT confirmed_evidence FROM session_delivery_acknowledgements")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    if independent == "none" {
        assert!(
            evidence.is_empty(),
            "manual ACK invented host evidence: {evidence:?}"
        );
        assert!(
            mailbox
                .delivery_observation_confirmation(nonce)
                .unwrap()
                .is_none()
        );
    } else {
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].starts_with(if independent == "acceptance" {
            oulipoly_provider::generated::PROMPT_ACCEPTANCE_V1
        } else {
            "observed_mailbox_delivery;turn_id=observed-user-turn;"
        }));
    }
    let rows = mailbox.list_mailbox(SESSION, true).unwrap();
    if point == "no_ack" {
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].delivered_by_invocation_uuid.as_deref(),
            Some("consumer")
        );
        assert!(rows[0].delivery_error.is_none());
        assert!(rows[1].delivered_at.is_none());
        assert_eq!(
            rows[1].delivery_error.as_deref(),
            Some("mailbox_delivery_unconfirmed")
        );
        // wake::finalize_unconfirmed_mailbox_delivery classifies the host
        // failure in error_category, preserving the provider terminal reason.
        assert_eq!(
            invocation.error_category.as_deref(),
            Some("mailbox_delivery_unconfirmed")
        );
        assert_eq!(invocation.terminal_reason, result.terminal_reason);
        return;
    }

    assert!(
        rows.iter()
            .all(|row| row.delivered_by_invocation_uuid.as_deref() == Some("consumer"))
    );
    assert!(rows.iter().all(|row| row.delivery_error.is_none()));
    assert!(mailbox.mailbox_observation_stop(SESSION).unwrap().is_none());
}

#[test]
fn ack_after_terminal_evidence_does_not_invent_confirmation() {
    terminal_ack_race("after_terminal_evidence", false, "none", 0);
}
#[test]
fn final_partial_ack_after_observation_rejection_does_not_invent_confirmation() {
    terminal_ack_race("before_failure_reconcile", true, "none", 0);
}
#[test]
fn ack_after_terminal_evidence_keeps_genuine_acceptance() {
    terminal_ack_race("after_terminal_evidence", true, "acceptance", 0);
}
#[test]
fn ack_after_terminal_evidence_keeps_genuine_observation() {
    terminal_ack_race("after_terminal_evidence", true, "observation", 0);
}
#[test]
fn ack_after_terminal_evidence_keeps_provider_failure() {
    terminal_ack_race("after_terminal_evidence", false, "none", 23);
}

#[test]
fn incomplete_partial_ack_still_requires_observation() {
    terminal_ack_race("no_ack", true, "none", 0);
}

// Paired offline tests supply real held scan admission and a native receipt.
// Keep this bridge beside the private production terminal handler rather than
// widening its production visibility just for cross-module fixtures.
pub(in crate::run::resume) fn correction4_unconfirmed_terminal(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
) -> String {
    let mut bound =
        super::super::lifecycle::setup_bound_resume_attempt(input, provider, 0).unwrap();
    let result = super::tests::clean_result();
    let outcome = handle_ordinary_resume_attempt_terminal_signal(
        input,
        &mut bound.attempt,
        provider,
        input.session_id,
        &result,
        wake::ResumeCompletionEvidence {
            zero_turn_action: ZeroTurnAction::Continue,
            recovered_generic_nonzero: false,
            prompt_acceptance_confirmation: None,
        },
        true,
        None,
    )
    .unwrap();
    assert!(matches!(outcome, ResumeAttemptLoopControl::Return(1)));
    let id = bound.attempt.invocation.id.clone();
    let invocation = input
        .env
        .state
        .get_invocation_by_uuid(&id)
        .unwrap()
        .unwrap();
    assert_eq!(invocation.success, Some(false));
    assert_eq!(invocation.exit_code, Some(1));
    assert_eq!(
        invocation.error_category.as_deref(),
        Some("mailbox_delivery_unconfirmed")
    );
    assert_eq!(invocation.terminal_reason, result.terminal_reason);
    id
}
