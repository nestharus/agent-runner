//! ## Declared roles
//!
//! `predicate`, `mapper`, `orchestration`, `accessor`.
//!
//! ## Component declared roles
//! component_declared_roles:
//!   component: crates/oulipoly-runtime/src/balancer/topology.rs
//!   roles:
//!     - predicate
//!     - mapper
//!     - orchestration
//!     - accessor

use super::BalanceContext;
use super::projection::live_window_count;
use super::snapshot::{QuotaSnapshot, cached_quota_record, cached_quota_windows};
use crate::quota::{
    RefreshOutcome, has_refresh_source, is_topology_probe_due, refresh_provider_for_routing,
};
use chrono::{DateTime, Utc};
use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};

struct TopologyProbeRefresh {
    provider_index: usize,
    live_window_count: usize,
    pool_expected_live_windows: usize,
    topology_peak_live_window_count: usize,
}

pub(super) fn repair_routing_topology(
    model: &ModelConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
    snapshot: &mut QuotaSnapshot,
) {
    let now = Utc::now();
    let live_window_counts = live_window_counts(snapshot.windows.as_slice(), now);
    let topology_peak_counts = topology_peak_counts(snapshot.quotas.as_slice());
    let pool_expected_live_windows =
        expected_pool_live_window_count(&live_window_counts, &topology_peak_counts);

    for (provider_index, provider) in model.providers.iter().enumerate() {
        if topology_probe_should_refresh(
            state,
            provider,
            ctx,
            live_window_counts[provider_index],
            pool_expected_live_windows,
        ) {
            let probe = TopologyProbeRefresh {
                provider_index,
                live_window_count: live_window_counts[provider_index],
                pool_expected_live_windows,
                topology_peak_live_window_count: topology_peak_counts[provider_index],
            };
            record_topology_probe_and_refresh(model, state, ctx, snapshot, probe);
        }
    }
}

fn live_window_counts(windows: &[Vec<QuotaWindow>], now: DateTime<Utc>) -> Vec<usize> {
    windows
        .iter()
        .map(|provider_windows| live_window_count(provider_windows, now))
        .collect()
}

fn topology_peak_counts(quotas: &[Option<QuotaRecord>]) -> Vec<usize> {
    quotas
        .iter()
        .map(|quota| {
            quota
                .as_ref()
                .map(|quota| quota.topology_peak_live_window_count)
                .unwrap_or(0)
        })
        .collect()
}

fn expected_pool_live_window_count(
    live_window_counts: &[usize],
    topology_peak_counts: &[usize],
) -> usize {
    live_window_counts
        .iter()
        .zip(topology_peak_counts.iter())
        .map(|(live, peak)| (*live).max(*peak))
        .max()
        .unwrap_or(0)
}

fn topology_probe_should_refresh(
    state: &StateDb,
    provider: &ProviderConfig,
    ctx: &BalanceContext<'_>,
    live_window_count: usize,
    pool_expected_live_windows: usize,
) -> bool {
    has_refresh_source(&provider.name, ctx.providers_cfg, ctx.sessions_cfg)
        && is_topology_probe_due(
            state,
            &provider.name,
            live_window_count,
            pool_expected_live_windows,
        )
}

fn record_topology_probe_and_refresh(
    model: &ModelConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
    snapshot: &mut QuotaSnapshot,
    probe: TopologyProbeRefresh,
) {
    let provider = &model.providers[probe.provider_index];
    let _ = state.record_topology_probe(&provider.name);
    trace_topology_probe(
        provider,
        probe.live_window_count,
        probe.pool_expected_live_windows,
        probe.topology_peak_live_window_count,
    );
    let _: RefreshOutcome = refresh_provider_for_routing(
        &provider.name,
        ctx.providers_cfg,
        ctx.sessions_cfg,
        ctx.in_flight,
        state,
    );
    snapshot.quotas[probe.provider_index] = cached_quota_record(state, &provider.name);
    snapshot.windows[probe.provider_index] = cached_quota_windows(state, &provider.name);
}

fn trace_topology_probe(
    provider: &ProviderConfig,
    live_window_count: usize,
    pool_expected_live_windows: usize,
    topology_peak_live_window_count: usize,
) {
    tracing::info!(
        provider_name = provider.name.as_str(),
        live_window_count = live_window_count,
        pool_expected_live_window_count = pool_expected_live_windows,
        topology_peak_live_window_count = topology_peak_live_window_count,
        "topology probe fired"
    );
}
