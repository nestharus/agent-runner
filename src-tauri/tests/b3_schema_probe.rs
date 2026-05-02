#![cfg(unix)]

mod fixtures;

use agent_runner_lib::runtime::RuntimePaths;
use agent_runner_lib::schema_probe::run_schema_probe_with_deps;
use agent_runner_lib::state::DefaultStateDbOpener;
use fixtures::b3_app_state::FixtureRuntimePaths;
use fixtures::initiative_06_schema_probe::{CURRENT_SCHEMA_VERSION, create_current_schema_db_at};

/// Risk: T11 (schema probe uses injected paths/opener and remains read-only for missing DBs)
/// Source: proposal §8 T11; B3 contract §3 schema_probe::run_schema_probe_with_deps
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs; src-tauri/tests/fixtures/initiative_06_schema_probe.rs
#[test]
fn schema_probe_with_deps_missing_db_returns_missing_report_without_creating_parent() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());
    let opener = DefaultStateDbOpener::default();
    let state_parent = paths
        .state_db_path()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let report = run_schema_probe_with_deps(&paths, &opener).unwrap();

    assert_eq!(report.state_db.path, paths.state_db_path().unwrap());
    assert!(!report.state_db.exists);
    assert!(!report.state_db.compatible);
    assert!(
        !state_parent.exists(),
        "read-only probe must not create missing state parent"
    );
}

/// Risk: T11/T13 (schema probe CLI wiring can share RuntimePaths without default path drift)
/// Source: proposal §8 T11/T13; B3 contract §3 schema_probe::run_schema_probe_with_deps
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs; src-tauri/tests/fixtures/initiative_06_schema_probe.rs
#[test]
fn schema_probe_with_deps_reads_runtime_path_state_db() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());
    create_current_schema_db_at(&paths.state_db_path().unwrap());
    let opener = DefaultStateDbOpener::default();

    let report = run_schema_probe_with_deps(&paths, &opener).unwrap();

    assert_eq!(report.state_db.path, paths.state_db_path().unwrap());
    assert!(report.state_db.exists);
    assert_eq!(report.state_db.user_version, CURRENT_SCHEMA_VERSION);
    assert!(report.state_db.compatible);
}
