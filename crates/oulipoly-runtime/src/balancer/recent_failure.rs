//! ## Declared roles
//!
//! - accessor
//! - filter
//! - predicate
//! - orchestration
//!
//! Role set: { accessor, filter, predicate, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/recent_failure.rs
//!     role: intrinsic-surface
//!     Domain: balancer-recent-failure-working-set
//!     Owns:
//!       - the pre-live-load recent-failure working set that prevents an idle,
//!         repeatedly failing provider from monopolizing concurrent dispatch
//!       - the all-suppressed liveness fallback that retains the original pool
//!         when no candidate is below the established recent-error threshold
//!       - intrinsic routing-input carriers subordinate to this domain:
//!         oulipoly_config::ModelConfig and oulipoly_state::StateDb
//! ```

use super::{ERROR_THRESHOLD, ERROR_WINDOW_MINUTES};
use oulipoly_config::ModelConfig;
use oulipoly_state::StateDb;

pub(super) fn restrict_to_recent_failure_working_set(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
) -> Vec<usize> {
    let available: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|provider_index| !provider_is_suppressed(model, state, *provider_index))
        .collect();

    if available.is_empty() {
        candidates.to_vec()
    } else {
        available
    }
}

fn provider_is_suppressed(model: &ModelConfig, state: &StateDb, provider_index: usize) -> bool {
    recent_error_count(model, state, provider_index) >= ERROR_THRESHOLD as i64
}

fn recent_error_count(model: &ModelConfig, state: &StateDb, provider_index: usize) -> i64 {
    state
        .recent_error_count(
            &model.name,
            &model.providers[provider_index].name,
            ERROR_WINDOW_MINUTES,
        )
        .unwrap_or(0)
}
