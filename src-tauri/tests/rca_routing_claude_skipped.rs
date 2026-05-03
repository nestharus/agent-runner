use agent_runner_balancer::select_provider;
use agent_runner_config::{ModelConfig, PromptMode, ProviderConfig};
use agent_runner_state::{InvocationStart, StateDb};
use std::path::Path;
use uuid::Uuid;

fn claude_accounts_model() -> ModelConfig {
    ModelConfig {
        name: "claude-balanced".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::model_provider("claude", vec![]),
            ProviderConfig::model_provider("claude2", vec![]),
            ProviderConfig::model_provider("claude3", vec![]),
        ],
        inputs: vec![],
    }
}

fn record_invocation(db: &StateDb, model_name: &str, provider_name: &str, provider_index: usize) {
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
}

// Risk: RC-1 fallback identity (claude pool 0% while claude2/claude3 absorb routing) | level: unit
// Source: proposals/10-routing-claude-skipped.md §Test-intent track; research/10-routing-claude-skipped-rca.md §Reproduction
#[test]
fn fallback_count_routing_uses_current_provider_identity_not_stale_index_history() {
    let model = claude_accounts_model();
    let db = StateDb::open(Path::new(":memory:")).unwrap();

    for _ in 0..6 {
        record_invocation(&db, &model.name, "claude2", 0);
    }
    for _ in 0..6 {
        record_invocation(&db, &model.name, "claude3", 1);
    }

    let selected_index = select_provider(&model, &db, None);
    let selected_name = &model.providers[selected_index].name;

    assert_eq!(
        selected_name, "claude",
        "provider claude has no invocation history by provider_name, but stale provider_index rows made selection pick {selected_name}"
    );
}
