//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_fresh.rs
//!     role: adapter
//!     Translates:
//!       - runtime-fresh-runner-port-contract
//!       - exact-StateDb-invocation-observation-contract
//!       - reserved-fresh-execution-callback-contract
//! ```

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
        validate_fresh_parent(reservation, resume)?;
        self.execute_or_observe(action, reservation, context, resume)
    }
}

impl<Execute> ContinuationFreshRunner<'_, Execute>
where
    Execute: FnMut(
        &ReservedRun,
        &ValidatedContinuation,
        &InvocationOutcome,
    ) -> Result<(), ContinuationBlock>,
{
    fn execute_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
        resume: &InvocationOutcome,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
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

    fn observe(
        &self,
        reservation: &ReservedInvocation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        continuation_outcome::observe_fresh_outcome(self.state, reservation)
            .map_err(ambiguous_state)
    }
}

fn validate_fresh_parent(
    reservation: &ReservedInvocation,
    resume: &InvocationOutcome,
) -> Result<(), ContinuationBlock> {
    if reservation.parent_invocation_id != resume.invocation_id {
        return Err(ContinuationBlock {
            kind: ContinuationBlockKind::Conflict,
            message: "Reserved fresh parent does not match the resume invocation".to_string(),
        });
    }
    Ok(())
}

fn ambiguous_state(message: String) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::AmbiguousState,
        message,
    }
}
