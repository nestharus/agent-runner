#![cfg(unix)]

mod fixtures;

use agent_runner_lib::config::{
    FilesystemModelConfigRepository, FilesystemProviderConfigSource,
    FilesystemSessionsConfigSource, ModelConfigRepository, ProviderConfigSource,
    SessionsConfigSource,
};
use agent_runner_lib::session_metadata::{MetadataError, locate_session_metadata};
use agent_runner_lib::state::SessionChainRepository;
use fixtures::b2_process_runner::FakeProcessRunner;
use fixtures::initiative_06_import_replace::{
    CLAUDE_PROVIDER, MODEL, prepared_claude_replace_fixture,
};
use std::fs;

/// Risk: T9 (session metadata consumes B1/B2 deps without concrete state/config inputs)
/// Source: proposal §8 T9; B3 contract §3 session_metadata::locate_session_metadata
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn session_metadata_locate_with_deps_resolves_transcript_through_runner() {
    let prepared = prepared_claude_replace_fixture();
    let db = prepared.fixture.open_db();
    let chain_repo: &dyn SessionChainRepository = &db;
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let model_repo: &dyn ModelConfigRepository = &model_repo;
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let provider_source: &dyn ProviderConfigSource = &provider_source;
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let sessions_source: &dyn SessionsConfigSource = &sessions_source;
    let runner = FakeProcessRunner::new();
    runner.push_stdout(prepared.jsonl_path.to_string_lossy().as_bytes());

    let metadata = locate_session_metadata(
        chain_repo,
        model_repo,
        provider_source,
        sessions_source,
        &runner,
        &prepared.session_id,
    )
    .unwrap();

    assert_eq!(metadata.chain_id, prepared.chain_id);
    assert_eq!(metadata.session_id, prepared.session_id);
    assert_eq!(metadata.provider_name, CLAUDE_PROVIDER);
    assert_eq!(
        metadata.jsonl_path,
        prepared.jsonl_path.canonicalize().unwrap()
    );
    let call = runner.only_call();
    assert_eq!(call.program, "sh");
    assert_eq!(call.args.first().map(String::as_str), Some("-c"));
    assert!(
        call.description.contains("transcript") || call.description.contains("locator"),
        "{call:?}"
    );
}

/// Risk: T9 (invalid UUID is rejected before repository or runner lookup)
/// Source: proposal §8 T9; B3 contract §6 session metadata invalid UUID
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn session_metadata_invalid_uuid_returns_invalid_session_id_without_runner_call() {
    let prepared = prepared_claude_replace_fixture();
    let db = prepared.fixture.open_db();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();

    let err = locate_session_metadata(
        &db,
        &model_repo,
        &provider_source,
        &sessions_source,
        &runner,
        "not-a-uuid",
    )
    .unwrap_err();

    assert!(
        matches!(err, MetadataError::InvalidSessionId { .. }),
        "{err:?}"
    );
    assert!(runner.calls().is_empty());
}

/// Risk: T9 (model/provider validation remains service-owned above resume facts)
/// Source: proposal §8 T9; B3 contract §3 session_metadata service validation
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn session_metadata_missing_model_config_maps_to_outward_metadata_error() {
    let prepared = prepared_claude_replace_fixture();
    fs::remove_file(prepared.fixture.models_dir().join(format!("{MODEL}.toml"))).unwrap();
    let db = prepared.fixture.open_db();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();

    let err = locate_session_metadata(
        &db,
        &model_repo,
        &provider_source,
        &sessions_source,
        &runner,
        &prepared.session_id,
    )
    .unwrap_err();

    let err = format!("{err:?}");
    assert!(err.contains("UnknownModel") || err.contains(MODEL), "{err}");
    assert!(
        runner.calls().is_empty(),
        "transcript locator must not run after model validation failure"
    );
}

/// Risk: T9 (missing transcript still maps to unsupported storage through DI locate path)
/// Source: proposal §8 T9; B3 contract §6 session metadata missing transcript
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn session_metadata_locator_missing_path_maps_to_unsupported_storage() {
    let prepared = prepared_claude_replace_fixture();
    let missing = prepared.fixture.root().join("missing-transcript.jsonl");
    let db = prepared.fixture.open_db();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();
    runner.push_stdout(missing.to_string_lossy().as_bytes());

    let err = locate_session_metadata(
        &db,
        &model_repo,
        &provider_source,
        &sessions_source,
        &runner,
        &prepared.session_id,
    )
    .unwrap_err();

    match err {
        MetadataError::UnsupportedStorage { reason, .. } => {
            assert!(
                reason.contains("missing") || reason.contains("path"),
                "{reason}"
            );
        }
        other => panic!("expected UnsupportedStorage, got {other:?}"),
    }
}
