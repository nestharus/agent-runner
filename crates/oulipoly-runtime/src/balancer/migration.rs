//! ## Declared roles
//!
//! `orchestration`, `predicate`, `mapper`, `accessor`, `validator`.

mod target_selection;

use super::projection::{ProviderProjection, compute_projections};
use crate::migration::{MigrationError, provider_has_derivable_claude_projects_dir};
use chrono::Utc;
use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_core::TransitionReason;
use oulipoly_state::{QuotaRecord, ResolvedResume, StateDb};
use target_selection::{lowest_load_migration_target, provider_load};

#[derive(Debug, Clone, PartialEq)]
pub enum MigrationDecision {
    Stay,
    Migrate {
        target_provider_index: usize,
        reason: TransitionReason,
    },
}

pub fn decide_migration(
    state: &StateDb,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    manual_target: Option<&str>,
) -> Result<MigrationDecision, MigrationError> {
    // AGE-163 WU-A.5: the seam's manual-rotate path now consults
    // `decide_manual_migration` directly (see
    // `services::migration::migrate`); when this function is invoked with
    // `manual_target.is_some()`, it falls back to the legacy
    // `manual_migration_decision` so non-seam callers (e.g. existing tests)
    // retain pre-WU-A.5 behavior. The seam path is the one that surfaces
    // typed `ManualMigrationRejection` to the operator.
    if !model_has_migration_alternative(model) {
        return Ok(MigrationDecision::Stay);
    }

    let Some(active_provider_index) = active_provider_index(model, resolved) else {
        return Ok(MigrationDecision::Stay);
    };
    let active = &model.providers[active_provider_index];

    if let Some(decision) = manual_migration_decision(model, active, manual_target) {
        return Ok(decision);
    }

    if !active_provider_supports_resume_migration(active) {
        return Ok(MigrationDecision::Stay);
    }

    let active_exhausted = active_provider_is_exhausted(state, active)?;
    let projections = compute_projections(model, state, None);

    if active_exhausted {
        return Ok(exhausted_migration_decision(
            model,
            &projections,
            active,
            active_provider_index,
        ));
    }

    Ok(quota_threshold_migration_decision(
        model,
        &projections,
        active,
        active_provider_index,
    ))
}

/// AGE-163 WU-A.5 typed manual-rotate decision. Translates the three
/// pre-existing silent-`Stay` fall-throughs into named rejection variants
/// the caller can render to the operator instead of silently dispatching
/// the original bound provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualMigrationRejection {
    SingleProviderPool { provider: String },
    ActiveProviderNotInPool { active: String },
    TargetNotInPool { target: String, pool: Vec<String> },
    NotMigratablePair { source: String, target: String },
}

pub fn decide_manual_migration(
    model: &ModelConfig,
    resolved: &ResolvedResume,
    manual_target: &str,
) -> Result<MigrationDecision, ManualMigrationRejection> {
    if !model_has_migration_alternative(model) {
        let provider = model
            .providers
            .first()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| resolved.active_provider.clone());
        return Err(ManualMigrationRejection::SingleProviderPool { provider });
    }
    let Some(active_index) = active_provider_index(model, resolved) else {
        return Err(ManualMigrationRejection::ActiveProviderNotInPool {
            active: resolved.active_provider.clone(),
        });
    };
    let active = &model.providers[active_index];
    let Some(target_index) = model
        .providers
        .iter()
        .position(|provider| provider.name == manual_target)
    else {
        return Err(ManualMigrationRejection::TargetNotInPool {
            target: manual_target.to_string(),
            pool: model.providers.iter().map(|p| p.name.clone()).collect(),
        });
    };
    if !is_resume_migratable_pair(active, &model.providers[target_index]) {
        return Err(ManualMigrationRejection::NotMigratablePair {
            source: active.name.clone(),
            target: manual_target.to_string(),
        });
    }
    Ok(MigrationDecision::Migrate {
        target_provider_index: target_index,
        reason: TransitionReason::Manual,
    })
}

fn model_has_migration_alternative(model: &ModelConfig) -> bool {
    model.providers.len() > 1
}

fn active_provider_index(model: &ModelConfig, resolved: &ResolvedResume) -> Option<usize> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == resolved.active_provider)
}

fn manual_migration_decision(
    model: &ModelConfig,
    active: &ProviderConfig,
    manual_target: Option<&str>,
) -> Option<MigrationDecision> {
    manual_target.map(|target| {
        manual_target_provider_index(model, active, target)
            .map(manual_migration_to)
            .unwrap_or(MigrationDecision::Stay)
    })
}

fn manual_target_provider_index(
    model: &ModelConfig,
    active: &ProviderConfig,
    target: &str,
) -> Option<usize> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == target)
        .filter(|target_provider_index| {
            is_resume_migratable_pair(active, &model.providers[*target_provider_index])
        })
}

fn manual_migration_to(target_provider_index: usize) -> MigrationDecision {
    MigrationDecision::Migrate {
        target_provider_index,
        reason: TransitionReason::Manual,
    }
}

fn active_provider_supports_resume_migration(active: &ProviderConfig) -> bool {
    is_resume_migratable_pair(active, active)
}

fn active_provider_is_exhausted(
    state: &StateDb,
    active: &ProviderConfig,
) -> Result<bool, MigrationError> {
    let quota = active_provider_quota(state, active)?;
    Ok(quota_is_exhausted(quota.as_ref()))
}

fn active_provider_quota(
    state: &StateDb,
    active: &ProviderConfig,
) -> Result<Option<QuotaRecord>, MigrationError> {
    state
        .get_quota(&active.name)
        .map_err(|message| MigrationError::Db { message })
}

fn quota_is_exhausted(quota: Option<&QuotaRecord>) -> bool {
    // AGE-163 WU-A.4: the typed forensics writer lands durable
    // unavailability on `next_available_at` (and the failure class). The
    // legacy `exhausted_at` column is preserved for back-compat read sites
    // and for the legacy reset-implied clear path. A provider is treated
    // as currently exhausted if either signal is active for the current
    // wall-clock — this aligns `quota_is_exhausted` with
    // `working_set_member` so the existing migration-decision path
    // honors the new typed state.
    let Some(quota) = quota else {
        return false;
    };
    quota.exhausted_at.is_some() || quota.next_available_at.is_some_and(|ts| ts > Utc::now())
}

fn exhausted_migration_decision(
    model: &ModelConfig,
    projections: &[ProviderProjection],
    active: &ProviderConfig,
    active_provider_index: usize,
) -> MigrationDecision {
    lowest_load_migration_target(model, projections, active, Some(active_provider_index))
        .map(|target| migration_to(target.provider_index, TransitionReason::Exhausted))
        .unwrap_or(MigrationDecision::Stay)
}

fn quota_threshold_migration_decision(
    model: &ModelConfig,
    projections: &[ProviderProjection],
    active: &ProviderConfig,
    active_provider_index: usize,
) -> MigrationDecision {
    let Some(best) = lowest_load_migration_target(model, projections, active, None) else {
        return MigrationDecision::Stay;
    };
    if best.provider_index == active_provider_index {
        return MigrationDecision::Stay;
    }
    // AGE-163 WU-A.2: only rotate when the best sibling's projected load is
    // strictly lower than the active provider's. The prior tie-break (lowest
    // provider_index) forced a migration when both candidates were unlearned
    // (load=0), which the new seam now surfaces as a working-set-exhaustion
    // RotationFailed if the chosen sibling's source JSONL is missing. The
    // design contract's "no rotation without reason" intent is preserved by
    // requiring strict-better evidence.
    let active_load = projections
        .iter()
        .find(|projection| projection.provider_index == active_provider_index)
        .map(provider_load);
    if let Some(active_load) = active_load
        && provider_load(best) >= active_load
    {
        return MigrationDecision::Stay;
    }

    migration_to(best.provider_index, TransitionReason::QuotaThreshold)
}

fn migration_to(target_provider_index: usize, reason: TransitionReason) -> MigrationDecision {
    MigrationDecision::Migrate {
        target_provider_index,
        reason,
    }
}

// S10/S11 provider-extraction move candidate: Claude-specific resume migration eligibility island.
fn is_resume_migratable_pair(source: &ProviderConfig, target: &ProviderConfig) -> bool {
    provider_has_derivable_claude_projects_dir(source)
        && provider_has_derivable_claude_projects_dir(target)
}

#[cfg(test)]
mod tests;
