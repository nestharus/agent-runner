use agent_runner_balancer::select_provider;
use agent_runner_config::{ModelConfig, PromptMode, ProviderConfig};
use agent_runner_quota::is_stale;
use agent_runner_state::{InvocationStart, StateDb};
use chrono::{DateTime, Utc};
use rusqlite::params;
use uuid::Uuid;

fn last_empty_refresh_at(path: &std::path::Path, provider_name: &str) -> Option<DateTime<Utc>> {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(
        "SELECT last_empty_refresh_at
         FROM provider_quotas
         WHERE provider_name = ?1",
        params![provider_name],
        |row| row.get::<_, Option<String>>(0),
    )
    .unwrap()
    .map(|value| {
        DateTime::parse_from_rfc3339(&value)
            .unwrap()
            .with_timezone(&Utc)
    })
}

fn record_provider_invocation(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
    success: bool,
    error_category: Option<&str>,
    stderr_snippet: Option<&str>,
) -> i64 {
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: model_name.to_string(),
            provider_name: provider_name.to_string(),
            provider_index,
            parent_invocation_id: None,
        })
        .unwrap();
    db.finalize_invocation(
        id,
        success,
        if success { 0 } else { 1 },
        error_category,
        stderr_snippet,
    )
    .unwrap();
    id
}

#[test]
fn upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = StateDb::open(&path).unwrap();
    let provider = "p";

    db.upsert_quota_refresh(provider, &[]).unwrap();

    let quota = db.get_quota(provider).unwrap().unwrap();
    assert!(quota.refreshed_at.is_some());
    assert!(last_empty_refresh_at(&path, provider).is_some());
    assert!(db.get_windows(provider).unwrap().is_empty());
    assert!(is_stale(&db, provider));
}

#[test]
fn provider_aggregate_round_trip_follows_name_after_reorder() {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    record_provider_invocation(&db, "routing-model", "claude2", 0, true, None, None);

    let claude2 = db
        .get_provider("routing-model", "claude2")
        .unwrap()
        .expect("claude2 aggregate should exist by provider name");
    assert_eq!(claude2.provider_name, "claude2");
    assert_eq!(claude2.invocation_count, 1);
    assert!(
        db.get_provider("routing-model", "claude")
            .unwrap()
            .is_none(),
        "claude must not inherit claude2 history after taking index 0"
    );

    let model = ModelConfig {
        name: "routing-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::model_provider("claude", vec![]),
            ProviderConfig::model_provider("claude3", vec![]),
            ProviderConfig::model_provider("claude2", vec![]),
        ],
        inputs: vec![],
    };
    let selected = select_provider(&model, &db, None);
    assert_eq!(
        model.providers[selected].name, "claude",
        "fallback scoring should treat the current claude provider as unused"
    );
}
