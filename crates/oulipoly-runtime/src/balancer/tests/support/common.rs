//! ## Declared roles
//!
//! `mapper`, `accessor`, `validator`, `orchestration`, `formatter`.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/tests/support/common.rs::balancer_state_fixture_adapter
//!     role: adapter
//!     Translates:
//!       - balancer test fixture setup into StateDb and provider config rows
//! ```

use super::super::*;

pub(in crate::balancer::tests) fn record_invocation_for_test(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
    success: bool,
) {
    let start = invocation_start_for_test(model_name, provider_name, provider_index);
    let id = db.start_invocation(&start).unwrap();
    db.finalize_invocation(id, success, if success { 0 } else { 1 }, None, None)
        .unwrap();
}

pub(in crate::balancer::tests) fn invocation_start_for_test(
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
) -> oulipoly_state::InvocationStart {
    oulipoly_state::InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id: None,
    }
}

pub(in crate::balancer::tests) fn provider_eval_with_fanout_usage(
    index: usize,
    binding_score: f64,
    worst_projected_used: Option<f64>,
    soonest_reset_hours: Option<f64>,
) -> ProviderEval {
    ProviderEval {
        index,
        binding_score: Some(binding_score),
        unlearned: false,
        fanout_usage: Some(FanoutUsageKey {
            worst_projected_used,
            soonest_reset_hours,
        }),
    }
}

pub(in crate::balancer::tests) fn providers_config_with_scripts(
    scripts: &[(&str, &str)],
) -> ProvidersConfig {
    let entries = scripts
        .iter()
        .map(|(provider_name, script)| {
            (
                (*provider_name).to_string(),
                ProviderEntry {
                    quota_script: Some((*script).to_string()),
                    ..ProviderEntry::default()
                },
            )
        })
        .collect();
    ProvidersConfig { entries }
}

pub(in crate::balancer::tests) fn sessions_config_with_scripts(
    scripts: &[(&str, &str)],
) -> SessionsConfig {
    let entries = scripts
        .iter()
        .map(|(provider_name, script)| {
            (
                (*provider_name).to_string(),
                SessionSourceEntry {
                    turn_script: (*script).to_string(),
                    transcript_locator: None,
                    state_dir: None,
                },
            )
        })
        .collect();
    SessionsConfig { entries }
}

pub(in crate::balancer::tests) fn file_backed_state(
    label: &str,
) -> (tempfile::TempDir, PathBuf, StateDb) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{label}.db"));
    let db = StateDb::open(&path).unwrap();
    (dir, path, db)
}

pub(in crate::balancer::tests) fn drop_table(path: &Path, table: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(&drop_table_sql(table)).unwrap();
}

pub(in crate::balancer::tests) fn drop_table_sql(table: &str) -> String {
    format!("DROP TABLE {table};")
}

pub(in crate::balancer::tests) fn two_provider_model() -> ModelConfig {
    ModelConfig {
        name: "test".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::new("a", vec![]),
            ProviderConfig::new("b", vec![]),
        ],
        inputs: vec![],
        provider: None,
    }
}

pub(in crate::balancer::tests) fn three_provider_model() -> ModelConfig {
    ModelConfig {
        name: "test3".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::new("a", vec![]),
            ProviderConfig::new("b", vec![]),
            ProviderConfig::new("c", vec![]),
        ],
        inputs: vec![],
        provider: None,
    }
}
pub(in crate::balancer::tests) fn quota_window(
    used: f64,
    hours_until_reset: i64,
) -> oulipoly_state::QuotaWindowInput {
    use chrono::Duration;
    oulipoly_state::QuotaWindowInput {
        used_percent: used,
        resets_at: Utc::now() + Duration::hours(hours_until_reset),
    }
}

pub(in crate::balancer::tests) fn one_window(
    used: f64,
    hours_until_reset: i64,
) -> Vec<oulipoly_state::QuotaWindowInput> {
    vec![quota_window(used, hours_until_reset)]
}

pub(in crate::balancer::tests) fn seed_windows_with_deltas(
    db: &StateDb,
    provider_name: &str,
    windows: &[(f64, i64, f64, u64)],
) {
    let inputs = quota_window_inputs(windows);
    db.upsert_quota_refresh(provider_name, &inputs).unwrap();
    seed_window_deltas(db, provider_name, windows);
}

pub(in crate::balancer::tests) fn quota_window_inputs(
    windows: &[(f64, i64, f64, u64)],
) -> Vec<oulipoly_state::QuotaWindowInput> {
    windows
        .iter()
        .map(|(used, hours, _, _)| quota_window(*used, *hours))
        .collect()
}

pub(in crate::balancer::tests) fn seed_window_deltas(
    db: &StateDb,
    provider_name: &str,
    windows: &[(f64, i64, f64, u64)],
) {
    for (window_id, window) in windows.iter().enumerate() {
        seed_window_delta(db, provider_name, window_id as u32, window);
    }
}

pub(in crate::balancer::tests) fn seed_window_delta(
    db: &StateDb,
    provider_name: &str,
    window_id: u32,
    window: &(f64, i64, f64, u64),
) {
    db.set_window_delta_for_test(provider_name, window_id, window.2, window.3)
        .unwrap();
}

pub(in crate::balancer::tests) fn quota_record(
    provider_name: &str,
    refreshed_at: Option<chrono::DateTime<Utc>>,
) -> QuotaRecord {
    QuotaRecord {
        provider_name: provider_name.to_string(),
        calls_since_refresh: 0,
        refreshed_at,
        exhausted_at: None,
        topology_peak_live_window_count: 0,
        last_topology_probe_at: None,
        next_available_at: None,
        last_refresh_at: None,
        failure_class: None,
    }
}

pub(in crate::balancer::tests) fn quota_window_record(
    provider_name: &str,
    window_id: u32,
    used_percent: f64,
    resets_at: chrono::DateTime<Utc>,
    last_delta_percent: Option<f64>,
    last_delta_calls: Option<u64>,
) -> QuotaWindow {
    QuotaWindow {
        provider_name: provider_name.to_string(),
        window_id,
        used_percent,
        resets_at,
        last_delta_percent,
        last_delta_calls,
    }
}

pub(in crate::balancer::tests) fn seed_assistant_turns_since_refresh(
    db: &StateDb,
    provider_name: &str,
    count: usize,
) {
    use chrono::Duration;

    let refreshed_at = Utc::now() - Duration::hours(1);
    db.set_refreshed_at_for_test(provider_name, &refreshed_at)
        .unwrap();
    let turns = assistant_turns_for_test(provider_name, count, refreshed_at);
    db.ingest_session_turns_batch(provider_name, &turns)
        .unwrap();
}

pub(in crate::balancer::tests) fn assistant_turns_for_test(
    provider_name: &str,
    count: usize,
    refreshed_at: chrono::DateTime<Utc>,
) -> Vec<oulipoly_state::SessionTurnIngest> {
    (0..count)
        .map(|i| assistant_turn_for_test(provider_name, i, refreshed_at))
        .collect()
}

pub(in crate::balancer::tests) fn assistant_turn_for_test(
    provider_name: &str,
    index: usize,
    refreshed_at: chrono::DateTime<Utc>,
) -> oulipoly_state::SessionTurnIngest {
    use chrono::Duration;

    oulipoly_state::SessionTurnIngest {
        session_id: format!("{provider_name}-session"),
        turn_id: format!("{provider_name}-turn-{index}"),
        timestamp: refreshed_at + Duration::seconds((index + 1) as i64),
        role: "assistant".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: None,
    }
}
