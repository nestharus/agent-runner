#![cfg(unix)]

mod fixtures;

use agent_runner_lib::balancer::{compute_projections, decide_migration, select_provider};
use agent_runner_lib::config::{ModelConfig, PromptMode, ProviderConfig};
use agent_runner_lib::state::{ResolvedResume, RoutingRepository};
use fixtures::b1_state_repos::StateRepoFixture;
use fixtures::b3_app_state::{
    CallLog, FakeBalanceEffects, GetQuotaMustNotBeCalled, LoggingRoutingRepository,
};

fn model(name: &str, providers: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: providers
            .iter()
            .map(|provider| ProviderConfig::model_provider(*provider, Vec::new()))
            .collect(),
        inputs: Vec::new(),
    }
}

fn resolved(model: &ModelConfig, active_provider: &str) -> ResolvedResume {
    ResolvedResume {
        chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: active_provider.to_string(),
        active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
    }
}

/// Risk: T3 (balancer service preserves single-provider fast path through DI seam)
/// Source: proposal §8 T3; B3 contract §5 balancer
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn balancer_select_provider_single_provider_returns_zero_without_effects_or_reads() {
    let log = CallLog::default();
    let repo = LoggingRoutingRepository::new(log.clone());
    let effects = FakeBalanceEffects::new(log.clone());
    let model = model("one-provider", &["alpha"]);

    let selected = select_provider(&model, &repo, Some(&effects));

    assert_eq!(selected, 0);
    assert!(log.events().is_empty(), "{:?}", log.events());
}

/// Risk: T3 (balancer effects preserve refresh/scan timing before routing reads)
/// Source: proposal §8 T3; B3 contract §5 balancer
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn balancer_select_provider_runs_all_effects_before_quota_or_window_reads() {
    let log = CallLog::default();
    let repo = LoggingRoutingRepository::new(log.clone());
    let effects = FakeBalanceEffects::new(log.clone());
    let model = model("two-provider", &["alpha", "beta"]);

    let _ = select_provider(&model, &repo, Some(&effects));

    let events = log.events();
    let first_repo_read = events
        .iter()
        .position(|event| {
            event.starts_with("repo:get_quota:") || event.starts_with("repo:get_windows:")
        })
        .expect("expected quota/window repository reads");
    let last_effect = events
        .iter()
        .rposition(|event| event.starts_with("effect:"))
        .expect("expected balancer effects");
    assert!(
        last_effect < first_repo_read,
        "effects must complete before quota/window reads: {events:?}"
    );
    for provider in ["alpha", "beta"] {
        assert!(
            events.contains(&format!("effect:refresh:{provider}")),
            "{events:?}"
        );
        assert!(
            events.contains(&format!("effect:scan:{provider}")),
            "{events:?}"
        );
    }
}

/// Risk: T3 (projection path preserves same balancer side-effect ordering)
/// Source: proposal §8 T3; B3 contract §5 balancer
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn balancer_compute_projections_runs_effects_before_projection_reads() {
    let log = CallLog::default();
    let repo = LoggingRoutingRepository::new(log.clone());
    let effects = FakeBalanceEffects::new(log.clone());
    let model = model("projection-model", &["alpha", "beta"]);

    let projections = compute_projections(&model, &repo, Some(&effects));

    assert_eq!(projections.len(), 2);
    let events = log.events();
    let first_repo_read = events
        .iter()
        .position(|event| {
            event.starts_with("repo:get_quota:") || event.starts_with("repo:get_windows:")
        })
        .expect("expected quota/window repository reads");
    let last_effect = events
        .iter()
        .rposition(|event| event.starts_with("effect:"))
        .expect("expected balancer effects");
    assert!(last_effect < first_repo_read, "{events:?}");
}

/// Risk: T3 (all-exhausted balancer tie-break stays deterministic)
/// Source: proposal §8 T3; B3 contract §6 balancer all-exhausted edge
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn balancer_select_provider_all_exhausted_ties_by_provider_index() {
    let fixture = StateRepoFixture::new();
    fixture.seed_quota_row("alpha", 1.0, 0, true);
    fixture.seed_quota_row("beta", 1.0, 0, true);
    let db = fixture.open_db();
    let repo: &dyn RoutingRepository = &db;
    let model = model("exhausted-model", &["alpha", "beta"]);

    let selected = select_provider(&model, repo, None);

    assert_eq!(selected, 0);
}

/// Risk: T3/T13 (manual migration target validation remains before active quota scoring)
/// Source: proposal §8 T3/T13; B3 contract §3 balancer::decide_migration
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn balancer_decide_migration_rejects_unknown_manual_target_before_quota_read() {
    let log = CallLog::default();
    let repo = GetQuotaMustNotBeCalled::new(log);
    let model = model("resume-model", &["alpha", "beta"]);
    let resolved = resolved(&model, "alpha");

    let err = decide_migration(&repo, &model, &resolved, Some("missing-provider")).unwrap_err();

    let err = format!("{err:?}");
    assert!(
        err.contains("manual") || err.contains("missing-provider") || err.contains("Provider"),
        "{err}"
    );
}
