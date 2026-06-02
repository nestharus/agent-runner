use chrono::{Duration, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
};
use oulipoly_runtime::balancer::{
    MigrationDecision, decide_migration, select_next_working_candidate,
};
use oulipoly_state::{QuotaWindowInput, ResolvedResume, StateDb};
use std::path::{Path, PathBuf};

fn memory_state() -> StateDb {
    StateDb::open(Path::new(":memory:")).expect("in-memory state")
}

fn claude_provider(name: &str) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: Some(ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }),
        session_capture: None,
        resume_acceptance: None,
        session_storage: Some(SessionStorage::ClaudeCode {
            projects_dir: PathBuf::from(format!("/tmp/age225-{name}/projects")),
        }),
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn model_with_providers(name: &str, provider_names: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: provider_names
            .iter()
            .map(|provider_name| claude_provider(provider_name))
            .collect(),
        inputs: Vec::new(),
        provider: None,
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

fn seed_window_with_delta(
    state: &StateDb,
    provider_name: &str,
    used_percent: f64,
    hours_until_reset: i64,
    last_delta_percent: f64,
    last_delta_calls: u64,
) {
    state
        .upsert_quota_refresh(
            provider_name,
            &[QuotaWindowInput {
                used_percent,
                resets_at: Utc::now() + Duration::hours(hours_until_reset),
            }],
        )
        .expect("seed quota window");
    state
        .set_window_delta_for_test(provider_name, 0, last_delta_percent, last_delta_calls)
        .expect("seed quota window delta");
}

#[test]
fn age225_quota_threshold_equal_nonzero_active_and_target_load_stays() {
    let state = memory_state();
    let model = model_with_providers("age225-equal-load", &["claude-a", "claude-b"]);
    seed_window_with_delta(&state, "claude-a", 0.30, 5, 0.01, 22);
    seed_window_with_delta(&state, "claude-b", 0.30, 5, 0.01, 22);

    let decision = decide_migration(&state, &model, &resolved_for(&model, 1), None)
        .expect("migration decision");

    assert_eq!(
        decision,
        MigrationDecision::Stay,
        "equal nonzero projected load must not migrate to the lower-index sibling"
    );
}

#[test]
fn age225_working_set_cursor_is_not_advanced_when_no_candidate_is_selected() {
    let state = memory_state();
    let model = model_with_providers("age225-working-set-none", &["claude-a", "claude-b"]);
    let now = Utc::now();
    state
        .advance_round_robin_index(&model.name, 1, now)
        .expect("seed cursor");
    let future = now + Duration::hours(1);
    for provider in &model.providers {
        state
            .record_provider_unavailable(&provider.name, Some(future), "RollingWindow5h")
            .expect("seed provider unavailable");
    }

    let picked =
        select_next_working_candidate(&state, &model, now, None).expect("working-set selection");

    assert_eq!(picked, None);
    assert_eq!(
        state
            .next_round_robin_index_for_model(&model.name)
            .expect("read cursor"),
        Some(1),
        "working-set cursor must only advance after selecting a candidate"
    );
}
