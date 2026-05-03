#![cfg(unix)]

mod fixtures;

use agent_runner_lib::config::{
    FilesystemModelConfigRepository, FilesystemProviderConfigSource, FilesystemSessionsConfigSource,
};
use agent_runner_lib::session_export::resolve_export_session_metadata_with_deps;
use fixtures::b2_process_runner::FakeProcessRunner;
use fixtures::initiative_06_import_replace::prepared_claude_replace_fixture;

/// Risk: T11/T13 (session export reuses DI metadata resolution instead of duplicate concrete state/config lookup)
/// Source: proposal §8 T11/T13; B3 contract §3 session_export wiring note
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn session_export_metadata_with_deps_uses_runner_locate_path_and_preserves_export_fields() {
    let prepared = prepared_claude_replace_fixture();
    let db = prepared.fixture.open_db();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();
    runner.push_stdout(prepared.jsonl_path.to_string_lossy().as_bytes());

    let metadata = resolve_export_session_metadata_with_deps(
        &db,
        &model_repo,
        &provider_source,
        &sessions_source,
        &runner,
        &prepared.session_id,
    )
    .unwrap();

    assert_eq!(metadata.session_id, prepared.session_id);
    assert_eq!(metadata.provider_name, prepared.provider_name);
    assert_eq!(
        metadata.jsonl_path,
        prepared.jsonl_path.canonicalize().unwrap()
    );
    assert_eq!(runner.calls().len(), 1);
}
