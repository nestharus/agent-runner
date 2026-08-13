//! ## Declared roles
//!
//! `orchestration`, `validator`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_resume_tests.rs
//!     role: adapter
//!     Translates:
//!       - continuation-resume-runner-behavior-contract
//!       - StateDb-invocation-test-fixture-contract
//! ```

use std::cell::Cell;
use std::path::{Path, PathBuf};

use oulipoly_runtime::fresh_continuation::{
    ArtifactIdentity, ContinuationBlock, ContinuationBlockKind, ContinuationEvidence,
    FreshContinuationRequest, InvocationAction, InvocationDisposition, InvocationOutcome,
    ReservedInvocation, ResumeAcceptance, ResumeRunner, ValidatedContinuation,
};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};

const PARENT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const RESERVED_UUID: &str = "22222222-2222-4222-8222-222222222222";
const DIFFERENT_PARENT_UUID: &str = "33333333-3333-4333-8333-333333333333";
const ORIGIN_SESSION_ID: &str = "origin-session";
const UNCONFIRMED: &str = "resume_completion_unconfirmed";

struct ResumeFixture {
    state: StateDb,
    parent_row_id: i64,
    reservation: ReservedInvocation,
    context: ValidatedContinuation,
}

impl ResumeFixture {
    fn new() -> Self {
        let state = StateDb::open(Path::new(":memory:")).unwrap();
        let parent_row_id = start_invocation(&state, PARENT_UUID, None);
        Self {
            state,
            parent_row_id,
            reservation: ReservedInvocation {
                invocation_id: RESERVED_UUID.to_string(),
                parent_invocation_id: PARENT_UUID.to_string(),
            },
            context: validated_continuation(),
        }
    }

    fn seed_terminal_resume(&self) {
        seed_terminal_resume(&self.state, RESERVED_UUID, self.parent_row_id);
    }
}

#[test]
fn observe_returns_exact_terminal_outcome_without_executing() {
    let fixture = ResumeFixture::new();
    fixture.seed_terminal_resume();
    let execute_calls = Cell::new(0);
    let mut runner = super::continuation_resume::ContinuationResumeRunner::new(
        &fixture.state,
        |_: &super::reservation::ReservedRun, _: &ValidatedContinuation| {
            execute_calls.set(execute_calls.get() + 1);
            Ok(())
        },
    );

    let outcome = runner
        .run_or_observe(
            InvocationAction::Observe,
            &fixture.reservation,
            &fixture.context,
        )
        .unwrap();

    assert_eq!(outcome, expected_outcome());
    assert_eq!(execute_calls.get(), 0);
}

#[test]
fn run_executes_exact_reserved_plan_once_then_observes_it() {
    let fixture = ResumeFixture::new();
    let execute_calls = Cell::new(0);
    let state = &fixture.state;
    let parent_row_id = fixture.parent_row_id;
    let mut runner = super::continuation_resume::ContinuationResumeRunner::new(
        state,
        |plan: &super::reservation::ReservedRun, context: &ValidatedContinuation| {
            execute_calls.set(execute_calls.get() + 1);
            assert_eq!(plan.invocation_id(), RESERVED_UUID);
            assert_eq!(plan.parent_invocation_row_id(), parent_row_id);
            assert_eq!(plan.max_attempts(), 1);
            assert_eq!(context.request().origin_session_id, ORIGIN_SESSION_ID);
            seed_terminal_resume(state, plan.invocation_id(), parent_row_id);
            Ok(())
        },
    );

    let outcome = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
        )
        .unwrap();

    assert_eq!(outcome, expected_outcome());
    assert_eq!(execute_calls.get(), 1);
}

#[test]
fn durable_terminal_row_wins_over_post_execution_error() {
    let fixture = ResumeFixture::new();
    let execute_calls = Cell::new(0);
    let state = &fixture.state;
    let parent_row_id = fixture.parent_row_id;
    let mut runner = super::continuation_resume::ContinuationResumeRunner::new(
        state,
        |plan: &super::reservation::ReservedRun, _: &ValidatedContinuation| {
            execute_calls.set(execute_calls.get() + 1);
            seed_terminal_resume(state, plan.invocation_id(), parent_row_id);
            Err(block(ContinuationBlockKind::Persistence))
        },
    );

    let outcome = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
        )
        .unwrap();

    assert_eq!(outcome, expected_outcome());
    assert_eq!(execute_calls.get(), 1);
}

#[test]
fn execution_error_without_exact_row_is_returned_without_retry_or_replacement() {
    let fixture = ResumeFixture::new();
    let execute_calls = Cell::new(0);
    let expected = block(ContinuationBlockKind::InvocationFailed);
    let callback_error = expected.clone();
    let mut runner = super::continuation_resume::ContinuationResumeRunner::new(
        &fixture.state,
        |_: &super::reservation::ReservedRun, _: &ValidatedContinuation| {
            execute_calls.set(execute_calls.get() + 1);
            Err(callback_error.clone())
        },
    );

    let error = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
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
            .list_invocation_children(fixture.parent_row_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reservation_parent_mismatch_conflicts_before_execution_or_observation() {
    let mut fixture = ResumeFixture::new();
    fixture.reservation.parent_invocation_id = DIFFERENT_PARENT_UUID.to_string();
    let execute_calls = Cell::new(0);
    let mut runner = super::continuation_resume::ContinuationResumeRunner::new(
        &fixture.state,
        |_: &super::reservation::ReservedRun, _: &ValidatedContinuation| {
            execute_calls.set(execute_calls.get() + 1);
            Ok(())
        },
    );

    let error = runner
        .run_or_observe(
            InvocationAction::Run,
            &fixture.reservation,
            &fixture.context,
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

fn seed_terminal_resume(state: &StateDb, invocation_uuid: &str, parent_row_id: i64) {
    let row_id = start_invocation(state, invocation_uuid, Some(parent_row_id));
    state
        .bind_invocation_provider_session_start(
            row_id,
            &ProviderSessionBinding {
                provider_session_id: ORIGIN_SESSION_ID.to_string(),
                capture_method: "resumed",
                resume_input_id: Some(ORIGIN_SESSION_ID.to_string()),
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    state
        .update_resume_acceptance(row_id, "accepted", Some("matched origin session"))
        .unwrap();
    state
        .finalize_invocation(row_id, false, 0, Some(UNCONFIRMED), Some(UNCONFIRMED))
        .unwrap();
}

fn validated_continuation() -> ValidatedContinuation {
    super::continuation_test_support::validated_continuation(FreshContinuationRequest {
        question_id: "question-1".to_string(),
        origin_invocation_id: PARENT_UUID.to_string(),
        origin_session_id: ORIGIN_SESSION_ID.to_string(),
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

fn expected_outcome() -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: RESERVED_UUID.to_string(),
        session_id: Some(ORIGIN_SESSION_ID.to_string()),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::Accepted,
        disposition: InvocationDisposition::Failed {
            error_category: UNCONFIRMED.to_string(),
            terminal_reason: UNCONFIRMED.to_string(),
        },
    }
}

fn block(kind: ContinuationBlockKind) -> ContinuationBlock {
    ContinuationBlock {
        kind,
        message: "fixture block".to_string(),
    }
}
