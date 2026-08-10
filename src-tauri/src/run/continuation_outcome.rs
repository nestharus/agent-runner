//! ## Declared roles
//!
//! `validator`, `mapper`, `accessor`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_outcome.rs
//!     role: adapter
//!     Translates:
//!       - runtime-continuation-invocation-outcome-contract
//!       - StateDb-invocation-record-contract
//! ```

use oulipoly_runtime::fresh_continuation::{
    InvocationDisposition, InvocationOutcome, ReservedInvocation, ResumeAcceptance,
};
use oulipoly_state::{InvocationRecord, InvocationStatus, StateDb};

use super::reservation::ReservedRun;

struct ValidatedOutcome {
    session_id: Option<String>,
    physical_exit_code: i32,
    acceptance: ResumeAcceptance,
    disposition: InvocationDisposition,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn observe_resume_outcome(
    state: &StateDb,
    reservation: &ReservedInvocation,
    expected_session_id: &str,
) -> Result<InvocationOutcome, String> {
    observe_exact_outcome(state, reservation, |row, _| {
        if row.provider_session_id.as_deref() != Some(expected_session_id) {
            return Err(format!(
                "Reserved resume invocation {} did not capture expected provider session",
                reservation.invocation_id
            ));
        }

        match row.resume_acceptance_status.as_deref() {
            Some("accepted") => Ok(ResumeAcceptance::Accepted),
            Some("rejected") => Ok(ResumeAcceptance::Rejected),
            Some("unconfirmed") => Ok(ResumeAcceptance::Unconfirmed),
            None => Ok(ResumeAcceptance::NotApplicable),
            Some(status) => Err(format!(
                "Reserved resume invocation {} has unknown acceptance status: {status}",
                reservation.invocation_id
            )),
        }
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn observe_fresh_outcome(
    state: &StateDb,
    reservation: &ReservedInvocation,
) -> Result<InvocationOutcome, String> {
    observe_exact_outcome(state, reservation, |row, disposition| {
        if is_successful(disposition) && !has_provider_session(row) {
            return Err(format!(
                "Successful reserved fresh invocation {} has no captured provider session",
                reservation.invocation_id
            ));
        }

        Ok(ResumeAcceptance::NotApplicable)
    })
}

fn is_successful(disposition: &InvocationDisposition) -> bool {
    match disposition {
        InvocationDisposition::Succeeded => true,
        InvocationDisposition::Failed { .. } => false,
    }
}

fn has_provider_session(row: &InvocationRecord) -> bool {
    row.provider_session_id
        .as_deref()
        .is_some_and(|session_id| !session_id.trim().is_empty())
}

fn observe_exact_outcome(
    state: &StateDb,
    reservation: &ReservedInvocation,
    acceptance: impl FnOnce(
        &InvocationRecord,
        &InvocationDisposition,
    ) -> Result<ResumeAcceptance, String>,
) -> Result<InvocationOutcome, String> {
    let reserved = ReservedRun::resolve(state, reservation)?;
    let row = exact_invocation_row(state, &reservation.invocation_id)?;
    let validated = validate_terminal_row(
        row,
        reserved.parent_invocation_row_id(),
        &reservation.invocation_id,
        acceptance,
    )?;
    Ok(invocation_outcome(reserved.invocation_id(), validated))
}

fn exact_invocation_row(state: &StateDb, invocation_id: &str) -> Result<InvocationRecord, String> {
    let row = state.get_invocation_by_uuid(invocation_id)?;
    match row {
        Some(row) => Ok(row),
        None => Err(format!("Reserved invocation not found: {invocation_id}")),
    }
}

fn validate_terminal_row(
    row: InvocationRecord,
    expected_parent_row_id: i64,
    invocation_id: &str,
    acceptance: impl FnOnce(
        &InvocationRecord,
        &InvocationDisposition,
    ) -> Result<ResumeAcceptance, String>,
) -> Result<ValidatedOutcome, String> {
    validate_parent(&row, expected_parent_row_id, invocation_id)?;
    let physical_exit_code = required_exit_code(&row, invocation_id)?;
    let disposition = validate_disposition(&row, invocation_id)?;
    let acceptance = acceptance(&row, &disposition)?;

    Ok(ValidatedOutcome {
        session_id: row.provider_session_id,
        physical_exit_code,
        acceptance,
        disposition,
    })
}

fn validate_parent(
    row: &InvocationRecord,
    expected_parent_row_id: i64,
    invocation_id: &str,
) -> Result<(), String> {
    if row.parent_invocation_id != Some(expected_parent_row_id) {
        return Err(format!(
            "Reserved invocation {invocation_id} is attached to the wrong parent"
        ));
    }
    Ok(())
}

fn required_exit_code(row: &InvocationRecord, invocation_id: &str) -> Result<i32, String> {
    row.exit_code
        .ok_or_else(|| format!("Reserved invocation {invocation_id} has no physical exit code"))
}

fn validate_disposition(
    row: &InvocationRecord,
    invocation_id: &str,
) -> Result<InvocationDisposition, String> {
    match row.status {
        InvocationStatus::Succeeded => {
            validate_successful_terminal(row, invocation_id)?;
            Ok(InvocationDisposition::Succeeded)
        }
        InvocationStatus::Failed => validate_failed_terminal(row, invocation_id),
        InvocationStatus::Running | InvocationStatus::Legacy => Err(format!(
            "Reserved invocation {invocation_id} is not coherently terminal"
        )),
    }
}

fn validate_successful_terminal(row: &InvocationRecord, invocation_id: &str) -> Result<(), String> {
    if row.finished_at.is_none() || row.success != Some(true) {
        return Err(format!(
            "Reserved invocation {invocation_id} has an incoherent successful terminal row"
        ));
    }
    Ok(())
}

fn validate_failed_terminal(
    row: &InvocationRecord,
    invocation_id: &str,
) -> Result<InvocationDisposition, String> {
    if row.finished_at.is_none() || row.success != Some(false) {
        return Err(format!(
            "Reserved invocation {invocation_id} has an incoherent failed terminal row"
        ));
    }

    let error_category = required_terminal_field(
        row.error_category.as_deref(),
        "error category",
        invocation_id,
    )?;
    let terminal_reason = required_terminal_field(
        row.terminal_reason.as_deref(),
        "terminal reason",
        invocation_id,
    )?;
    Ok(InvocationDisposition::Failed {
        error_category: error_category.to_string(),
        terminal_reason: terminal_reason.to_string(),
    })
}

fn invocation_outcome(invocation_id: &str, validated: ValidatedOutcome) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: invocation_id.to_string(),
        session_id: validated.session_id,
        physical_exit_code: validated.physical_exit_code,
        acceptance: validated.acceptance,
        disposition: validated.disposition,
    }
}

fn required_terminal_field<'a>(
    value: Option<&'a str>,
    field: &str,
    invocation_id: &str,
) -> Result<&'a str, String> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Failed reserved invocation {invocation_id} has no {field}"))
}
