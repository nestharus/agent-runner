#![cfg(unix)]

#[path = "fixtures/b3_app_state.rs"]
mod b3_app_state;

use agent_runner_schema_probe::run_schema_probe_with_deps;
use agent_runner_state::DefaultStateDbOpener;
use b3_app_state::B3RuntimePaths;

/// Risk: T11 - schema-probe can keep deriving the default state path or use a
/// mutating open while the new dependency-aware wrapper exists.
/// Level: component.
/// Source: proposal §8 T11; B3 contract §3 `schema_probe`.
/// Observable: missing injected DB path returns the existing missing report and
/// does not create the parent directory.
#[test]
fn run_schema_probe_with_deps_uses_injected_path_and_does_not_create_missing_db() {
    let dir = tempfile::tempdir().unwrap();
    let paths = B3RuntimePaths::under(dir.path());
    let opener = DefaultStateDbOpener;

    let report = run_schema_probe_with_deps(&paths, &opener).unwrap();

    let json = serde_json::to_value(report).unwrap();
    assert_eq!(
        json["state_db"]["path"],
        paths.state_db_path.to_string_lossy().as_ref()
    );
    assert_eq!(json["state_db"]["exists"], false);
    assert!(!paths.state_db_path.exists());
    assert!(!paths.state_db_path.parent().unwrap().exists());
}
