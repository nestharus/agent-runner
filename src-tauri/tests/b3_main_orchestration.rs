#![cfg(unix)]

mod fixtures;

use agent_runner_lib::runtime::{RuntimePaths, cli_services_for_paths};
use fixtures::b3_app_state::FixtureRuntimePaths;

/// Risk: T13 (CLI composition root shares RuntimePaths without drifting state/config defaults)
/// Source: proposal §8 T13; B3 contract §3 main.rs CLI orchestration
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn cli_services_model_dir_override_does_not_move_state_db_path() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());
    let override_models = dir.path().join("custom-models");

    let services = cli_services_for_paths(paths.clone(), Some(&override_models)).unwrap();

    assert_eq!(services.paths.models_dir(), override_models);
    assert_eq!(
        services.paths.state_db_path().unwrap(),
        paths.state_db_path().unwrap()
    );
    assert_eq!(services.paths.providers_path(), paths.providers_path());
    assert_eq!(services.paths.sessions_path(), paths.sessions_path());
}

/// Risk: T13 (CLI service bundle contains the same shared services as AppState except setup channel)
/// Source: proposal §8 T13; B3 contract §3 main.rs CLI orchestration
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn cli_services_bundle_exposes_session_and_process_dependencies_for_dispatch_paths() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());

    let services = cli_services_for_paths(paths, None).unwrap();

    services.state_opener.default_path().unwrap();
    services.model_repo.load_models().unwrap();
    services.provider_source.load_providers().unwrap();
    services.sessions_source.load_sessions().unwrap();
    let _runner = services.process_runner.clone();
    let _lock_provider = services.lock_provider.clone();
}
