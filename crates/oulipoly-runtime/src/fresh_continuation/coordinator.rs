use super::contract::{
    AcceptDecision, ContinuationBlock, ContinuationBlockKind, ContinuationEvidenceValidator,
    ContinuationHandoff, ContinuationStore, FreshContinuation, FreshContinuationOutcome,
    FreshContinuationRequest, FreshRunner, HandoffPublisher, InvocationAction,
    InvocationDisposition, InvocationOutcome, ResumeAcceptance, ResumeRunner, RunDecision,
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
            Err(reason) => {
                return FreshContinuationOutcome::Blocked {
                    continuation_id: None,
                    resume: None,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let continuation = match self.store.accept(&context) {
            Ok(AcceptDecision::Accepted(continuation)) => continuation,
            Ok(AcceptDecision::Replay(outcome)) => return *outcome,
            Err(reason) => {
                return FreshContinuationOutcome::Blocked {
                    continuation_id: None,
                    resume: None,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let (resume_action, resume_reservation) = match self.store.begin_resume(&continuation) {
            Ok(RunDecision::Run(reservation)) => (InvocationAction::Run, reservation),
            Ok(RunDecision::Observe(reservation)) => (InvocationAction::Observe, reservation),
            Ok(RunDecision::Terminal(outcome)) => return *outcome,
            Err(reason) => {
                return FreshContinuationOutcome::Blocked {
                    continuation_id: Some(continuation.continuation_id.clone()),
                    resume: None,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let resume = match self.resume.run_or_observe(
            resume_action,
            &resume_reservation,
            &continuation.context,
        ) {
            Ok(resume) => resume,
            Err(reason) => {
                return FreshContinuationOutcome::Blocked {
                    continuation_id: Some(continuation.continuation_id.clone()),
                    resume: None,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };
        if let Err(reason) = self.store.record_resume(&continuation, &resume) {
            return FreshContinuationOutcome::Failed {
                continuation_id: continuation.continuation_id.clone(),
                resume,
                fresh: None,
                handoff: None,
                reason,
            };
        }

        if !is_fresh_continuation_trigger(&resume) {
            return FreshContinuationOutcome::Blocked {
                continuation_id: Some(continuation.continuation_id.clone()),
                resume: Some(resume),
                fresh: None,
                handoff: None,
                reason: ContinuationBlock {
                    kind: ContinuationBlockKind::TriggerNotMet,
                    message: "resume outcome does not meet the fresh-continuation trigger"
                        .to_string(),
                },
            };
        }

        let (fresh_action, fresh_reservation) = match self.store.begin_fresh(&continuation) {
            Ok(RunDecision::Run(reservation)) => (InvocationAction::Run, reservation),
            Ok(RunDecision::Observe(reservation)) => (InvocationAction::Observe, reservation),
            Ok(RunDecision::Terminal(outcome)) => return *outcome,
            Err(reason) => {
                return FreshContinuationOutcome::Failed {
                    continuation_id: continuation.continuation_id.clone(),
                    resume,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };

        let fresh = match self.fresh.run_or_observe(
            fresh_action,
            &fresh_reservation,
            &continuation.context,
            &resume,
        ) {
            Ok(fresh) => fresh,
            Err(reason) => {
                return FreshContinuationOutcome::Failed {
                    continuation_id: continuation.continuation_id.clone(),
                    resume,
                    fresh: None,
                    handoff: None,
                    reason,
                };
            }
        };
        if let Err(reason) = self.store.record_fresh(&continuation, &fresh) {
            return FreshContinuationOutcome::Failed {
                continuation_id: continuation.continuation_id.clone(),
                resume,
                fresh: Some(fresh),
                handoff: None,
                reason,
            };
        }

        let handoff = match self.publisher.publish(ContinuationHandoff {
            continuation_id: continuation.continuation_id.clone(),
            resume: resume.clone(),
            fresh: Some(fresh.clone()),
        }) {
            Ok(handoff) => handoff,
            Err(reason) => {
                return FreshContinuationOutcome::Failed {
                    continuation_id: continuation.continuation_id.clone(),
                    resume,
                    fresh: Some(fresh),
                    handoff: None,
                    reason,
                };
            }
        };

        match self.store.finish(&continuation, &handoff) {
            Ok(outcome) => outcome,
            Err(reason) => FreshContinuationOutcome::Failed {
                continuation_id: continuation.continuation_id,
                resume,
                fresh: Some(fresh),
                handoff: Some(handoff),
                reason,
            },
        }
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
