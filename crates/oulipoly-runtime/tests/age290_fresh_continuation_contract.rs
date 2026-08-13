//! ## Declared roles
//!
//! `orchestration`, `validator`, `mapper`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age290_fresh_continuation_contract.rs
//!     role: adapter
//!     Translates:
//!       - fresh-continuation-coordinator-behavior-contract
//!       - Rust-port-fake-contract
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use oulipoly_runtime::fresh_continuation::{
    AcceptDecision, AcceptedContinuation, ArtifactIdentity, ContinuationArtifactSource,
    ContinuationBlock, ContinuationBlockKind, ContinuationEvidence, ContinuationEvidenceValidator,
    ContinuationHandoff, ContinuationStore, DefaultContinuationEvidenceValidator,
    FreshContinuation, FreshContinuationCoordinator, FreshContinuationOutcome,
    FreshContinuationRequest, FreshRunner, HandoffPublisher, InvocationAction,
    InvocationDisposition, InvocationOutcome, PublishedHandoff, ReservedInvocation,
    ResumeAcceptance, ResumeRunner, RunDecision, ValidatedContinuation,
};
use sha2::{Digest, Sha256};

#[derive(Default)]
struct Calls {
    validate: usize,
    validation_requests: Vec<FreshContinuationRequest>,
    accept: usize,
    accepted_contexts: Vec<ValidatedContinuation>,
    begin_resume: usize,
    begin_resume_continuations: Vec<AcceptedContinuation>,
    resume: usize,
    resume_actions: Vec<InvocationAction>,
    resume_inputs: Vec<ResumeInput>,
    record_resume: usize,
    recorded_resumes: Vec<(AcceptedContinuation, InvocationOutcome)>,
    begin_fresh: usize,
    begin_fresh_continuations: Vec<AcceptedContinuation>,
    fresh: usize,
    fresh_actions: Vec<InvocationAction>,
    fresh_inputs: Vec<FreshInput>,
    record_fresh: usize,
    recorded_fresh: Vec<(AcceptedContinuation, InvocationOutcome)>,
    publish: usize,
    published_handoffs: Vec<ContinuationHandoff>,
    verify: usize,
    verified_handoffs: Vec<(String, PublishedHandoff)>,
    finish: usize,
    finish_inputs: Vec<(AcceptedContinuation, PublishedHandoff)>,
}

type SharedCalls = Rc<RefCell<Calls>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResumeInput {
    action: InvocationAction,
    reservation: ReservedInvocation,
    context: ValidatedContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FreshInput {
    action: InvocationAction,
    reservation: ReservedInvocation,
    context: ValidatedContinuation,
    resume: InvocationOutcome,
}

struct ValidatorFake {
    calls: SharedCalls,
    result: Result<ValidatedContinuation, ContinuationBlock>,
}

impl ContinuationEvidenceValidator for ValidatorFake {
    fn validate(
        &mut self,
        request: &FreshContinuationRequest,
    ) -> Result<ValidatedContinuation, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.validate += 1;
        calls.validation_requests.push(request.clone());
        self.result.clone()
    }
}

struct StoreFake {
    calls: SharedCalls,
    accept: Result<AcceptDecision, ContinuationBlock>,
    resume: Result<RunDecision, ContinuationBlock>,
    fresh: Result<RunDecision, ContinuationBlock>,
    record_resume: Result<(), ContinuationBlock>,
    record_fresh: Result<(), ContinuationBlock>,
    finish: Result<FreshContinuationOutcome, ContinuationBlock>,
}

impl ContinuationStore for StoreFake {
    fn accept(
        &mut self,
        context: &ValidatedContinuation,
    ) -> Result<AcceptDecision, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.accept += 1;
        calls.accepted_contexts.push(context.clone());
        self.accept.clone()
    }

    fn begin_resume(
        &mut self,
        continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.begin_resume += 1;
        calls.begin_resume_continuations.push(continuation.clone());
        self.resume.clone()
    }

    fn record_resume(
        &mut self,
        continuation: &AcceptedContinuation,
        outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.record_resume += 1;
        calls
            .recorded_resumes
            .push((continuation.clone(), outcome.clone()));
        self.record_resume.clone()
    }

    fn begin_fresh(
        &mut self,
        continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.begin_fresh += 1;
        calls.begin_fresh_continuations.push(continuation.clone());
        self.fresh.clone()
    }

    fn record_fresh(
        &mut self,
        continuation: &AcceptedContinuation,
        outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.record_fresh += 1;
        calls
            .recorded_fresh
            .push((continuation.clone(), outcome.clone()));
        self.record_fresh.clone()
    }

    fn finish(
        &mut self,
        continuation: &AcceptedContinuation,
        handoff: &PublishedHandoff,
    ) -> Result<FreshContinuationOutcome, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.finish += 1;
        calls
            .finish_inputs
            .push((continuation.clone(), handoff.clone()));
        self.finish.clone()
    }
}

struct ResumeFake {
    calls: SharedCalls,
    result: Result<InvocationOutcome, ContinuationBlock>,
}

impl ResumeRunner for ResumeFake {
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.resume += 1;
        calls.resume_actions.push(action);
        calls.resume_inputs.push(ResumeInput {
            action,
            reservation: reservation.clone(),
            context: context.clone(),
        });
        self.result.clone()
    }
}

struct FreshFake {
    calls: SharedCalls,
    result: Result<InvocationOutcome, ContinuationBlock>,
}

impl FreshRunner for FreshFake {
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
        resume: &InvocationOutcome,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.fresh += 1;
        calls.fresh_actions.push(action);
        calls.fresh_inputs.push(FreshInput {
            action,
            reservation: reservation.clone(),
            context: context.clone(),
            resume: resume.clone(),
        });
        self.result.clone()
    }
}

struct PublisherFake {
    calls: SharedCalls,
    result: Result<PublishedHandoff, ContinuationBlock>,
    verification: Result<(), ContinuationBlock>,
}

impl HandoffPublisher for PublisherFake {
    fn publish(
        &mut self,
        handoff: ContinuationHandoff,
    ) -> Result<PublishedHandoff, ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.publish += 1;
        calls.published_handoffs.push(handoff);
        self.result.clone()
    }

    fn verify(
        &mut self,
        continuation_id: &str,
        handoff: &PublishedHandoff,
    ) -> Result<(), ContinuationBlock> {
        let mut calls = self.calls.borrow_mut();
        calls.verify += 1;
        calls
            .verified_handoffs
            .push((continuation_id.to_string(), handoff.clone()));
        self.verification.clone()
    }
}

#[test]
fn exact_unconfirmed_resume_runs_one_fresh_invocation_and_returns_both_results() {
    let fixture = Fixture::happy();
    let calls = fixture.calls.clone();
    let expected = fixture.terminal.clone();
    let context = validated();
    let continuation = accepted_continuation(context.clone());
    let resume = unconfirmed_resume();
    let fresh = succeeded("fresh-1", Some("fresh-session"));
    let publication = published_handoff();
    let mut submitted_request = request();
    submitted_request.origin_invocation_id = "nearby-origin-invocation".to_string();
    submitted_request.target_model = "nearby-target-model".to_string();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(submitted_request.clone());

    assert_eq!(actual, expected);
    let calls = calls.borrow();
    assert_eq!(calls.validate, 1);
    assert_eq!(calls.accept, 1);
    assert_eq!(calls.resume, 1);
    assert_eq!(calls.resume_actions.as_slice(), &[InvocationAction::Run]);
    assert_eq!(calls.fresh, 1);
    assert_eq!(calls.fresh_actions.as_slice(), &[InvocationAction::Run]);
    assert_eq!(calls.publish, 1);
    assert_eq!(calls.finish, 1);
    assert_eq!(calls.validation_requests.as_slice(), &[submitted_request]);
    assert_eq!(
        calls.accepted_contexts.as_slice(),
        std::slice::from_ref(&context)
    );
    assert_eq!(
        calls.begin_resume_continuations.as_slice(),
        std::slice::from_ref(&continuation)
    );
    assert_eq!(
        calls.resume_inputs.as_slice(),
        &[ResumeInput {
            action: InvocationAction::Run,
            reservation: continuation.resume.clone(),
            context: context.clone(),
        }]
    );
    assert_eq!(
        calls.recorded_resumes.as_slice(),
        &[(continuation.clone(), resume.clone())]
    );
    assert_eq!(
        calls.begin_fresh_continuations.as_slice(),
        std::slice::from_ref(&continuation)
    );
    assert_eq!(
        calls.fresh_inputs.as_slice(),
        &[FreshInput {
            action: InvocationAction::Run,
            reservation: continuation.fresh.clone(),
            context: context.clone(),
            resume: resume.clone(),
        }]
    );
    assert_eq!(
        calls.recorded_fresh.as_slice(),
        &[(continuation.clone(), fresh.clone())]
    );
    assert_eq!(
        calls.published_handoffs.as_slice(),
        &[ContinuationHandoff {
            continuation_id: continuation.continuation_id.clone(),
            fresh_prompt: expected_fresh_prompt(),
            request: context.request().clone(),
            resume,
            fresh: Some(fresh),
        }]
    );
    assert_eq!(
        calls.finish_inputs.as_slice(),
        &[(continuation, publication)]
    );
}

#[test]
fn observe_decisions_are_transported_without_changing_the_successful_continuation() {
    let mut fixture = Fixture::happy();
    fixture.resume_decision = Ok(RunDecision::Observe(reservation(
        "resume-1",
        "origin-invocation",
    )));
    fixture.fresh_decision = Ok(RunDecision::Observe(reservation("fresh-1", "resume-1")));
    let calls = fixture.calls.clone();
    let expected = fixture.terminal.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert_eq!(actual, expected);
    let calls = calls.borrow();
    assert_eq!(calls.validate, 1);
    assert_eq!(calls.accept, 1);
    assert_eq!(calls.resume, 1);
    assert_eq!(
        calls.resume_actions.as_slice(),
        &[InvocationAction::Observe]
    );
    assert_eq!(calls.fresh, 1);
    assert_eq!(calls.fresh_actions.as_slice(), &[InvocationAction::Observe]);
    assert_eq!(calls.publish, 1);
    assert_eq!(calls.finish, 1);
}

#[test]
fn invalid_evidence_blocks_before_any_invocation() {
    let mut fixture = Fixture::happy();
    fixture.validator = Err(block(ContinuationBlockKind::InvalidEvidence));
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert!(matches!(
        actual,
        FreshContinuationOutcome::Blocked {
            continuation_id: None,
            reason: ContinuationBlock {
                kind: ContinuationBlockKind::InvalidEvidence,
                ..
            },
            ..
        }
    ));
    let calls = calls.borrow();
    assert_eq!(calls.validate, 1);
    assert_eq!(calls.accept, 0);
    assert_eq!(calls.resume, 0);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.publish, 0);
}

#[test]
fn unavailable_resume_outcome_returns_its_exact_typed_block() {
    let mut fixture = Fixture::happy();
    let reason = ContinuationBlock {
        kind: ContinuationBlockKind::AmbiguousState,
        message: "reserved resume outcome is unavailable".to_string(),
    };
    fixture.resume_outcome = Err(reason.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_resume_port_blocked(&actual, &calls, reason);
}

#[test]
fn mismatched_resume_outcome_returns_its_exact_typed_block() {
    let mut fixture = Fixture::happy();
    let reason = ContinuationBlock {
        kind: ContinuationBlockKind::Conflict,
        message: "reserved resume outcome identity mismatched".to_string(),
    };
    fixture.resume_outcome = Err(reason.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_resume_port_blocked(&actual, &calls, reason);
}

#[test]
fn nonzero_resume_exit_does_not_trigger_fresh_continuation() {
    let mut fixture = Fixture::happy();
    let mut resume = unconfirmed_resume();
    resume.physical_exit_code = 9;
    fixture.resume_outcome = Ok(resume.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_trigger_not_met(&actual, &calls, resume);
}

#[test]
fn rejected_resume_acceptance_does_not_trigger_fresh_continuation() {
    let mut fixture = Fixture::happy();
    let mut resume = unconfirmed_resume();
    resume.acceptance = ResumeAcceptance::Rejected;
    fixture.resume_outcome = Ok(resume.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_trigger_not_met(&actual, &calls, resume);
}

#[test]
fn unconfirmed_resume_acceptance_does_not_trigger_fresh_continuation() {
    let mut fixture = Fixture::happy();
    let mut resume = unconfirmed_resume();
    resume.acceptance = ResumeAcceptance::Unconfirmed;
    fixture.resume_outcome = Ok(resume.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_trigger_not_met(&actual, &calls, resume);
}

#[test]
fn not_applicable_resume_acceptance_does_not_trigger_fresh_continuation() {
    let mut fixture = Fixture::happy();
    let mut resume = unconfirmed_resume();
    resume.acceptance = ResumeAcceptance::NotApplicable;
    fixture.resume_outcome = Ok(resume.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_trigger_not_met(&actual, &calls, resume);
}

#[test]
fn successful_resume_disposition_does_not_trigger_fresh_continuation() {
    let mut fixture = Fixture::happy();
    let mut resume = unconfirmed_resume();
    resume.disposition = InvocationDisposition::Succeeded;
    fixture.resume_outcome = Ok(resume.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_trigger_not_met(&actual, &calls, resume);
}

#[test]
fn wrong_resume_error_category_does_not_trigger_fresh_continuation() {
    let mut fixture = Fixture::happy();
    let mut resume = unconfirmed_resume();
    let InvocationDisposition::Failed { error_category, .. } = &mut resume.disposition else {
        unreachable!("unconfirmed resume fixture must be failed");
    };
    *error_category = "quota_exhausted".to_string();
    fixture.resume_outcome = Ok(resume.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_trigger_not_met(&actual, &calls, resume);
}

#[test]
fn wrong_resume_terminal_reason_does_not_trigger_fresh_continuation() {
    let mut fixture = Fixture::happy();
    let mut resume = unconfirmed_resume();
    let InvocationDisposition::Failed {
        terminal_reason, ..
    } = &mut resume.disposition
    else {
        unreachable!("unconfirmed resume fixture must be failed");
    };
    *terminal_reason = "provider_failed".to_string();
    fixture.resume_outcome = Ok(resume.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    let calls = calls.borrow();
    assert_trigger_not_met(&actual, &calls, resume);
}

#[test]
fn fresh_failure_preserves_the_original_resume_failure() {
    let mut fixture = Fixture::happy();
    let fresh = failed("fresh-1", Some("fresh-session"), 9, "provider_failed");
    fixture.fresh_outcome = Ok(fresh.clone());
    fixture.terminal = FreshContinuationOutcome::Failed {
        continuation_id: "continuation-1".to_string(),
        resume: fixture.resume_outcome.clone().unwrap(),
        fresh: Some(fresh),
        handoff: fixture.publication.clone().ok(),
        reason: block(ContinuationBlockKind::InvocationFailed),
    };
    let calls = fixture.calls.clone();
    let resume = fixture.resume_outcome.clone().unwrap();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert!(matches!(
        actual,
        FreshContinuationOutcome::Failed {
            resume: actual_resume,
            fresh: Some(_),
            ..
        } if actual_resume == resume
    ));
    let calls = calls.borrow();
    assert_eq!(calls.resume, 1);
    assert_eq!(calls.fresh, 1);
    assert_eq!(calls.record_fresh, 1);
    assert_eq!(calls.publish, 1);
    assert_eq!(calls.finish, 1);
}

#[test]
fn fresh_adapter_error_fails_without_recording_or_publishing_fresh() {
    let mut fixture = Fixture::happy();
    let reason = block(ContinuationBlockKind::Persistence);
    fixture.fresh_outcome = Err(reason.clone());
    let resume = fixture.resume_outcome.clone().unwrap();
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert_eq!(
        actual,
        FreshContinuationOutcome::Failed {
            continuation_id: "continuation-1".to_string(),
            resume,
            fresh: None,
            handoff: None,
            reason,
        }
    );
    let calls = calls.borrow();
    assert_eq!(calls.record_resume, 1);
    assert_eq!(calls.begin_fresh, 1);
    assert_eq!(calls.fresh, 1);
    assert_eq!(calls.record_fresh, 0);
    assert_eq!(calls.publish, 0);
    assert_eq!(calls.finish, 0);
}

#[test]
fn resume_recording_error_preserves_the_resume_outcome() {
    let mut fixture = Fixture::happy();
    let reason = block(ContinuationBlockKind::Persistence);
    fixture.record_resume = Err(reason.clone());
    let resume = fixture.resume_outcome.clone().unwrap();
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert_eq!(
        actual,
        FreshContinuationOutcome::Failed {
            continuation_id: "continuation-1".to_string(),
            resume,
            fresh: None,
            handoff: None,
            reason,
        }
    );
    let calls = calls.borrow();
    assert_eq!(calls.record_resume, 1);
    assert_eq!(calls.begin_fresh, 0);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.publish, 0);
    assert_eq!(calls.finish, 0);
}

#[test]
fn fresh_recording_error_preserves_both_invocation_outcomes() {
    let mut fixture = Fixture::happy();
    let reason = block(ContinuationBlockKind::Persistence);
    fixture.record_fresh = Err(reason.clone());
    let resume = fixture.resume_outcome.clone().unwrap();
    let fresh = fixture.fresh_outcome.clone().unwrap();
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert_eq!(
        actual,
        FreshContinuationOutcome::Failed {
            continuation_id: "continuation-1".to_string(),
            resume,
            fresh: Some(fresh),
            handoff: None,
            reason,
        }
    );
    let calls = calls.borrow();
    assert_eq!(calls.record_fresh, 1);
    assert_eq!(calls.publish, 0);
    assert_eq!(calls.finish, 0);
}

#[test]
fn terminal_fresh_decision_replays_without_calling_the_fresh_runner() {
    let mut fixture = Fixture::happy();
    let expected = fixture.terminal.clone();
    fixture.fresh_decision = Ok(RunDecision::Terminal(Box::new(expected.clone())));
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert_eq!(actual, expected);
    let calls = calls.borrow();
    assert_eq!(calls.resume, 1);
    assert_eq!(calls.record_resume, 1);
    assert_eq!(calls.begin_fresh, 1);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.record_fresh, 0);
    assert_eq!(calls.publish, 0);
    assert_eq!(calls.verify, 1);
    assert_eq!(calls.finish, 0);
}

#[test]
fn finish_error_preserves_the_published_handoff() {
    let mut fixture = Fixture::happy();
    let reason = block(ContinuationBlockKind::Persistence);
    fixture.finish = Some(Err(reason.clone()));
    let resume = fixture.resume_outcome.clone().unwrap();
    let fresh = fixture.fresh_outcome.clone().unwrap();
    let handoff = fixture.publication.clone().unwrap();
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert_eq!(
        actual,
        FreshContinuationOutcome::Failed {
            continuation_id: "continuation-1".to_string(),
            resume,
            fresh: Some(fresh),
            handoff: Some(handoff),
            reason,
        }
    );
    let calls = calls.borrow();
    assert_eq!(calls.publish, 1);
    assert_eq!(calls.finish, 1);
}

#[test]
fn terminal_replay_returns_the_same_outcome_without_calling_runners() {
    let mut fixture = Fixture::happy();
    fixture.accept = Ok(AcceptDecision::Replay(Box::new(fixture.terminal.clone())));
    let expected = fixture.terminal.clone();
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert_eq!(actual, expected);
    let calls = calls.borrow();
    assert_eq!(calls.validate, 1);
    assert_eq!(calls.accept, 1);
    assert_eq!(calls.begin_resume, 0);
    assert_eq!(calls.resume, 0);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.publish, 0);
    assert_eq!(calls.verify, 1);
    assert_eq!(
        calls.verified_handoffs.as_slice(),
        &[("continuation-1".to_string(), published_handoff())]
    );
}

#[test]
fn terminal_replay_fails_when_the_published_handoff_cannot_be_verified() {
    let mut fixture = Fixture::happy();
    let reason = ContinuationBlock {
        kind: ContinuationBlockKind::Conflict,
        message: "recorded handoff integrity check failed".to_string(),
    };
    fixture.accept = Ok(AcceptDecision::Replay(Box::new(fixture.terminal.clone())));
    fixture.verification = Err(reason.clone());
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert!(matches!(
        actual,
        FreshContinuationOutcome::Failed {
            continuation_id,
            handoff: Some(_),
            reason: actual_reason,
            ..
        } if continuation_id == "continuation-1" && actual_reason == reason
    ));
    let calls = calls.borrow();
    assert_eq!(calls.verify, 1);
    assert_eq!(calls.resume, 0);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.publish, 0);
}

fn assert_resume_port_blocked(
    actual: &FreshContinuationOutcome,
    calls: &Calls,
    reason: ContinuationBlock,
) {
    assert_eq!(
        actual,
        &FreshContinuationOutcome::Blocked {
            continuation_id: Some("continuation-1".to_string()),
            resume: None,
            fresh: None,
            handoff: None,
            reason,
        }
    );
    assert_eq!(calls.validate, 1);
    assert_eq!(calls.accept, 1);
    assert_eq!(calls.begin_resume, 1);
    assert_eq!(calls.resume, 1);
    assert_eq!(calls.record_resume, 0);
    assert_eq!(calls.begin_fresh, 0);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.record_fresh, 0);
    assert_eq!(calls.publish, 0);
    assert_eq!(calls.finish, 0);
    assert_eq!(calls.validation_requests.as_slice(), &[request()]);
    assert_eq!(calls.accepted_contexts.as_slice(), &[validated()]);
    assert_eq!(
        calls.begin_resume_continuations.as_slice(),
        &[accepted_continuation(validated())]
    );
    assert_eq!(
        calls.resume_inputs.as_slice(),
        &[ResumeInput {
            action: InvocationAction::Run,
            reservation: reservation("resume-1", "origin-invocation"),
            context: validated(),
        }]
    );
    assert!(calls.recorded_resumes.is_empty());
    assert!(calls.begin_fresh_continuations.is_empty());
    assert!(calls.fresh_inputs.is_empty());
    assert!(calls.recorded_fresh.is_empty());
    assert!(calls.published_handoffs.is_empty());
    assert!(calls.finish_inputs.is_empty());
}

fn assert_trigger_not_met(
    actual: &FreshContinuationOutcome,
    calls: &Calls,
    resume: InvocationOutcome,
) {
    let continuation = accepted_continuation(validated());
    assert_eq!(
        actual,
        &FreshContinuationOutcome::Blocked {
            continuation_id: Some("continuation-1".to_string()),
            resume: Some(resume.clone()),
            fresh: None,
            handoff: None,
            reason: ContinuationBlock {
                kind: ContinuationBlockKind::TriggerNotMet,
                message: "resume outcome does not meet the fresh-continuation trigger".to_string(),
            },
        }
    );
    assert_eq!(calls.validate, 1);
    assert_eq!(calls.accept, 1);
    assert_eq!(calls.begin_resume, 1);
    assert_eq!(calls.resume, 1);
    assert_eq!(calls.record_resume, 1);
    assert_eq!(calls.begin_fresh, 0);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.record_fresh, 0);
    assert_eq!(calls.publish, 0);
    assert_eq!(calls.finish, 0);
    assert_eq!(calls.validation_requests.as_slice(), &[request()]);
    assert_eq!(calls.accepted_contexts.as_slice(), &[validated()]);
    assert_eq!(
        calls.begin_resume_continuations.as_slice(),
        std::slice::from_ref(&continuation)
    );
    assert_eq!(
        calls.resume_inputs.as_slice(),
        &[ResumeInput {
            action: InvocationAction::Run,
            reservation: continuation.resume.clone(),
            context: validated(),
        }]
    );
    assert_eq!(calls.recorded_resumes.as_slice(), &[(continuation, resume)]);
    assert!(calls.begin_fresh_continuations.is_empty());
    assert!(calls.fresh_inputs.is_empty());
    assert!(calls.recorded_fresh.is_empty());
    assert!(calls.published_handoffs.is_empty());
    assert!(calls.finish_inputs.is_empty());
}

struct Fixture {
    calls: SharedCalls,
    validator: Result<ValidatedContinuation, ContinuationBlock>,
    accept: Result<AcceptDecision, ContinuationBlock>,
    resume_decision: Result<RunDecision, ContinuationBlock>,
    fresh_decision: Result<RunDecision, ContinuationBlock>,
    resume_outcome: Result<InvocationOutcome, ContinuationBlock>,
    fresh_outcome: Result<InvocationOutcome, ContinuationBlock>,
    publication: Result<PublishedHandoff, ContinuationBlock>,
    verification: Result<(), ContinuationBlock>,
    record_resume: Result<(), ContinuationBlock>,
    record_fresh: Result<(), ContinuationBlock>,
    finish: Option<Result<FreshContinuationOutcome, ContinuationBlock>>,
    terminal: FreshContinuationOutcome,
}

impl Fixture {
    fn happy() -> Self {
        let context = validated();
        let resume_reservation = reservation("resume-1", "origin-invocation");
        let fresh_reservation = reservation("fresh-1", "resume-1");
        let accepted = accepted_continuation(context.clone());
        let resume = unconfirmed_resume();
        let fresh = succeeded("fresh-1", Some("fresh-session"));
        let handoff = published_handoff();
        let terminal = FreshContinuationOutcome::Continued {
            continuation_id: accepted.continuation_id.clone(),
            resume: resume.clone(),
            fresh: fresh.clone(),
            handoff: handoff.clone(),
        };
        Self {
            calls: Rc::new(RefCell::new(Calls::default())),
            validator: Ok(context),
            accept: Ok(AcceptDecision::Accepted(Box::new(accepted))),
            resume_decision: Ok(RunDecision::Run(resume_reservation)),
            fresh_decision: Ok(RunDecision::Run(fresh_reservation)),
            resume_outcome: Ok(resume),
            fresh_outcome: Ok(fresh),
            publication: Ok(handoff),
            verification: Ok(()),
            record_resume: Ok(()),
            record_fresh: Ok(()),
            finish: None,
            terminal,
        }
    }

    fn coordinator(
        self,
    ) -> FreshContinuationCoordinator<ValidatorFake, StoreFake, ResumeFake, FreshFake, PublisherFake>
    {
        FreshContinuationCoordinator::new(
            ValidatorFake {
                calls: self.calls.clone(),
                result: self.validator,
            },
            StoreFake {
                calls: self.calls.clone(),
                accept: self.accept,
                resume: self.resume_decision,
                fresh: self.fresh_decision,
                record_resume: self.record_resume,
                record_fresh: self.record_fresh,
                finish: self.finish.unwrap_or(Ok(self.terminal)),
            },
            ResumeFake {
                calls: self.calls.clone(),
                result: self.resume_outcome,
            },
            FreshFake {
                calls: self.calls.clone(),
                result: self.fresh_outcome,
            },
            PublisherFake {
                calls: self.calls,
                result: self.publication,
                verification: self.verification,
            },
        )
    }
}

fn request() -> FreshContinuationRequest {
    let files = validation_files();
    FreshContinuationRequest {
        question_id: "question-1".to_string(),
        origin_invocation_id: "origin-invocation".to_string(),
        origin_session_id: "origin-session".to_string(),
        planning_root: PathBuf::from("/planning"),
        worktree: PathBuf::from("/worktree"),
        last_successful_boundary: "verified".to_string(),
        active_blocked_boundary: "apply".to_string(),
        target_model: "fresh-model".to_string(),
        evidence: ContinuationEvidence {
            question: artifact(&files, "question"),
            answer: artifact(&files, "answer"),
            session_graph: artifact(&files, "graph"),
            origin_trace: artifact(&files, "trace"),
            ticket_snapshot: artifact(&files, "ticket"),
        },
    }
}

fn validated() -> ValidatedContinuation {
    DefaultContinuationEvidenceValidator::new(ArtifactSourceFake {
        files: validation_files(),
    })
    .validate(&request())
    .expect("production evidence validation")
}

fn accepted_continuation(context: ValidatedContinuation) -> AcceptedContinuation {
    AcceptedContinuation::without_historical_authority(
        "continuation-1".to_string(),
        context,
        reservation("resume-1", "origin-invocation"),
        reservation("fresh-1", "resume-1"),
    )
}

fn published_handoff() -> PublishedHandoff {
    PublishedHandoff {
        path: PathBuf::from("/planning/continuation.json"),
        sha256: "handoff-sha".to_string(),
    }
}

fn expected_fresh_prompt() -> String {
    let request = request();
    format!(
        concat!(
            "Continue the blocked workflow in this fresh provider session.\n",
            "Do not retry or mutate the origin session.\n",
            "Origin invocation: origin-invocation\n",
            "Origin session: origin-session\n",
            "Failed resume invocation: resume-1\n",
            "Worktree: /worktree\n",
            "Last successful boundary: verified\n",
            "Active blocked boundary: apply\n",
            "Read these exact artifacts before continuing:\n",
            "- question: /planning/question.json (sha256 {})\n",
            "- answer: /planning/answer.json (sha256 {})\n",
            "- session graph: /planning/graph.json (sha256 {})\n",
            "- origin trace: /planning/trace.json (sha256 {})\n",
            "- ticket snapshot: /planning/ticket.json (sha256 {})\n",
        ),
        request.evidence.question.sha256,
        request.evidence.answer.sha256,
        request.evidence.session_graph.sha256,
        request.evidence.origin_trace.sha256,
        request.evidence.ticket_snapshot.sha256,
    )
}

struct ArtifactSourceFake {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl ContinuationArtifactSource for ArtifactSourceFake {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock> {
        self.files
            .get(&artifact.path)
            .cloned()
            .ok_or_else(|| block(ContinuationBlockKind::InvalidEvidence))
    }
}

fn validation_files() -> HashMap<PathBuf, Vec<u8>> {
    HashMap::from([
        (
            PathBuf::from("/planning/question.json"),
            br#"{"schema_version":1,"kind":"agent_question","question_id":"question-1","origin":{"invocation_uuid":"origin-invocation","session_id":"origin-session","worktree_path":"/worktree"},"state_refs":{"session_graph_manifest":"/planning/graph.json"}}"#.to_vec(),
        ),
        (
            PathBuf::from("/planning/answer.json"),
            br#"{"schema_version":1,"kind":"agent_answer","question_id":"question-1","answered_by":"user-via-root-orchestrator","continuation_plan":{"session_graph_manifest":"/planning/graph.json"}}"#.to_vec(),
        ),
        (
            PathBuf::from("/planning/graph.json"),
            br#"{"root_invocation_uuid":"origin-invocation","invocation_ids":["origin-invocation"],"session_ids":["origin-session"],"question_ids":["question-1"]}"#.to_vec(),
        ),
        (
            PathBuf::from("/planning/trace.json"),
            br#"{"root":{"invocation":{"id":"origin-invocation"},"session":{"provider_session_id":"origin-session"}}}"#.to_vec(),
        ),
        (
            PathBuf::from("/planning/ticket.json"),
            b"ticket snapshot".to_vec(),
        ),
    ])
}

fn artifact(files: &HashMap<PathBuf, Vec<u8>>, name: &str) -> ArtifactIdentity {
    let path = PathBuf::from(format!("/planning/{name}.json"));
    ArtifactIdentity {
        sha256: format!("{:x}", Sha256::digest(files.get(&path).unwrap())),
        path,
    }
}

fn reservation(invocation_id: &str, parent_invocation_id: &str) -> ReservedInvocation {
    ReservedInvocation {
        invocation_id: invocation_id.to_string(),
        parent_invocation_id: parent_invocation_id.to_string(),
    }
}

fn unconfirmed_resume() -> InvocationOutcome {
    failed(
        "resume-1",
        Some("origin-session"),
        0,
        "resume_completion_unconfirmed",
    )
}

fn succeeded(invocation_id: &str, session_id: Option<&str>) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: invocation_id.to_string(),
        session_id: session_id.map(str::to_string),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::NotApplicable,
        disposition: InvocationDisposition::Succeeded,
    }
}

fn failed(
    invocation_id: &str,
    session_id: Option<&str>,
    physical_exit_code: i32,
    reason: &str,
) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: invocation_id.to_string(),
        session_id: session_id.map(str::to_string),
        physical_exit_code,
        acceptance: ResumeAcceptance::Accepted,
        disposition: InvocationDisposition::Failed {
            error_category: reason.to_string(),
            terminal_reason: reason.to_string(),
        },
    }
}

fn block(kind: ContinuationBlockKind) -> ContinuationBlock {
    ContinuationBlock {
        kind,
        message: "blocked".to_string(),
    }
}
