use oulipoly_runtime::fresh_continuation::{
    InvocationDisposition, InvocationOutcome, ReservedInvocation, ResumeAcceptance,
};
use oulipoly_state::{InvocationRecord, InvocationStatus, StateDb};

use super::reservation::ReservedRun;

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
        if matches!(disposition, InvocationDisposition::Succeeded)
            && row
                .provider_session_id
                .as_deref()
                .is_none_or(|session_id| session_id.trim().is_empty())
        {
            return Err(format!(
                "Successful reserved fresh invocation {} has no captured provider session",
                reservation.invocation_id
            ));
        }

        Ok(ResumeAcceptance::NotApplicable)
    })
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
    let row = state
        .get_invocation_by_uuid(&reservation.invocation_id)?
        .ok_or_else(|| {
            format!(
                "Reserved invocation not found: {}",
                reservation.invocation_id
            )
        })?;

    if row.parent_invocation_id != Some(reserved.parent_invocation_row_id()) {
        return Err(format!(
            "Reserved invocation {} is attached to the wrong parent",
            reservation.invocation_id
        ));
    }

    let physical_exit_code = row.exit_code.ok_or_else(|| {
        format!(
            "Reserved invocation {} has no physical exit code",
            reservation.invocation_id
        )
    })?;
    let disposition = match row.status {
        InvocationStatus::Succeeded => {
            if row.finished_at.is_none() || row.success != Some(true) {
                return Err(format!(
                    "Reserved invocation {} has an incoherent successful terminal row",
                    reservation.invocation_id
                ));
            }
            InvocationDisposition::Succeeded
        }
        InvocationStatus::Failed => {
            if row.finished_at.is_none() || row.success != Some(false) {
                return Err(format!(
                    "Reserved invocation {} has an incoherent failed terminal row",
                    reservation.invocation_id
                ));
            }
            InvocationDisposition::Failed {
                error_category: required_terminal_field(
                    row.error_category.as_deref(),
                    "error category",
                    &reservation.invocation_id,
                )?
                .to_string(),
                terminal_reason: required_terminal_field(
                    row.terminal_reason.as_deref(),
                    "terminal reason",
                    &reservation.invocation_id,
                )?
                .to_string(),
            }
        }
        InvocationStatus::Running | InvocationStatus::Legacy => {
            return Err(format!(
                "Reserved invocation {} is not coherently terminal",
                reservation.invocation_id
            ));
        }
    };
    let acceptance = acceptance(&row, &disposition)?;

    Ok(InvocationOutcome {
        invocation_id: reserved.invocation_id().to_string(),
        session_id: row.provider_session_id,
        physical_exit_code,
        acceptance,
        disposition,
    })
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
