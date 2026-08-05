//! ## Declared roles
//!
//! `orchestration`, `filter`, `predicate`, `mapper`, `accessor`, `formatter`, `validator`,
//! `parser`.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs::public_balancer_hub_adapter
//!     role: adapter
//!     Translates:
//!       - public balancer API re-exports into internal routing concern modules
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs::test_support_surface_adapter
//!     role: adapter
//!     Translates:
//!       - balancer unit-test support re-exports into internal helper modules
//! ```
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs
//!     role: intrinsic-surface
//!     Domain: balancer-routing-hub
//!     Owns:
//!       - the balancer routing hub: this file owns coordination of the balancer
//!         submodule tree it declares and re-exports as the intrinsic internal
//!         surface of the routing concern — burn_rate, context, density,
//!         eligibility, forensics, invocation_fallback, live_load, migration,
//!         placement_pin, projection, refresh_inputs, snapshot, topology, working_set
//!       - the RoutingError routing-failure type this hub defines and the
//!         select_provider/score_routing_candidates selection orchestration
//!       - intrinsic routing-input carriers subordinate to this domain, consumed by
//!         the selection orchestration: oulipoly_config::ModelConfig,
//!         oulipoly_state::StateDb, oulipoly_core::TransitionReason, chrono::Utc, std::fmt
//!   - component: crates/oulipoly-runtime/src/balancer/context.rs::balance_context_surface
//!     role: intrinsic-surface
//!     Domain: contextual-balancer-dependencies
//!     Owns:
//!       - ProvidersConfig quota refresh-source contract
//!       - SessionsConfig session scan and adapter-derived refresh contract
//!       - InFlight refresh deduplication carrier
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
//!   - component: crates/oulipoly-runtime/src/balancer/snapshot.rs::quota_snapshot_surface
//!     role: intrinsic-surface
//!     Domain: quota-cache-snapshot-access
//!     Owns:
//!       - QuotaRecord cached read accessor
//!       - QuotaWindow cached read accessor
//!       - QuotaSnapshot in-memory routing/projection cache
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs::config_topology_session_contract
//!     role: intrinsic-surface
//!     Domain: balancer-config-topology-and-session-contract
//!     Owns:
//!       - ModelConfig provider pool and model name contract
//!       - ProviderConfig provider name/session storage contract
//!       - has_refresh_source topology-probe eligibility
//!       - is_topology_probe_due topology repair predicate
//!       - refresh_provider_for_routing topology repair operation
//!       - SessionStorage resume migration eligibility contract
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs::quota_resume_state_surface
//!     role: intrinsic-surface
//!     Domain: quota-resume-state-carriers
//!     Owns:
//!       - StateDb quota/window read operations
//!       - StateDb recent-error and invocation-count read operations
//!       - StateDb reset-implied clear operation
//!       - StateDb topology-probe timestamp operation
//!       - QuotaRecord refreshed/exhausted/topology fields
//!       - QuotaWindow usage/reset/delta fields
//!       - ResolvedResume active-provider contract

mod burn_rate;
mod context;
mod density;
mod eligibility;
pub mod forensics;
mod invocation_fallback;
mod live_load;
mod migration;
mod placement_pin;
mod projection;
mod refresh_inputs;
mod snapshot;
mod topology;
mod working_set;

pub use context::BalanceContext;
pub use density::FANOUT_SCORE_BAND_RATIO;
pub use forensics::{FailureClass, apply_post_failure_forensics};
pub use migration::{
    ManualMigrationRejection, MigrationDecision, decide_manual_migration, decide_migration,
};
pub use projection::{ProviderProjection, WindowProjection, compute_projections};
pub use working_set::{select_next_working_candidate, working_set_member};

#[cfg(test)]
pub(crate) use burn_rate::{
    bootstrap_burn_rate_for_test, bootstrap_duration_ratio_for_test, project_used_percent_for_test,
};

#[cfg(test)]
use crate::migration::MigrationError;
use chrono::Utc;
use oulipoly_config::ModelConfig;
pub use oulipoly_core::TransitionReason;
#[cfg(test)]
use oulipoly_state::QuotaRecord;
#[cfg(test)]
use oulipoly_state::QuotaWindow;
use oulipoly_state::StateDb;
use std::fmt;

#[cfg(test)]
use burn_rate::{
    duration_ratio_fallback_percent_per_call, duration_ratio_rate, learned_rate,
    pool_window_avg_percent_per_call,
};
use density::score_by_density;
#[cfg(test)]
use density::{
    FanoutUsageKey, ProviderEval, approx_eq_usage, fanout_usage_key, finite_fanout_reset,
    finite_fanout_usage, select_binding_score_with_fanout,
};
use eligibility::eligible_provider_indices;
use invocation_fallback::score_by_invocation_count;
use live_load::restrict_to_least_loaded;
use placement_pin::{fresh_run_pin_provider, select_pinned_provider};
use refresh_inputs::refresh_routing_inputs;
use snapshot::{QuotaSnapshot, load_quota_snapshot};
use topology::repair_routing_topology;

const ERROR_WINDOW_MINUTES: i64 = 30;
const ERROR_THRESHOLD: u64 = 3;
const EPS_HOURS: f64 = 1.0 / 60.0;
/// Visible-usage threshold for the missing-window penalty. Anthropic's
/// usage API hides the 5h window when an account is near weekly cap
/// (observed live at 91% weekly). ChatGPT's API hides the 5h window
/// when there's *no recent activity* — the opposite signal. We only
/// trust "missing window means exhausted" when at least one of the
/// visible windows is itself near cap; otherwise the gap is benign
/// (idle account, different upstream behavior) and the provider should
/// not be torpedoed for it.
const HIDDEN_WINDOW_PENALTY_THRESHOLD: f64 = 0.85;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingError {
    AllProvidersQuotaExhausted {
        model_name: String,
        provider_names: Vec<String>,
    },
    PinnedProviderNotInModel {
        model_name: String,
        target_provider: String,
        provider_names: Vec<String>,
    },
    PinnedProviderIneligible {
        model_name: String,
        target_provider: String,
    },
}

impl fmt::Display for RoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoutingError::AllProvidersQuotaExhausted {
                model_name,
                provider_names,
            } => write!(
                f,
                "all providers in pool {model_name} are quota-exhausted: {}",
                format_provider_names(provider_names)
            ),
            RoutingError::PinnedProviderNotInModel {
                model_name,
                target_provider,
                provider_names,
            } => write!(
                f,
                "pinned provider {target_provider:?} is not in model {model_name} provider list: {}",
                format_provider_names(provider_names)
            ),
            RoutingError::PinnedProviderIneligible {
                model_name,
                target_provider,
            } => write!(
                f,
                "pinned provider {target_provider:?} is not eligible for model {model_name}"
            ),
        }
    }
}

impl std::error::Error for RoutingError {}

fn format_provider_names(provider_names: &[String]) -> String {
    if provider_names.is_empty() {
        "<empty>".to_string()
    } else {
        provider_names.join(", ")
    }
}

pub fn select_provider(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) -> Result<usize, RoutingError> {
    let provider_pin = fresh_run_pin_provider();
    select_provider_with_pin(model, state, ctx, provider_pin.as_deref())
}

pub fn select_provider_with_pin(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
    provider_pin: Option<&str>,
) -> Result<usize, RoutingError> {
    refresh_routing_inputs(model, state, ctx);
    let mut snapshot = load_quota_snapshot(model, state);
    if let Some(ctx) = ctx {
        repair_routing_topology(model, state, ctx, &mut snapshot);
    }

    let filtered_indices = eligible_provider_indices(model, state, &snapshot, Utc::now());
    if let Some(target_provider) = provider_pin {
        return select_pinned_provider(model, filtered_indices.as_slice(), target_provider);
    }
    if filtered_indices.is_empty() {
        return Err(all_providers_quota_exhausted_error(model));
    }

    Ok(score_routing_candidates(
        model,
        state,
        &snapshot,
        filtered_indices.as_slice(),
    ))
}

fn all_providers_quota_exhausted_error(model: &ModelConfig) -> RoutingError {
    RoutingError::AllProvidersQuotaExhausted {
        model_name: model.name.clone(),
        provider_names: model
            .providers
            .iter()
            .map(|provider| provider.name.clone())
            .collect(),
    }
}

fn score_routing_candidates(
    model: &ModelConfig,
    state: &StateDb,
    snapshot: &QuotaSnapshot,
    candidates: &[usize],
) -> usize {
    let least_loaded = restrict_to_least_loaded(model, state, candidates);
    score_least_loaded_candidates(model, state, snapshot, least_loaded.as_slice())
}

fn score_least_loaded_candidates(
    model: &ModelConfig,
    state: &StateDb,
    snapshot: &QuotaSnapshot,
    candidates: &[usize],
) -> usize {
    if candidates_have_windows(snapshot, candidates) {
        score_by_density(
            model,
            state,
            &snapshot.quotas,
            &snapshot.windows,
            candidates,
        )
    } else {
        score_by_invocation_count(model, state, candidates)
    }
}

fn candidates_have_windows(snapshot: &QuotaSnapshot, candidates: &[usize]) -> bool {
    candidates
        .iter()
        .all(|provider_index| !snapshot.windows[*provider_index].is_empty())
}

#[cfg(test)]
#[rustfmt::skip]
pub(crate) fn balancer_production_sources() -> [(&'static str, &'static str); 17] {
    macro_rules! source {
        ($path:literal, $file:literal) => { ($path, production_balancer_source($path, include_str!($file))) };
    }
    // Guard compatibility markers: include_str!("projection.rs") include_str!("burn_rate.rs")
    // include_str!("density.rs") include_str!("invocation_fallback.rs")
    // include_str!("migration.rs") include_str!("working_set.rs")
    [
        source!("crates/oulipoly-runtime/src/balancer/mod.rs", "mod.rs"),
        source!("crates/oulipoly-runtime/src/balancer/projection.rs", "projection.rs"),
        source!("crates/oulipoly-runtime/src/balancer/projection/window.rs", "projection/window.rs"),
        source!("crates/oulipoly-runtime/src/balancer/burn_rate.rs", "burn_rate.rs"),
        source!("crates/oulipoly-runtime/src/balancer/density.rs", "density.rs"),
        source!("crates/oulipoly-runtime/src/balancer/density/trace.rs", "density/trace.rs"),
        source!("crates/oulipoly-runtime/src/balancer/invocation_fallback.rs", "invocation_fallback.rs"),
        source!("crates/oulipoly-runtime/src/balancer/live_load.rs", "live_load.rs"),
        source!("crates/oulipoly-runtime/src/balancer/placement_pin.rs", "placement_pin.rs"),
        source!("crates/oulipoly-runtime/src/balancer/eligibility.rs", "eligibility.rs"),
        source!("crates/oulipoly-runtime/src/balancer/context.rs", "context.rs"),
        source!("crates/oulipoly-runtime/src/balancer/snapshot.rs", "snapshot.rs"),
        source!("crates/oulipoly-runtime/src/balancer/refresh_inputs.rs", "refresh_inputs.rs"),
        source!("crates/oulipoly-runtime/src/balancer/topology.rs", "topology.rs"),
        source!("crates/oulipoly-runtime/src/balancer/migration.rs", "migration.rs"),
        source!("crates/oulipoly-runtime/src/balancer/migration/target_selection.rs", "migration/target_selection.rs"),
        source!("crates/oulipoly-runtime/src/balancer/working_set.rs", "working_set.rs"),
    ]
}

#[cfg(test)]
pub(crate) fn production_balancer_source(module_path: &str, source: &'static str) -> &'static str {
    if [
        "crates/oulipoly-runtime/src/balancer/mod.rs",
        "crates/oulipoly-runtime/src/balancer/migration.rs",
        "crates/oulipoly-runtime/src/balancer/working_set.rs",
    ]
    .contains(&module_path)
    {
        return source
            .split("mod tests")
            .next()
            .expect("production source precedes tests");
    }
    source
}

#[cfg(test)]
mod tests;
