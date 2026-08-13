//! ## Declared roles
//!
//! `orchestration`, `validator`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_fresh_tests.rs
//!     role: adapter
//!     Translates:
//!       - continuation-fresh-runner-behavior-contract
//!       - StateDb-invocation-test-fixture-contract
//! ```

use std::cell::Cell;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationBlock, ContinuationBlockKind, ContinuationEvidence,
    FreshContinuationRequest, FreshRunner, InvocationAction, InvocationDisposition,
    InvocationOutcome, ReservedInvocation, ResumeAcceptance, ValidatedContinuation,
};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};

const RESUME_UUID: &str = "11111111-1111-4111-8111-111111111111";
const RESERVED_UUID: &str = "22222222-2222-4222-8222-222222222222";
const DIFFERENT_PARENT_UUID: &str = "33333333-3333-4333-8333-333333333333";
const FRESH_SESSION_ID: &str = "fresh-session";

struct FreshFixture {
    state: StateDb,
    resume_row_id: i64,
    reservation: ReservedInvocation,
    context: ValidatedContinuation,
    resume: InvocationOutcome,
}

impl FreshFixture {
    fn new() -> Self {
        let state = StateDb::open(Path::new(":memory:")).unwrap();
        let resume_row_id = start_invocation(&state, RESUME_UUID, None);
        Self {
            state,
            resume_row_id,
            reservation: ReservedInvocation {
                invocation_id: RESERVED_UUID.to_string(),
                parent_invocation_id: RESUME_UUID.to_string(),
            },
            context: validated_continuation(),
            resume: resume_outcome(),
        }
    }

    fn seed_terminal_fresh(&self) {
        seed_terminal_fresh(&self.state, RESERVED_UUID, self.resume_row_id);
    }
}

#[test]
fn observe_returns_exact_fresh_outcome_without_executing() {
    let fixture = FreshFixture::new();
    fixture.seed_terminal_fresh();
    let execute_calls = Cell::new(0);
    let mut runner = super::continuation_fresh::ContinuationFreshRunner::new(
        &fixture.state,
        |_: &super::reservation::ReservedRun, _: &ValidatedContinuation, _: &InvocationOutcome| {
            execute_calls.set(execute_calls.get() + 1);
            Ok(())
        },
    );

    let outcome = runner
        .run_or_observe(
            InvocationAction::Observe,
            &fixture.reservation,
            &fixture.context,
            &fixture.resume,
        )
        .unwrap();

    assert_eq!(outcome, expected_fresh_outcome());
    assert_eq!(execute_calls.get(), 0);
}

#[test]
fn observe_without_fresh_row_fails_closed_without_executing() {
    let fixture = FreshFixture::new();
    let execute_calls = Cell::new(0);
    let mut runner = super::continuation_fresh::ContinuationFreshRunner::new(
        &fixture.state,
        |_: &super::reservation::ReservedRun, _: &ValidatedContinuation, _: &InvocationOutcome| {
            execute_calls.set(execute_calls.get() + 1);
            Ok(())
        },
    );

    let error = runner
        .run_or_observe(
            InvocationAction::Observe,
            &fixture.reservation,
            &fixture.context,
            &fixture.resume,
        )
        .unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::AmbiguousState);
    assert_eq!(execute_calls.get(), 0);
}

#[test]
fn run_executes_exact_fresh_plan_once_then_observes_it() {
    let fixture = FreshFixture::new();
    let execute_calls = Cell::new(0);
    let state = &fixture.state;
    let resume_row_id = fixture.resume_row_id;
    let mut runner = super::continuation_fresh::ContinuationFreshRunner::new(
        state,
        |plan: &super::reservation::ReservedRun,
         _: &ValidatedContinuation,
         resume: &InvocationOutcome| {
            execute_calls.set(execute_calls.get() + 1);
            assert_eq!(plan.invocation_id(), RESERVED_UUID);
            assert_eq!(plan.parent_invocation_row_id(), resume_row_id);
            assert_eq!(plan.max_attempts(), 1);
            assert_eq!(resume.invocation_id, RESUME_UUID);
            seed_terminal_fresh(state, plan.invocation_id(), resume_row_id);
            Ok(())
        },
    );

    let outcome = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
            &fixture.resume,
        )
        .unwrap();

    assert_eq!(outcome, expected_fresh_outcome());
    assert_eq!(execute_calls.get(), 1);
}

#[test]
fn durable_fresh_row_wins_over_post_execution_error() {
    let fixture = FreshFixture::new();
    let execute_calls = Cell::new(0);
    let state = &fixture.state;
    let resume_row_id = fixture.resume_row_id;
    let mut runner = super::continuation_fresh::ContinuationFreshRunner::new(
        state,
        |plan: &super::reservation::ReservedRun,
         _: &ValidatedContinuation,
         _: &InvocationOutcome| {
            execute_calls.set(execute_calls.get() + 1);
            seed_terminal_fresh(state, plan.invocation_id(), resume_row_id);
            Err(block(ContinuationBlockKind::Persistence))
        },
    );

    let outcome = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
            &fixture.resume,
        )
        .unwrap();

    assert_eq!(outcome, expected_fresh_outcome());
    assert_eq!(execute_calls.get(), 1);
}

#[test]
fn fresh_execution_error_without_exact_row_is_returned_without_replacement() {
    let fixture = FreshFixture::new();
    let execute_calls = Cell::new(0);
    let expected = block(ContinuationBlockKind::InvocationFailed);
    let callback_error = expected.clone();
    let mut runner = super::continuation_fresh::ContinuationFreshRunner::new(
        &fixture.state,
        |_: &super::reservation::ReservedRun, _: &ValidatedContinuation, _: &InvocationOutcome| {
            execute_calls.set(execute_calls.get() + 1);
            Err(callback_error.clone())
        },
    );

    let error = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
            &fixture.resume,
        )
        .unwrap_err();

    assert_eq!(error, expected);
    assert_eq!(execute_calls.get(), 1);
    assert!(
        fixture
            .state
            .get_invocation_by_uuid(RESERVED_UUID)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .state
            .list_invocation_children(fixture.resume_row_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fresh_parent_mismatch_conflicts_before_execution_or_observation() {
    let mut fixture = FreshFixture::new();
    fixture.reservation.parent_invocation_id = DIFFERENT_PARENT_UUID.to_string();
    let execute_calls = Cell::new(0);
    let mut runner = super::continuation_fresh::ContinuationFreshRunner::new(
        &fixture.state,
        |_: &super::reservation::ReservedRun, _: &ValidatedContinuation, _: &InvocationOutcome| {
            execute_calls.set(execute_calls.get() + 1);
            Ok(())
        },
    );

    let error = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
            &fixture.resume,
        )
        .unwrap_err();

    assert_eq!(error.kind, ContinuationBlockKind::Conflict);
    assert_eq!(execute_calls.get(), 0);
}

fn start_invocation(state: &StateDb, invocation_uuid: &str, parent: Option<i64>) -> i64 {
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: parent,
        })
        .unwrap()
}

fn seed_terminal_fresh(state: &StateDb, invocation_uuid: &str, parent_row_id: i64) {
    let row_id = start_invocation(state, invocation_uuid, Some(parent_row_id));
    state
        .bind_invocation_provider_session_start(
            row_id,
            &ProviderSessionBinding {
                provider_session_id: FRESH_SESSION_ID.to_string(),
                capture_method: "forced_flag_verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    state
        .finalize_invocation(row_id, true, 0, None, None)
        .unwrap();
}

fn validated_continuation() -> ValidatedContinuation {
    super::continuation_test_support::validated_continuation(FreshContinuationRequest {
        question_id: "question-1".to_string(),
        origin_invocation_id: "origin-invocation".to_string(),
        origin_session_id: "origin-session".to_string(),
        planning_root: PathBuf::from("/planning"),
        worktree: PathBuf::from("/worktree"),
        last_successful_boundary: "verified".to_string(),
        active_blocked_boundary: "apply".to_string(),
        target_model: "fresh-model".to_string(),
        evidence: ContinuationEvidence {
            question: artifact("question"),
            answer: artifact("answer"),
            session_graph: artifact("graph"),
            origin_trace: artifact("trace"),
            ticket_snapshot: artifact("ticket"),
        },
    })
}

fn artifact(name: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        path: PathBuf::from(format!("/planning/{name}.json")),
        sha256: format!("{name}-sha"),
    }
}

fn resume_outcome() -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: RESUME_UUID.to_string(),
        session_id: Some("origin-session".to_string()),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::Accepted,
        disposition: InvocationDisposition::Failed {
            error_category: "resume_completion_unconfirmed".to_string(),
            terminal_reason: "resume_completion_unconfirmed".to_string(),
        },
    }
}

fn expected_fresh_outcome() -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: RESERVED_UUID.to_string(),
        session_id: Some(FRESH_SESSION_ID.to_string()),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::NotApplicable,
        disposition: InvocationDisposition::Succeeded,
    }
}

fn block(kind: ContinuationBlockKind) -> ContinuationBlock {
    ContinuationBlock {
        kind,
        message: "fixture block".to_string(),
    }
}
