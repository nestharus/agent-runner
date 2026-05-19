//! Declared roles: formatter, accessor, mapper, orchestration.

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

#[cfg(test)]
pub mod age158_characterization;

pub fn model_named(name: &str, providers: &[&str]) -> ModelConfig {
    assemble_routing_model(name, provider_configs_named(providers))
}

fn provider_configs_named(providers: &[&str]) -> Vec<ProviderConfig> {
    providers
        .iter()
        .map(|provider| ProviderConfig::model_provider(*provider, vec![]))
        .collect()
}

fn assemble_routing_model(name: &str, providers: Vec<ProviderConfig>) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers,
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
    upsert_initial_quota_windows(db, provider_name, &initial_quota_windows(final_windows));
    let refreshed_at = quota_refreshed_at(db, provider_name);
    let turns = assistant_turns_for_quota_delta(
        provider_name,
        refreshed_at,
        max_delta_calls(final_windows),
    );
    ingest_quota_delta_turns(db, provider_name, &turns);
    upsert_final_quota_windows(db, provider_name, &final_quota_windows(final_windows));
}

pub fn provider_config_with_scripts(scripts: &[(&str, &str)]) -> ProvidersConfig {
    assemble_providers_config(
        scripts
            .iter()
            .map(|(provider_name, script)| quota_script_entry(provider_name, script))
            .collect(),
    )
}

type LearnedWindow = (f64, i64, f64, u64);

fn initial_quota_windows(final_windows: &[LearnedWindow]) -> Vec<QuotaWindowInput> {
    final_windows
        .iter()
        .map(
            |(final_used, hours_until_reset, delta_percent, _)| QuotaWindowInput {
                used_percent: final_used - delta_percent,
                resets_at: Utc::now() + Duration::hours(*hours_until_reset),
            },
        )
        .collect()
}

fn upsert_initial_quota_windows(db: &StateDb, provider_name: &str, windows: &[QuotaWindowInput]) {
    db.upsert_quota_refresh(provider_name, windows).unwrap();
}

fn quota_refreshed_at(db: &StateDb, provider_name: &str) -> chrono::DateTime<Utc> {
    db.get_quota(provider_name)
        .unwrap()
        .unwrap()
        .refreshed_at
        .unwrap()
}

fn max_delta_calls(final_windows: &[LearnedWindow]) -> u64 {
    final_windows
        .iter()
        .map(|(_, _, _, delta_calls)| *delta_calls)
        .max()
        .unwrap_or(0)
}

fn assistant_turns_for_quota_delta(
    provider_name: &str,
    refreshed_at: chrono::DateTime<Utc>,
    max_delta_calls: u64,
) -> Vec<SessionTurnIngest> {
    let mut turns = Vec::new();
    for index in 0..max_delta_calls {
        turns.push(assistant_turn_for_quota_delta(
            provider_name,
            refreshed_at,
            index,
        ));
    }
    turns
}

fn assistant_turn_for_quota_delta(
    provider_name: &str,
    refreshed_at: chrono::DateTime<Utc>,
    index: u64,
) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: quota_delta_session_id(provider_name),
        turn_id: quota_delta_turn_id(provider_name, index),
        timestamp: refreshed_at + Duration::seconds((index + 1) as i64),
        role: assistant_role_name(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: None,
    }
}

fn quota_delta_session_id(provider_name: &str) -> String {
    format!("{provider_name}-quota-learn")
}

fn quota_delta_turn_id(provider_name: &str, index: u64) -> String {
    format!("{provider_name}-turn-{index}")
}

fn assistant_role_name() -> String {
    "assistant".to_string()
}

fn ingest_quota_delta_turns(db: &StateDb, provider_name: &str, turns: &[SessionTurnIngest]) {
    db.ingest_session_turns_batch(provider_name, turns).unwrap();
}

fn final_quota_windows(final_windows: &[LearnedWindow]) -> Vec<QuotaWindowInput> {
    final_windows
        .iter()
        .map(|(final_used, hours_until_reset, _, _)| QuotaWindowInput {
            used_percent: *final_used,
            resets_at: Utc::now() + Duration::hours(*hours_until_reset),
        })
        .collect()
}

fn upsert_final_quota_windows(db: &StateDb, provider_name: &str, windows: &[QuotaWindowInput]) {
    db.upsert_quota_refresh(provider_name, windows).unwrap();
}

fn quota_script_entry(provider_name: &str, script: &str) -> (String, ProviderEntry) {
    let entry = ProviderEntry {
        quota_script: Some(script.to_string()),
        ..ProviderEntry::default()
    };
    (provider_name.to_string(), entry)
}

fn assemble_providers_config(entries: HashMap<String, ProviderEntry>) -> ProvidersConfig {
    ProvidersConfig { entries }
}

pub fn select_provider_name_with_ctx(
    model: &ModelConfig,
    db: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    in_flight: &InFlight,
) -> String {
    let ctx = balance_context_for_test(providers_cfg, sessions_cfg, in_flight);
    let index = selected_provider_index(model, db, &ctx);
    provider_name_at_index(model, index)
}

fn balance_context_for_test<'a>(
    providers_cfg: &'a ProvidersConfig,
    sessions_cfg: &'a SessionsConfig,
    in_flight: &'a InFlight,
) -> BalanceContext<'a> {
    BalanceContext {
        providers_cfg,
        sessions_cfg,
        in_flight,
    }
}

fn selected_provider_index(model: &ModelConfig, db: &StateDb, ctx: &BalanceContext<'_>) -> usize {
    select_provider(model, db, Some(ctx)).unwrap()
}

fn provider_name_at_index(model: &ModelConfig, index: usize) -> String {
    model.providers[index].name.clone()
}

pub fn record_successful_invocation(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
) {
    let id = start_test_invocation(db, model_name, provider_name, provider_index);
    finalize_test_invocation_success(db, id);
    increment_provider_call_count(db, provider_name);
}

fn successful_invocation_start(
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id: None,
    }
}

fn start_test_invocation(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
) -> i64 {
    db.start_invocation(&successful_invocation_start(
        model_name,
        provider_name,
        provider_index,
    ))
    .unwrap()
}

fn finalize_test_invocation_success(db: &StateDb, invocation_id: i64) {
    db.finalize_invocation(invocation_id, true, 0, None, None)
        .unwrap();
}

fn increment_provider_call_count(db: &StateDb, provider_name: &str) {
    db.increment_calls_since_refresh(provider_name).unwrap();
}
