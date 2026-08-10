use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use oulipoly_runtime::fresh_continuation::{
    AcceptDecision, AcceptedContinuation, ArtifactIdentity, ContinuationBlock,
    ContinuationBlockKind, ContinuationEvidence, ContinuationEvidenceValidator,
    ContinuationHandoff, ContinuationStore, FreshContinuation, FreshContinuationCoordinator,
    FreshContinuationOutcome, FreshContinuationRequest, FreshRunner, HandoffPublisher,
    InvocationAction, InvocationDisposition, InvocationOutcome, PublishedHandoff,
    ReservedInvocation, ResumeAcceptance, ResumeRunner, RunDecision, ValidatedContinuation,
};

#[derive(Default)]
struct Calls {
    validate: usize,
    accept: usize,
    begin_resume: usize,
    resume: usize,
    resume_actions: Vec<InvocationAction>,
    record_resume: usize,
    begin_fresh: usize,
    fresh: usize,
    fresh_actions: Vec<InvocationAction>,
    record_fresh: usize,
    publish: usize,
    finish: usize,
}

type SharedCalls = Rc<RefCell<Calls>>;

struct ValidatorFake {
    calls: SharedCalls,
    result: Result<ValidatedContinuation, ContinuationBlock>,
}

impl ContinuationEvidenceValidator for ValidatorFake {
    fn validate(
        &mut self,
        _request: &FreshContinuationRequest,
    ) -> Result<ValidatedContinuation, ContinuationBlock> {
        self.calls.borrow_mut().validate += 1;
        self.result.clone()
    }
}

struct StoreFake {
    calls: SharedCalls,
    accept: Result<AcceptDecision, ContinuationBlock>,
    resume: Result<RunDecision, ContinuationBlock>,
    fresh: Result<RunDecision, ContinuationBlock>,
    finish: Result<FreshContinuationOutcome, ContinuationBlock>,
}

impl ContinuationStore for StoreFake {
    fn accept(
        &mut self,
        _context: &ValidatedContinuation,
    ) -> Result<AcceptDecision, ContinuationBlock> {
        self.calls.borrow_mut().accept += 1;
        self.accept.clone()
    }

    fn begin_resume(
        &mut self,
        _continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock> {
        self.calls.borrow_mut().begin_resume += 1;
        self.resume.clone()
    }

    fn record_resume(
        &mut self,
        _continuation: &AcceptedContinuation,
        _outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock> {
        self.calls.borrow_mut().record_resume += 1;
        Ok(())
    }

    fn begin_fresh(
        &mut self,
        _continuation: &AcceptedContinuation,
    ) -> Result<RunDecision, ContinuationBlock> {
        self.calls.borrow_mut().begin_fresh += 1;
        self.fresh.clone()
    }

    fn record_fresh(
        &mut self,
        _continuation: &AcceptedContinuation,
        _outcome: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock> {
        self.calls.borrow_mut().record_fresh += 1;
        Ok(())
    }

    fn finish(
        &mut self,
        _continuation: &AcceptedContinuation,
        _handoff: &PublishedHandoff,
    ) -> Result<FreshContinuationOutcome, ContinuationBlock> {
        self.calls.borrow_mut().finish += 1;
        self.finish.clone()
    }
}

struct ResumeFake {
    calls: SharedCalls,
    outcome: InvocationOutcome,
}

impl ResumeRunner for ResumeFake {
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        _context: &ValidatedContinuation,
    ) -> InvocationOutcome {
        let mut calls = self.calls.borrow_mut();
        calls.resume += 1;
        calls.resume_actions.push(action);
        assert_eq!(reservation.invocation_id, self.outcome.invocation_id);
        self.outcome.clone()
    }
}

struct FreshFake {
    calls: SharedCalls,
    outcome: InvocationOutcome,
}

impl FreshRunner for FreshFake {
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        _context: &ValidatedContinuation,
        resume: &InvocationOutcome,
    ) -> InvocationOutcome {
        let mut calls = self.calls.borrow_mut();
        calls.fresh += 1;
        calls.fresh_actions.push(action);
        assert_eq!(reservation.invocation_id, self.outcome.invocation_id);
        assert_eq!(reservation.parent_invocation_id, resume.invocation_id);
        self.outcome.clone()
    }
}

struct PublisherFake {
    calls: SharedCalls,
    result: Result<PublishedHandoff, ContinuationBlock>,
}

impl HandoffPublisher for PublisherFake {
    fn publish(
        &mut self,
        handoff: ContinuationHandoff,
    ) -> Result<PublishedHandoff, ContinuationBlock> {
        self.calls.borrow_mut().publish += 1;
        assert_eq!(handoff.continuation_id, "continuation-1");
        self.result.clone()
    }
}

#[test]
fn exact_unconfirmed_resume_runs_one_fresh_invocation_and_returns_both_results() {
    let fixture = Fixture::happy();
    let calls = fixture.calls.clone();
    let expected = fixture.terminal.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

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
fn non_triggering_resume_never_runs_the_fresh_component() {
    let mut fixture = Fixture::happy();
    fixture.resume_outcome = succeeded("resume-1", Some("origin-session"));
    let calls = fixture.calls.clone();
    let mut coordinator = fixture.coordinator();

    let actual = coordinator.execute(request());

    assert!(matches!(
        actual,
        FreshContinuationOutcome::Blocked {
            reason: ContinuationBlock {
                kind: ContinuationBlockKind::TriggerNotMet,
                ..
            },
            ..
        }
    ));
    let calls = calls.borrow();
    assert_eq!(calls.resume, 1);
    assert_eq!(calls.record_resume, 1);
    assert_eq!(calls.fresh, 0);
    assert_eq!(calls.publish, 0);
}

#[test]
fn fresh_failure_preserves_the_original_resume_failure() {
    let mut fixture = Fixture::happy();
    let fresh = failed("fresh-1", Some("fresh-session"), 9, "provider_failed");
    fixture.fresh_outcome = fresh.clone();
    fixture.terminal = FreshContinuationOutcome::Failed {
        continuation_id: "continuation-1".to_string(),
        resume: fixture.resume_outcome.clone(),
        fresh: Some(fresh),
        handoff: fixture.publication.clone().ok(),
        reason: block(ContinuationBlockKind::InvocationFailed),
    };
    let calls = fixture.calls.clone();
    let resume = fixture.resume_outcome.clone();
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
}

struct Fixture {
    calls: SharedCalls,
    validator: Result<ValidatedContinuation, ContinuationBlock>,
    accept: Result<AcceptDecision, ContinuationBlock>,
    resume_decision: Result<RunDecision, ContinuationBlock>,
    fresh_decision: Result<RunDecision, ContinuationBlock>,
    resume_outcome: InvocationOutcome,
    fresh_outcome: InvocationOutcome,
    publication: Result<PublishedHandoff, ContinuationBlock>,
    terminal: FreshContinuationOutcome,
}

impl Fixture {
    fn happy() -> Self {
        let context = validated();
        let resume_reservation = reservation("resume-1", "origin-invocation");
        let fresh_reservation = reservation("fresh-1", "resume-1");
        let accepted = AcceptedContinuation {
            continuation_id: "continuation-1".to_string(),
            context: context.clone(),
            resume: resume_reservation.clone(),
            fresh: fresh_reservation.clone(),
        };
        let resume = unconfirmed_resume();
        let fresh = succeeded("fresh-1", Some("fresh-session"));
        let handoff = PublishedHandoff {
            path: PathBuf::from("/planning/continuation.json"),
            sha256: "handoff-sha".to_string(),
        };
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
            resume_outcome: resume,
            fresh_outcome: fresh,
            publication: Ok(handoff),
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
                finish: Ok(self.terminal),
            },
            ResumeFake {
                calls: self.calls.clone(),
                outcome: self.resume_outcome,
            },
            FreshFake {
                calls: self.calls.clone(),
                outcome: self.fresh_outcome,
            },
            PublisherFake {
                calls: self.calls,
                result: self.publication,
            },
        )
    }
}

fn request() -> FreshContinuationRequest {
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
            question: artifact("question"),
            answer: artifact("answer"),
            session_graph: artifact("graph"),
            origin_trace: artifact("trace"),
            ticket_snapshot: artifact("ticket"),
        },
    }
}

fn validated() -> ValidatedContinuation {
    ValidatedContinuation {
        request: request(),
        fingerprint: "request-fingerprint".to_string(),
    }
}

fn artifact(name: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        path: PathBuf::from(format!("/planning/{name}.json")),
        sha256: format!("{name}-sha"),
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
