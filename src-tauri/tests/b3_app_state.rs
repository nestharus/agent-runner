#![cfg(unix)]

mod fixtures;

use agent_runner_lib::test_support::AppStateCommandHarness;
use fixtures::b2_process_runner::success_output;
use fixtures::b3_app_state::FixtureRuntimePaths;
use serde_json::json;

/// Risk: T8 (AppState model commands use repository as authority, not stale raw model cache)
/// Source: proposal §8 T8; B3 contract §3 AppState command rules
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs
#[test]
fn app_state_list_models_observes_model_repository_file_effects() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());
    let harness = AppStateCommandHarness::new(paths);
    harness.write_model_toml("alpha", "[[providers]]\nname = \"claude\"\n");

    let models = harness.invoke_json("list_models", json!({})).unwrap();

    assert_eq!(models.as_array().unwrap().len(), 1);
    assert_eq!(models[0]["name"], "alpha");
}

/// Risk: T14 (refresh_quotas command uses AppState provider source, opener, runner, and in-flight guard)
/// Source: proposal §8 T14; B3 contract §3 AppState command rules
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn app_state_refresh_quotas_uses_injected_runner_and_persists_updated_status() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());
    let harness = AppStateCommandHarness::new(paths);
    harness.write_providers_toml(
        r#"[claude]
quota_script = "quota command"
"#,
    );
    harness.runner().push_response(Ok(success_output(
        br#"{"windows":[{"used_percent":12,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
    )));

    let response = harness
        .invoke_json("refresh_quotas", json!({ "providers": ["claude"] }))
        .unwrap();

    assert_eq!(response[0]["provider"], "claude");
    assert_eq!(response[0]["status"], "updated");
    assert_eq!(harness.runner().only_call().program, "sh");
    assert_eq!(harness.quota_window_count("claude"), 1);
}

/// Risk: T15 (test_model command uses injected balancer/executor path and quota repository)
/// Source: proposal §8 T15; B3 contract §3 AppState command rules
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn app_state_test_model_marks_selected_provider_exhausted_on_quota_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());
    let harness = AppStateCommandHarness::new(paths);
    harness.write_model_toml("alpha", "[[providers]]\nname = \"claude\"\n");
    harness.write_providers_toml("[claude]\ncommand = \"claude\"\n");
    harness.runner().push_stderr_exit("quota exceeded", 1);

    let response = harness
        .invoke_json("test_model", json!({ "model": "alpha" }))
        .unwrap();

    assert_eq!(response["success"], false);
    assert!(harness.provider_is_exhausted("claude"));
}

/// Risk: T17 (discover_models_cmd keeps stale deletion conditional in AppState wiring)
/// Source: proposal §8 T17; B3 contract §5 discover_models_cmd hookpoint
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b3_app_state.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn app_state_discover_models_deletes_stale_rows_only_after_non_empty_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let paths = FixtureRuntimePaths::new(dir.path());
    let harness = AppStateCommandHarness::new(paths);
    harness.seed_discovered_model("old-model", "claude", "old-version");
    harness.runner().push_stdout(b"claude 2.0.0\n");
    harness.runner().push_stdout(b"claude-3-opus\n");

    let response = harness
        .invoke_json("discover_models_cmd", json!({ "provider": "claude" }))
        .unwrap();

    assert_eq!(response["provider"], "claude");
    assert_eq!(harness.discovered_model_count("claude"), 1);
    assert!(harness.has_discovered_model("claude-3-opus", "claude"));
}
