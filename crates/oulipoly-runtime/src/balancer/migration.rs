use super::projection::{ProviderProjection, compute_projections};
use crate::migration::{MigrationError, provider_has_derivable_claude_projects_dir};
use chrono::Utc;
use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_core::TransitionReason;
use oulipoly_state::{QuotaRecord, ResolvedResume, StateDb};

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

fn provider_load(projection: &ProviderProjection) -> f64 {
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

fn lowest_load_migration_target<'a>(
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

#[cfg(test)]
mod tests {
    use super::super::projection::WindowProjection;
    use super::*;
    use oulipoly_config::{SessionStorage, model::PromptMode};
    use std::path::PathBuf;

    #[test]
    fn decide_manual_migration_emits_target_not_in_pool_rejection() {
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        let resolved = resolved_for(&model, 0);
        let result = decide_manual_migration(&model, &resolved, "claude-missing");
        match result {
            Err(ManualMigrationRejection::TargetNotInPool { target, pool }) => {
                assert_eq!(target, "claude-missing");
                assert_eq!(pool, vec!["claude".to_string(), "claude2".to_string()]);
            }
            other => panic!("expected TargetNotInPool, got {other:?}"),
        }
    }

    #[test]
    fn decide_manual_migration_emits_single_provider_pool_rejection() {
        let model = migratable_model(&[("claude", "claude_code")]);
        let resolved = resolved_for(&model, 0);
        let result = decide_manual_migration(&model, &resolved, "claude");
        assert!(matches!(
            result,
            Err(ManualMigrationRejection::SingleProviderPool { ref provider })
                if provider == "claude"
        ));
    }

    #[test]
    fn decide_manual_migration_emits_active_not_in_pool_rejection() {
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        let mut resolved = resolved_for(&model, 0);
        resolved.active_provider = "archived".to_string();
        let result = decide_manual_migration(&model, &resolved, "claude2");
        assert!(matches!(
            result,
            Err(ManualMigrationRejection::ActiveProviderNotInPool { ref active })
                if active == "archived"
        ));
    }

    #[test]
    fn decide_manual_migration_emits_not_migratable_pair_rejection() {
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "none")]);
        let resolved = resolved_for(&model, 0);
        let result = decide_manual_migration(&model, &resolved, "claude2");
        match result {
            Err(ManualMigrationRejection::NotMigratablePair { source, target }) => {
                assert_eq!(source, "claude");
                assert_eq!(target, "claude2");
            }
            other => panic!("expected NotMigratablePair, got {other:?}"),
        }
    }

    #[test]
    fn decide_manual_migration_rejects_unparseable_claude_script_target() {
        let model = migratable_model(&[
            ("claude", "claude_code"),
            ("claude2", "claude_custom_script"),
        ]);
        let resolved = resolved_for(&model, 0);
        let result = decide_manual_migration(&model, &resolved, "claude2");

        assert!(matches!(
            result,
            Err(ManualMigrationRejection::NotMigratablePair { ref source, ref target })
                if source == "claude" && target == "claude2"
        ));
    }

    #[test]
    fn provider_load_falls_back_to_zero_without_finite_window_projection() {
        let projection = ProviderProjection {
            provider_index: 0,
            projections_per_window: vec![
                WindowProjection {
                    window_id: 0,
                    projected_used: f64::NAN,
                    hours_until_reset: 2.0,
                    remaining_headroom: 0.0,
                },
                WindowProjection {
                    window_id: 1,
                    projected_used: f64::INFINITY,
                    hours_until_reset: 4.0,
                    remaining_headroom: 0.0,
                },
            ],
            binding_score: Some(1.0),
            recent_error_count: 0,
        };

        assert_eq!(provider_load(&projection), 0.0);
    }

    fn migratable_model(provider_names: &[(&str, &str)]) -> ModelConfig {
        let providers = provider_names
            .iter()
            .map(|(name, storage_kind)| migratable_provider(name, storage_kind))
            .collect();
        ModelConfig {
            name: "migration-fixture".to_string(),
            prompt_mode: PromptMode::Arg,
            providers,
            inputs: Vec::new(),
            provider: None,
        }
    }

    fn migratable_provider(name: &str, storage_kind: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            command: name.to_string(),
            args: Vec::new(),
            interactive_args: Some(vec!["launch".to_string()]),
            resume: Some(resume_strategy_for_test()),
            session_capture: None,
            resume_acceptance: None,
            session_storage: session_storage_for_test(name, storage_kind),
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }
    }

    fn resume_strategy_for_test() -> oulipoly_config::ResumeStrategy {
        oulipoly_config::ResumeStrategy {
            kind: oulipoly_config::ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }
    }

    fn session_storage_for_test(name: &str, storage_kind: &str) -> Option<SessionStorage> {
        match storage_kind {
            "claude_code" => Some(claude_code_storage_for_test(name)),
            "claude_custom_script" => Some(claude_custom_script_storage_for_test()),
            "none" => None,
            other => panic!("unknown storage kind fixture {other}"),
        }
    }

    fn claude_code_storage_for_test(name: &str) -> SessionStorage {
        SessionStorage::ClaudeCode {
            projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
        }
    }

    fn claude_custom_script_storage_for_test() -> SessionStorage {
        SessionStorage::Script {
            cwd_script: "custom-cwd /tmp/custom/projects".to_string(),
            transcript_script: Some("custom-locate-transcript /tmp/custom/projects".to_string()),
            storage_type: Some(oulipoly_config::ScriptSessionStorageType::ClaudeCode),
        }
    }

    fn resolved_for(model: &ModelConfig, provider_index: usize) -> ResolvedResume {
        let provider = &model.providers[provider_index];
        ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: Some(model.name.clone()),
            model: Some(model.clone()),
            active_provider: provider.name.clone(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        }
    }
}
