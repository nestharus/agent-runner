//! ## Declared roles
//!
//! `accessor`, `filter`, `predicate`, `mapper`.

use super::is_resume_migratable_pair;
use crate::balancer::projection::ProviderProjection;
use oulipoly_config::{ModelConfig, ProviderConfig};

pub(super) fn provider_load(projection: &ProviderProjection) -> f64 {
    let max_projected_used = projection
        .projections_per_window
        .iter()
        .map(|window| window.projected_used)
        .fold(f64::NEG_INFINITY, f64::max);
    if max_projected_used.is_finite() {
        max_projected_used
    } else {
        0.0
    }
}

pub(super) fn lowest_load_migration_target<'a>(
    model: &ModelConfig,
    projections: &'a [ProviderProjection],
    source_provider: &ProviderConfig,
    exclude_provider_index: Option<usize>,
) -> Option<&'a ProviderProjection> {
    projections
        .iter()
        .filter(|projection| {
            migration_projection_is_eligible(
                model,
                source_provider,
                exclude_provider_index,
                projection,
            )
        })
        .min_by(|a, b| migration_load_order(a, b))
}

fn migration_projection_is_eligible(
    model: &ModelConfig,
    source_provider: &ProviderConfig,
    exclude_provider_index: Option<usize>,
    projection: &ProviderProjection,
) -> bool {
    Some(projection.provider_index) != exclude_provider_index
        && model
            .providers
            .get(projection.provider_index)
            .is_some_and(|candidate| is_resume_migratable_pair(source_provider, candidate))
}

fn migration_load_order(a: &ProviderProjection, b: &ProviderProjection) -> std::cmp::Ordering {
    provider_load(a)
        .partial_cmp(&provider_load(b))
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.provider_index.cmp(&b.provider_index))
}
