//! ## Declared roles
//!
//! `accessor`, `mapper`, `predicate`, `orchestration`.

use oulipoly_config::ModelConfig;
use oulipoly_state::StateDb;
use std::collections::HashMap;

const RECENT_LIVE_LOAD_WINDOW_MINUTES: i64 = 30;

pub(super) fn live_loads_for_model(model: &ModelConfig, state: &StateDb) -> Vec<u64> {
    let provider_names = provider_name_refs(model);
    let counts = live_load_counts(state, provider_names.as_slice(), live_load_since());
    live_load_vector(model, &counts)
}

fn provider_name_refs(model: &ModelConfig) -> Vec<&str> {
    model
        .providers
        .iter()
        .map(|provider| provider.name.as_str())
        .collect()
}

fn live_load_since() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::minutes(RECENT_LIVE_LOAD_WINDOW_MINUTES)
}

fn live_load_counts(
    state: &StateDb,
    provider_names: &[&str],
    since: chrono::DateTime<chrono::Utc>,
) -> HashMap<String, u64> {
    state
        .running_invocation_counts_by_provider(provider_names, since)
        .unwrap_or_else(|error| live_load_count_fallback(error.as_str()))
}

fn live_load_count_fallback(error: &str) -> HashMap<String, u64> {
    tracing::warn!(error, "failed to read routing live-load signal");
    Default::default()
}

fn live_load_vector(model: &ModelConfig, counts: &HashMap<String, u64>) -> Vec<u64> {
    model
        .providers
        .iter()
        .map(|provider| provider_live_load(counts, &provider.name))
        .collect()
}

fn provider_live_load(counts: &HashMap<String, u64>, provider_name: &str) -> u64 {
    counts.get(provider_name).copied().unwrap_or(0)
}

pub(super) fn live_load_at(live_loads: &[u64], provider_index: usize) -> u64 {
    live_loads.get(provider_index).copied().unwrap_or(0)
}

pub(super) fn live_load_then_index_order(
    a_index: usize,
    b_index: usize,
    live_loads: &[u64],
) -> std::cmp::Ordering {
    live_load_order(live_load_pair(a_index, b_index, live_loads))
        .then_with(|| provider_index_order(a_index, b_index))
}

fn live_load_pair(a_index: usize, b_index: usize, live_loads: &[u64]) -> (u64, u64) {
    (
        live_load_at(live_loads, a_index),
        live_load_at(live_loads, b_index),
    )
}

fn live_load_order((a_load, b_load): (u64, u64)) -> std::cmp::Ordering {
    a_load.cmp(&b_load)
}

fn provider_index_order(a_index: usize, b_index: usize) -> std::cmp::Ordering {
    a_index.cmp(&b_index)
}
