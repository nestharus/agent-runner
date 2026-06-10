//! ## Declared roles
//!
//! `orchestration`, `predicate`, `accessor`.
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/refresh_inputs.rs::contextual_refresh_surface
//!     role: intrinsic-surface
//!     Domain: quota_routing_orchestration
//!     Owns:
//!       - RefreshOutcome ignored/degraded routing result
//!       - is_routing_stale route-selection refresh TTL
//!       - is_stale projection refresh TTL
//!       - refresh_provider projection refresh operation
//!       - refresh_provider_for_routing route-selection refresh operation
//!       - verify_or_clear_marker route-selection marker verification
//!       - scan_provider session scan operation

use super::BalanceContext;
use crate::quota::{
    RefreshOutcome, is_routing_stale, is_stale, refresh_provider, refresh_provider_for_routing,
    verify_or_clear_marker,
};
use crate::sessions::scan_provider;
use chrono::{DateTime, Utc};
use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_state::StateDb;

pub(super) fn refresh_routing_inputs(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) {
    let Some(ctx) = ctx else {
        return;
    };
    let now = Utc::now();
    for provider in &model.providers {
        verify_provider_marker_before_routing(provider, state, ctx, now);
        refresh_provider_for_stale_routing(provider, state, ctx);
        scan_provider_for_routing(provider, state, ctx);
    }
}

fn verify_provider_marker_before_routing(
    provider: &ProviderConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
    now: DateTime<Utc>,
) {
    verify_or_clear_marker(
        state,
        ctx.providers_cfg,
        ctx.sessions_cfg,
        ctx.in_flight,
        &provider.name,
        now,
    );
}

fn refresh_provider_for_stale_routing(
    provider: &ProviderConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
) {
    if is_routing_stale(state, &provider.name) {
        let _: RefreshOutcome = refresh_provider_for_routing(
            &provider.name,
            ctx.providers_cfg,
            ctx.sessions_cfg,
            ctx.in_flight,
            state,
        );
    }
}

fn scan_provider_for_routing(provider: &ProviderConfig, state: &StateDb, ctx: &BalanceContext<'_>) {
    let _ = scan_provider(&provider.name, ctx.sessions_cfg, state);
}

pub(super) fn refresh_projection_inputs(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) {
    let Some(ctx) = ctx else {
        return;
    };
    for provider in &model.providers {
        refresh_provider_for_stale_projection(provider, state, ctx);
        scan_provider_for_projection(provider, state, ctx);
    }
}

fn refresh_provider_for_stale_projection(
    provider: &ProviderConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
) {
    if is_stale(state, &provider.name) {
        let _: RefreshOutcome =
            refresh_provider(&provider.name, ctx.providers_cfg, ctx.in_flight, state);
    }
}

fn scan_provider_for_projection(
    provider: &ProviderConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
) {
    let _ = scan_provider(&provider.name, ctx.sessions_cfg, state);
}
