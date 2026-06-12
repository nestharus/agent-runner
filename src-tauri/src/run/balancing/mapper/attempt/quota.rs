use super::super::super::disposition::{MaybeQuotaVerifyInput, TypedDispositionInput};
use super::disposition::typed_disposition_input_for_attempt;
use super::shared::{
    AttemptBudgetInput, AttemptLifecycleInput, AttemptProviderInput, AttemptTerminalInput,
};
use crate::zero_turn_orchestration::ZeroTurnAction;

pub(in crate::run::balancing) fn maybe_quota_verify_input<'a, 'state, 'ctx>(
    typed: TypedDispositionInput<'a, 'state, 'ctx>,
    zero_turn_action: ZeroTurnAction,
    pending_same_provider_verification: &'a mut Option<(usize, Option<String>)>,
    signal_already_applied: bool,
) -> MaybeQuotaVerifyInput<'a, 'state, 'ctx> {
    MaybeQuotaVerifyInput {
        typed,
        zero_turn_action,
        pending_same_provider_verification,
        signal_already_applied,
    }
}

pub(in crate::run::balancing) fn maybe_quota_verify_input_for_attempt<'a, 'state, 'ctx>(
    lifecycle: AttemptLifecycleInput<'a, 'state>,
    provider: AttemptProviderInput<'a>,
    terminal: AttemptTerminalInput<'a, 'ctx>,
    budget: AttemptBudgetInput,
    zero_turn_action: ZeroTurnAction,
    pending_same_provider_verification: &'a mut Option<(usize, Option<String>)>,
    signal_already_applied: bool,
) -> MaybeQuotaVerifyInput<'a, 'state, 'ctx> {
    maybe_quota_verify_input(
        typed_disposition_input_for_attempt(lifecycle, provider, terminal, budget),
        zero_turn_action,
        pending_same_provider_verification,
        signal_already_applied,
    )
}
