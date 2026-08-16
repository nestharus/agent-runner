//! Retained provider-outcome finalization policy shared by command carriers.
//!
//! ## Declared roles
//! `orchestration`

use super::{
    InvocationLifecycleFinalizeOutput, InvocationLifecycleFinalizeRequest,
    InvocationLifecycleServicePort, ServiceError,
};

pub const SUCCESS_FINALIZE_MAX_ATTEMPTS: usize = 3;
const SUCCESS_FINALIZE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

pub fn finalize_retained_outcome_with_contention_retry(
    service: &dyn InvocationLifecycleServicePort,
    request: InvocationLifecycleFinalizeRequest<'_>,
) -> Result<InvocationLifecycleFinalizeOutput, ServiceError> {
    let InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success,
        exit_code,
        error_category,
        terminal_reason,
    } = request;
    let attempts = if success {
        SUCCESS_FINALIZE_MAX_ATTEMPTS
    } else {
        1
    };
    for attempt in 1..=attempts {
        let result = service.finalize_invocation(InvocationLifecycleFinalizeRequest {
            state,
            invocation_row_id,
            success,
            exit_code,
            error_category,
            terminal_reason,
        });
        match result {
            Err(ServiceError::Contention { .. }) if attempt < attempts => {
                std::thread::sleep(SUCCESS_FINALIZE_RETRY_DELAY);
            }
            result => return result,
        }
    }
    unreachable!("the bounded finalization retry loop always returns")
}
