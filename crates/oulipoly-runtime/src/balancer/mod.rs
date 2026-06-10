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
mod migration;
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
}

impl fmt::Display for RoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoutingError::AllProvidersQuotaExhausted {
                model_name,
                provider_names,
            } => {
                let providers = if provider_names.is_empty() {
                    "<empty>".to_string()
                } else {
                    provider_names.join(", ")
                };
                write!(
                    f,
                    "all providers in pool {model_name} are quota-exhausted: {providers}"
                )
            }
        }
    }
}

impl std::error::Error for RoutingError {}

pub fn select_provider(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) -> Result<usize, RoutingError> {
    refresh_routing_inputs(model, state, ctx);
    let mut snapshot = load_quota_snapshot(model, state);
    if let Some(ctx) = ctx {
        repair_routing_topology(model, state, ctx, &mut snapshot);
    }

    let filtered_indices = eligible_provider_indices(model, state, &snapshot, Utc::now());
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
pub(crate) fn balancer_production_sources() -> [(&'static str, &'static str); 15] {
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
