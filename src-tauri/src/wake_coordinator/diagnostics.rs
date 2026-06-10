//! ## Declared roles
//!
//! `formatter`

use oulipoly_state::mailbox::WakeClaimRow;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WakeDiagnostic {
    pub(crate) attempted: bool,
    pub(crate) status: String,
    pub(crate) claim_token: Option<String>,
    pub(crate) wake_pid: Option<i64>,
    pub(crate) auto_wake_count: Option<i64>,
    pub(crate) message: Option<String>,
}

impl WakeDiagnostic {
    pub(super) fn status(status: &str) -> Self {
        Self {
            attempted: false,
            status: status.to_string(),
            claim_token: None,
            wake_pid: None,
            auto_wake_count: None,
            message: None,
        }
    }

    pub(super) fn with_message(status: &str, message: String) -> Self {
        Self {
            attempted: false,
            status: status.to_string(),
            claim_token: None,
            wake_pid: None,
            auto_wake_count: None,
            message: Some(message),
        }
    }
}

pub(super) fn auto_wake_cap_diagnostic(current_count: i64) -> WakeDiagnostic {
    let mut diagnostic = WakeDiagnostic::status("auto_wake_cap_reached");
    diagnostic.auto_wake_count = Some(current_count);
    diagnostic
}

pub(super) fn storage_error_diagnostic(err: String) -> WakeDiagnostic {
    WakeDiagnostic::with_message("storage_error", err)
}

pub(super) fn already_in_flight_diagnostic(claim: WakeClaimRow) -> WakeDiagnostic {
    let mut diagnostic = WakeDiagnostic::status("already_in_flight");
    diagnostic.claim_token = Some(claim.claim_token);
    diagnostic.wake_pid = claim.wake_pid;
    diagnostic.auto_wake_count = Some(claim.auto_wake_count);
    diagnostic
}

pub(super) fn spawned_wake_diagnostic(
    claim_token: String,
    wake_pid: i64,
    auto_wake_count: i64,
) -> WakeDiagnostic {
    WakeDiagnostic {
        attempted: true,
        status: "spawned".to_string(),
        claim_token: Some(claim_token),
        wake_pid: Some(wake_pid),
        auto_wake_count: Some(auto_wake_count),
        message: None,
    }
}

pub(super) fn spawn_error_diagnostic(
    claim_token: String,
    auto_wake_count: i64,
    err: String,
) -> WakeDiagnostic {
    WakeDiagnostic {
        attempted: true,
        status: "spawn_error".to_string(),
        claim_token: Some(claim_token),
        wake_pid: None,
        auto_wake_count: Some(auto_wake_count),
        message: Some(err),
    }
}
