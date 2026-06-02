use agent_runner_lib::setup::setup_provider_ops::{
    build_setup_provider_context,
    test_support::{SetupOperationFailure, SetupProviderFixture, SetupProviderFlowHarness},
};
use oulipoly_config::ProviderImplementationRef;
use oulipoly_config::app::SetupBrainConfig;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn setup_provider_operations_are_recorded_and_mapped_to_exact_host_context() {
    let provider = SetupProviderFixture::new("fixture-setup-provider")
        .with_detect_report(json!({
            "summary": "neutral detection complete",
            "items": [{"id": "fixture-tool", "installed": true}]
        }))
        .with_install_plan_steps(vec![
            json!({"id": "prepare", "label": "Prepare neutral setup"}),
            json!({"id": "verify", "label": "Verify neutral setup"}),
        ])
        .with_sync_plan_operations(vec![
            json!({"id": "sync-neutral-skill", "kind": "skill"}),
            json!({"id": "sync-neutral-mcp", "kind": "mcp"}),
        ])
        .with_discovered_accounts(vec!["profile-alpha", "profile-beta"]);

    let outcome = SetupProviderFlowHarness::configured("fixture-setup-brain")
        .with_setup_provider(provider)
        .run_until_first_brain_turn();

    assert_eq!(
        outcome.setup_provider_calls(),
        [
            "setup.detect",
            "setup.install_plan",
            "setup.sync_plan",
            "discovery.accounts"
        ]
    );

    let context = outcome.only_turn_request().host_context();
    assert_eq!(
        context.pointer("/setup/detect/summary"),
        Some(&json!("neutral detection complete"))
    );
    assert_eq!(
        context.pointer("/setup/install_plan/steps"),
        Some(&json!([
            {"id": "prepare", "label": "Prepare neutral setup"},
            {"id": "verify", "label": "Verify neutral setup"}
        ]))
    );
    assert_eq!(
        context.pointer("/setup/sync_plan/operations"),
        Some(&json!([
            {"id": "sync-neutral-skill", "kind": "skill"},
            {"id": "sync-neutral-mcp", "kind": "mcp"}
        ]))
    );
    assert_eq!(
        context.pointer("/discovery/accounts"),
        Some(&json!(["profile-alpha", "profile-beta"]))
    );
    assert_eq!(outcome.runner_recipe_fallbacks(), [] as [&str; 0]);
}

#[test]
fn production_setup_provider_context_invokes_contract_operations_through_artifact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let calls_path = temp.path().join("calls.txt");
    let script_path = temp.path().join("fixture-setup-provider.py");
    let script = format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

calls = pathlib.Path({calls_path:?})
request = json.load(sys.stdin)
operation = sys.argv[1]
with calls.open("a", encoding="utf-8") as handle:
    handle.write(operation + "\n")

if operation == "setup.detect":
    result = {{"installed": True, "binary": {{"path": "/fixture/bin/tool"}}, "auth": "ready", "profiles": [{{"id": "profile-alpha"}}], "warnings": []}}
elif operation == "setup.install_plan":
    result = {{"steps": [{{"id": "prepare", "label": "Prepare neutral setup"}}]}}
elif operation == "setup.sync_plan":
    result = {{"operations": [{{"id": "sync-neutral", "kind": "skill"}}], "diagnostics": []}}
elif operation == "discovery.accounts":
    result = {{"accounts": [{{"id": "profile-alpha"}}, {{"id": "profile-beta"}}], "warnings": []}}
else:
    print(json.dumps({{"contract": request["contract"], "request_id": request["request_id"], "ok": False, "error": {{"code": "unsupported", "category": "unsupported", "message": "unsupported operation", "retryable": False}}}}))
    sys.exit(0)

print(json.dumps({{"contract": request["contract"], "request_id": request["request_id"], "ok": True, "result": result}}))
"#,
        calls_path = calls_path.to_string_lossy()
    );
    fs::write(&script_path, script).expect("write fake setup provider");
    let mut permissions = fs::metadata(&script_path)
        .expect("metadata for fake setup provider")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).expect("chmod fake setup provider");

    let config = SetupBrainConfig {
        artifact: ProviderImplementationRef {
            path: None,
            crate_name: None,
            version: None,
            binary: None,
            script: Some(script_path.to_string_lossy().into_owned()),
        },
        settings_id: Some("fixture-settings".into()),
    };

    let context = build_setup_provider_context(&config);

    assert_eq!(context.diagnostics, []);
    assert_eq!(
        context.operation_calls,
        [
            "setup.detect",
            "setup.install_plan",
            "setup.sync_plan",
            "discovery.accounts"
        ]
    );
    assert_eq!(
        fs::read_to_string(calls_path).expect("read provider calls"),
        "setup.detect\nsetup.install_plan\nsetup.sync_plan\ndiscovery.accounts\n"
    );
    assert_eq!(
        context.context.pointer("/setup/detect/installed"),
        Some(&json!(true))
    );
    assert_eq!(
        context.context.pointer("/setup/install_plan/steps/0/id"),
        Some(&json!("prepare"))
    );
    assert_eq!(
        context.context.pointer("/setup/sync_plan/operations/0/id"),
        Some(&json!("sync-neutral"))
    );
    assert_eq!(
        context.context.pointer("/discovery/accounts/1/id"),
        Some(&json!("profile-beta"))
    );
}

#[test]
fn unsupported_setup_operation_records_neutral_diagnostic_without_recipe_fallback() {
    let provider = SetupProviderFixture::new("fixture-setup-provider")
        .with_failure("setup.install_plan", SetupOperationFailure::Unsupported);

    let outcome = SetupProviderFlowHarness::configured("fixture-setup-brain")
        .with_setup_provider(provider)
        .run_until_first_brain_turn();

    assert_eq!(outcome.diagnostics().len(), 1);
    assert_eq!(outcome.diagnostics()[0].kind, "unsupported_setup_operation");
    assert_eq!(outcome.diagnostics()[0].operation, "setup.install_plan");
    assert_eq!(outcome.diagnostics()[0].fallback_used, false);
    assert_eq!(outcome.runner_recipe_fallbacks(), [] as [&str; 0]);
    assert!(
        outcome
            .only_turn_request()
            .host_context()
            .get("setup")
            .is_some()
    );
}

#[test]
fn setup_provider_error_records_neutral_diagnostic_without_recipe_fallback() {
    let provider = SetupProviderFixture::new("fixture-setup-provider").with_failure(
        "discovery.accounts",
        SetupOperationFailure::ProviderError {
            message: "fixture account discovery failure".into(),
        },
    );

    let outcome = SetupProviderFlowHarness::configured("fixture-setup-brain")
        .with_setup_provider(provider)
        .run_until_first_brain_turn();

    assert_eq!(outcome.diagnostics().len(), 1);
    assert_eq!(outcome.diagnostics()[0].kind, "setup_provider_error");
    assert_eq!(outcome.diagnostics()[0].operation, "discovery.accounts");
    assert_eq!(outcome.diagnostics()[0].fallback_used, false);
    assert_eq!(outcome.runner_recipe_fallbacks(), [] as [&str; 0]);
    assert!(
        outcome
            .only_turn_request()
            .host_context()
            .pointer("/discovery/accounts")
            .is_none()
    );
}
