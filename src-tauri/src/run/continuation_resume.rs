use oulipoly_runtime::fresh_continuation::{
    ContinuationBlock, ContinuationBlockKind, InvocationAction, InvocationOutcome,
    ReservedInvocation, ResumeRunner, ValidatedContinuation,
};
use oulipoly_state::StateDb;

use super::{continuation_outcome, reservation::ReservedRun};

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ContinuationResumeRunner<'state, Execute> {
    state: &'state StateDb,
    execute: Execute,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<'state, Execute> ContinuationResumeRunner<'state, Execute> {
    pub(crate) fn new(state: &'state StateDb, execute: Execute) -> Self {
        Self { state, execute }
    }
}

impl<Execute> ResumeRunner for ContinuationResumeRunner<'_, Execute>
where
    Execute: FnMut(&ReservedRun, &ValidatedContinuation) -> Result<(), ContinuationBlock>,
{
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        if reservation.parent_invocation_id != context.request.origin_invocation_id {
            return Err(ContinuationBlock {
                kind: ContinuationBlockKind::Conflict,
                message: "Reserved resume parent does not match the continuation origin"
                    .to_string(),
            });
        }

        if action == InvocationAction::Observe {
            return self.observe(reservation, context);
        }

        let reserved = ReservedRun::resolve(self.state, reservation).map_err(ambiguous_state)?;
        let execution = (self.execute)(&reserved, context);

        match (execution, self.observe(reservation, context)) {
            (_, Ok(outcome)) => Ok(outcome),
            (Err(error), Err(_)) => Err(error),
            (Ok(()), Err(error)) => Err(error),
        }
    }
}

impl<Execute> ContinuationResumeRunner<'_, Execute> {
    fn observe(
        &self,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        continuation_outcome::observe_resume_outcome(
            self.state,
            reservation,
            &context.request.origin_session_id,
        )
        .map_err(ambiguous_state)
    }
}

fn ambiguous_state(message: String) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::AmbiguousState,
        message,
    }
}
