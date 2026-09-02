#![cfg(unix)]

//! A sibling-account retry requires invocation lifecycle transfer authority.
//! Until that authority exists, rotatable transport failures must fail closed
//! without invoking or persisting a sibling account.
//!
//! The fake provider spawned here is the single shared artifact for every
//! account in the model; it branches on `params.settings_id` (the per-account
//! identity) so one account can hang past the host handshake timeout or launch
//! heartbeat gap while a sibling account answers immediately.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry,
    ProvidersConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_provider::client::ProviderClientOptions;
use oulipoly_runtime::executor;
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest, ServiceError};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(1);
const SLOW_SLEEP_SECONDS: u64 = 5;
static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

struct Fixture {
    _dir: tempfile::TempDir,
    provider_path: PathBuf,
    order_path: PathBuf,
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fake provider");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod fake provider");
}

fn make_fixture(slow: &[&str], unavailable: &[&str]) -> Fixture {
    make_fixture_with_launch_stalls(slow, unavailable, &[])
}

fn make_fixture_with_launch_stalls(
    slow: &[&str],
    unavailable: &[&str],
    launch_stalls: &[&str],
) -> Fixture {
    TEST_DATA_DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("test data dir");
        unsafe {
            std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, dir.path());
        }
        dir
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let order_path = dir.path().join("order.txt");
    let launch_record_path = dir.path().join("launch-request.json");
    let provider_path = dir.path().join("fake-provider.py");
    write_executable(
        &provider_path,
        &fake_provider_body(
            &order_path,
            &launch_record_path,
            slow,
            unavailable,
            launch_stalls,
        ),
    );
    Fixture {
        _dir: dir,
        provider_path,
        order_path,
    }
}

fn py_set(values: &[&str]) -> String {
    if values.is_empty() {
        return "set()".to_string();
    }
    let items = values
        .iter()
        .map(|value| serde_json::to_string(&settings_id(value)).unwrap())
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{items}}}")
}

fn settings_id(account: &str) -> String {
    format!("{account}-settings-record")
}

fn fake_provider_body(
    order_path: &Path,
    launch_record_path: &Path,
    slow: &[&str],
    unavailable: &[&str],
    launch_stalls: &[&str],
) -> String {
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys
import time

CONTRACT = "oulipoly.provider/v1"
ORDER = pathlib.Path({order_path})
LAUNCH_RECORD = pathlib.Path({launch_record_path})
SLOW = {slow}
UNAVAILABLE = {unavailable}
LAUNCH_STALLS = {launch_stalls}
SLEEP_SECONDS = {sleep}


def read_request():
    text = sys.stdin.read()
    return json.loads(text) if text else {{}}


def write_json(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def append_order(value):
    existing = ORDER.read_text() if ORDER.exists() else ""
    ORDER.write_text(existing + value + "\n")


def request_id(request):
    return request.get("request_id", "request-example-001")


def response(request, result):
    write_json({{
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(request),
        "ok": True,
        "result": result,
    }})


def settings_id(request):
    return (request.get("params") or {{}}).get("settings_id")


def describe(request):
    response(request, {{
        "provider_id": "fake-provider",
        "display_name": "Fake Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": True,
            "launch_output_v1": True,
            "policy": True,
            "quota": False,
            "session": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False
        }}
    }})


def policy(request):
    sid = settings_id(request)
    append_order("policy:" + str(sid))
    if sid in SLOW:
        time.sleep(SLEEP_SECONDS)
        return
    if sid in UNAVAILABLE:
        write_json({{
            "contract": request.get("contract", CONTRACT),
            "request_id": request_id(request),
            "ok": False,
            "error": {{
                "code": "auth_expired",
                "category": "unavailable",
                "message": "account token expired",
                "retryable": True
            }}
        }})
        return
    response(request, {{
        "accepted": True,
        "stdin": None,
        "prompt": None,
        "diagnostics": [],
        "markers": []
    }})


def exit_event(request, seq, code, signal):
    return {{
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {{"kind": "exited", "code": code}},
        "terminal_signal": {{
            "kind": signal,
            "evidence": "fake-provider exit event",
            "observed_at_unix_ms": 1000 + seq
        }},
        "session": {{"provider_session_id": "example-session"}}
    }}


def launch(request):
    sid = settings_id(request)
    append_order("launch:" + str(sid))
    LAUNCH_RECORD.write_text(json.dumps(request, sort_keys=True))
    reqid = request_id(request)
    write_json({{"contract": CONTRACT, "request_id": reqid, "seq": 1, "time_unix_ms": 1001, "kind": "stdout", "data_base64": "AAH/"}})
    if sid in LAUNCH_STALLS:
        write_json({{"contract": CONTRACT, "request_id": reqid, "seq": 2, "time_unix_ms": 1002, "kind": "heartbeat", "detail": "stall before final exit"}})
        time.sleep(SLEEP_SECONDS)
        return 0
    write_json({{"contract": CONTRACT, "request_id": reqid, "seq": 2, "time_unix_ms": 1002, "kind": "marker", "name": "oulipoly.launch_output_complete/v1", "value": {{"protocol": "oulipoly.launch_output/v1", "stdout": {{"bytes": 3, "sha256": "26a66b061e8f48f39927c312f25293959729eee95978e2892d49d3512a5cc092"}}, "stderr": {{"bytes": 0, "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}}, "data_event_count": 1}}}})
    write_json(exit_event(request, 3, 0, "clean_exit"))
    return 0


def main():
    subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
    request = read_request()
    if subcommand == "describe":
        describe(request)
        return 0
    if subcommand == "policy.evaluate":
        policy(request)
        return 0
    if subcommand == "launch":
        return launch(request)
    return 64


if __name__ == "__main__":
    raise SystemExit(main())
"#,
        order_path = serde_json::to_string(&order_path.display().to_string()).unwrap(),
        launch_record_path =
            serde_json::to_string(&launch_record_path.display().to_string()).unwrap(),
        slow = py_set(slow),
        unavailable = py_set(unavailable),
        launch_stalls = py_set(launch_stalls),
        sleep = SLOW_SLEEP_SECONDS,
    )
}

fn provider_ref_path(path: &Path) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some(path.display().to_string()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn rotation_model(fixture: &Fixture, accounts: &[&str]) -> ModelConfig {
    let providers = accounts
        .iter()
        .map(|name| {
            let mut provider = ProviderConfig::new("agent-stub", Vec::new());
            provider.name = (*name).to_string();
            provider
        })
        .collect();
    ModelConfig {
        name: "rotation-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers,
        inputs: Vec::new(),
        provider: Some(provider_ref_path(&fixture.provider_path)),
    }
}

fn registry_with_client_options(
    model: &ModelConfig,
    fixture: &Fixture,
    client_options: ProviderClientOptions,
) -> ProviderRegistry {
    let options = ProviderRegistryOptions::default().with_client_options(client_options);
    let providers = ProvidersConfig {
        entries: model
            .providers
            .iter()
            .map(|provider| {
                (
                    provider.name.clone(),
                    ProviderEntry {
                        implementation: Some(ProviderEndpointConfig {
                            family: "transport-rotation-family".to_string(),
                            executable: fixture.provider_path.display().to_string(),
                        }),
                        settings_id: Some(settings_id(&provider.name)),
                        ..ProviderEntry::default()
                    },
                )
            })
            .collect(),
    };
    ProviderRegistry::from_configs(std::slice::from_ref(model), &providers, options)
        .expect("registry should construct from rotation model")
}

fn execute(
    fixture: &Fixture,
    model: ModelConfig,
    provider_index: usize,
) -> Result<executor::ExecutionResult, ServiceError> {
    execute_with_client_options(
        fixture,
        model,
        provider_index,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    )
}

fn execute_with_client_options(
    fixture: &Fixture,
    model: ModelConfig,
    provider_index: usize,
    client_options: ProviderClientOptions,
) -> Result<executor::ExecutionResult, ServiceError> {
    let registry = registry_with_client_options(&model, fixture, client_options);
    let service = executor::RuntimeExecutorService::new(Arc::new(registry));
    service
        .execute(ExecutorServiceRequest::Facade {
            model,
            provider_index,
            prompt: "prompt-value".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        })
        .map(|output| output.result)
}

fn order_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("order should be recorded")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn external_transport_timeout_fails_closed_before_sibling_account() {
    let fixture = make_fixture(&["slow-1"], &[]);
    let model = rotation_model(&fixture, &["slow-1", "fast-2"]);

    let error = execute(&fixture, model, 0).expect_err("lifecycle transfer is unavailable");

    assert_lifecycle_transfer_unavailable(error);
    assert_eq!(
        order_lines(&fixture.order_path),
        [format!("policy:{}", settings_id("slow-1"))],
        "dispatch must not invoke the sibling account"
    );
}

#[test]
fn external_provider_unavailable_fails_closed_before_sibling_account() {
    let fixture = make_fixture(&[], &["unavail-1"]);
    let model = rotation_model(&fixture, &["unavail-1", "fast-2"]);

    let error = execute(&fixture, model, 0).expect_err("lifecycle transfer is unavailable");

    assert_lifecycle_transfer_unavailable(error);
    assert_eq!(
        order_lines(&fixture.order_path),
        [format!("policy:{}", settings_id("unavail-1"))],
        "dispatch must not invoke the sibling account"
    );
}

#[test]
fn external_launch_heartbeat_gap_timeout_fails_closed_before_sibling_account() {
    let fixture = make_fixture_with_launch_stalls(&[], &[], &["stall-1"]);
    let model = rotation_model(&fixture, &["stall-1", "fast-2"]);

    let result = execute_with_client_options(
        &fixture,
        model,
        0,
        ProviderClientOptions::default().with_launch_heartbeat_gap(HANDSHAKE_TIMEOUT),
    )
    .expect_err("lifecycle transfer is unavailable");

    assert_lifecycle_transfer_unavailable(result);
    assert_eq!(
        order_lines(&fixture.order_path),
        [
            format!("policy:{}", settings_id("stall-1")),
            format!("launch:{}", settings_id("stall-1")),
        ],
        "launch gap timeout must not invoke the sibling account"
    );
}

#[test]
fn external_transport_pool_fails_closed_at_first_required_lifecycle_transfer() {
    let fixture = make_fixture(&["slow-1", "slow-2"], &[]);
    let model = rotation_model(&fixture, &["slow-1", "slow-2"]);

    let error = execute(&fixture, model, 0).expect_err("lifecycle transfer is unavailable");

    assert_lifecycle_transfer_unavailable(error);
    assert_eq!(
        order_lines(&fixture.order_path),
        [format!("policy:{}", settings_id("slow-1"))],
        "the sibling must not be attempted without lifecycle transfer authority"
    );
}

fn assert_lifecycle_transfer_unavailable(error: ServiceError) {
    assert!(matches!(
        error,
        ServiceError::Unavailable { ref code, ref message }
            if code.as_deref() == Some("account_lifecycle_transfer_unavailable")
                && message.contains("sibling launch was not attempted")
    ));
}
