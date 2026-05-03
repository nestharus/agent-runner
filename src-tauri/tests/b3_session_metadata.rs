#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;
mod fixtures;

use agent_runner_lib::config::{
    ModelConfig, ModelConfigRepository, ProviderConfigSource, ProvidersConfig, SessionsConfig,
    SessionsConfigSource,
};
use agent_runner_lib::session_metadata::locate_session_metadata;
use agent_runner_lib::state::SessionChainRepository;
use b2_process_runner::{FakeProcessRunner, ok_output};
use fixtures::initiative_06::{CLAUDE_PROVIDER, component_claude_success_fixture};
use std::collections::HashMap;

struct StaticModelRepo(HashMap<String, ModelConfig>);
struct StaticProviderSource(ProvidersConfig);
struct StaticSessionsSource(SessionsConfig);

impl ModelConfigRepository for StaticModelRepo {
    fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String> {
        Ok(self.0.clone())
    }

    fn save_model(&self, _model: &ModelConfig) -> Result<(), String> {
        unreachable!("metadata lookup must not mutate models")
    }

    fn delete_model(&self, _name: &str) -> Result<(), String> {
        unreachable!("metadata lookup must not mutate models")
    }
}

impl ProviderConfigSource for StaticProviderSource {
    fn load_providers(&self) -> Result<ProvidersConfig, String> {
        Ok(self.0.clone())
    }
}

impl SessionsConfigSource for StaticSessionsSource {
    fn load_sessions(&self) -> Result<SessionsConfig, String> {
        Ok(self.0.clone())
    }
}

/// Risk: T9 - moving locate metadata onto B1/B2 ports can still execute the
/// transcript locator through the old shell helper or skip model/provider
/// validation above repository facts.
/// Level: component.
/// Source: proposal §8 T9; B3 contract §3 `session_metadata`.
/// Observable: lookup succeeds through trait objects and the locator command is
/// executed by the injected `ProcessRunner`.
#[test]
fn locate_session_metadata_uses_sources_and_injected_locator_runner() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    let db = prepared.fixture.open_db();
    let chain_repo: &dyn SessionChainRepository = &db;
    let model_repo = StaticModelRepo(prepared.fixture.models());
    let provider_source = StaticProviderSource(prepared.fixture.providers_config());
    let sessions_source = StaticSessionsSource(prepared.fixture.sessions_config());
    let runner = FakeProcessRunner::with_responses(vec![ok_output(
        format!("{}\n", prepared.jsonl_path.display()),
        "",
        0,
    )]);

    let metadata = locate_session_metadata(
        chain_repo,
        &model_repo,
        &provider_source,
        &sessions_source,
        &runner,
        &prepared.session_id,
    )
    .unwrap();

    assert_eq!(metadata.session_id, prepared.session_id);
    assert_eq!(metadata.provider_name, prepared.provider_name);
    assert_eq!(metadata.chain_id, prepared.chain_id);
    assert_eq!(
        metadata.jsonl_path,
        prepared.jsonl_path.canonicalize().unwrap()
    );
    assert_eq!(runner.single_call().program, "sh");
}

/// Risk: T9 and anti-drift §1.5 - repository fact lookup no longer owns model
/// validation, but the outward error for missing model config must not drift.
/// Level: component.
/// Source: B3 contract §3 `session_metadata`.
/// Observable: a DB-resolved session whose inferred model is absent from the
/// model repository still returns a model-resolution metadata error instead of
/// being accepted from repository facts alone.
#[test]
fn locate_session_metadata_preserves_unknown_model_error_mapping() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    let db = prepared.fixture.open_db();
    let empty_models = StaticModelRepo(HashMap::new());
    let provider_source = StaticProviderSource(prepared.fixture.providers_config());
    let sessions_source = StaticSessionsSource(prepared.fixture.sessions_config());
    let runner = FakeProcessRunner::new();

    let result = locate_session_metadata(
        &db,
        &empty_models,
        &provider_source,
        &sessions_source,
        &runner,
        &prepared.session_id,
    );

    let err = result.expect_err("missing model must reject located DB facts");
    assert!(
        format!("{err:?}").contains("Model") || format!("{err:?}").contains("model"),
        "expected model-resolution error, got {err:?}"
    );
}
