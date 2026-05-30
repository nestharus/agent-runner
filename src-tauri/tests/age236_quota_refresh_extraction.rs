//! ## Declared roles
//!
//! `validator`, `mapper`, `accessor`

use agent_runner_lib::AppState;
use agent_runner_lib::commands::quota_refresh::{
    QuotaRefreshEntry, QuotaRefreshWindow, refresh_quotas_inner,
};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn test_state(models_dir: PathBuf, models: HashMap<String, ModelConfig>) -> AppState {
    AppState::test_default(models_dir, models)
}

fn make_model(name: &str, commands: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: commands
            .iter()
            .map(|command| ProviderConfig::new((*command).to_string(), vec![]))
            .collect(),
        inputs: vec![],
        provider: None,
    }
}

fn write_providers(root: &Path, content: &str) {
    std::fs::write(root.join("providers.toml"), content).unwrap();
}

fn provider_status<'a>(results: &'a [QuotaRefreshEntry], provider_name: &str) -> &'a str {
    results
        .iter()
        .find(|entry| entry.provider_name == provider_name)
        .unwrap()
        .status
        .as_str()
}

#[cfg(unix)]
fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, body).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn refresh_quotas_filters_to_multi_provider_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([
            (
                "single".to_string(),
                make_model("single", &["single-provider"]),
            ),
            (
                "multi".to_string(),
                make_model("multi", &["multi-a", "multi-b"]),
            ),
        ]),
    );

    let results = refresh_quotas_inner(&state).unwrap();

    assert_eq!(
        results
            .iter()
            .map(|entry| entry.provider_name.as_str())
            .collect::<Vec<_>>(),
        vec!["multi-a", "multi-b"]
    );
    assert!(results.iter().all(|entry| entry.status == "no_script"));
}

#[test]
fn refresh_quotas_skips_fresh_providers() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([(
            "multi".to_string(),
            make_model("multi", &["fresh-provider", "stale-provider"]),
        )]),
    );
    let db = StateDb::open(&state.db_path()).unwrap();
    db.upsert_quota_refresh(
        "fresh-provider",
        &[QuotaWindowInput {
            used_percent: 0.20,
            resets_at: chrono::Utc::now() + chrono::Duration::hours(24),
        }],
    )
    .unwrap();
    drop(db);

    let results = refresh_quotas_inner(&state).unwrap();

    assert_eq!(provider_status(&results, "fresh-provider"), "fresh");
    assert_eq!(provider_status(&results, "stale-provider"), "no_script");
    assert!(
        results
            .iter()
            .find(|entry| entry.provider_name == "fresh-provider")
            .unwrap()
            .windows
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn age38_refresh_quotas_keeps_fresh_gate_before_quota_service() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("fresh-provider-called");
    let script = dir.path().join("fresh-quota.sh");
    write_executable(
        &script,
        &format!(
            r#"#!/usr/bin/env bash
touch '{}'
printf '%s\n' '{{"windows":[{{"used_percent":42,"resets_at":"2099-01-01T00:00:00Z"}}]}}'
"#,
            marker.display()
        ),
    );
    write_providers(
        dir.path(),
        &format!(
            r#"[fresh-provider]
quota_script = "{}"
"#,
            script.display()
        ),
    );
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([(
            "multi".to_string(),
            make_model("multi", &["fresh-provider", "stale-provider"]),
        )]),
    );
    let db = StateDb::open(&state.db_path()).unwrap();
    db.upsert_quota_refresh(
        "fresh-provider",
        &[QuotaWindowInput {
            used_percent: 0.10,
            resets_at: chrono::Utc::now() + chrono::Duration::hours(24),
        }],
    )
    .unwrap();
    drop(db);

    let results = refresh_quotas_inner(&state).unwrap();

    assert_eq!(provider_status(&results, "fresh-provider"), "fresh");
    assert!(
        !marker.exists(),
        "fresh provider must not call the quota service"
    );
}

#[test]
fn age38_refresh_quotas_wraps_db_open_error_before_quota_service() {
    let dir = tempfile::tempdir().unwrap();
    let not_a_directory = dir.path().join("not-a-directory");
    std::fs::write(&not_a_directory, "file").unwrap();
    let state = test_state(
        not_a_directory.join("models"),
        HashMap::from([(
            "multi".to_string(),
            make_model("multi", &["age38-a", "age38-b"]),
        )]),
    );

    let error = refresh_quotas_inner(&state).unwrap_err();

    assert!(
        error.starts_with("Failed to open state DB:"),
        "unexpected error: {error}"
    );
}

#[cfg(unix)]
#[test]
fn age38_refresh_quotas_routes_load_open_and_refresh_through_real_paths() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("updated-quota.sh");
    write_executable(
        &script,
        r#"#!/usr/bin/env bash
printf '%s\n' '{"windows":[{"used_percent":42,"resets_at":"2099-01-01T00:00:00Z"}]}'
"#,
    );
    write_providers(
        dir.path(),
        &format!(
            r#"[age38-a]
quota_script = "{}"

[age38-b]
"#,
            script.display()
        ),
    );
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([(
            "multi".to_string(),
            make_model("multi", &["age38-b", "age38-a"]),
        )]),
    );

    let results = refresh_quotas_inner(&state).unwrap();

    assert!(dir.path().join("state.db").exists());
    assert_eq!(
        results
            .iter()
            .map(|entry| entry.provider_name.as_str())
            .collect::<Vec<_>>(),
        vec!["age38-a", "age38-b"]
    );
    assert_eq!(provider_status(&results, "age38-a"), "updated");
    assert_eq!(provider_status(&results, "age38-b"), "no_script");
}

#[cfg(unix)]
#[test]
fn refresh_quotas_marks_in_flight_providers() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("quota.sh");
    write_executable(
        &script,
        r#"#!/usr/bin/env bash
printf '%s\n' '{"windows":[{"used_percent":42,"resets_at":"2099-01-01T00:00:00Z"}]}'
"#,
    );
    write_providers(
        dir.path(),
        &format!(
            r#"[in-flight-provider]
quota_script = "{}"
"#,
            script.display()
        ),
    );
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([(
            "multi".to_string(),
            make_model("multi", &["in-flight-provider", "other-provider"]),
        )]),
    );
    let _guard = state
        .quota_in_flight
        .try_claim("in-flight-provider")
        .unwrap();

    let results = refresh_quotas_inner(&state).unwrap();

    let entry = results
        .iter()
        .find(|entry| entry.provider_name == "in-flight-provider")
        .unwrap();
    assert_eq!(entry.status, "in_flight");
    assert!(entry.windows.is_empty());
}

#[cfg(unix)]
#[test]
fn refresh_quotas_maps_refresh_outcome_to_dto() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("quota.sh");
    write_executable(
        &script,
        r#"#!/usr/bin/env bash
printf '%s\n' '{"windows":[{"used_percent":42,"resets_at":"2099-01-01T00:00:00Z"}]}'
"#,
    );
    write_providers(
        dir.path(),
        &format!(
            r#"[updated-provider]
quota_script = "{}"

[no-script-provider]
"#,
            script.display()
        ),
    );
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([(
            "multi".to_string(),
            make_model("multi", &["updated-provider", "no-script-provider"]),
        )]),
    );

    let results = refresh_quotas_inner(&state).unwrap();

    let updated = results
        .iter()
        .find(|entry| entry.provider_name == "updated-provider")
        .unwrap();
    assert_eq!(updated.status, "updated");
    assert_eq!(updated.windows.len(), 1);
    assert!((updated.windows[0].used_percent - 0.42).abs() < 1e-6);
    assert_eq!(updated.windows[0].resets_at, "2099-01-01T00:00:00+00:00");

    let no_script = results
        .iter()
        .find(|entry| entry.provider_name == "no-script-provider")
        .unwrap();
    assert_eq!(no_script.status, "no_script");
    assert!(no_script.windows.is_empty());
    assert!(no_script.message.is_none());
}

#[cfg(unix)]
#[test]
fn refresh_quotas_maps_failed_outcome_to_dto() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([(
            "multi".to_string(),
            make_model("multi", &["failed-provider", "other-provider"]),
        )]),
    );
    write_providers(
        dir.path(),
        r#"[failed-provider]
quota_script = "printf 'quota denied\n' >&2; exit 1"
"#,
    );

    let results = refresh_quotas_inner(&state).unwrap();

    let failed = results
        .iter()
        .find(|entry| entry.provider_name == "failed-provider")
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert!(failed.windows.is_empty());
    assert!(
        failed
            .message
            .as_deref()
            .is_some_and(|message| message.contains("quota denied"))
    );
}

#[test]
fn quota_refresh_entry_serializes_stable_backend_shape() {
    let entry = QuotaRefreshEntry {
        provider_name: "provider-a".to_string(),
        status: "updated".to_string(),
        windows: vec![QuotaRefreshWindow {
            used_percent: 0.42,
            resets_at: "2099-01-01T00:00:00+00:00".to_string(),
        }],
        message: None,
    };

    let json = serde_json::to_value(entry).unwrap();

    assert_eq!(json["provider_name"], "provider-a");
    assert_eq!(json["status"], "updated");
    assert_eq!(json["windows"][0]["used_percent"], 0.42);
    assert_eq!(json["windows"][0]["resets_at"], "2099-01-01T00:00:00+00:00");
    assert!(json["message"].is_null());
}
