#![cfg(unix)]

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::executor::RuntimeExecutorService;
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{
    ExecutorServicePort, ExecutorServiceRequest, ProviderSessionStartMode,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Fixture {
    _dir: tempfile::TempDir,
    provider_path: PathBuf,
    record_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let record_path = dir.path().join("provider-records.jsonl");
        fs::write(&record_path, "").expect("record file");
        let provider_path = write_external_provider_fixture(dir.path(), &record_path);
        Self {
            _dir: dir,
            provider_path,
            record_path,
        }
    }

    fn registry(&self) -> ProviderRegistry {
        ProviderRegistry::from_model_configs(
            &[external_model(&self.provider_path)],
            ProviderRegistryOptions::default(),
        )
        .expect("registry")
    }

    fn records_for(&self, subcommand: &str) -> Vec<Value> {
        let records = read_provider_record_text(&self.record_path);
        records_for_subcommand(parse_provider_records(&records), subcommand)
    }
}

fn read_provider_record_text(record_path: &Path) -> String {
    fs::read_to_string(record_path).expect("records")
}

fn parse_provider_records(records: &str) -> Vec<Value> {
    records.lines().map(parse_provider_record).collect()
}

fn parse_provider_record(line: &str) -> Value {
    serde_json::from_str::<Value>(line).expect("record json")
}

fn records_for_subcommand(records: Vec<Value>, subcommand: &str) -> Vec<Value> {
    records
        .into_iter()
        .filter(|record| record["subcommand"] == subcommand)
        .collect()
}

#[test]
fn external_launch_exit_session_populates_capture_and_resume_request() {
    let fixture = Fixture::new();
    let service = RuntimeExecutorService::new(Arc::new(fixture.registry()));
    let expected_session = session_id();

    let first = execute_external(&service, None);

    assert_eq!(String::from_utf8_lossy(&first.result.stdout), "ok\n");
    assert_eq!(
        first.result.session_capture.session_id.as_deref(),
        Some(expected_session.as_str())
    );
    assert_eq!(
        first.result.session_capture.method.db_value(),
        "external_provider_launch"
    );

    let captured = first
        .result
        .session_capture
        .session_id
        .as_deref()
        .expect("first launch captured provider session id");
    let second = execute_external(&service, Some(captured));

    assert_eq!(
        second.result.session_capture.session_id.as_deref(),
        Some(expected_session.as_str())
    );
    let launch_records = fixture.records_for("launch");
    assert_eq!(
        launch_records.len(),
        2,
        "launch records: {launch_records:?}"
    );
    assert!(
        launch_records[0]["request"]["params"]
            .get("session")
            .is_none(),
        "first launch without a known provider session id must omit session params"
    );
    assert_eq!(
        launch_records[1]["request"]["params"]["session"]["known_provider_session_id"].as_str(),
        Some(expected_session.as_str()),
        "resume launch must receive the provider session captured from the prior external launch"
    );
    assert_eq!(
        launch_records[1]["request"]["params"]["session"]["start_mode"].as_str(),
        Some("resume"),
        "resume launch must tell external providers to resume the known session"
    );
    let expected_path = std::env::var("PATH").expect("test process should have PATH");
    assert_eq!(
        launch_records[0]["request"]["params"]["env"]["PATH"].as_str(),
        Some(expected_path.as_str()),
        "external launch must pass PATH because provider launch clears inherited child env"
    );
    let policy_records = fixture.records_for("policy.evaluate");
    assert_eq!(
        policy_records.len(),
        2,
        "policy records: {policy_records:?}"
    );
    let policy_launch = &policy_records[0]["request"]["params"]["launch"];
    let provider = provider_name();
    assert_eq!(policy_launch["command"].as_str(), Some(provider.as_str()));
    assert_eq!(policy_launch["args"], json!([]));
    assert_eq!(policy_launch["prompt_mode"].as_str(), Some("arg"));
    assert_eq!(
        policy_records[0]["request"]["params"]["model"]["provider_args"],
        json!(["--model", "sonnet"])
    );
}

#[test]
fn external_launch_with_forced_session_id_marks_start_mode_create() {
    let fixture = Fixture::new();
    let service = RuntimeExecutorService::new(Arc::new(fixture.registry()));
    let expected_session = session_id();

    let result = execute_external_with_start_mode(
        &service,
        expected_session.as_str(),
        ProviderSessionStartMode::Create,
    );

    assert_eq!(String::from_utf8_lossy(&result.result.stdout), "ok\n");
    let launch_records = fixture.records_for("launch");
    assert_eq!(
        launch_records.len(),
        1,
        "launch records: {launch_records:?}"
    );
    assert_eq!(
        launch_records[0]["request"]["params"]["session"]["known_provider_session_id"].as_str(),
        Some(expected_session.as_str())
    );
    assert_eq!(
        launch_records[0]["request"]["params"]["session"]["start_mode"].as_str(),
        Some("create"),
        "fresh forced launches must tell external providers to create the known session"
    );
}

fn execute_external(
    service: &RuntimeExecutorService,
    known_session: Option<&str>,
) -> oulipoly_runtime::services::ExecutorServiceOutput {
    service
        .execute(external_execute_request(
            known_session.map(|id| (id, ProviderSessionStartMode::Resume)),
        ))
        .expect("external execution")
}

fn execute_external_with_start_mode(
    service: &RuntimeExecutorService,
    known_session: &str,
    start_mode: ProviderSessionStartMode,
) -> oulipoly_runtime::services::ExecutorServiceOutput {
    service
        .execute(external_execute_request(Some((known_session, start_mode))))
        .expect("external execution")
}

fn external_execute_request(
    known_session: Option<(&str, ProviderSessionStartMode)>,
) -> ExecutorServiceRequest {
    let model = external_model(Path::new("unused-by-registry-lookup"));
    let provider = ProviderConfig::new(
        provider_name(),
        vec!["--model".to_string(), "sonnet".to_string()],
    );
    match known_session {
        Some((start_known_provider_session_id, ProviderSessionStartMode::Create)) => {
            ExecutorServiceRequest::EffectiveWithCreateKnownProviderSessionId {
                model,
                provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "resume prompt".to_string(),
                working_dir: None,
                models_dir: None,
                extra_inputs: HashMap::new(),
                parent_invocation_env: None,
                start_known_provider_session_id: start_known_provider_session_id.to_string(),
            }
        }
        Some((start_known_provider_session_id, ProviderSessionStartMode::Resume)) => {
            ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
                model,
                provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "resume prompt".to_string(),
                working_dir: None,
                models_dir: None,
                extra_inputs: HashMap::new(),
                parent_invocation_env: None,
                start_known_provider_session_id: start_known_provider_session_id.to_string(),
                mailbox_delivery_correlation: None,
            }
        }
        None => ExecutorServiceRequest::Effective {
            model,
            provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "first prompt".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    }
}

fn external_model(provider_path: &Path) -> ModelConfig {
    ModelConfig {
        name: model_name(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            provider_name(),
            vec!["--model".to_string(), "sonnet".to_string()],
        )],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(provider_path.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn write_external_provider_fixture(dir: &Path, record_path: &Path) -> PathBuf {
    let path = dir.join("external-provider-fixture.py");
    fs::write(&path, external_provider_fixture_body(record_path)).expect("provider script");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod");
    path
}

fn external_provider_fixture_body(record_path: &Path) -> String {
    r#"#!/usr/bin/env python3
import base64
import json
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
RECORD_PATH = pathlib.Path(@@RECORD_PATH@@)
SESSION_ID = "@@SESSION_ID@@"

subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
raw = sys.stdin.read() or "{}"
request = json.loads(raw)
with RECORD_PATH.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({"subcommand": subcommand, "request": request}, sort_keys=True) + "\n")

def request_id():
    return request.get("request_id", "s10-fixture-request")

def envelope(result):
    return {
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(),
        "ok": True,
        "result": result,
    }

def describe():
    return envelope({
        "provider_id": "@@PROVIDER_ID@@",
        "display_name": "External Provider Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
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
            "migration": False,
        },
    })

def policy_evaluate():
    return envelope({
        "accepted": True,
        "env": {},
        "stdin": None,
        "prompt": None,
        "diagnostics": [],
        "markers": [],
    })

def emit_event(event):
    print(json.dumps(event, separators=(",", ":")))

def launch():
    rid = request_id()
    emit_event({
        "contract": CONTRACT,
        "request_id": rid,
        "seq": 1,
        "time_unix_ms": 1001,
        "kind": "stdout",
        "data_base64": base64.b64encode(b"ok\n").decode("ascii"),
    })
    emit_event({
        "contract": CONTRACT,
        "request_id": rid,
        "seq": 2,
        "time_unix_ms": 1002,
        "kind": "exit",
        "status": {"kind": "exited", "code": 0},
        "terminal_signal": {
            "kind": "clean_exit",
            "evidence": "fixture clean exit",
            "observed_at_unix_ms": 1002,
        },
        "session": {
            "provider_session_id": SESSION_ID,
            "state": {"cursor": "after-launch"},
        },
    })

if subcommand == "describe":
    print(json.dumps(describe()))
elif subcommand == "policy.evaluate":
    print(json.dumps(policy_evaluate()))
elif subcommand == "launch":
    launch()
else:
    print(json.dumps({
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(),
        "ok": False,
        "error": {
            "category": "failed",
            "code": "unsupported_subcommand",
            "message": subcommand,
            "retryable": False,
        },
    }))
"#
    .replace("@@RECORD_PATH@@", &json_string(record_path))
    .replace("@@PROVIDER_ID@@", &model_name())
    .replace("@@SESSION_ID@@", &session_id())
}

fn model_name() -> String {
    format!("external-{}-fixture", provider_name())
}

fn session_id() -> String {
    format!("ses-external-{}-captured", provider_name())
}

fn provider_name() -> String {
    ["cla", "ude"].concat()
}

fn json_string(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).expect("json path")
}
