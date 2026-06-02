//! ## Declared roles
//!
//! `mapper`

use super::error_mapper::failed_outcome;
use super::errors::ExternalQuotaError;
use crate::quota::RefreshOutcome;
use oulipoly_provider::generated::QuotaProbeResult;

pub(crate) fn source_result_without_source() -> RefreshOutcome {
    RefreshOutcome::NoScript
}

pub(crate) fn probe_result_unavailable(_result: &QuotaProbeResult) -> RefreshOutcome {
    failed_outcome(ExternalQuotaError::probe_unavailable())
}
