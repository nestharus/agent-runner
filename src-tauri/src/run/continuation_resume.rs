//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_resume.rs
//!     role: adapter
//!     Translates:
//!       - runtime-resume-runner-port-contract
//!       - exact-StateDb-invocation-observation-contract
//!       - reserved-resume-execution-callback-contract
//! ```

use oulipoly_runtime::fresh_continuation::{
    ContinuationBlock, ContinuationBlockKind, InvocationAction, InvocationOutcome,
    ReservedInvocation, ResumeAcceptance, ResumeRunner, ValidatedContinuation,
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
    Execute: FnMut(&ReservedRun, &ValidatedContinuation) -> Result<bool, ContinuationBlock>,
{
    fn run_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        validate_resume_parent(reservation, context)?;
        self.execute_or_observe(action, reservation, context)
    }
}

impl<Execute> ContinuationResumeRunner<'_, Execute>
where
    Execute: FnMut(&ReservedRun, &ValidatedContinuation) -> Result<bool, ContinuationBlock>,
{
    fn execute_or_observe(
        &mut self,
        action: InvocationAction,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        if action == InvocationAction::Observe {
            return self.observe(reservation, context);
        }

        let reserved = ReservedRun::resolve(self.state, reservation).map_err(ambiguous_state)?;
        let execution = (self.execute)(&reserved, context);

        match (execution, self.observe(reservation, context)) {
            (Ok(provider_prompt_accepted), Ok(mut outcome)) => {
                if provider_prompt_accepted && outcome.acceptance == ResumeAcceptance::NotApplicable
                {
                    outcome.acceptance = ResumeAcceptance::Accepted;
                }
                Ok(outcome)
            }
            (Err(_), Ok(outcome)) => Ok(outcome),
            (Err(error), Err(_)) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    fn observe(
        &self,
        reservation: &ReservedInvocation,
        context: &ValidatedContinuation,
    ) -> Result<InvocationOutcome, ContinuationBlock> {
        continuation_outcome::observe_resume_outcome(
            self.state,
            reservation,
            &context.request().origin_session_id,
        )
        .map_err(ambiguous_state)
    }
}

fn validate_resume_parent(
    reservation: &ReservedInvocation,
    context: &ValidatedContinuation,
) -> Result<(), ContinuationBlock> {
    if reservation.parent_invocation_id != context.request().origin_invocation_id {
        return Err(ContinuationBlock {
            kind: ContinuationBlockKind::Conflict,
            message: "Reserved resume parent does not match the continuation origin".to_string(),
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
