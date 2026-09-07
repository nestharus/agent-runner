//! Declared roles: orchestration, validator, accessor.
use super::*;
use crate::mailbox_delivery::{PreparedMailboxDelivery, prepare_headless_resume_delivery_on};
use crate::usage::cli::{Cli, Subcommands};
use clap::Parser;
use oulipoly_runtime::services::{ExecutorServiceOutput, ExecutorServicePort, ServiceError};
use oulipoly_state::InboxTargetKind;
use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, MailboxDb};
use std::sync::Mutex;

const SESSION: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN: &str = "730ba99f-8689-429e-8305-59d0b1e4f5a5";
const TEXT: &str = "  Résumé 🦀\n第二行\n\nkeep trailing whitespace  \n";

struct Fixture {
    root: tempfile::TempDir,
    mailbox: MailboxDb,
}
impl Fixture {
    fn new(paused: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut mailbox = MailboxDb::open(&root.path().join("mailbox.db")).unwrap();
        mailbox.set_notifications_paused(SESSION, paused).unwrap();
        Self { root, mailbox }
    }
    fn enqueue(
        &mut self,
        text: &str,
        token: &str,
        kind: InboxTargetKind,
        target: &str,
    ) -> Result<i64, String> {
        persist_tokenized_resume_input_on(&mut self.mailbox, text.into(), token, kind, target).map(
            |(inline, seq)| {
                assert!(inline.is_none());
                seq.unwrap()
            },
        )
    }
    fn prepare(
        &mut self,
        seq: Option<i64>,
        answer: Option<String>,
    ) -> Result<PreparedMailboxDelivery, String> {
        prepare_headless_resume_delivery_on(&mut self.mailbox, SESSION, CHAIN, answer, seq)
    }
    fn notification(&mut self) -> i64 {
        let result = self
            .mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle: "unrelated-notification",
                payload_json: "{}",
                owner_invocation_uuid: Some("owner"),
                matched_os_pid: Some(1),
                matched_os_boot_id: Some("boot"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: "/unused/state",
                meta_path: "/unused/meta",
                log_path: "/unused/log",
                rc_path: "/unused/rc",
                rc: 0,
            })
            .unwrap();
        let oulipoly_state::mailbox::EnqueueResult::Inserted(row) = result else {
            panic!()
        };
        row.seq
    }
}

fn parsed_answer(args: &[&str]) -> Result<(Option<String>, Option<String>), String> {
    let cli = Cli::try_parse_from(args).map_err(|e| e.to_string())?;
    let Some(Subcommands::Resume {
        prompt,
        file,
        submission_token,
        ..
    }) = cli.command
    else {
        panic!()
    };
    Ok((
        resolve_resume_answer(prompt.as_deref(), file.as_deref())?,
        submission_token,
    ))
}

// Records the real production executor dispatch, and deliberately fails:
// fixture tests must retain executor failures rather than convert them to success.
#[derive(Default)]
struct RecordingExecutor(Mutex<Vec<ExecutorServiceRequest>>);
impl ExecutorServicePort for RecordingExecutor {
    fn execute(
        &self,
        request: ExecutorServiceRequest,
    ) -> Result<ExecutorServiceOutput, ServiceError> {
        self.0.lock().unwrap().push(request);
        Err(ServiceError::Dependency {
            message: "recorded executor failure".into(),
        })
    }
}
fn record_request(root: &Path, delivery: &PreparedMailboxDelivery) -> ExecutorServiceRequest {
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/providers.toml"),
        r#"
[fixture]
settings_id = "fixture"
[fixture.implementation]
family = "fixture"
executable = "/nonexistent/age346-must-never-launch"
"#,
    )
    .unwrap();
    let mut services = wiring::AgentRuntimeServices::production(wiring::RuntimePaths {
        config_root: root.join("config"),
        models_dir: root.join("config/models"),
        agents_dir: root.join("config/agents"),
        data_root: root.join("data"),
        state_db_path: root.join("data/state.db"),
        lock_dir: root.join("locks"),
        working_dir: root.into(),
    })
    .unwrap();
    let env = ResumeExecutionEnvironment {
        state: oulipoly_state::StateDb::open(&root.join("fixture-state.db")).unwrap(),
        providers_cfg: oulipoly_config::ProvidersConfig {
            entries: HashMap::new(),
        },
        models: HashMap::new(),
        sessions_cfg: oulipoly_config::SessionsConfig {
            entries: HashMap::new(),
        },
        config_root: root.join("config"),
        models_dir: root.join("config/models"),
    };
    let provider = oulipoly_config::ProviderConfig::model_provider("fixture", vec![]);
    let model = oulipoly_config::ModelConfig {
        name: "fixture".into(),
        prompt_mode: oulipoly_config::PromptMode::Stdin,
        providers: vec![provider.clone()],
        inputs: vec![],
        provider: None,
    };
    let mut resolved = oulipoly_state::ResolvedResume {
        chain_id: CHAIN.into(),
        active_session_id: SESSION.into(),
        active_provider: "fixture".into(),
        model_name: Some("fixture".into()),
        model: Some(model.clone()),
    };
    let executor = Arc::new(RecordingExecutor::default());
    services.executor_service = executor.clone();
    let mut zero_turn = crate::zero_turn_orchestration::ZeroTurnConfirmationState::new();
    let mut accepted = false;
    let input = ResumeAttemptInput {
        agent_runtime_services: &services,
        env: &env,
        resolved: &mut resolved,
        answer: delivery.answer.as_deref(),
        mailbox_session_id: &delivery.session_id,
        mailbox_delivery_seqs: &delivery.seqs,
        mailbox_delivery_nonce: delivery.delivery_nonce.as_deref(),
        mailbox_delivery_requires_turn_confirmation: delivery.requires_turn_confirmation,
        manual_migrate: None,
        reservation: None,
        session_id: SESSION,
        working_dir: Some(root),
        attempts: 1,
        max_attempts: 1,
        parent_invocation_id: None,
        effective_spawn_cwd: root,
        zero_turn_confirmation: &mut zero_turn,
        provider_prompt_accepted: &mut accepted,
    };
    let error = execute_resume_attempt_command(
        &input,
        &provider,
        0,
        model.prompt_mode,
        "fixture-invocation",
        None,
    )
    .unwrap_err();
    assert!(error.contains("recorded executor failure"));
    let mut requests = executor.0.lock().unwrap();
    assert_eq!(requests.len(), 1);
    requests.pop().unwrap()
}

#[test]
fn age346_parser_file_paused_receipt_to_recording_executor() {
    for paused in [true, false] {
        for (kind, target) in [
            (InboxTargetKind::Session, SESSION),
            (InboxTargetKind::Chain, CHAIN),
        ] {
            let mut f = Fixture::new(paused);
            let unrelated = f.notification();
            let file = f.root.path().join("answer.txt");
            std::fs::write(&file, TEXT).unwrap();
            let (answer, token) = parsed_answer(&[
                "runner",
                "resume",
                "--session-id",
                SESSION,
                "--submission-token",
                "manual",
                "-f",
                file.to_str().unwrap(),
            ])
            .unwrap();
            assert_eq!(answer.as_deref(), Some(TEXT));
            let seq = f
                .enqueue(&answer.unwrap(), token.as_deref().unwrap(), kind, target)
                .unwrap();
            assert_eq!(f.enqueue(TEXT, "manual", kind, target).unwrap(), seq);
            let delivery = f.prepare(Some(seq), None).unwrap();
            assert_eq!(delivery.seqs, vec![seq]);
            assert!(!delivery.requires_turn_confirmation);
            let request = record_request(f.root.path(), &delivery);
            let ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
                prompt,
                start_known_provider_session_id,
                mailbox_delivery_correlation,
                ..
            } = request
            else {
                panic!()
            };
            assert_eq!(start_known_provider_session_id, SESSION);
            assert_eq!(
                mailbox_delivery_correlation.unwrap().delivery_nonce,
                delivery.delivery_nonce.clone().unwrap()
            );
            assert_eq!(prompt.matches(TEXT).count(), 1);
            assert!(!prompt.contains("unrelated-notification"));
            assert_eq!(f.mailbox.notifications_paused(SESSION).unwrap(), paused);
            assert!(
                f.mailbox
                    .list_pending(SESSION)
                    .unwrap()
                    .iter()
                    .any(|r| r.seq == unrelated)
            );
            // Preparation/executor failure does not ACK input.
            assert!(
                f.mailbox
                    .list_pending_for_delivery(SESSION, Some(CHAIN))
                    .unwrap()
                    .iter()
                    .any(|r| r.seq == seq)
            );
            f.mailbox
                .mark_delivered(SESSION, Some(CHAIN), &[seq], "confirmed-fixture")
                .unwrap();
            assert_eq!(f.enqueue(TEXT, "manual", kind, target).unwrap(), seq);
            assert!(
                f.prepare(Some(seq), None)
                    .err()
                    .unwrap()
                    .contains("no launch")
            );
        }
    }
}

#[test]
fn age346_conflict_target_unavailable_and_inflight_controls() {
    let mut f = Fixture::new(true);
    let seq = f
        .enqueue(TEXT, "token", InboxTargetKind::Session, SESSION)
        .unwrap();
    for (text, kind, target) in [
        ("different", InboxTargetKind::Session, SESSION),
        (TEXT, InboxTargetKind::Session, "wrong"),
        (TEXT, InboxTargetKind::Chain, SESSION),
    ] {
        assert!(
            f.enqueue(text, "token", kind, target)
                .unwrap_err()
                .contains("conflicts")
        );
    }
    let wrong = f
        .enqueue(TEXT, "wrong-target", InboxTargetKind::Session, "wrong")
        .unwrap();
    assert!(
        f.prepare(Some(wrong), None)
            .err()
            .unwrap()
            .contains("no launch")
    );
    assert!(
        f.prepare(Some(seq), Some(TEXT.into()))
            .err()
            .unwrap()
            .contains("inline copy")
    );
    let delivery = f.prepare(Some(seq), None).unwrap();
    f.mailbox
        .begin_delivery_attempt_submission(delivery.delivery_nonce.as_deref().unwrap())
        .unwrap();
    assert!(
        f.prepare(Some(seq), None)
            .err()
            .unwrap()
            .contains("unresolved delivery")
    );
    assert!(f.mailbox.notifications_paused(SESSION).unwrap());
}

#[test]
fn age346_inline_nontokenized_missing_and_empty_controls() {
    let mut f = Fixture::new(true);
    f.notification();
    let (answer, token) = parsed_answer(&[
        "runner",
        "resume",
        "--session-id",
        SESSION,
        "--prompt",
        TEXT,
    ])
    .unwrap();
    let (answer, seq) =
        persist_tokenized_resume_input(answer, token.as_deref(), InboxTargetKind::Session, SESSION)
            .unwrap();
    assert_eq!(
        f.prepare(seq, answer).unwrap().answer.as_deref(),
        Some(TEXT)
    );
    let file = f.root.path().join("plain.txt");
    std::fs::write(&file, TEXT).unwrap();
    let (answer, token) = parsed_answer(&[
        "runner",
        "resume",
        "--session-id",
        SESSION,
        "-f",
        file.to_str().unwrap(),
    ])
    .unwrap();
    assert!(token.is_none());
    assert_eq!(
        f.prepare(None, answer).unwrap().answer.as_deref(),
        Some(TEXT)
    );
    std::fs::remove_file(&file).unwrap();
    assert!(
        parsed_answer(&[
            "runner",
            "resume",
            "--session-id",
            SESSION,
            "--submission-token",
            "missing",
            "-f",
            file.to_str().unwrap()
        ])
        .is_err()
    );
    assert!(
        persist_tokenized_resume_input(None, Some("absent"), InboxTargetKind::Session, SESSION)
            .unwrap_err()
            .contains("no launch")
    );
    assert!(
        f.enqueue(" \n", "empty", InboxTargetKind::Session, SESSION)
            .unwrap_err()
            .contains("nonempty")
    );
    assert!(f.prepare(None, None).unwrap().answer.is_none());
    f.mailbox.set_notifications_paused(SESSION, false).unwrap();
    assert!(
        f.prepare(None, None)
            .unwrap()
            .answer
            .unwrap()
            .contains("unrelated-notification")
    );
}

#[test]
fn age346_ack_abandonment_and_payload_failure_do_not_replay() {
    let mut f = Fixture::new(true);
    let seq = f
        .enqueue(TEXT, "acked", InboxTargetKind::Session, SESSION)
        .unwrap();
    let delivery = f.prepare(Some(seq), None).unwrap();
    f.mailbox
        .record_delivery_attempt_transport_ack(delivery.delivery_nonce.as_deref().unwrap())
        .unwrap();
    assert!(
        f.prepare(Some(seq), None)
            .err()
            .unwrap()
            .contains("unresolved delivery")
    );
    let abandoned = f
        .enqueue(TEXT, "abandoned", InboxTargetKind::Session, SESSION)
        .unwrap();
    // Frozen terminal/cancellation fixture state, not a production DB mutation.
    rusqlite::Connection::open(f.root.path().join("mailbox.db"))
        .unwrap()
        .execute(
            "UPDATE mailbox SET delivery_error = 'wake_sweep_abandoned' WHERE seq = ?1",
            [abandoned],
        )
        .unwrap();
    assert!(
        f.prepare(Some(abandoned), None)
            .err()
            .unwrap()
            .contains("no launch")
    );
    let missing = f
        .enqueue(
            "distinct bytes",
            "missing-payload",
            InboxTargetKind::Session,
            SESSION,
        )
        .unwrap();
    let row = f
        .mailbox
        .list_pending(SESSION)
        .unwrap()
        .into_iter()
        .find(|r| r.seq == missing)
        .unwrap();
    std::fs::remove_file(row.payload_file_path.unwrap()).unwrap();
    assert!(
        f.prepare(Some(missing), None)
            .err()
            .unwrap()
            .contains("payload unavailable")
    );
    assert!(f.mailbox.notifications_paused(SESSION).unwrap());
}

#[test]
fn age346_atomic_admission_rechecks_settled_target_and_bound_attempt() {
    let mut f = Fixture::new(true);
    let seq = f
        .enqueue(TEXT, "bound", InboxTargetKind::Session, SESSION)
        .unwrap();
    let delivery = f.prepare(Some(seq), None).unwrap();
    let nonce = delivery.delivery_nonce.as_deref().unwrap();
    f.mailbox
        .bind_delivery_attempt_invocation(nonce, SESSION, "running-fixture")
        .unwrap();
    assert!(
        f.prepare(Some(seq), None)
            .err()
            .unwrap()
            .contains("unresolved delivery")
    );
    assert!(
        f.mailbox
            .delivery_attempt_window(nonce)
            .unwrap()
            .unwrap()
            .resolved_at
            .is_none()
    );
    f.mailbox
        .mark_delivered(SESSION, Some(CHAIN), &[seq], "running-fixture")
        .unwrap();
    assert!(
        f.mailbox
            .register_explicit_input_delivery_attempt("stale-selection", SESSION, Some(CHAIN), seq)
            .unwrap_err()
            .contains("no launch")
    );
    let wrong = f
        .enqueue(TEXT, "foreign-chain", InboxTargetKind::Chain, "other-chain")
        .unwrap();
    assert!(
        f.mailbox
            .register_explicit_input_delivery_attempt("wrong-target", SESSION, Some(CHAIN), wrong)
            .unwrap_err()
            .contains("no launch")
    );
    let notification = f.notification();
    assert!(
        f.mailbox
            .register_explicit_input_delivery_attempt(
                "not-input",
                SESSION,
                Some(CHAIN),
                notification
            )
            .unwrap_err()
            .contains("no launch")
    );
}

#[test]
fn age346_tokenized_inline_pending_retry_uses_only_durable_copy() {
    let mut f = Fixture::new(true);
    let (answer, token) = parsed_answer(&[
        "runner",
        "resume",
        "--session-id",
        SESSION,
        "--submission-token",
        "inline-token",
        "--prompt",
        TEXT,
    ])
    .unwrap();
    let seq = f
        .enqueue(
            &answer.unwrap(),
            token.as_deref().unwrap(),
            InboxTargetKind::Session,
            SESSION,
        )
        .unwrap();
    let first = f.prepare(Some(seq), None).unwrap();
    let replay = f
        .enqueue(TEXT, "inline-token", InboxTargetKind::Session, SESSION)
        .unwrap();
    let second = f.prepare(Some(replay), None).unwrap();
    assert_eq!(first.seqs, second.seqs);
    assert_ne!(first.delivery_nonce, second.delivery_nonce);
    assert_eq!(second.answer.unwrap().matches(TEXT).count(), 1);
    // Replacing an unbound preparation fences its old invocation out.
    assert!(
        f.mailbox
            .bind_delivery_attempt_invocation(
                first.delivery_nonce.as_deref().unwrap(),
                SESSION,
                "stale-invocation"
            )
            .is_err()
    );
    assert!(f.mailbox.notifications_paused(SESSION).unwrap());
}
