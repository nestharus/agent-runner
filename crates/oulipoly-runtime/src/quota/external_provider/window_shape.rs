//! ## Declared roles
//!
//! `validator`
//! `accessor`

use super::errors::ExternalQuotaError;
use oulipoly_provider::generated::{QuotaProbeResult, QuotaProbeWindow};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidatedQuotaWindow {
    pub(crate) remaining_ratio: f64,
    pub(crate) resets_at_unix_ms: u64,
}

pub(crate) fn validate_probe_windows(
    result: &QuotaProbeResult,
) -> Result<Vec<ValidatedQuotaWindow>, ExternalQuotaError> {
    result.windows.iter().map(validate_probe_window).collect()
}

pub(crate) fn validate_probe_window(
    window: &QuotaProbeWindow,
) -> Result<ValidatedQuotaWindow, ExternalQuotaError> {
    let remaining_ratio = remaining_ratio(window)?;
    Ok(ValidatedQuotaWindow {
        remaining_ratio,
        resets_at_unix_ms: window.resets_at_unix_ms,
    })
}

fn remaining_ratio(window: &QuotaProbeWindow) -> Result<f64, ExternalQuotaError> {
    if (0.0..=1.0).contains(&window.remaining_ratio) {
        return Ok(window.remaining_ratio);
    }
    Err(ExternalQuotaError::projection("remaining_ratio_range"))
}
