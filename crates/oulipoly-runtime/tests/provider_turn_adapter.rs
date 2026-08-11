#![cfg(unix)]

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use chrono::{TimeZone, Utc};
use oulipoly_agent_messenger::{ReturnedArtifactRef, ReturnedArtifactSource, StoreAddress};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeAcceptanceRules, ResumeKind, ResumeStrategy,
};
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{
    CapturedChildInvocation, ExecutionResult, ResumeAcceptanceResult, ResumeAcceptanceStatus,
    SessionCaptureMethod, SessionCaptureResult, SubmittedUserTurn, TerminalSignal,
};
use oulipoly_runtime::provider_turn_adapter::{
    EffectWrite, EvidenceStrength, FencedProviderEvidence, InvocationOwnership,
    LegacyCliResumeRequest, MailboxBatchIdentity, ProductionProviderTurnExecutor, ProviderEvidence,
    ProviderExecutionError, ProviderExecutionOutcome, ProviderExecutionStatus, ProviderTurnAdapter,
    ProviderTurnAdapterError, ProviderTurnCallerResult, ProviderTurnEffects,
    ProviderTurnExecutionRequest, ProviderTurnExecutor, ProviderTurnLaunch,
    classify_provider_evidence, prompt_sha256,
};
use oulipoly_runtime::services::{
    ExecutorServiceOutput, ExecutorServicePort, ExecutorServiceRequest, ServiceError,
};
use oulipoly_runtime::session_supervisor::{
    ProcessObservation, ProcessObserver, SessionNotification, SessionSupervisor, SupervisorConfig,
    TurnOutcome, TurnRequest, TurnResult,
};
use oulipoly_state::{
    AcknowledgementStage, CompositeInvocationId, ExactProcessIdentity, ExternalIngress,
    InvocationStart, InvocationStatus, ProviderTurnGeneration, SessionLifecycleRepository, StateDb,
    SupervisorFence, TurnFence, TurnState,
};
use uuid::Uuid;

const SESSION: &str = "provider-session-a";
const PARENT_UUID: &str = "11111111-1111-1111-1111-111111111111";
const FIRST_UUID: &str = "22222222-2222-2222-2222-222222222222";
const SECOND_UUID: &str = "33333333-3333-3333-3333-333333333333";

type AdapterTurn = TurnRequest<ProviderTurnLaunch, ProviderTurnCallerResult>;
type AdapterSupervisor = SessionSupervisor<ProviderTurnLaunch, ProviderTurnCallerResult>;

#[derive(Clone)]
struct ExactLiveProcesses;

impl ProcessObserver for ExactLiveProcesses {
    fn observe(&self, _expected: &ExactProcessIdentity) -> ProcessObservation {
        ProcessObservation::ExactLive
    }
}

struct Harness {
    supervisor: Option<AdapterSupervisor>,
    turns: Receiver<AdapterTurn>,
    results: Receiver<TurnResult<ProviderTurnCallerResult>>,
    _events: Receiver<oulipoly_state::LifecycleEvent>,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn supervisor(&self) -> &AdapterSupervisor {
        self.supervisor.as_ref().expect("supervisor remains open")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.close(10_000);
        }
    }
}

struct QueueExecutor {
    calls: Arc<AtomicUsize>,
    kinds: Arc<Mutex<Vec<&'static str>>>,
    outcomes: Mutex<VecDeque<ProviderExecutionOutcome>>,
}

impl QueueExecutor {
    fn new(outcomes: impl IntoIterator<Item = ProviderExecutionOutcome>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            kinds: Arc::new(Mutex::new(Vec::new())),
            outcomes: Mutex::new(outcomes.into_iter().collect()),
        }
    }
}

impl ProviderTurnExecutor for QueueExecutor {
    fn execute(&self, request: &ProviderTurnExecutionRequest) -> ProviderExecutionOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.kinds.lock().unwrap().push(match request {
            ProviderTurnExecutionRequest::LegacyCliResume(_) => "legacy",
            ProviderTurnExecutionRequest::ExternalProvider(_) => "external",
        });
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("fake executor has one outcome per exact launch")
    }
}

struct RecordingService {
    requests: Arc<Mutex<Vec<ExecutorServiceRequest>>>,
    result: Result<ExecutionResult, ServiceError>,
}

impl ExecutorServicePort for RecordingService {
    fn execute(
        &self,
        request: ExecutorServiceRequest,
    ) -> Result<ExecutorServiceOutput, ServiceError> {
        self.requests.lock().unwrap().push(request);
        self.result
            .clone()
            .map(|result| ExecutorServiceOutput { result })
    }
}

fn process(pid: i64, suffix: &str) -> ExactProcessIdentity {
    ExactProcessIdentity {
        pid,
        boot_id: format!("boot-{suffix}"),
        start_time_ticks: pid * 10,
    }
}

fn start_harness() -> Harness {
    static NEXT_OWNER: AtomicI64 = AtomicI64::new(1);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = StateDb::open(&path).unwrap();
    let owner_number = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
    let owner = SupervisorFence {
        generation: 1,
        token: format!("owner-{owner_number}"),
        process: process(100 + owner_number, &format!("owner-{owner_number}")),
    };
    let (turn_tx, turns) = mpsc::channel();
    let (event_tx, events) = mpsc::channel();
    let (supervisor, results) = SessionSupervisor::start(
        SESSION,
        owner,
        1,
        Box::new(db),
        Arc::new(ExactLiveProcesses),
        SupervisorConfig::default(),
        turn_tx,
        event_tx,
    )
    .unwrap();
    Harness {
        supervisor: Some(supervisor),
        turns,
        results,
        _events: events,
        path,
        _dir: dir,
    }
}

fn receive_turn(turns: &Receiver<AdapterTurn>) -> AdapterTurn {
    turns
        .recv_timeout(Duration::from_secs(5))
        .expect("resident owner sends the exact provider turn")
}

fn receive_result(
    results: &Receiver<TurnResult<ProviderTurnCallerResult>>,
) -> TurnResult<ProviderTurnCallerResult> {
    results
        .recv_timeout(Duration::from_secs(5))
        .expect("resident owner publishes the exact caller result")
}

fn seed_invocation(
    state: &StateDb,
    uuid: &str,
    parent_invocation_id: Option<i64>,
) -> InvocationOwnership {
    let invocation_row_id = state
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 4,
            parent_invocation_id,
        })
        .unwrap();
    InvocationOwnership {
        invocation_row_id,
        invocation: CompositeInvocationId {
            source: "fixture".to_string(),
            id: uuid.to_string(),
        },
        parent_invocation_id,
    }
}

fn turn(sequence: i64, invocation_uuid: &str) -> ProviderTurnGeneration {
    ProviderTurnGeneration {
        generation_id: format!("generation-{sequence}"),
        spawn_invocation_id: invocation_uuid.to_string(),
        session_id: Some(SESSION.to_string()),
        state: TurnState::Running,
        child: process(200 + sequence, &format!("turn-{sequence}")),
    }
}

fn resume_strategy() -> ResumeStrategy {
    ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    }
}

fn legacy_request(prompt: &str) -> ProviderTurnExecutionRequest {
    ProviderTurnExecutionRequest::LegacyCliResume(LegacyCliResumeRequest {
        provider: ProviderConfig::new("fixture-provider", vec!["--fixed".to_string()]),
        provider_index: 4,
        prompt_mode: PromptMode::Arg,
        prompt: Some(prompt.to_string()),
        working_dir: None,
        parent_invocation_env: None,
        session_id: SESSION.to_string(),
        strategy: resume_strategy(),
        model_name: "fixture-model".to_string(),
        models_dir: None,
    })
}

fn model() -> ModelConfig {
    ModelConfig {
        name: "fixture-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new("fixture-provider", Vec::new())],
        inputs: Vec::new(),
        provider: None,
    }
}

fn external_request(start_mode: &str) -> ProviderTurnExecutionRequest {
    let provider = ProviderConfig::new("fixture-provider", vec!["--fixed".to_string()]);
    let fields = (
        model(),
        provider,
        4,
        PromptMode::Arg,
        "external prompt".to_string(),
        None,
        None,
        HashMap::new(),
        None,
        SESSION.to_string(),
    );
    let request = match start_mode {
        "resume" => ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
            model: fields.0,
            provider: fields.1,
            provider_index: fields.2,
            prompt_mode: fields.3,
            prompt: fields.4,
            working_dir: fields.5,
            models_dir: fields.6,
            extra_inputs: fields.7,
            parent_invocation_env: fields.8,
            start_known_provider_session_id: fields.9,
        },
        "create" => ExecutorServiceRequest::EffectiveWithCreateKnownProviderSessionId {
            model: fields.0,
            provider: fields.1,
            provider_index: fields.2,
            prompt_mode: fields.3,
            prompt: fields.4,
            working_dir: fields.5,
            models_dir: fields.6,
            extra_inputs: fields.7,
            parent_invocation_env: fields.8,
            start_known_provider_session_id: fields.9,
        },
        _ => panic!("unsupported fixture start mode"),
    };
    ProviderTurnExecutionRequest::ExternalProvider(request)
}

fn mailbox(delivery_id: &str, sequence: i64, nonce: &str) -> MailboxBatchIdentity {
    MailboxBatchIdentity {
        session_id: SESSION.to_string(),
        delivery_ids: vec![delivery_id.to_string()],
        sequences: vec![sequence],
        delivery_nonce: Some(nonce.to_string()),
    }
}

fn launch(
    launch_id: &str,
    request: ProviderTurnExecutionRequest,
    invocation: InvocationOwnership,
    mailbox_batch: MailboxBatchIdentity,
) -> ProviderTurnLaunch {
    ProviderTurnLaunch {
        launch_id: launch_id.to_string(),
        request,
        invocation,
        mailbox_batch,
    }
}

fn artifact(invocation_uuid: &str) -> ReturnedArtifactRef {
    let invocation_uuid = Uuid::parse_str(invocation_uuid).unwrap();
    ReturnedArtifactRef {
        version_id: format!("store://return/{invocation_uuid}/report/1"),
        name: "report".to_string(),
        store_address: StoreAddress {
            workflow_run_id: format!("return:{invocation_uuid}"),
            artifact_name: "report".to_string(),
            version: 1,
        },
        sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        content_len: 3,
        format_hint: Some("application/octet-stream".to_string()),
        verdict_line: Some("READY".to_string()),
        source: ReturnedArtifactSource::InlineBytes,
        producer_invocation_uuid: invocation_uuid,
        returned_at: Utc.with_ymd_and_hms(2026, 8, 10, 12, 0, 0).unwrap(),
    }
}

fn execution_result(
    session_id: &str,
    prompt: &str,
    nonce: Option<&str>,
    produced_assistant_response: bool,
    returned_artifacts: Vec<ReturnedArtifactRef>,
) -> ExecutionResult {
    ExecutionResult {
        stdout: vec![b'r', b'a', b'w', 0, 255],
        stderr: "stderr-exact".to_string(),
        exit_code: 0,
        provider_index: 4,
        session_capture: SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
        resume_acceptance: Some(ResumeAcceptanceResult {
            status: ResumeAcceptanceStatus::Accepted,
            evidence: Some("provider accepted exact resume".to_string()),
        }),
        terminal_reason: None,
        terminal_signal: None,
        produced_assistant_response,
        submitted_user_turn: Some(SubmittedUserTurn {
            provider_session_id: session_id.to_string(),
            prompt_sha256: prompt_sha256(prompt),
            delivery_nonce: nonce.map(str::to_string),
            source: Some("fixture".to_string()),
            message_id: Some("message-1".to_string()),
        }),
        captured_child_invocations: Vec::new(),
        returned_artifacts,
    }
}

fn complete_outcome(result: ExecutionResult) -> ProviderExecutionOutcome {
    ProviderExecutionOutcome::completed(result)
}

#[test]
fn resident_owner_publishes_exact_results_and_accepts_a_later_turn() {
    let harness = start_harness();
    let mut state = StateDb::open(&harness.path).unwrap();
    let parent = seed_invocation(&state, PARENT_UUID, None);
    let first_invocation = seed_invocation(&state, FIRST_UUID, Some(parent.invocation_row_id));
    let second_invocation = seed_invocation(&state, SECOND_UUID, Some(parent.invocation_row_id));
    let expected_artifact = artifact(FIRST_UUID);
    let mut first_execution = execution_result(
        SESSION,
        "first prompt",
        Some("nonce-1"),
        true,
        vec![expected_artifact.clone()],
    );
    first_execution
        .captured_child_invocations
        .push(CapturedChildInvocation {
            composite_id: CompositeInvocationId {
                source: "nested-fixture".to_string(),
                id: "44444444-4444-4444-4444-444444444444".to_string(),
            },
            raw_marker_line: "OULIPOLY_INVOCATION=nested-fixture".to_string(),
        });
    let expected_child_invocations = first_execution.captured_child_invocations.clone();
    let executor = QueueExecutor::new([
        complete_outcome(first_execution),
        complete_outcome(execution_result(
            SESSION,
            "second prompt",
            None,
            true,
            Vec::new(),
        )),
    ]);
    let calls = executor.calls.clone();
    let mut adapter = ProviderTurnAdapter::new(executor);

    let first_launch = launch(
        "launch-1",
        legacy_request("first prompt"),
        first_invocation.clone(),
        mailbox("delivery-1", 1, "nonce-1"),
    );
    harness
        .supervisor()
        .notify_external(
            ExternalIngress {
                session_id: SESSION.to_string(),
                sequence: 1,
                ingress_id: "delivery-1".to_string(),
                payload: "first prompt".to_string(),
            },
            SessionNotification::new(1, first_launch, turn(1, FIRST_UUID)),
            10,
        )
        .unwrap();
    adapter
        .consume(receive_turn(&harness.turns), &mut state, 11)
        .unwrap();

    let first = receive_result(&harness.results);
    let first = match first.outcome {
        TurnOutcome::Completed(result) => result,
        other => panic!("unexpected first outcome: {other:?}"),
    };
    assert_eq!(first.launch_id, "launch-1");
    assert_eq!(first.invocation, first_invocation);
    assert_eq!(first.execution.status, ProviderExecutionStatus::Completed);
    let execution = first.execution.result.as_ref().unwrap();
    assert_eq!(execution.stdout, vec![b'r', b'a', b'w', 0, 255]);
    assert_eq!(execution.stderr, "stderr-exact");
    assert_eq!(first.execution.caller_exit_code, 0);
    assert_eq!(
        execution.captured_child_invocations,
        expected_child_invocations
    );
    assert_eq!(
        execution.returned_artifacts,
        vec![expected_artifact.clone()]
    );
    assert!(matches!(
        first.effects,
        ProviderTurnEffects::Ready(ref report)
            if report.acknowledgement == EffectWrite::Applied
                && report.invocation_finalization == EffectWrite::Applied
    ));
    let acknowledgement = state.acknowledgement("delivery-1").unwrap().unwrap();
    assert_eq!(acknowledgement.stage(), AcknowledgementStage::Confirmed);
    assert!(
        acknowledgement
            .submitted_evidence
            .as_deref()
            .unwrap()
            .contains("submitted_user_turn")
    );
    assert!(
        acknowledgement
            .confirmed_evidence
            .as_deref()
            .unwrap()
            .contains("assistant_output")
    );
    let stored = state.get_invocation_by_uuid(FIRST_UUID).unwrap().unwrap();
    assert_eq!(stored.status, InvocationStatus::Succeeded);
    assert_eq!(stored.parent_invocation_id, Some(parent.invocation_row_id));
    assert_eq!(
        state.list_returned_artifacts(stored.id).unwrap(),
        vec![expected_artifact]
    );

    harness
        .supervisor()
        .notify(
            SessionNotification::new(
                2,
                launch(
                    "launch-2",
                    legacy_request("second prompt"),
                    second_invocation.clone(),
                    MailboxBatchIdentity::empty(SESSION),
                ),
                turn(2, SECOND_UUID),
            ),
            12,
        )
        .unwrap();
    adapter
        .consume(receive_turn(&harness.turns), &mut state, 13)
        .unwrap();
    let second = receive_result(&harness.results);
    assert_eq!(second.notification_sequence, 2);
    assert!(matches!(second.outcome, TurnOutcome::Completed(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        harness
            .supervisor()
            .status()
            .unwrap()
            .active_generation
            .is_none()
    );
}

#[test]
fn launch_and_state_effect_replay_are_exact_and_idempotent() {
    let harness = start_harness();
    let mut state = StateDb::open(&harness.path).unwrap();
    let parent = seed_invocation(&state, PARENT_UUID, None);
    let invocation = seed_invocation(&state, FIRST_UUID, Some(parent.invocation_row_id));
    let executor = QueueExecutor::new([complete_outcome(execution_result(
        SESSION,
        "replay prompt",
        Some("replay-nonce"),
        true,
        Vec::new(),
    ))]);
    let calls = executor.calls.clone();
    let mut adapter = ProviderTurnAdapter::new(executor);
    harness
        .supervisor()
        .notify(
            SessionNotification::new(
                1,
                launch(
                    "replay-launch",
                    legacy_request("replay prompt"),
                    invocation.clone(),
                    mailbox("replay-delivery", 1, "replay-nonce"),
                ),
                turn(1, FIRST_UUID),
            ),
            10,
        )
        .unwrap();
    let mut request = receive_turn(&harness.turns);
    state
        .accept_pending("replay-delivery", SESSION, "generation-1", 10)
        .unwrap();

    let first_launch = adapter.execute_once(&request).unwrap();
    let second_launch = adapter.execute_once(&request).unwrap();
    assert_eq!(first_launch.write, EffectWrite::Applied);
    assert_eq!(second_launch.write, EffectWrite::AlreadyApplied);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let first_effects = adapter
        .apply_effects(&request, &mut state, &first_launch.outcome, &[], 11)
        .unwrap();
    let second_effects = adapter
        .apply_effects(&request, &mut state, &first_launch.outcome, &[], 12)
        .unwrap();
    assert_eq!(first_effects.acknowledgement, EffectWrite::Applied);
    assert_eq!(first_effects.invocation_finalization, EffectWrite::Applied);
    assert_eq!(second_effects.acknowledgement, EffectWrite::AlreadyApplied);
    assert_eq!(
        second_effects.invocation_finalization,
        EffectWrite::AlreadyApplied
    );

    request.notification.input.launch_id = "conflicting-launch".to_string();
    assert_eq!(
        adapter.execute_once(&request).unwrap_err(),
        ProviderTurnAdapterError::ConflictingReplay
    );
    request.notification.input.launch_id = "replay-launch".to_string();
    request
        .completion
        .complete(
            ProviderTurnCallerResult {
                launch_id: "replay-launch".to_string(),
                invocation,
                execution: first_launch.outcome,
                effects: ProviderTurnEffects::Ready(second_effects),
            },
            13,
        )
        .unwrap();
    assert!(matches!(
        receive_result(&harness.results).outcome,
        TurnOutcome::Completed(_)
    ));
    assert!(matches!(
        harness.results.try_recv(),
        Err(TryRecvError::Empty)
    ));
}

#[test]
fn create_known_external_turn_is_supported_and_wrong_fences_do_not_execute() {
    let harness = start_harness();
    let state = StateDb::open(&harness.path).unwrap();
    let parent = seed_invocation(&state, PARENT_UUID, None);
    let invocation = seed_invocation(&state, FIRST_UUID, Some(parent.invocation_row_id));
    let executor = QueueExecutor::new([complete_outcome(execution_result(
        SESSION,
        "external prompt",
        None,
        true,
        Vec::new(),
    ))]);
    let calls = executor.calls.clone();
    let mut adapter = ProviderTurnAdapter::new(executor);
    harness
        .supervisor()
        .notify(
            SessionNotification::new(
                1,
                launch(
                    "external-create",
                    external_request("create"),
                    invocation.clone(),
                    MailboxBatchIdentity::empty(SESSION),
                ),
                turn(1, FIRST_UUID),
            ),
            10,
        )
        .unwrap();
    let mut request = receive_turn(&harness.turns);

    request.notification.input.invocation.invocation.id = SECOND_UUID.to_string();
    assert_eq!(
        adapter.execute_once(&request).unwrap_err(),
        ProviderTurnAdapterError::InvalidFence("spawn invocation")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    request.notification.input.invocation.invocation.id = FIRST_UUID.to_string();

    if let ProviderTurnExecutionRequest::ExternalProvider(
        ExecutorServiceRequest::EffectiveWithCreateKnownProviderSessionId {
            start_known_provider_session_id,
            ..
        },
    ) = &mut request.notification.input.request
    {
        *start_known_provider_session_id = "wrong-session".to_string();
    } else {
        panic!("expected create-known external request");
    }
    assert_eq!(
        adapter.execute_once(&request).unwrap_err(),
        ProviderTurnAdapterError::InvalidFence("execution session")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    if let ProviderTurnExecutionRequest::ExternalProvider(
        ExecutorServiceRequest::EffectiveWithCreateKnownProviderSessionId {
            start_known_provider_session_id,
            ..
        },
    ) = &mut request.notification.input.request
    {
        *start_known_provider_session_id = SESSION.to_string();
    }

    let effect = adapter.execute_once(&request).unwrap();
    assert_eq!(effect.write, EffectWrite::Applied);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    request
        .completion
        .complete(
            ProviderTurnCallerResult {
                launch_id: "external-create".to_string(),
                invocation,
                execution: effect.outcome,
                effects: ProviderTurnEffects::Ready(
                    oulipoly_runtime::provider_turn_adapter::ProviderTurnEffectReport {
                        acknowledgement: EffectWrite::NotApplicable,
                        invocation_finalization: EffectWrite::NotApplicable,
                    },
                ),
            },
            11,
        )
        .unwrap();
}

#[test]
fn evidence_strength_is_conservative_exact_fenced_and_monotonic() {
    let fence = TurnFence {
        session_id: SESSION.to_string(),
        generation_id: "generation-1".to_string(),
        spawn_invocation_id: FIRST_UUID.to_string(),
    };
    let prompt_hash = prompt_sha256("prompt");
    let cases = vec![
        (
            ProviderEvidence::ProcessLaunched,
            EvidenceStrength::Informational,
        ),
        (
            ProviderEvidence::TransportAccepted,
            EvidenceStrength::Informational,
        ),
        (
            ProviderEvidence::SubmittedUserTurn {
                provider_session_id: SESSION.to_string(),
                prompt_sha256: prompt_hash.clone(),
                delivery_nonce: None,
                source: None,
                message_id: None,
            },
            EvidenceStrength::Submitted,
        ),
        (
            ProviderEvidence::ResumeAccepted {
                provider_session_id: SESSION.to_string(),
                evidence: "accepted pattern".to_string(),
            },
            EvidenceStrength::Submitted,
        ),
        (
            ProviderEvidence::IngestedUserTurn {
                provider_session_id: SESSION.to_string(),
                turn_id: "user-turn".to_string(),
            },
            EvidenceStrength::Submitted,
        ),
        (
            ProviderEvidence::AssistantOutput {
                provider_session_id: SESSION.to_string(),
            },
            EvidenceStrength::Confirmed,
        ),
        (
            ProviderEvidence::IngestedAssistantTurn {
                provider_session_id: SESSION.to_string(),
                turn_id: "assistant-turn".to_string(),
            },
            EvidenceStrength::Confirmed,
        ),
        (
            ProviderEvidence::AffirmativeProviderCompletion {
                provider_session_id: SESSION.to_string(),
                evidence: "provider completion id".to_string(),
            },
            EvidenceStrength::Confirmed,
        ),
        (
            ProviderEvidence::AssistantOutputAbsent,
            EvidenceStrength::Informational,
        ),
        (
            ProviderEvidence::ResumeCompletionUnconfirmed,
            EvidenceStrength::Informational,
        ),
        (
            ProviderEvidence::Malformed {
                reason: "bad marker".to_string(),
            },
            EvidenceStrength::Informational,
        ),
        (
            ProviderEvidence::Manual {
                evidence: "operator note".to_string(),
            },
            EvidenceStrength::Informational,
        ),
    ];
    for (evidence, expected) in cases {
        assert_eq!(
            classify_provider_evidence(
                &fence,
                SESSION,
                Some(&prompt_hash),
                None,
                &FencedProviderEvidence {
                    fence: fence.clone(),
                    evidence,
                },
            )
            .unwrap(),
            expected
        );
    }

    let wrong_session = FencedProviderEvidence {
        fence: fence.clone(),
        evidence: ProviderEvidence::AssistantOutput {
            provider_session_id: "other-session".to_string(),
        },
    };
    assert_eq!(
        classify_provider_evidence(&fence, SESSION, Some(&prompt_hash), None, &wrong_session)
            .unwrap(),
        EvidenceStrength::Informational
    );
    let wrong_fence = FencedProviderEvidence {
        fence: TurnFence {
            generation_id: "other-generation".to_string(),
            ..fence.clone()
        },
        evidence: ProviderEvidence::ProcessLaunched,
    };
    assert_eq!(
        classify_provider_evidence(&fence, SESSION, Some(&prompt_hash), None, &wrong_fence)
            .unwrap_err(),
        ProviderTurnAdapterError::InvalidFence("provider evidence")
    );
}

#[test]
fn confirmation_only_evidence_advances_both_required_acknowledgement_stages() {
    let harness = start_harness();
    let mut state = StateDb::open(&harness.path).unwrap();
    let parent = seed_invocation(&state, PARENT_UUID, None);
    let invocation = seed_invocation(&state, FIRST_UUID, Some(parent.invocation_row_id));
    let mut result = execution_result(SESSION, "prompt", None, true, Vec::new());
    result.submitted_user_turn = None;
    result.resume_acceptance = None;
    let executor = QueueExecutor::new([complete_outcome(result)]);
    let mut adapter = ProviderTurnAdapter::new(executor);
    harness
        .supervisor()
        .notify(
            SessionNotification::new(
                1,
                launch(
                    "confirmation-only",
                    legacy_request("prompt"),
                    invocation,
                    mailbox("confirmation-delivery", 1, "nonce"),
                ),
                turn(1, FIRST_UUID),
            ),
            10,
        )
        .unwrap();
    let request = receive_turn(&harness.turns);
    state
        .accept_pending("confirmation-delivery", SESSION, "generation-1", 10)
        .unwrap();
    adapter.consume(request, &mut state, 11).unwrap();
    let acknowledgement = state
        .acknowledgement("confirmation-delivery")
        .unwrap()
        .unwrap();
    assert_eq!(acknowledgement.stage(), AcknowledgementStage::Confirmed);
    assert!(
        acknowledgement
            .submitted_evidence
            .as_deref()
            .unwrap()
            .contains("assistant_output")
    );
    assert_eq!(
        acknowledgement.submitted_evidence,
        acknowledgement.confirmed_evidence
    );
}

#[test]
fn execution_status_does_not_overstate_unconfirmed_or_failure_evidence() {
    let mut unconfirmed = execution_result(SESSION, "prompt", None, false, Vec::new());
    unconfirmed.submitted_user_turn = None;
    unconfirmed.resume_acceptance = Some(ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Unconfirmed,
        evidence: Some("exit zero without affirmative resume evidence".to_string()),
    });
    assert_eq!(
        complete_outcome(unconfirmed.clone()).status,
        ProviderExecutionStatus::ResumeCompletionUnconfirmed
    );
    unconfirmed.produced_assistant_response = true;
    assert_eq!(
        complete_outcome(unconfirmed).status,
        ProviderExecutionStatus::Completed
    );

    let mut rejected = execution_result(SESSION, "prompt", None, false, Vec::new());
    rejected.resume_acceptance = Some(ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Rejected,
        evidence: Some("missing provider session".to_string()),
    });
    assert_eq!(
        complete_outcome(rejected).status,
        ProviderExecutionStatus::ProviderRejected
    );

    let mut quota = execution_result(SESSION, "prompt", None, false, Vec::new());
    quota.terminal_signal = Some(TerminalSignal {
        kind: TerminalSignalKind::RateLimited,
        provider_name: "fixture-provider".to_string(),
        evidence: "429".to_string(),
        observed_at: SystemTime::UNIX_EPOCH,
    });
    assert_eq!(
        complete_outcome(quota).status,
        ProviderExecutionStatus::QuotaExhausted
    );

    let mut abnormal = execution_result(SESSION, "prompt", None, false, Vec::new());
    abnormal.exit_code = 17;
    assert_eq!(
        complete_outcome(abnormal).status,
        ProviderExecutionStatus::AbnormalExit
    );
}

#[test]
fn production_executors_preserve_legacy_grammar_and_external_request_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake-provider.sh");
    let args_path = dir.path().join("args.txt");
    let cwd_path = dir.path().join("cwd.txt");
    fs::write(
        &script,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\" > '{}'\npwd > '{}'\nprintf 'stderr-exact' >&2\nprintf 'assistant\\000\\377'\n",
            args_path.display(),
            cwd_path.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    let mut provider = ProviderConfig::new(
        script.to_string_lossy().into_owned(),
        vec!["--fixed".to_string()],
    );
    provider.resume_acceptance = Some(ResumeAcceptanceRules {
        accepted_output_patterns: Some(vec!["assistant".to_string()]),
        rejected_output_patterns: None,
    });
    let requests = Arc::new(Mutex::new(Vec::new()));
    let service_result = execution_result(SESSION, "external prompt", None, true, Vec::new());
    let production = ProductionProviderTurnExecutor::new(Arc::new(RecordingService {
        requests: requests.clone(),
        result: Ok(service_result.clone()),
    }));

    let legacy = ProviderTurnExecutionRequest::LegacyCliResume(LegacyCliResumeRequest {
        provider,
        provider_index: 7,
        prompt_mode: PromptMode::Arg,
        prompt: Some("legacy prompt".to_string()),
        working_dir: Some(dir.path().to_path_buf()),
        parent_invocation_env: None,
        session_id: SESSION.to_string(),
        strategy: resume_strategy(),
        model_name: "legacy-model".to_string(),
        models_dir: None,
    });
    let legacy_outcome = production.execute(&legacy);
    assert_eq!(legacy_outcome.status, ProviderExecutionStatus::Completed);
    let legacy_result = legacy_outcome.result.unwrap();
    assert_eq!(legacy_result.provider_index, 7);
    assert_eq!(legacy_result.stdout, b"assistant\0\xff");
    assert_eq!(legacy_result.stderr, "stderr-exact");
    assert_eq!(
        fs::read_to_string(args_path).unwrap(),
        format!("--fixed\n--resume\n{SESSION}\nlegacy prompt\n")
    );
    assert_eq!(
        fs::read_to_string(cwd_path).unwrap().trim(),
        dir.path().to_string_lossy()
    );

    let external = external_request("resume");
    let external_outcome = production.execute(&external);
    assert_eq!(external_outcome.result, Some(service_result));
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
            model,
            provider,
            provider_index,
            prompt_mode,
            prompt,
            start_known_provider_session_id,
            ..
        } => {
            assert_eq!(model.name, "fixture-model");
            assert_eq!(provider.command, "fixture-provider");
            assert_eq!(*provider_index, 4);
            assert_eq!(*prompt_mode, PromptMode::Arg);
            assert_eq!(prompt, "external prompt");
            assert_eq!(start_known_provider_session_id, SESSION);
        }
        other => panic!("unexpected external request: {other:?}"),
    }

    let malformed = ProductionProviderTurnExecutor::new(Arc::new(RecordingService {
        requests: Arc::new(Mutex::new(Vec::new())),
        result: Err(ServiceError::InvalidRequest {
            message: "malformed provider completion".to_string(),
        }),
    }))
    .execute(&external);
    assert_eq!(malformed.status, ProviderExecutionStatus::MalformedEvidence);
    assert!(matches!(
        malformed.error,
        Some(ProviderExecutionError::Service(
            ServiceError::InvalidRequest { .. }
        ))
    ));
}

#[test]
fn provider_failures_publish_one_bounded_result_without_scheduling_another_turn() {
    let harness = start_harness();
    let mut state = StateDb::open(&harness.path).unwrap();
    let parent = seed_invocation(&state, PARENT_UUID, None);
    let invocation = seed_invocation(&state, FIRST_UUID, Some(parent.invocation_row_id));
    let executor = QueueExecutor::new([ProviderExecutionOutcome::failed(
        ProviderExecutionStatus::LaunchFailed,
        ProviderExecutionError::LegacyCli("spawn denied".to_string()),
    )]);
    let mut adapter = ProviderTurnAdapter::new(executor);
    harness
        .supervisor()
        .notify(
            SessionNotification::new(
                1,
                launch(
                    "failed-launch",
                    legacy_request("prompt"),
                    invocation,
                    MailboxBatchIdentity::empty(SESSION),
                ),
                turn(1, FIRST_UUID),
            ),
            10,
        )
        .unwrap();
    adapter
        .consume(receive_turn(&harness.turns), &mut state, 11)
        .unwrap();
    let result = receive_result(&harness.results);
    assert!(matches!(
        result.outcome,
        TurnOutcome::Completed(ProviderTurnCallerResult {
            execution: ProviderExecutionOutcome {
                status: ProviderExecutionStatus::LaunchFailed,
                caller_exit_code: 1,
                ..
            },
            ..
        })
    ));
    assert!(matches!(
        harness.results.try_recv(),
        Err(TryRecvError::Empty)
    ));
    assert!(matches!(harness.turns.try_recv(), Err(TryRecvError::Empty)));
    assert!(
        harness
            .supervisor()
            .status()
            .unwrap()
            .active_generation
            .is_none()
    );
    let stored = state.get_invocation_by_uuid(FIRST_UUID).unwrap().unwrap();
    assert_eq!(stored.status, InvocationStatus::Failed);
    assert_eq!(
        stored.error_category.as_deref(),
        Some("provider_launch_failed")
    );
}
