use oulipoly_runtime::fresh_continuation::{
    ContinuationBlock, ContinuationBlockKind, FreshRunner, InvocationAction, InvocationOutcome,
    ReservedInvocation, ValidatedContinuation,
};
use oulipoly_state::StateDb;

use super::{continuation_outcome, reservation::ReservedRun};

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ContinuationFreshRunner<'state, Execute> {
    state: &'state StateDb,
    execute: Execute,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<'state, Execute> ContinuationFreshRunner<'state, Execute> {
    pub(crate) fn new(state: &'state StateDb, execute: Execute) -> Self {
        Self { state, execute }
    }
}

impl<Execute> FreshRunner for ContinuationFreshRunner<'_, Execute>
where
    Execute: FnMut(
        &ReservedRun,
        &ValidatedContinuation,
        &InvocationOutcome,
    ) -> Result<(), ContinuationBlock>,
{
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
        resume: &InvocationOutcome,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        if reservation.parent_invocation_id != resume.invocation_id {
            return Err(ContinuationBlock {
                kind: ContinuationBlockKind::Conflict,
                message: "Reserved fresh parent does not match the resume invocation".to_string(),
            });
        }

        if action == InvocationAction::Observe {
            return self.observe(reservation);
        }

        let reserved = ReservedRun::resolve(self.state, reservation).map_err(ambiguous_state)?;
        let execution = (self.execute)(&reserved, context, resume);

        match (execution, self.observe(reservation)) {
            (_, Ok(outcome)) => Ok(outcome),
            (Err(error), Err(_)) => Err(error),
            (Ok(()), Err(error)) => Err(error),
        }
    }
}

impl<Execute> ContinuationFreshRunner<'_, Execute> {
    fn observe(
        &self,
        reservation: &ReservedInvocation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        continuation_outcome::observe_fresh_outcome(self.state, reservation)
            .map_err(ambiguous_state)
    }
}

fn ambiguous_state(message: String) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::AmbiguousState,
        message,
    }
}
