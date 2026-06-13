//! ## Declared roles
//!
//! - accessor
//! - filter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, filter, mapper, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/live_load.rs
//!     role: intrinsic-surface
//!     Domain: balancer-live-load-signal
//!     Owns:
//!       - the live per-account session-load routing signal: the count of
//!         recently-started, not-yet-finalized invocations per candidate that
//!         anti-storm rotation consumes to spread a concurrent dispatch burst
//!       - intrinsic routing-input carriers subordinate to this domain:
//!         oulipoly_config::ModelConfig (its `name` and `providers[].name`
//!         routing-identity fields) and oulipoly_state::StateDb (its
//!         running_invocation_count live-load query)
//! ```
//!
//! Anti-storm live-load restriction: among already-eligible routing candidates,
//! keep only those carrying the fewest live (recently-started, not-yet-finalized)
//! invocations so a concurrent dispatch burst spreads across accounts instead of
//! storming the single account that scores cheapest by history or quota. When all
//! candidates carry equal live load — including the common zero-load idle and
//! unit-test case — every candidate is retained and downstream density/invocation
//! scoring is unchanged.

use oulipoly_config::ModelConfig;
use oulipoly_state::StateDb;

/// Live-load lookback window. A `running` invocation older than this is treated
/// as dead (crashed without finalize), not live load, so a stale row cannot
/// permanently inflate an account's apparent load and exclude it from routing.
const LIVE_LOAD_WINDOW_MINUTES: i64 = 60;

pub(super) fn restrict_to_least_loaded(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
) -> Vec<usize> {
    let loads = candidate_live_loads(model, state, candidates);
    let min_load = minimum_live_load(loads.as_slice());
    retain_candidates_at_load(loads.as_slice(), min_load)
}

fn candidate_live_loads(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
) -> Vec<(usize, i64)> {
    candidates
        .iter()
        .map(|&provider_index| (provider_index, provider_live_load(model, state, provider_index)))
        .collect()
}

fn provider_live_load(model: &ModelConfig, state: &StateDb, provider_index: usize) -> i64 {
    state
        .running_invocation_count(
            &model.name,
            &model.providers[provider_index].name,
            LIVE_LOAD_WINDOW_MINUTES,
        )
        .unwrap_or(0)
}

fn minimum_live_load(loads: &[(usize, i64)]) -> i64 {
    loads.iter().map(|(_, load)| *load).min().unwrap_or(0)
}

fn retain_candidates_at_load(loads: &[(usize, i64)], target_load: i64) -> Vec<usize> {
    candidate_indices(candidates_at_load(loads, target_load).as_slice())
}

fn candidates_at_load(loads: &[(usize, i64)], target_load: i64) -> Vec<(usize, i64)> {
    loads
        .iter()
        .filter(|(_, load)| *load == target_load)
        .copied()
        .collect()
}

fn candidate_indices(loads: &[(usize, i64)]) -> Vec<usize> {
    loads
        .iter()
        .map(|(provider_index, _)| *provider_index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loads(pairs: &[(usize, i64)]) -> Vec<(usize, i64)> {
        pairs.to_vec()
    }

    #[test]
    fn minimum_live_load_of_empty_is_zero() {
        assert_eq!(minimum_live_load(&[]), 0);
    }

    #[test]
    fn retains_only_candidates_at_minimum_load() {
        let l = loads(&[(0, 3), (1, 1), (2, 1), (3, 5)]);
        let min = minimum_live_load(l.as_slice());
        assert_eq!(min, 1);
        assert_eq!(retain_candidates_at_load(l.as_slice(), min), vec![1, 2]);
    }

    #[test]
    fn equal_load_retains_every_candidate() {
        let l = loads(&[(0, 0), (1, 0), (2, 0)]);
        let min = minimum_live_load(l.as_slice());
        assert_eq!(retain_candidates_at_load(l.as_slice(), min), vec![0, 1, 2]);
    }
}
