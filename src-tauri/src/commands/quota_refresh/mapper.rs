//! ## Declared roles
//!
//! `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/quota_refresh/mapper.rs
//!     role: adapter
//!     Translates:
//!       - quota refresh outcome to QuotaRefreshEntry DTO contract
//!       - in-flight status string wire contract
//!       - quota window timestamp string wire contract
//! ```

use super::{QuotaRefreshEntry, QuotaRefreshWindow};
use oulipoly_runtime::quota;
use oulipoly_state::QuotaWindowInput;

const STATUS_FRESH: &str = "fresh";
const STATUS_UPDATED: &str = "updated";
const STATUS_NO_SCRIPT: &str = "no_script";
const STATUS_IN_FLIGHT: &str = "in_flight";
const STATUS_FAILED: &str = "failed";

pub(crate) fn fresh_entry(provider_name: String) -> QuotaRefreshEntry {
    empty_entry(provider_name, STATUS_FRESH, None)
}

pub(crate) fn entry_from_refresh_outcome(
    provider_name: String,
    outcome: quota::RefreshOutcome,
) -> QuotaRefreshEntry {
    match outcome {
        quota::RefreshOutcome::Updated { windows } => updated_entry(provider_name, windows),
        quota::RefreshOutcome::NoScript => no_script_entry(provider_name),
        quota::RefreshOutcome::AlreadyInFlight => in_flight_entry(provider_name),
        quota::RefreshOutcome::Failed(message) => failed_entry(provider_name, message),
    }
}

fn updated_entry(provider_name: String, windows: Vec<QuotaWindowInput>) -> QuotaRefreshEntry {
    QuotaRefreshEntry {
        provider_name,
        status: STATUS_UPDATED.to_string(),
        windows: windows.into_iter().map(quota_window_to_dto).collect(),
        message: None,
    }
}

fn no_script_entry(provider_name: String) -> QuotaRefreshEntry {
    empty_entry(provider_name, STATUS_NO_SCRIPT, None)
}

fn in_flight_entry(provider_name: String) -> QuotaRefreshEntry {
    empty_entry(provider_name, STATUS_IN_FLIGHT, None)
}

fn failed_entry(provider_name: String, message: String) -> QuotaRefreshEntry {
    empty_entry(provider_name, STATUS_FAILED, Some(message))
}

fn empty_entry(
    provider_name: String,
    status: &'static str,
    message: Option<String>,
) -> QuotaRefreshEntry {
    QuotaRefreshEntry {
        provider_name,
        status: status.to_string(),
        windows: Vec::new(),
        message,
    }
}

fn quota_window_to_dto(window: QuotaWindowInput) -> QuotaRefreshWindow {
    QuotaRefreshWindow {
        used_percent: window.used_percent,
        resets_at: window.resets_at.to_rfc3339(),
    }
}
