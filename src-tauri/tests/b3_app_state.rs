#![cfg(unix)]

#[path = "fixtures/b3_app_state.rs"]
mod b3_app_state;

use agent_runner_lib::process::StdinSpec;
use agent_runner_lib::state::{CliProviderRepository, DiscoveryRepository, QuotaRepository};
use b3_app_state::{B3ServiceBundle, invoke_json, ok_output};
use serde_json::json;

fn quota_json() -> &'static str {
    r#"[{"window_id":0,"used_percent":0.25,"resets_at":"2027-01-01T00:00:00Z"}]"#
}

/// Risk: T14 - `refresh_quotas` can keep constructing concrete paths/runners
/// inside the command even after `AppState` stores services.
/// Level: hookpoint via real Tauri IPC.
/// Source: proposal §8 T14; B3 contract §4 `lib.rs::refresh_quotas`.
/// Observable: real command handler uses injected provider source, state
/// opener, runner, and in-flight guard, then persists quota windows.
#[test]
fn refresh_quotas_command_uses_app_state_services() {
    let bundle = B3ServiceBundle::new();
    bundle.write_providers(
        r#"[claude]
quota_script = "quota-json"
"#,
    );
    bundle.runner.push_response(ok_output(quota_json(), "", 0));
    let (_app, webview) = bundle.mock_app();

    let response =
        invoke_json(&webview, "refresh_quotas", json!({"providers": ["claude"]})).unwrap();

    assert!(response.to_string().contains("updated"), "{response}");
    assert_eq!(
        bundle.open_state_db().get_windows("claude").unwrap().len(),
        1
    );
    let call = bundle.runner.single_call();
    assert_eq!(call.program, "sh");
    assert_eq!(call.args, ["-c", "quota-json"]);
}

/// Drift pin F-01: pre-B `refresh_quotas` accepted only `state`; an incoming
/// `providers` field was unknown and had no observable effect.
#[test]
fn refresh_quotas_ignores_providers_ipc_field_and_refreshes_all_candidates() {
    let bundle = B3ServiceBundle::new();
    bundle.write_model(
        "multi-provider-model",
        r#"[[providers]]
name = "claude"

[[providers]]
name = "codex"
"#,
    );
    bundle.write_providers(
        r#"[claude]
quota_script = "claude-quota"

[codex]
quota_script = "codex-quota"
"#,
    );
    bundle.runner.push_response(ok_output(quota_json(), "", 0));
    bundle.runner.push_response(ok_output(quota_json(), "", 0));
    let (_app, webview) = bundle.mock_app();

    let response =
        invoke_json(&webview, "refresh_quotas", json!({"providers": ["claude"]})).unwrap();
    let mut providers: Vec<_> = response
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["provider_name"].as_str().unwrap().to_string())
        .collect();
    providers.sort();

    assert_eq!(providers, ["claude", "codex"]);
    assert_eq!(
        bundle.open_state_db().get_windows("claude").unwrap().len(),
        1
    );
    assert_eq!(
        bundle.open_state_db().get_windows("codex").unwrap().len(),
        1
    );
}

/// Risk: T15 - `test_model` can bypass the AppState process runner while still
/// returning a successful command response through the GUI.
/// Level: hookpoint via real Tauri IPC.
/// Source: proposal §8 T15; B3 contract §4 `lib.rs::test_model`.
/// Observable: real command handler loads the model from the repository and
/// sends the representative prompt through the injected runner.
#[test]
fn test_model_command_uses_model_repo_and_process_runner() {
    let bundle = B3ServiceBundle::new();
    bundle.write_model(
        "fixture-model",
        r#"[[providers]]
name = "fixture-cli"
"#,
    );
    bundle.write_providers(
        r#"[fixture-cli]
command = "fixture-cli"
prompt_mode = "stdin"
"#,
    );
    bundle.runner.push_response(ok_output("model ok\n", "", 0));
    let (_app, webview) = bundle.mock_app();

    let response = invoke_json(
        &webview,
        "test_model",
        json!({"modelName": "fixture-model"}),
    )
    .unwrap();

    assert!(response.to_string().contains("model ok"), "{response}");
    let call = bundle.runner.single_call();
    assert_eq!(call.program, "fixture-cli");
    let StdinSpec::Bytes(stdin) = call.stdin else {
        panic!("expected representative test prompt on stdin: {call:?}");
    };
    let stdin = String::from_utf8(stdin).unwrap();
    assert!(stdin.contains("Say hello"), "expected test prompt: {stdin}");
}

/// Drift pin F-02: pre-B `test_model` accepted `name` only; `modelName` was an
/// unknown IPC field, not an alias.
#[test]
fn test_model_ignores_model_name_ipc_field_and_rejects_alias_only_request() {
    let bundle = B3ServiceBundle::new();
    bundle.write_model(
        "name-model",
        r#"[[providers]]
name = "name-cli"
"#,
    );
    bundle.write_model(
        "alias-model",
        r#"[[providers]]
name = "alias-cli"
"#,
    );
    bundle.write_providers(
        r#"[name-cli]
command = "name-cli"
prompt_mode = "stdin"

[alias-cli]
command = "alias-cli"
prompt_mode = "stdin"
"#,
    );
    bundle.runner.push_response(ok_output("name model ok\n", "", 0));
    let (_app, webview) = bundle.mock_app();

    let response = invoke_json(
        &webview,
        "test_model",
        json!({"name": "name-model", "modelName": "alias-model"}),
    )
    .unwrap();

    assert!(response.to_string().contains("name model ok"), "{response}");
    let calls = bundle.runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "name-cli");

    let err = invoke_json(&webview, "test_model", json!({"modelName": "alias-model"}))
        .expect_err("modelName-only request should be rejected because name is required");
    assert!(
        err.to_string().contains("name") || err.to_string().contains("Model"),
        "unexpected error response: {err}"
    );
    assert_eq!(
        bundle.runner.calls().len(),
        1,
        "alias-only request must not execute a model"
    );
}

/// Risk: T16 - `sync_provider` can keep calling the old no-runner detection
/// wrapper, making AppState process injection irrelevant for user syncs.
/// Level: hookpoint via real Tauri IPC.
/// Source: proposal §8 T16; B3 contract §4 `lib.rs::sync_provider`.
/// Observable: real command handler detects Claude through the injected runner
/// and persists the synced provider record.
#[test]
fn sync_provider_command_uses_injected_detection_runner_and_state_repo() {
    let bundle = B3ServiceBundle::new();
    bundle
        .runner
        .push_response(ok_output("/usr/bin/claude\n", "", 0));
    bundle
        .runner
        .push_response(ok_output("claude 1.2.3\n", "", 0));
    bundle
        .runner
        .push_response(ok_output("Logged in as user@example.com\n", "", 0));
    let (_app, webview) = bundle.mock_app();

    let response = invoke_json(&webview, "sync_provider", json!({"cliName": "claude"})).unwrap();

    assert!(response.to_string().contains("Anthropic"), "{response}");
    let db = bundle.open_state_db();
    assert!(db.get_cli_provider("claude").unwrap().is_some());
    assert!(
        bundle
            .runner
            .calls()
            .iter()
            .any(|call| call.program == "claude" && call.args == ["auth", "status"])
    );
}

/// Risk: T17 - `discover_models_cmd` can call the discovery wrapper with an OS
/// runner or delete stale rows even when discovery returns an empty model list.
/// Level: hookpoint via real Tauri IPC.
/// Source: proposal §8 T17; B3 contract §4 `lib.rs::discover_models_cmd`.
/// Observable: real command handler uses the injected runner and preserves the
/// current OK-empty discovery behavior.
#[test]
fn discover_models_command_uses_runner_and_preserves_empty_discovery_success() {
    let bundle = B3ServiceBundle::new();
    bundle
        .runner
        .push_response(ok_output("claude 1.2.3\n", "", 0));
    bundle
        .runner
        .push_response(ok_output("", "no models from strategy one", 1));
    bundle.runner.push_response(ok_output("", "", 1));
    bundle.runner.push_response(ok_output("", "", 1));
    let (_app, webview) = bundle.mock_app();

    let response = invoke_json(
        &webview,
        "discover_models_cmd",
        json!({"cliName": "claude"}),
    )
    .unwrap();

    assert_eq!(response, json!([]));
    assert!(
        bundle
            .open_state_db()
            .list_discovered_models(Some("claude"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(bundle.runner.calls()[0].program, "claude");
    assert_eq!(bundle.runner.calls()[0].args, ["--version"]);
}
