//! ## Declared roles
//!
//! `accessor`, `mapper`, `predicate`.

use oulipoly_config::ModelConfig;
use oulipoly_state::StateDb;

const RECENT_LIVE_LOAD_WINDOW_MINUTES: i64 = 30;

pub(super) fn live_loads_for_model(model: &ModelConfig, state: &StateDb) -> Vec<u64> {
    let provider_names = model
        .providers
        .iter()
        .map(|provider| provider.name.as_str())
        .collect::<Vec<_>>();
    let since = chrono::Utc::now() - chrono::Duration::minutes(RECENT_LIVE_LOAD_WINDOW_MINUTES);
    let counts = state
        .running_invocation_counts_by_provider(provider_names.as_slice(), since)
        .unwrap_or_else(|error| {
            tracing::warn!(
                error = error.as_str(),
                "failed to read routing live-load signal"
            );
            Default::default()
        });
    model
        .providers
        .iter()
        .map(|provider| counts.get(&provider.name).copied().unwrap_or(0))
        .collect()
}

pub(super) fn live_load_at(live_loads: &[u64], provider_index: usize) -> u64 {
    live_loads.get(provider_index).copied().unwrap_or(0)
}

pub(super) fn live_load_then_index_order(
    a_index: usize,
    b_index: usize,
    live_loads: &[u64],
) -> std::cmp::Ordering {
    live_load_at(live_loads, a_index)
        .cmp(&live_load_at(live_loads, b_index))
        .then_with(|| a_index.cmp(&b_index))
}
