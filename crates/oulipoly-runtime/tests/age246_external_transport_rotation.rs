#![cfg(unix)]

//! FIX #32: an external-provider transport timeout (and account-unavailable /
//! auth-expired classes) must ROTATE to the next account in the model pool with
//! the same bounded policy the in-tree path uses, terminal-failing only when the
//! whole pool is exhausted.
//!
//! The fake provider spawned here is the single shared artifact for every
//! account in the model; it branches on `params.settings_id` (the per-account
//! identity) so one account can hang past the host handshake timeout or launch
//! heartbeat gap while a sibling account answers immediately.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_provider::client::ProviderClientOptions;
use oulipoly_runtime::executor;
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest, ServiceError};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(1);
const SLOW_SLEEP_SECONDS: u64 = 5;

struct Fixture {
    _dir: tempfile::TempDir,
    provider_path: PathBuf,
    order_path: PathBuf,
    launch_record_path: PathBuf,
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
        launch_record_path,
    }
}

fn py_set(values: &[&str]) -> String {
    if values.is_empty() {
        return "set()".to_string();
    }
    let items = values
        .iter()
        .map(|value| serde_json::to_string(value).unwrap())
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{items}}}")
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
    write_json(exit_event(request, 2, 0, "clean_exit"))
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

fn registry_with_handshake_timeout(model: &ModelConfig) -> ProviderRegistry {
    let options = ProviderRegistryOptions::default()
        .with_client_options(ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT));
    ProviderRegistry::from_model_configs(std::slice::from_ref(model), options)
        .expect("registry should construct from rotation model")
}

fn execute(
    model: ModelConfig,
    provider_index: usize,
) -> Result<executor::ExecutionResult, ServiceError> {
    let registry = registry_with_handshake_timeout(&model);
    let service = executor::RuntimeExecutorService::new(Arc::new(registry));
    service
        .execute(ExecutorServiceRequest::Facade {
            model,
            provider_index,
            prompt: "prompt-value".to_string(),
            working_dir: None,
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
fn external_transport_timeout_rotates_to_next_account_and_succeeds() {
    let fixture = make_fixture(&["slow-1"], &[]);
    let model = rotation_model(&fixture, &["slow-1", "fast-2"]);

    let result = execute(model, 0)
        .expect("a host handshake timeout on the first account must rotate, not terminal-fail");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, vec![0, 1, 255]);
    assert_eq!(
        order_lines(&fixture.order_path),
        ["policy:slow-1", "policy:fast-2", "launch:fast-2"],
        "dispatch must rotate from the slow account to the next pool account"
    );
    let launch: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&fixture.launch_record_path).unwrap()).unwrap();
    assert_eq!(
        launch["params"]["settings_id"], "fast-2",
        "the surviving account must be the one that launched"
    );
}

#[test]
fn external_provider_unavailable_rotates_to_next_account_and_succeeds() {
    let fixture = make_fixture(&[], &["unavail-1"]);
    let model = rotation_model(&fixture, &["unavail-1", "fast-2"]);

    let result = execute(model, 0)
        .expect("an account-unavailable (auth-expired) class must rotate, not terminal-fail");

    assert_eq!(result.exit_code, 0);
    assert_eq!(
        order_lines(&fixture.order_path),
        ["policy:unavail-1", "policy:fast-2", "launch:fast-2"],
        "an unavailable/auth-expired account must rotate to the next pool account"
    );
}

#[test]
fn external_launch_heartbeat_gap_timeout_rotates_to_next_account_and_succeeds() {
    let fixture = make_fixture_with_launch_stalls(&[], &[], &["stall-1"]);
    let model = rotation_model(&fixture, &["stall-1", "fast-2"]);

    let result =
        execute(model, 0).expect("a launch heartbeat-gap timeout on the first account must rotate");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, vec![0, 1, 255]);
    assert_eq!(
        order_lines(&fixture.order_path),
        [
            "policy:stall-1",
            "launch:stall-1",
            "policy:fast-2",
            "launch:fast-2",
        ],
        "launch gap timeout must rotate to the next pool account"
    );
}

#[test]
fn external_transport_all_slow_pool_is_bounded_terminal_failure_with_honest_category() {
    let fixture = make_fixture(&["slow-1", "slow-2"], &[]);
    let model = rotation_model(&fixture, &["slow-1", "slow-2"]);

    let error = execute(model, 0)
        .expect_err("an all-slow pool must terminal-fail once every account is exhausted");

    let message = error.to_string();
    assert!(
        message.contains("transport") && message.contains("host_timeout"),
        "pool-exhausted failure must carry the honest transport category: {message:?}"
    );
    assert_eq!(
        order_lines(&fixture.order_path),
        ["policy:slow-1", "policy:slow-2"],
        "every account in the pool must be attempted before terminal failure"
    );
}
