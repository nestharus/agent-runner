//! ## Declared roles
//!
//! `mapper`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/external_provider/window_projection.rs
//!     role: intrinsic-surface
//!     Domain: provider_quota_window_projection
//!     Owns:
//!       - project_quota_probe_windows
//!       - QuotaWindowInput
//!       - remaining_ratio to consumed-ratio conversion
//!       - Unix millisecond reset conversion
//! ```

use super::errors::ExternalQuotaError;
use super::window_shape::ValidatedQuotaWindow;
use chrono::{DateTime, Utc};
use oulipoly_state::QuotaWindowInput;
use std::time::{Duration, UNIX_EPOCH};

pub(crate) fn project_quota_probe_windows(
    windows: &[ValidatedQuotaWindow],
) -> Result<Vec<QuotaWindowInput>, ExternalQuotaError> {
    windows.iter().map(quota_window_input).collect()
}

fn quota_window_input(
    window: &ValidatedQuotaWindow,
) -> Result<QuotaWindowInput, ExternalQuotaError> {
    Ok(QuotaWindowInput {
        used_percent: 1.0 - window.remaining_ratio,
        resets_at: resets_at(window.resets_at_unix_ms)?,
    })
}

fn resets_at(unix_ms: u64) -> Result<DateTime<Utc>, ExternalQuotaError> {
    let duration = Duration::from_millis(unix_ms);
    Ok(DateTime::<Utc>::from(UNIX_EPOCH + duration))
}
