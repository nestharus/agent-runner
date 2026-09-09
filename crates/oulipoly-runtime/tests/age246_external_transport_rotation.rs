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

    let error =
        execute(&fixture, model, 0).expect_err("single-account failure; no sibling authority");

    assert_honest_error(error, "host_timeout");
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

    let error =
        execute(&fixture, model, 0).expect_err("single-account failure; no sibling authority");

    assert_honest_error(error, "auth_expired");
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
    .expect_err("single-account failure; no sibling authority");

    assert_honest_error(result, "host_timeout");
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

    let error =
        execute(&fixture, model, 0).expect_err("single-account failure; no sibling authority");

    assert_honest_error(error, "host_timeout");
    assert_eq!(
        order_lines(&fixture.order_path),
        [format!("policy:{}", settings_id("slow-1"))],
        "the sibling must not be attempted without lifecycle transfer authority"
    );
}

fn assert_honest_error(error: ServiceError, expected: &str) {
    assert!(
        matches!(error, ServiceError::Dependency { ref message } if message.contains(expected)),
        "{error:?}"
    );
    assert!(!error.to_string().contains("lifecycle_transfer_unavailable"));
}

fn allocated_input(
    fixture: &Fixture,
    model: &ModelConfig,
    active: bool,
) -> executor::AllocatedProviderLaunchAttempt {
    use oulipoly_state::*;
    use uuid::Uuid;
    let state_path = fixture
        ._dir
        .path()
        .join(format!("state-{}.db", Uuid::new_v4()));
    let db = StateDb::open(&state_path).unwrap();
    let allocation = ProviderLaunchAttemptAllocation::allocate().unwrap();
    let request = BeginProviderLaunchRequest {
        logical_launch_id: Uuid::new_v4(),
        request_identity_sha256: "a".repeat(64),
        model_name: model.name.clone(),
        start_mode: ProviderLaunchStartMode::Create,
        expected_provider_session_id: None,
        candidates: model
            .providers
            .iter()
            .enumerate()
            .map(|(provider_index, p)| ProviderLaunchCandidate {
                provider_index,
                account_name: p.name.clone(),
            })
            .collect(),
        parent_invocation_id: None,
        allocation: allocation.clone(),
    };
    let lease = db.begin_launch(&request).unwrap();
    if active {
        db.activate_attempt(&lease, &allocation.completion_authority)
            .unwrap();
    }
    executor::AllocatedProviderLaunchAttempt {
        lease,
        completion_authority: allocation.completion_authority,
        state_db_path: state_path,
        mailbox_db_path: {
            let dir = fixture
                ._dir
                .path()
                .join(format!("runtime-{}", Uuid::new_v4()));
            fs::create_dir(&dir).unwrap();
            dir.join("pid-identity.db")
        },
        channel_root: fixture._dir.path().to_path_buf(),
        parent_invocation_uuid: Uuid::new_v4(),
    }
}
fn allocated_request(model: ModelConfig) -> ExecutorServiceRequest {
    ExecutorServiceRequest::Facade {
        model,
        provider_index: 0,
        prompt: "prompt-value".into(),
        working_dir: None,
        models_dir: None,
        extra_inputs: HashMap::new(),
        parent_invocation_env: None,
    }
}
#[test]
fn allocated_policy_failures_account_for_cold_and_cached_describe_without_sibling_work() {
    use oulipoly_provider::custody::ProviderOperation;
    let fixture = make_fixture(&[], &["unavail-1"]);
    let model = rotation_model(&fixture, &["unavail-1", "fast-2"]);
    let registry = registry_with_client_options(
        &model,
        &fixture,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    );
    let mut previous = None;
    for cached in [false, true] {
        let allocation = allocated_input(&fixture, &model, true);
        let owner = allocation.lease.owner.clone();
        let expected_generation = allocation.lease.runtime_generation_uuid;
        let result = executor::execute_allocated_provider_attempt(
            &registry,
            allocated_request(model.clone()),
            allocation,
        );
        let executor::ProviderLaunchAttemptOutcome::Failed(failure) = result else {
            panic!("expected honest failure: {result:?}");
        };
        assert_eq!(failure.owner, owner);
        assert!(
            matches!(failure.error, executor::ProviderLaunchFailure::Provider(_)),
            "{failure:?}"
        );
        assert_eq!(
            failure.rotatable_kind,
            Some(oulipoly_state::RotatableLaunchFailureKind::ProviderUnavailable)
        );
        assert_eq!(failure.actor_settlement.len(), 3);
        assert!(
            failure
                .actor_settlement
                .iter()
                .all(|r| r.effect_incapable()),
            "{:?}",
            failure.actor_settlement
        );
        let describe = failure
            .actor_settlement
            .iter()
            .find(|r| r.operation == ProviderOperation::Describe)
            .unwrap();
        assert_eq!(describe.spawned, !cached);
        assert!(
            failure.runtime_settlement.effect_incapable,
            "{:?}",
            failure.runtime_settlement
        );
        assert_eq!(
            failure.runtime_settlement.runtime_generation_uuid,
            expected_generation
        );
        assert_eq!(
            failure.runtime_settlement.spawn_invocation_uuid,
            owner.invocation_uuid
        );
        assert_eq!(
            failure.return_channel_settlement,
            executor::ReturnChannelSettlement::NotCreated
        );
        assert!(!failure.observations.transfer_forbidden());
        let policy = failure
            .requests
            .iter()
            .find(|r| r.operation == ProviderOperation::Policy)
            .unwrap();
        if let executor::ProviderLaunchFailure::Provider(error) = &failure.error {
            assert_eq!(error.request_id(), Some(policy.wire_request_id.as_str()));
        }
        if let Some((old_owner, old_generation, old_request)) = previous {
            assert_ne!(owner, old_owner);
            assert_ne!(expected_generation, old_generation);
            assert_ne!(policy.correlation, old_request);
        }
        previous = Some((owner, expected_generation, policy.correlation));
    }
    assert_eq!(
        order_lines(&fixture.order_path),
        vec![format!("policy:{}", settings_id("unavail-1")); 2]
    );
}
#[test]
fn allocated_launch_timeout_settles_exact_generation_and_strict_empty_channel() {
    let fixture = make_fixture_with_launch_stalls(&[], &[], &["stall-1"]);
    let model = rotation_model(&fixture, &["stall-1", "fast-2"]);
    let registry = registry_with_client_options(
        &model,
        &fixture,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    );
    let allocation = allocated_input(&fixture, &model, true);
    let channel = allocation
        .channel_root
        .join(allocation.parent_invocation_uuid.to_string())
        .join(allocation.lease.owner.logical_launch_id.to_string())
        .join(allocation.lease.owner.attempt_id.to_string())
        .join("returns.jsonl");
    let outcome = executor::execute_allocated_provider_attempt(
        &registry,
        allocated_request(model),
        allocation,
    );
    let executor::ProviderLaunchAttemptOutcome::Failed(failure) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        failure.rotatable_kind,
        Some(oulipoly_state::RotatableLaunchFailureKind::HostTimeout),
        "{failure:?}"
    );
    assert_eq!(failure.actor_settlement.len(), 3);
    assert!(
        failure
            .actor_settlement
            .iter()
            .all(|a| a.spawned && a.effect_incapable()),
        "{:?}",
        failure.actor_settlement
    );
    assert!(
        failure.runtime_settlement.effect_incapable,
        "{:?}",
        failure.runtime_settlement
    );
    assert_eq!(
        failure.return_channel_settlement,
        executor::ReturnChannelSettlement::EmptyRemoved
    );
    assert!(!channel.exists());
    assert!(
        !failure.observations.transfer_forbidden(),
        "partial stdout and heartbeat are not promotion"
    );
    if let executor::ProviderLaunchFailure::Provider(error) = &failure.error {
        assert_eq!(
            error.request_id(),
            None,
            "host timeout has no observed response ID"
        );
    }
    assert_eq!(failure.requests.len(), 3);
    assert_eq!(
        failure
            .output_spool
            .unwrap()
            .incomplete_output_bytes()
            .unwrap()
            .0,
        vec![0, 1, 255]
    );
    assert_eq!(
        order_lines(&fixture.order_path),
        [
            format!("policy:{}", settings_id("stall-1")),
            format!("launch:{}", settings_id("stall-1"))
        ]
    );
}
#[test]
fn unactivated_or_changed_allocated_lease_starts_no_describe_or_runtime() {
    let fixture = make_fixture(&[], &[]);
    let model = rotation_model(&fixture, &["fast-1", "fast-2"]);
    let registry = registry_with_client_options(&model, &fixture, ProviderClientOptions::default());
    for active in [false, true] {
        let mut allocation = allocated_input(&fixture, &model, active);
        if active {
            allocation.lease.runtime_generation_uuid = uuid::Uuid::new_v4();
        }
        let sidecar = allocation.mailbox_db_path.clone();
        let outcome = executor::execute_allocated_provider_attempt(
            &registry,
            allocated_request(model.clone()),
            allocation,
        );
        let executor::ProviderLaunchAttemptOutcome::Failed(failure) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(failure.actor_settlement.is_empty());
        assert!(!failure.runtime_settlement.effect_incapable);
        assert!(!sidecar.exists());
        assert!(!fixture.order_path.exists());
        assert!(failure.requests.is_empty());
    }
}

#[test]
fn allocated_failure_retains_artifacts_and_captured_children_on_the_producing_invocation() {
    assert_allocated_failure_retains_artifacts(false);
}

#[test]
fn allocated_failure_retains_versioned_messenger_artifacts_on_the_producing_invocation() {
    assert_allocated_failure_retains_artifacts(true);
}

fn assert_allocated_failure_retains_artifacts(versioned: bool) {
    let fixture = make_fixture_with_launch_stalls(&[], &[], &["stall-1"]);
    let body = fs::read_to_string(&fixture.provider_path).unwrap().replace(
        "        time.sleep(SLEEP_SECONDS)\n        return 0",
        r#"        env = request["params"]["env"]
        producer = json.loads(env["OULIPOLY_PARENT_INVOCATION"])["id"]
        ref = {"version_id": "store://return/" + producer + "/fixture/1", "name":"fixture", "store_address":{"workflow_run_id":"return:" + producer,"artifact_name":"fixture","version":1}, "sha256":"a"*64,"content_len":1,"format_hint":None,"verdict_line":None,"source":{"kind":"inline_bytes"},"producer_invocation_uuid":producer,"returned_at":"2026-09-07T00:00:00Z"}
        pathlib.Path(env["OULIPOLY_RETURN_CHANNEL"]).write_text(json.dumps(ref) + "\n")
        sys.stderr.write('OULIPOLY_INVOCATION={"source":"fake-child","id":"22222222-2222-4222-8222-222222222222"}\n')
        sys.stderr.flush()
        time.sleep(SLEEP_SECONDS)
        return 0"#,
    );
    let body = if versioned {
        body.replace("        pathlib.Path(env[\"OULIPOLY_RETURN_CHANNEL\"]).write_text", "        ref[\"schema_version\"] = 1\n        pathlib.Path(env[\"OULIPOLY_RETURN_CHANNEL\"]).write_text")
    } else {
        body
    };
    write_executable(&fixture.provider_path, &body);
    let model = rotation_model(&fixture, &["stall-1", "fast-2"]);
    let registry = registry_with_client_options(
        &model,
        &fixture,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    );
    let allocation = allocated_input(&fixture, &model, true);
    let owner = allocation.lease.owner.clone();
    let state_path = allocation.state_db_path.clone();
    let outcome = executor::execute_allocated_provider_attempt(
        &registry,
        allocated_request(model),
        allocation,
    );
    let executor::ProviderLaunchAttemptOutcome::Failed(failure) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        failure.rotatable_kind,
        Some(oulipoly_state::RotatableLaunchFailureKind::HostTimeout)
    );
    assert!(failure.observations.returned_artifact && failure.observations.captured_child);
    assert!(failure.observations.transfer_forbidden());
    assert_eq!(failure.captured_child_invocations.len(), 1);
    assert_eq!(
        failure.captured_child_invocations[0].composite_id.id,
        "22222222-2222-4222-8222-222222222222"
    );
    assert!(
        matches!(
            failure.return_channel_settlement,
            executor::ReturnChannelSettlement::ArtifactsCommitted(_)
        ),
        "{:?}",
        failure.return_channel_settlement
    );
    let refs = failure.return_channel_settlement.artifacts();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].producer_invocation_uuid, owner.invocation_uuid);
    let db = oulipoly_state::StateDb::open(&state_path).unwrap();
    assert_eq!(
        db.list_returned_artifacts(owner.invocation_row_id).unwrap(),
        refs
    );
    assert_eq!(
        order_lines(&fixture.order_path),
        [
            format!("policy:{}", settings_id("stall-1")),
            format!("launch:{}", settings_id("stall-1"))
        ]
    );
}

#[test]
fn repeating_an_allocated_lease_is_nonexecuting_and_cannot_settle_the_existing_generation() {
    let fixture = make_fixture(&[], &["unavail-1"]);
    let model = rotation_model(&fixture, &["unavail-1", "fast-2"]);
    let registry = registry_with_client_options(
        &model,
        &fixture,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    );
    let allocation = allocated_input(&fixture, &model, true);
    let first = executor::execute_allocated_provider_attempt(
        &registry,
        allocated_request(model.clone()),
        allocation.clone(),
    );
    let executor::ProviderLaunchAttemptOutcome::Failed(first) = first else {
        panic!("expected policy failure");
    };
    let before = order_lines(&fixture.order_path);
    let second = executor::execute_allocated_provider_attempt(
        &registry,
        allocated_request(model),
        allocation,
    );
    let executor::ProviderLaunchAttemptOutcome::Failed(second) = second else {
        panic!("replay must be non-executing");
    };
    assert_eq!(
        order_lines(&fixture.order_path),
        before,
        "same-lease retry repeated provider work"
    );
    assert!(second.actor_settlement.is_empty());
    assert!(second.rotatable_kind.is_none());
    assert_eq!(
        second.runtime_settlement.row_sha256,
        first.runtime_settlement.row_sha256
    );
}

#[test]
fn concurrent_same_lease_entries_admit_only_one_process_capable_attempt() {
    let fixture = make_fixture(&["slow-1"], &[]);
    let model = rotation_model(&fixture, &["slow-1", "fast-2"]);
    let registry = registry_with_client_options(
        &model,
        &fixture,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    );
    let allocation = allocated_input(&fixture, &model, true);
    let outcomes = std::thread::scope(|scope| {
        let run = || {
            executor::execute_allocated_provider_attempt(
                &registry,
                allocated_request(model.clone()),
                allocation.clone(),
            )
        };
        let left = scope.spawn(run);
        let right = scope.spawn(run);
        [left.join().unwrap(), right.join().unwrap()]
    });
    let failures = outcomes
        .into_iter()
        .map(|outcome| match outcome {
            executor::ProviderLaunchAttemptOutcome::Failed(failure) => failure,
            _ => panic!("both calls should fail without any sibling"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failures
            .iter()
            .filter(|f| !f.actor_settlement.is_empty())
            .count(),
        1
    );
    assert_eq!(
        failures
            .iter()
            .filter(|f| f.rotatable_kind
                == Some(oulipoly_state::RotatableLaunchFailureKind::HostTimeout))
            .count(),
        1
    );
    assert_eq!(
        order_lines(&fixture.order_path),
        [format!("policy:{}", settings_id("slow-1"))]
    );
}

fn decoded_final_stderr_timeout(chunks: &[&str], expected_child: bool) {
    let fixture = make_fixture_with_launch_stalls(&[], &[], &["stall-1"]);
    let emission = format!(
        r#"        import base64
        chunks = {}
        for seq, chunk in enumerate(chunks, start=3):
            write_json({{"contract": CONTRACT, "request_id": reqid, "seq": seq, "time_unix_ms": 1000 + seq, "kind": "stderr", "data_base64": base64.b64encode(chunk.encode()).decode()}})
        time.sleep(SLEEP_SECONDS)
        return 0"#,
        serde_json::to_string(chunks).unwrap()
    );
    let body = fs::read_to_string(&fixture.provider_path).unwrap().replace(
        "        time.sleep(SLEEP_SECONDS)\n        return 0",
        &emission,
    );
    write_executable(&fixture.provider_path, &body);
    let model = rotation_model(&fixture, &["stall-1", "fast-2"]);
    let registry = registry_with_client_options(
        &model,
        &fixture,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    );
    let allocation = allocated_input(&fixture, &model, true);
    let owner = allocation.lease.owner.clone();
    let state_path = allocation.state_db_path.clone();
    let generation = allocation.lease.runtime_generation_uuid;
    let outcome = executor::execute_allocated_provider_attempt(
        &registry,
        allocated_request(model),
        allocation,
    );
    let executor::ProviderLaunchAttemptOutcome::Failed(failure) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(failure.owner, owner);
    assert_eq!(
        failure.runtime_settlement.runtime_generation_uuid,
        generation
    );
    assert_eq!(
        failure.runtime_settlement.spawn_invocation_uuid,
        owner.invocation_uuid
    );
    assert_eq!(
        failure.rotatable_kind,
        Some(oulipoly_state::RotatableLaunchFailureKind::HostTimeout)
    );
    let executor::ProviderLaunchFailure::Provider(error) = &failure.error else {
        panic!("{:?}", failure.error);
    };
    assert_eq!(error.transport_kind(), "host_timeout");
    assert_eq!(error.request_id(), None);
    assert!(
        error.diagnostics().stderr.bytes.is_empty(),
        "fixture must not write raw OS stderr"
    );
    assert!(
        failure
            .actor_settlement
            .iter()
            .all(|a| a.attempt_id == owner.attempt_id && a.effect_incapable())
    );
    assert!(failure.runtime_settlement.effect_incapable);
    assert_eq!(
        failure.return_channel_settlement,
        executor::ReturnChannelSettlement::EmptyRemoved
    );
    assert!(failure.evidence_retention_failure.is_none());
    assert!(!failure.observations.persistence_failed);
    assert_eq!(
        failure
            .output_spool
            .as_ref()
            .unwrap()
            .incomplete_output_bytes()
            .unwrap()
            .1,
        chunks.concat().as_bytes()
    );
    assert_eq!(
        order_lines(&fixture.order_path),
        [
            format!("policy:{}", settings_id("stall-1")),
            format!("launch:{}", settings_id("stall-1"))
        ]
    );
    let db = oulipoly_state::StateDb::open(&state_path).unwrap();
    let promotions = db.provider_launch_promotions(&owner).unwrap();
    assert_eq!(
        failure.observations.captured_child, expected_child,
        "{failure:?}"
    );
    assert_eq!(failure.observations.transfer_forbidden(), expected_child);
    assert_eq!(
        promotions.contains(&oulipoly_state::ProviderLaunchPromotion::CapturedChild),
        expected_child
    );
    assert_eq!(
        failure.captured_child_invocations.len(),
        usize::from(expected_child)
    );
    assert!(
        !failure.observations.provider_session_observed
            && !failure.observations.prompt_accepted
            && !failure.observations.assistant_response_observed
            && !failure.observations.returned_artifact
            && !failure.observations.mailbox_submission_accepted
    );
    if expected_child {
        let child = &failure.captured_child_invocations[0];
        assert_eq!(child.composite_id.source, "decoded-child");
        assert_eq!(
            child.composite_id.id,
            "22222222-2222-4222-8222-222222222222"
        );
        assert_eq!(child.raw_marker_line, chunks.concat());
    }
}

#[test]
fn allocated_decoded_final_marker_without_newline_promotes_on_timeout() {
    decoded_final_stderr_timeout(
        &[
            r#"OULIPOLY_INVOCATION={"source":"decoded-child","id":"22222222-2222-4222-8222-222222222222"}"#,
        ],
        true,
    );
}

#[test]
fn allocated_decoded_final_marker_split_across_events_promotes_on_timeout() {
    decoded_final_stderr_timeout(
        &[
            "OULIPOLY_INVO",
            r#"CATION={"source":"decoded-child","id":"22222222-2222-4222-8222-222222222222"}"#,
        ],
        true,
    );
}

#[test]
fn allocated_decoded_final_partial_bytes_do_not_promote_on_timeout() {
    decoded_final_stderr_timeout(&["unpublished ", "partial bytes"], false);
    decoded_final_stderr_timeout(
        &[
            "OULIPOLY_INVOCATION=",
            r#"{"source":"decoded-child","id":"22222222-2222-4222-8222-222222222222""#,
        ],
        false,
    );
}

fn verified_missing_final_output(has_data: bool, complete: bool, retention_fault: bool) {
    let fixture = make_fixture(&[], &[]);
    let body = fs::read_to_string(&fixture.provider_path).unwrap();
    let start = body.find("def launch(request):").unwrap();
    let end = body[start..].find("\ndef main():").unwrap() + start;
    let replacement = format!(
        r#"def launch(request):
    import base64, hashlib
    append_order("launch:" + str(settings_id(request)))
    reqid = request_id(request)
    seq = 0
    def emit(kind, **fields):
        nonlocal seq
        seq += 1
        write_json(dict(contract=CONTRACT, request_id=reqid, seq=seq, time_unix_ms=1000+seq, kind=kind, **fields))
    emit("marker", name="oulipoly.provider_session", value={{"provider_session_id":"example-session"}})
    stdout = bytes([0,1,255,90]) if {has_data} else b""
    stderr = bytes([101,114,114,255,254]) if {has_data} else b""
    if {has_data}:
        emit("stdout", data_base64=base64.b64encode(stdout).decode())
        emit("stderr", data_base64=base64.b64encode(stderr).decode())
    if {complete}:
        emit("marker", name="oulipoly.launch_output_complete/v1", value={{"protocol":"oulipoly.launch_output/v1", "stdout":{{"bytes":len(stdout),"sha256":hashlib.sha256(stdout).hexdigest()}},"stderr":{{"bytes":len(stderr),"sha256":hashlib.sha256(stderr).hexdigest()}},"data_event_count":2 if {has_data} else 0}})
        write_json(exit_event(request, seq+1, 0, "clean_exit"))
    return 0

"#,
        has_data = if has_data { "True" } else { "False" },
        complete = if complete { "True" } else { "False" }
    );
    write_executable(
        &fixture.provider_path,
        &format!("{}{}{}", &body[..start], replacement, &body[end..]),
    );
    let model = rotation_model(&fixture, &["first", "sibling"]);
    let registry = registry_with_client_options(
        &model,
        &fixture,
        ProviderClientOptions::default().with_timeout(HANDSHAKE_TIMEOUT),
    );
    let allocation = allocated_input(&fixture, &model, true);
    let owner = allocation.lease.owner.clone();
    let db = oulipoly_state::StateDb::open(&allocation.state_db_path).unwrap();
    let paths = db
        .invocation_output_artifact_paths(&format!("{}.partial", owner.invocation_uuid))
        .unwrap()
        .unwrap();
    if retention_fault {
        fs::create_dir_all(&paths.stdout).unwrap();
    }
    let outcome = executor::execute_allocated_provider_attempt(
        &registry,
        allocated_request(model),
        allocation,
    );
    let executor::ProviderLaunchAttemptOutcome::Completed(result) = outcome else {
        panic!("{outcome:?}")
    };
    eprintln!(
        "missing-final has_data={has_data} complete={complete} retention_fault={retention_fault}; result={result:?}"
    );
    assert_eq!(
        result.session_capture.session_id.as_deref(),
        Some("example-session")
    );
    assert!(result.prompt_acceptance_attestation.is_none());
    assert_eq!(
        order_lines(&fixture.order_path),
        [
            format!("policy:{}", settings_id("first")),
            format!("launch:{}", settings_id("first"))
        ]
    );
    assert!(
        db.provider_launch_promotions(&owner)
            .unwrap()
            .contains(&oulipoly_state::ProviderLaunchPromotion::ProviderSessionObserved)
    );
    let stdout = if has_data {
        vec![0, 1, 255, 90]
    } else {
        vec![]
    };
    let stderr = if has_data {
        vec![101, 114, 114, 255, 254]
    } else {
        vec![]
    };
    if complete {
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.complete_stdout_bytes().unwrap(), stdout);
        assert!(result.output_spool.as_ref().unwrap().summary().is_ok());
        assert!(!paths.stdout.exists());
        return;
    }
    assert_ne!(result.exit_code, 0);
    assert_eq!(
        result.terminal_reason.as_deref(),
        Some("external_provider_missing_final_exit")
    );
    assert!(!result.produced_assistant_response);
    let spool = result
        .output_spool
        .as_ref()
        .expect("observed prefix must survive missing-final mapping");
    assert_eq!(
        spool.incomplete_output_bytes().unwrap(),
        (stdout.clone(), stderr.clone())
    );
    assert!(spool.summary().is_err());
    assert!(result.complete_stdout_bytes().is_err());
    assert!(result.write_stdout_to(&mut Vec::new()).is_err());
    let signal = result.terminal_signal.as_ref().unwrap();
    assert!(signal.evidence.contains("output=incomplete"));
    if retention_fault {
        assert!(
            signal.evidence.contains("output_retention=failed"),
            "{}",
            signal.evidence
        );
        assert!(
            result
                .persist_output_for_invocation(
                    &db,
                    owner.invocation_row_id,
                    &owner.invocation_uuid.to_string()
                )
                .is_err()
        );
    } else {
        assert_eq!(fs::read(&paths.stdout).unwrap(), stdout);
        assert_eq!(fs::read(&paths.stderr).unwrap(), stderr);
    }
    let complete_paths = db
        .invocation_output_artifact_paths(&owner.invocation_uuid.to_string())
        .unwrap()
        .unwrap();
    assert!(!complete_paths.stdout.exists());
    let count: i64 = db
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM invocation_output_deliveries WHERE invocation_id=?1",
            [owner.invocation_row_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "incomplete evidence is not complete output delivery"
    );
}
#[test]
fn allocated_verified_missing_final_retains_binary_prefix() {
    verified_missing_final_output(true, false, false);
}
#[test]
fn allocated_verified_missing_final_retains_genuine_no_data() {
    verified_missing_final_output(false, false, false);
}
#[test]
fn allocated_verified_missing_final_retention_failure_is_explicit() {
    verified_missing_final_output(true, false, true);
}
#[test]
fn allocated_verified_complete_output_remains_complete() {
    verified_missing_final_output(true, true, false);
}
