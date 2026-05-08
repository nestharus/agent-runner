#![cfg(unix)]

use chrono::{Duration, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, SessionSourceEntry,
    SessionsConfig,
};
use oulipoly_runtime::balancer::{BalanceContext, compute_projections, select_provider};
use oulipoly_runtime::quota::InFlight;
use oulipoly_runtime::services::{
    ProductionRoutingService, RoutingServicePort, RoutingServiceRequest,
};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct ScriptFixture {
    _dir: tempfile::TempDir,
}

impl ScriptFixture {
    fn new() -> Self {
        Self {
            _dir: tempfile::tempdir().unwrap(),
        }
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self._dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }
}

fn two_provider_model() -> ModelConfig {
    ModelConfig {
        name: "age35-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::model_provider("age35-a", vec![]),
            ProviderConfig::model_provider("age35-b", vec![]),
        ],
        inputs: vec![],
    }
}

fn quota_script(fixture: &ScriptFixture, provider: &str, used_percent: u32) -> String {
    let resets_at = (Utc::now() + chrono::Duration::hours(24)).to_rfc3339();
    let script = fixture.write_script(
        &format!("{provider}-quota.sh"),
        &format!(
            r#"printf '%s\n' '{{"windows":[{{"used_percent":{used_percent},"resets_at":"{resets_at}"}}]}}'"#
        ),
    );
    script.display().to_string()
}

fn providers_config(fixture: &ScriptFixture) -> ProvidersConfig {
    let mut entries = HashMap::new();
    for (provider, used_percent) in [("age35-a", 10), ("age35-b", 20)] {
        entries.insert(
            provider.to_string(),
            ProviderEntry {
                quota_script: Some(quota_script(fixture, provider, used_percent)),
                ..ProviderEntry::default()
            },
        );
    }
    ProvidersConfig { entries }
}

fn providers_config_with_script_bodies(
    fixture: &ScriptFixture,
    scripts: &[(&str, &str)],
) -> ProvidersConfig {
    let mut entries = HashMap::new();
    for (provider, body) in scripts {
        let script = fixture.write_script(&format!("{provider}-quota.sh"), body);
        entries.insert(
            (*provider).to_string(),
            ProviderEntry {
                quota_script: Some(script.display().to_string()),
                ..ProviderEntry::default()
            },
        );
    }
    ProvidersConfig { entries }
}

fn seed_windows(db: &StateDb, provider: &str, windows: &[(f64, i64)]) {
    let inputs: Vec<QuotaWindowInput> = windows
        .iter()
        .map(|(used_percent, reset_hours)| QuotaWindowInput {
            used_percent: *used_percent,
            resets_at: Utc::now() + Duration::hours(*reset_hours),
        })
        .collect();
    db.upsert_quota_refresh(provider, &inputs).unwrap();
}

fn session_script(fixture: &ScriptFixture, provider: &str) -> String {
    let script = fixture.write_script(
        &format!("{provider}-sessions.sh"),
        &format!(
            r#"printf '%s\n' '{{"session_id":"{provider}-session","turn_id":"turn-1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}'"#
        ),
    );
    script.display().to_string()
}

fn sessions_config(fixture: &ScriptFixture) -> SessionsConfig {
    let mut entries = HashMap::new();
    for provider in ["age35-a", "age35-b"] {
        entries.insert(
            provider.to_string(),
            SessionSourceEntry {
                turn_script: session_script(fixture, provider),
                transcript_locator: None,
                state_dir: Some(
                    fixture
                        ._dir
                        .path()
                        .join(format!("{provider}-session-state")),
                ),
            },
        );
    }
    SessionsConfig { entries }
}

fn assert_live_route_side_effects(db: &StateDb, label: &str) {
    for provider in ["age35-a", "age35-b"] {
        let quota = db
            .get_quota(provider)
            .unwrap()
            .unwrap_or_else(|| panic!("missing refreshed quota row for {provider} in {label}"));
        assert_eq!(
            quota.calls_since_refresh, 0,
            "quota refresh should reset calls_since_refresh for {provider} in {label}"
        );
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            1,
            "live routing should persist one quota window for {provider} in {label}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            1,
            "live routing should scan session turns for {provider} in {label}"
        );
    }
}

fn assert_cached_route_has_no_live_side_effects(db: &StateDb, label: &str) {
    for provider in ["age35-a", "age35-b"] {
        assert!(
            db.get_quota(provider).unwrap().is_none(),
            "cached-only routing should not refresh quota for {provider} in {label}"
        );
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            0,
            "cached-only routing should not write quota windows for {provider} in {label}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            0,
            "cached-only routing should not scan session turns for {provider} in {label}"
        );
    }
}

#[test]
fn age_35_select_provider_with_balance_context_refreshes_stale_quotas_and_scans_sessions() {
    let fixture = ScriptFixture::new();
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    let providers_cfg = providers_config(&fixture);
    let sessions_cfg = sessions_config(&fixture);
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx));

    assert!(selected < model.providers.len());
    for provider in ["age35-a", "age35-b"] {
        let quota = db
            .get_quota(provider)
            .unwrap()
            .unwrap_or_else(|| panic!("missing refreshed quota row for {provider}"));
        assert_eq!(
            quota.calls_since_refresh, 0,
            "quota refresh should reset calls_since_refresh for {provider}"
        );
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            1,
            "stale quota refresh should persist one window for {provider}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            1,
            "select_provider(Some(ctx)) should scan session turns for {provider}"
        );
    }
}

#[test]
fn age_35_production_routing_service_matches_direct_select_provider_with_live_context() {
    let direct_fixture = ScriptFixture::new();
    let service_fixture = ScriptFixture::new();
    let direct_db = StateDb::open(Path::new(":memory:")).unwrap();
    let service_db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    let direct_providers_cfg = providers_config(&direct_fixture);
    let direct_sessions_cfg = sessions_config(&direct_fixture);
    let direct_in_flight = InFlight::new();
    let direct_ctx = BalanceContext {
        providers_cfg: &direct_providers_cfg,
        sessions_cfg: &direct_sessions_cfg,
        in_flight: &direct_in_flight,
    };

    let service_providers_cfg = providers_config(&service_fixture);
    let service_sessions_cfg = sessions_config(&service_fixture);
    let service_in_flight = InFlight::new();
    let service_ctx = BalanceContext {
        providers_cfg: &service_providers_cfg,
        sessions_cfg: &service_sessions_cfg,
        in_flight: &service_in_flight,
    };

    let direct_index = select_provider(&model, &direct_db, Some(&direct_ctx));
    let service_index = ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &service_db,
            ctx: Some(&service_ctx),
        })
        .unwrap()
        .provider_index;

    assert_eq!(
        service_index, direct_index,
        "production routing service must return the same provider index as direct select_provider"
    );
    assert_live_route_side_effects(&direct_db, "direct select_provider");
    assert_live_route_side_effects(&service_db, "ProductionRoutingService");
}

#[test]
fn age_35_production_routing_service_matches_direct_select_provider_cached_only() {
    let direct_db = StateDb::open(Path::new(":memory:")).unwrap();
    let service_db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    let direct_index = select_provider(&model, &direct_db, None);
    let service_index = ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &service_db,
            ctx: None,
        })
        .unwrap()
        .provider_index;

    assert_eq!(
        service_index, direct_index,
        "cached-only service routing must return the same provider index as direct select_provider"
    );
    assert_cached_route_has_no_live_side_effects(&direct_db, "direct select_provider");
    assert_cached_route_has_no_live_side_effects(&service_db, "ProductionRoutingService");
}

#[test]
fn age_35_routing_service_preserves_select_provider_topology_probe_drift_from_compute_projections()
{
    let service_fixture = ScriptFixture::new();
    let projection_fixture = ScriptFixture::new();
    let service_db = StateDb::open(Path::new(":memory:")).unwrap();
    let projection_db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    for db in [&service_db, &projection_db] {
        seed_windows(db, "age35-a", &[(0.02, 24 * 7)]);
        seed_windows(db, "age35-b", &[(0.66, 80), (0.16, 3)]);
    }

    let long_resets = (Utc::now() + Duration::hours(80)).to_rfc3339();
    let short_resets = (Utc::now() + Duration::hours(5)).to_rfc3339();
    let repair_a = format!(
        r#"printf '%s' '{{"windows":[{{"used_percent":4,"resets_at":"{long_resets}"}},{{"used_percent":90,"resets_at":"{short_resets}"}}]}}'"#
    );
    let empty_b = r#"printf '%s' '{"windows":[]}'"#;

    let service_providers_cfg = providers_config_with_script_bodies(
        &service_fixture,
        &[("age35-a", repair_a.as_str()), ("age35-b", empty_b)],
    );
    let service_sessions_cfg = SessionsConfig::default();
    let service_in_flight = InFlight::new();
    let service_ctx = BalanceContext {
        providers_cfg: &service_providers_cfg,
        sessions_cfg: &service_sessions_cfg,
        in_flight: &service_in_flight,
    };

    let projection_providers_cfg = providers_config_with_script_bodies(
        &projection_fixture,
        &[("age35-a", repair_a.as_str()), ("age35-b", empty_b)],
    );
    let projection_sessions_cfg = SessionsConfig::default();
    let projection_in_flight = InFlight::new();
    let projection_ctx = BalanceContext {
        providers_cfg: &projection_providers_cfg,
        sessions_cfg: &projection_sessions_cfg,
        in_flight: &projection_in_flight,
    };

    let _ = ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &service_db,
            ctx: Some(&service_ctx),
        })
        .unwrap();
    let _ = compute_projections(&model, &projection_db, Some(&projection_ctx));

    assert_eq!(
        service_db.get_windows("age35-a").unwrap().len(),
        2,
        "routing service must preserve select_provider(Some(ctx)) topology probe behavior"
    );
    assert_eq!(
        projection_db.get_windows("age35-a").unwrap().len(),
        1,
        "compute_projections(Some(ctx)) must keep the accepted topology-probe drift"
    );
}
