use chrono::{Duration, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, SessionsConfig,
};
use oulipoly_runtime::balancer::{BalanceContext, select_provider};
use oulipoly_runtime::quota::InFlight;
use oulipoly_state::{InvocationStart, QuotaWindowInput, SessionTurnIngest, StateDb};
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

pub mod rc1_incomplete_quota_topology;
pub mod rc2_argmax_concentration;

pub fn model_named(name: &str, providers: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: providers
            .iter()
            .map(|provider| ProviderConfig::model_provider(*provider, vec![]))
            .collect(),
        inputs: vec![],
    }
}

pub fn in_memory_state() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

pub fn seed_learned_windows(
    db: &StateDb,
    provider_name: &str,
    final_windows: &[(f64, i64, f64, u64)],
) {
    let initial_windows: Vec<_> = final_windows
        .iter()
        .map(
            |(final_used, hours_until_reset, delta_percent, _)| QuotaWindowInput {
                used_percent: final_used - delta_percent,
                resets_at: Utc::now() + Duration::hours(*hours_until_reset),
            },
        )
        .collect();
    db.upsert_quota_refresh(provider_name, &initial_windows)
        .unwrap();

    let refreshed_at = db
        .get_quota(provider_name)
        .unwrap()
        .unwrap()
        .refreshed_at
        .unwrap();
    let max_delta_calls = final_windows
        .iter()
        .map(|(_, _, _, delta_calls)| *delta_calls)
        .max()
        .unwrap_or(0);
    let turns: Vec<_> = (0..max_delta_calls)
        .map(|i| SessionTurnIngest {
            session_id: format!("{provider_name}-quota-learn"),
            turn_id: format!("{provider_name}-turn-{i}"),
            timestamp: refreshed_at + Duration::seconds((i + 1) as i64),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        })
        .collect();
    db.ingest_session_turns_batch(provider_name, &turns)
        .unwrap();

    let refreshed_windows: Vec<_> = final_windows
        .iter()
        .map(|(final_used, hours_until_reset, _, _)| QuotaWindowInput {
            used_percent: *final_used,
            resets_at: Utc::now() + Duration::hours(*hours_until_reset),
        })
        .collect();
    db.upsert_quota_refresh(provider_name, &refreshed_windows)
        .unwrap();
}

pub fn provider_config_with_scripts(scripts: &[(&str, &str)]) -> ProvidersConfig {
    let entries = scripts
        .iter()
        .map(|(provider_name, script)| {
            let entry = ProviderEntry {
                quota_script: Some((*script).to_string()),
                ..ProviderEntry::default()
            };
            ((*provider_name).to_string(), entry)
        })
        .collect::<HashMap<_, _>>();
    ProvidersConfig { entries }
}

pub fn select_provider_name_with_ctx(
    model: &ModelConfig,
    db: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    in_flight: &InFlight,
) -> String {
    let ctx = BalanceContext {
        providers_cfg,
        sessions_cfg,
        in_flight,
    };
    let index = select_provider(model, db, Some(&ctx)).unwrap();
    model.providers[index].name.clone()
}

pub fn record_successful_invocation(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
) {
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: model_name.to_string(),
            provider_name: provider_name.to_string(),
            provider_index,
            parent_invocation_id: None,
        })
        .unwrap();
    db.finalize_invocation(id, true, 0, None, None).unwrap();
    db.increment_calls_since_refresh(provider_name).unwrap();
}
