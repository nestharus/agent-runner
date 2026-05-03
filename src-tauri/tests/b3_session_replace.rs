#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;
#[path = "fixtures/b3_app_state.rs"]
mod b3_app_state;
mod fixtures;

use agent_runner_config::{
    FilesystemModelConfigRepository, FilesystemProviderConfigSource, FilesystemSessionsConfigSource,
};
use agent_runner_session::{FilesystemSessionLockProvider, SessionLockProvider};
use agent_runner_session::{ImportReplaceDeps, run_import_replace_with_deps};
use agent_runner_state::DefaultStateDbOpener;
use b2_process_runner::{FakeProcessRunner, ok_output};
use b3_app_state::B3RuntimePaths;
use fixtures::initiative_06_import_replace::{canonical_jsonl, prepared_claude_replace_fixture};
use std::{fs, time::Duration};

fn deps<'a>(
    paths: &'a B3RuntimePaths,
    opener: &'a DefaultStateDbOpener,
    models: &'a FilesystemModelConfigRepository,
    providers: &'a FilesystemProviderConfigSource,
    sessions: &'a FilesystemSessionsConfigSource,
    runner: &'a FakeProcessRunner,
    locks: &'a dyn SessionLockProvider,
) -> ImportReplaceDeps<'a> {
    ImportReplaceDeps {
        paths,
        state_opener: opener,
        model_repo: models,
        provider_source: providers,
        sessions_source: sessions,
        locator_runner: runner,
        lock_provider: locks,
    }
}

/// Risk: T10 - import-replace may keep deriving default roots or opening raw
/// SQLite after dependency-aware entry points are added.
/// Level: component.
/// Source: proposal §8 T10; B3 contract §3 `session_replace`.
/// Observable: `run_import_replace_with_deps` uses injected paths/repos/runner
/// and completes a valid replace without relying on process environment roots.
#[test]
fn run_import_replace_with_deps_uses_injected_dependency_bundle() {
    let prepared = prepared_claude_replace_fixture();

    let paths = B3RuntimePaths {
        data_root: prepared.fixture.data_home().join("oulipoly-agent-runner"),
        config_root: prepared
            .fixture
            .models_dir()
            .parent()
            .unwrap()
            .to_path_buf(),
        models_dir: prepared.fixture.models_dir().to_path_buf(),
        agents_dir: prepared
            .fixture
            .models_dir()
            .parent()
            .unwrap()
            .join("agents"),
        state_db_path: prepared.fixture.db_path(),
        providers_path: prepared.fixture.providers_path(),
        sessions_path: prepared.fixture.sessions_path(),
        lock_dir: prepared.fixture.locks_dir(),
        replace_journal_dir: prepared.fixture.replace_journal_dir(),
    };
    let opener = DefaultStateDbOpener;
    let models = FilesystemModelConfigRepository::new(paths.models_dir.clone());
    let providers = FilesystemProviderConfigSource::new(paths.providers_path.clone());
    let sessions = FilesystemSessionsConfigSource::new(paths.sessions_path.clone());
    let runner = FakeProcessRunner::with_responses(vec![ok_output(
        format!("{}\n", prepared.jsonl_path.display()),
        "",
        0,
    )]);
    let locks = FilesystemSessionLockProvider;
    let input = prepared.fixture.stage_jsonl(
        "replacement.jsonl",
        &canonical_jsonl(
            &prepared.session_id,
            &prepared.provider_name,
            &prepared.jsonl_path,
            "b3-deps",
        ),
    );

    let receipt = run_import_replace_with_deps(
        &deps(
            &paths, &opener, &models, &providers, &sessions, &runner, &locks,
        ),
        &prepared.session_id,
        Some(&input),
        None,
    )
    .unwrap();

    assert_eq!(receipt.session_id, prepared.session_id);
    assert_eq!(
        prepared
            .fixture
            .turn_rows(&prepared.provider_name, &prepared.session_id)
            .len(),
        2
    );
    assert!(
        !prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists()
    );
    assert!(
        String::from_utf8(fs::read(&prepared.jsonl_path).unwrap())
            .unwrap()
            .contains("b3-deps assistant")
    );
}

/// Risk: T10/A5 - a dependency refactor can write the pending journal before
/// the session lock is acquired, changing recovery semantics for busy sessions.
/// Level: component.
/// Source: B3 contract §6 session replace edge cases.
/// Observable: when the filesystem lock is already held, replace returns a
/// lock error and does not leave a pending journal.
#[test]
fn busy_lock_returns_before_pending_journal_is_written() {
    let prepared = fixtures::initiative_06_import_replace::prepared_claude_replace_fixture();
    let paths = B3RuntimePaths {
        data_root: prepared.fixture.data_home().join("oulipoly-agent-runner"),
        config_root: prepared
            .fixture
            .models_dir()
            .parent()
            .unwrap()
            .to_path_buf(),
        models_dir: prepared.fixture.models_dir().to_path_buf(),
        agents_dir: prepared
            .fixture
            .models_dir()
            .parent()
            .unwrap()
            .join("agents"),
        state_db_path: prepared.fixture.db_path(),
        providers_path: prepared.fixture.providers_path(),
        sessions_path: prepared.fixture.sessions_path(),
        lock_dir: prepared.fixture.locks_dir(),
        replace_journal_dir: prepared.fixture.replace_journal_dir(),
    };
    let opener = DefaultStateDbOpener;
    let models = FilesystemModelConfigRepository::new(paths.models_dir.clone());
    let providers = FilesystemProviderConfigSource::new(paths.providers_path.clone());
    let sessions = FilesystemSessionsConfigSource::new(paths.sessions_path.clone());
    let runner = FakeProcessRunner::new();
    let locks = FilesystemSessionLockProvider;
    let lock = agent_runner_session::SessionLock::new(&paths.lock_dir).unwrap();
    let held = lock
        .acquire(
            &prepared.session_id,
            &prepared.provider_name,
            Duration::from_secs(60),
        )
        .unwrap();
    let input_path = prepared.fixture.stage_jsonl(
        "busy.jsonl",
        &canonical_jsonl(
            &prepared.session_id,
            &prepared.provider_name,
            &prepared.jsonl_path,
            "busy-lock",
        ),
    );

    let result = run_import_replace_with_deps(
        &deps(
            &paths, &opener, &models, &providers, &sessions, &runner, &locks,
        ),
        &prepared.session_id,
        Some(&input_path),
        None,
    );

    drop(held);
    assert!(result.is_err(), "busy lock should fail replace");
    assert!(
        !prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists()
    );
}
