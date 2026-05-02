#![cfg(unix)]

mod fixtures;

use agent_runner_lib::config::{
    FilesystemModelConfigRepository, FilesystemProviderConfigSource, FilesystemSessionsConfigSource,
};
use agent_runner_lib::session_lock::FilesystemSessionLockProvider;
use agent_runner_lib::session_replace::{
    ImportReplaceDeps, recover_pending_replaces_with_deps, run_import_replace_with_deps,
};
use agent_runner_lib::state::{
    DefaultStateDbOpener, SessionTurnReplacement, SessionTurnReplacementTurn, SessionTurnRepository,
};
use fixtures::b2_process_runner::FakeProcessRunner;
use fixtures::b3_app_state::FixtureRuntimePaths;
use fixtures::initiative_06_import_replace::{
    ImportReplaceFixture, assert_no_replace_journal_pollution, canonical_jsonl,
    prepared_claude_replace_fixture,
};
use std::fs;

fn runtime_paths(fixture: &ImportReplaceFixture) -> FixtureRuntimePaths {
    FixtureRuntimePaths::with_existing_paths(
        fixture.db_path().parent().unwrap().to_path_buf(),
        fixture.providers_path().parent().unwrap().to_path_buf(),
        fixture.models_dir().to_path_buf(),
        fixture.providers_path(),
        fixture.sessions_path(),
        fixture.db_path(),
    )
}

/// Risk: T10 (SessionTurnRepository owns replacement transaction semantics)
/// Source: proposal §8 T10; B3 contract §2 SessionTurnRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs
#[test]
fn session_turn_repository_replace_session_turns_deletes_inserts_and_refreshes_chain() {
    let prepared = prepared_claude_replace_fixture();
    let db = prepared.fixture.open_db();
    let repo: &dyn SessionTurnRepository = &db;
    let active_segment_id = prepared.fixture.active_segment_id(&prepared.chain_id);
    let replacement = SessionTurnReplacement {
        provider_name: prepared.provider_name.clone(),
        session_id: prepared.session_id.clone(),
        chain_id: prepared.chain_id.clone(),
        active_segment_id,
        source_file: prepared.jsonl_path.clone(),
        turns: vec![
            SessionTurnReplacementTurn {
                turn_id: "replacement-turn-1".to_string(),
                timestamp: "2026-04-17T09:00:00Z".to_string(),
                role: "user".to_string(),
            },
            SessionTurnReplacementTurn {
                turn_id: "replacement-turn-2".to_string(),
                timestamp: "2026-04-17T09:00:01Z".to_string(),
                role: "assistant".to_string(),
            },
        ],
    };

    repo.replace_session_turns(&replacement).unwrap();

    let rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].turn_id, "replacement-turn-1");
    assert_eq!(rows[1].turn_id, "replacement-turn-2");
    assert_eq!(
        prepared.fixture.segment_state(&prepared.chain_id)["last_turn_id"],
        "replacement-turn-2"
    );
    assert_eq!(
        prepared.fixture.chain_last_used_at(&prepared.chain_id),
        "2026-04-17T09:00:01Z"
    );
}

/// Risk: T10 (import-replace dependency bundle preserves successful mutation ordering)
/// Source: proposal §8 T10; B3 contract §3 session_replace::run_import_replace_with_deps
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn session_replace_with_deps_rewrites_transcript_replaces_db_and_cleans_journal() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "deps-success",
    );
    let input_path = prepared.fixture.stage_jsonl("deps-success.jsonl", &input);
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    let paths = runtime_paths(&prepared.fixture);
    let opener = DefaultStateDbOpener::default();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();
    runner.push_stdout(prepared.jsonl_path.to_string_lossy().as_bytes());
    runner.push_stdout(prepared.jsonl_path.to_string_lossy().as_bytes());
    runner.push_stdout(prepared.jsonl_path.to_string_lossy().as_bytes());
    let lock_provider = FilesystemSessionLockProvider::default();
    let deps = ImportReplaceDeps {
        paths: &paths,
        state_opener: &opener,
        model_repo: &model_repo,
        provider_source: &provider_source,
        sessions_source: &sessions_source,
        locator_runner: &runner,
        lock_provider: &lock_provider,
    };

    let _receipt =
        run_import_replace_with_deps(&deps, &prepared.session_id, Some(&input_path), None).unwrap();

    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_ne!(before.transcript_bytes, after.transcript_bytes);
    assert!(
        after
            .turn_rows
            .iter()
            .any(|row| row.turn_id == "deps-success-turn-1"),
        "{:?}",
        after.turn_rows
    );
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

/// Risk: T10 (lock acquisition failure returns before pending journal publication)
/// Source: proposal §8 T10; B3 contract §6 session replace lock-held edge
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn session_replace_with_deps_busy_lock_does_not_publish_pending_journal() {
    let prepared = prepared_claude_replace_fixture();
    let _lease = prepared
        .fixture
        .write_active_lock(&prepared.provider_name, &prepared.session_id);
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "busy",
    );
    let input_path = prepared.fixture.stage_jsonl("busy.jsonl", &input);
    let paths = runtime_paths(&prepared.fixture);
    let opener = DefaultStateDbOpener::default();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();
    runner.push_stdout(prepared.jsonl_path.to_string_lossy().as_bytes());
    let lock_provider = FilesystemSessionLockProvider::default();
    let deps = ImportReplaceDeps {
        paths: &paths,
        state_opener: &opener,
        model_repo: &model_repo,
        provider_source: &provider_source,
        sessions_source: &sessions_source,
        locator_runner: &runner,
        lock_provider: &lock_provider,
    };

    let err = run_import_replace_with_deps(&deps, &prepared.session_id, Some(&input_path), None)
        .unwrap_err();

    let err = format!("{err:?}");
    assert!(
        err.contains("Busy") || err.contains("session-busy"),
        "{err}"
    );
    assert!(
        !prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists()
    );
}

/// Risk: T10 (verification gates run before transcript and DB mutation)
/// Source: proposal §8 T10; B3 contract §3 import-replace dependency order
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn session_replace_with_deps_preimage_mismatch_leaves_transcript_and_db_unchanged() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "preimage",
    );
    let input_path = prepared.fixture.stage_jsonl("preimage.jsonl", &input);
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    let paths = runtime_paths(&prepared.fixture);
    let opener = DefaultStateDbOpener::default();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();
    runner.push_stdout(prepared.jsonl_path.to_string_lossy().as_bytes());
    let lock_provider = FilesystemSessionLockProvider::default();
    let deps = ImportReplaceDeps {
        paths: &paths,
        state_opener: &opener,
        model_repo: &model_repo,
        provider_source: &provider_source,
        sessions_source: &sessions_source,
        locator_runner: &runner,
        lock_provider: &lock_provider,
    };

    let err = run_import_replace_with_deps(
        &deps,
        &prepared.session_id,
        Some(&input_path),
        Some("0000000000000000000000000000000000000000000000000000000000000000"),
    )
    .unwrap_err();

    let err = format!("{err:?}");
    assert!(err.contains("preimage") || err.contains("hash"), "{err}");
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(before, after);
}

/// Risk: T10 (recovery uses injected dependency bundle and repository replacement)
/// Source: proposal §8 T10; B3 contract §3 session_replace::recover_pending_replaces_with_deps
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/initiative_06_import_replace.rs; src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn session_replace_recovery_with_deps_accepts_same_dependency_bundle() {
    let prepared = prepared_claude_replace_fixture();
    fs::create_dir_all(prepared.fixture.replace_journal_dir()).unwrap();
    let paths = runtime_paths(&prepared.fixture);
    let opener = DefaultStateDbOpener::default();
    let model_repo =
        FilesystemModelConfigRepository::new(prepared.fixture.models_dir().to_path_buf());
    let provider_source = FilesystemProviderConfigSource::new(prepared.fixture.providers_path());
    let sessions_source = FilesystemSessionsConfigSource::new(prepared.fixture.sessions_path());
    let runner = FakeProcessRunner::new();
    let lock_provider = FilesystemSessionLockProvider::default();
    let deps = ImportReplaceDeps {
        paths: &paths,
        state_opener: &opener,
        model_repo: &model_repo,
        provider_source: &provider_source,
        sessions_source: &sessions_source,
        locator_runner: &runner,
        lock_provider: &lock_provider,
    };

    recover_pending_replaces_with_deps(&deps).unwrap();

    assert!(prepared.fixture.replace_journal_dir().exists());
}
