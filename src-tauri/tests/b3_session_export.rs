#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;
mod fixtures;

use agent_runner_config::{
    ModelConfig, ModelConfigRepository, ProviderConfigSource, ProvidersConfig, SessionsConfig,
    SessionsConfigSource,
};
use agent_runner_session::{
    canonical_jsonl_bytes, read_canonical_transcript, resolve_export_session_metadata_with_deps,
};
use b2_process_runner::{FakeProcessRunner, ok_output};
use fixtures::initiative_06::{CLAUDE_PROVIDER, component_claude_success_fixture};
use std::collections::HashMap;
use std::fs;

struct StaticModelRepo(HashMap<String, ModelConfig>);
struct StaticProviderSource(ProvidersConfig);
struct StaticSessionsSource(SessionsConfig);

impl ModelConfigRepository for StaticModelRepo {
    fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String> {
        Ok(self.0.clone())
    }

    fn save_model(&self, _model: &ModelConfig) -> Result<(), String> {
        unreachable!("export metadata lookup must not mutate models")
    }

    fn delete_model(&self, _name: &str) -> Result<(), String> {
        unreachable!("export metadata lookup must not mutate models")
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

/// Risk: T9/T11 - export metadata resolution can keep duplicated concrete
/// resume/provider/locator logic while parser tests still pass.
/// Level: component.
/// Source: proposal §8 T9/T11; B3 contract §3 `session_export`.
/// Observable: canonical export bytes are produced from metadata resolved by
/// the shared DI locate service and remain compact JSONL.
#[test]
fn canonical_export_uses_metadata_resolved_through_di_service() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    fs::write(
        &prepared.jsonl_path,
        format!(
            "{{\"sessionId\":\"{}\",\"type\":\"user\",\"uuid\":\"550e8400-e29b-41d4-a716-446655440000\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"message\":\"hello from user\"}}\n\
             {{\"sessionId\":\"{}\",\"type\":\"assistant\",\"uuid\":\"6f1b7d4c-7f1d-4c0d-9c5f-3910c57e2a11\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"message\":\"hello from assistant\"}}\n",
            prepared.session_id, prepared.session_id
        ),
    )
    .unwrap();
    let db = prepared.fixture.open_db();
    let runner = FakeProcessRunner::with_responses(vec![ok_output(
        format!("{}\n", prepared.jsonl_path.display()),
        "",
        0,
    )]);
    let metadata = resolve_export_session_metadata_with_deps(
        &db,
        &StaticModelRepo(prepared.fixture.models()),
        &StaticProviderSource(prepared.fixture.providers_config()),
        &StaticSessionsSource(prepared.fixture.sessions_config()),
        &runner,
        &prepared.session_id,
    )
    .unwrap();

    let records = read_canonical_transcript(&metadata).unwrap();
    let bytes = canonical_jsonl_bytes(&records).unwrap();
    let stdout = String::from_utf8(bytes).unwrap();

    assert!(stdout.ends_with('\n'));
    assert_eq!(stdout.lines().count(), records.len());
    assert!(!stdout.contains("\n  "));
}
