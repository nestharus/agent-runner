//! ## Declared roles
//!
//! `orchestration`, `predicate`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/fresh_continuation/coordinator.rs
//!     role: adapter
//!     Translates:
//!       - fresh-continuation-port-and-outcome-contract
//! ```

use super::contract::{
    AcceptDecision, ContinuationBlock, ContinuationBlockKind, ContinuationEvidenceValidator,
    ContinuationHandoff, ContinuationStore, FreshContinuation, FreshContinuationOutcome,
    FreshContinuationRequest, FreshRunner, HandoffPublisher, InvocationAction,
    InvocationDisposition, InvocationOutcome, PublishedHandoff, ReservedInvocation,
    ResumeAcceptance, ResumeRunner, RunDecision, ValidatedContinuation, fresh_prompt,
};

pub struct FreshContinuationCoordinator<Validator, Store, Resume, Fresh, Publisher> {
    validator: Validator,
    store: Store,
    resume: Resume,
    fresh: Fresh,
    publisher: Publisher,
}

impl<Validator, Store, Resume, Fresh, Publisher>
    FreshContinuationCoordinator<Validator, Store, Resume, Fresh, Publisher>
{
    pub fn new(
        validator: Validator,
        store: Store,
        resume: Resume,
        fresh: Fresh,
        publisher: Publisher,
    ) -> Self {
        Self {
            validator,
            store,
            resume,
            fresh,
            publisher,
        }
    }
}

impl<Validator, Store, Resume, Fresh, Publisher> FreshContinuation
    for FreshContinuationCoordinator<Validator, Store, Resume, Fresh, Publisher>
where
    Validator: ContinuationEvidenceValidator,
    Store: ContinuationStore,
    Resume: ResumeRunner,
    Fresh: FreshRunner,
    Publisher: HandoffPublisher,
{
    fn execute(&mut self, request: FreshContinuationRequest) -> FreshContinuationOutcome {
        let context = match self.validator.validate(&request) {
            Ok(context) => context,
            Err(reason) => return blocked_without_continuation(reason),
        };

        let continuation = match self.store.accept(&context) {
            Ok(AcceptDecision::Accepted(continuation)) => continuation,
            Ok(AcceptDecision::Replay(outcome)) => return *outcome,
            Err(reason) => return blocked_without_continuation(reason),
        };

        let resume_decision = match self.store.begin_resume(&continuation) {
            Ok(decision) => decision,
            Err(reason) => return blocked_continuation(&continuation.continuation_id, reason),
        };
        let (resume_action, resume_reservation) = match runnable_invocation(resume_decision) {
            Ok(run) => run,
            Err(outcome) => return *outcome,
        };

        let resume = match self.resume.run_or_observe(
            resume_action,
            &resume_reservation,
            &continuation.context,
        ) {
            Ok(resume) => resume,
            Err(reason) => return blocked_continuation(&continuation.continuation_id, reason),
        };
        if let Err(reason) = self.store.record_resume(&continuation, &resume) {
            return failed_after_resume(&continuation.continuation_id, resume, reason);
        }

        if !is_fresh_continuation_trigger(&resume) {
            return trigger_not_met(&continuation.continuation_id, resume);
        }

        let fresh_decision = match self.store.begin_fresh(&continuation) {
            Ok(decision) => decision,
            Err(reason) => {
                return failed_after_resume(&continuation.continuation_id, resume, reason);
            }
        };
        let (fresh_action, fresh_reservation) = match runnable_invocation(fresh_decision) {
            Ok(run) => run,
            Err(outcome) => return *outcome,
        };

        let fresh = match self.fresh.run_or_observe(
            fresh_action,
            &fresh_reservation,
            &continuation.context,
            &resume,
        ) {
            Ok(fresh) => fresh,
            Err(reason) => {
                return failed_after_resume(&continuation.continuation_id, resume, reason);
            }
        };
        if let Err(reason) = self.store.record_fresh(&continuation, &fresh) {
            return failed_after_fresh(&continuation.continuation_id, resume, fresh, reason);
        }

        let handoff_request = continuation_handoff(
            &continuation.continuation_id,
            &continuation.context,
            &resume,
            &fresh,
        );
        let handoff = match self.publisher.publish(handoff_request) {
            Ok(handoff) => handoff,
            Err(reason) => {
                return failed_after_fresh(&continuation.continuation_id, resume, fresh, reason);
            }
        };

        match self.store.finish(&continuation, &handoff) {
            Ok(outcome) => outcome,
            Err(reason) => {
                failed_after_handoff(continuation.continuation_id, resume, fresh, handoff, reason)
            }
        }
    }
}

fn runnable_invocation(
    decision: RunDecision,
) -> Result<(InvocationAction, ReservedInvocation), Box<FreshContinuationOutcome>> {
    match decision {
        RunDecision::Run(reservation) => Ok((InvocationAction::Run, reservation)),
        RunDecision::Observe(reservation) => Ok((InvocationAction::Observe, reservation)),
        RunDecision::Terminal(outcome) => Err(outcome),
    }
}

fn blocked_without_continuation(reason: ContinuationBlock) -> FreshContinuationOutcome {
    FreshContinuationOutcome::Blocked {
        continuation_id: None,
        resume: None,
        fresh: None,
        handoff: None,
        reason,
    }
}

fn blocked_continuation(
    continuation_id: &str,
    reason: ContinuationBlock,
) -> FreshContinuationOutcome {
    FreshContinuationOutcome::Blocked {
        continuation_id: Some(continuation_id.to_string()),
        resume: None,
        fresh: None,
        handoff: None,
        reason,
    }
}

fn trigger_not_met(continuation_id: &str, resume: InvocationOutcome) -> FreshContinuationOutcome {
    FreshContinuationOutcome::Blocked {
        continuation_id: Some(continuation_id.to_string()),
        resume: Some(resume),
        fresh: None,
        handoff: None,
        reason: ContinuationBlock {
            kind: ContinuationBlockKind::TriggerNotMet,
            message: "resume outcome does not meet the fresh-continuation trigger".to_string(),
        },
    }
}

fn failed_after_resume(
    continuation_id: &str,
    resume: InvocationOutcome,
    reason: ContinuationBlock,
) -> FreshContinuationOutcome {
    FreshContinuationOutcome::Failed {
        continuation_id: continuation_id.to_string(),
        resume,
        fresh: None,
        handoff: None,
        reason,
    }
}

fn failed_after_fresh(
    continuation_id: &str,
    resume: InvocationOutcome,
    fresh: InvocationOutcome,
    reason: ContinuationBlock,
) -> FreshContinuationOutcome {
    FreshContinuationOutcome::Failed {
        continuation_id: continuation_id.to_string(),
        resume,
        fresh: Some(fresh),
        handoff: None,
        reason,
    }
}

fn continuation_handoff(
    continuation_id: &str,
    context: &ValidatedContinuation,
    resume: &InvocationOutcome,
    fresh: &InvocationOutcome,
) -> ContinuationHandoff {
    ContinuationHandoff {
        continuation_id: continuation_id.to_string(),
        fresh_prompt: fresh_prompt(context, resume),
        request: context.request.clone(),
        resume: resume.clone(),
        fresh: Some(fresh.clone()),
    }
}

fn failed_after_handoff(
    continuation_id: String,
    resume: InvocationOutcome,
    fresh: InvocationOutcome,
    handoff: PublishedHandoff,
    reason: ContinuationBlock,
) -> FreshContinuationOutcome {
    FreshContinuationOutcome::Failed {
        continuation_id,
        resume,
        fresh: Some(fresh),
        handoff: Some(handoff),
        reason,
    }
}

fn is_fresh_continuation_trigger(outcome: &InvocationOutcome) -> bool {
    outcome.physical_exit_code == 0
        && outcome.acceptance == ResumeAcceptance::Accepted
        && matches!(
            &outcome.disposition,
            InvocationDisposition::Failed {
                error_category,
                terminal_reason,
            } if error_category == "resume_completion_unconfirmed"
                && terminal_reason == "resume_completion_unconfirmed"
        )
}
