use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "../../src/testkit.rs"]
pub mod testkit;

pub const REQUEST_ID: &str = "request-example-001";
pub const PROVIDER_INSTANCE_ID: &str = "fake-provider";

pub fn provider_client_fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/provider_client")
}

pub fn fake_provider_source() -> PathBuf {
    provider_client_fixture_dir().join("fake_provider.rs")
}

pub fn executable_script() -> PathBuf {
    provider_client_fixture_dir().join("executable-script.sh")
}

pub fn non_executable_script() -> PathBuf {
    provider_client_fixture_dir().join("non-executable-script.sh")
}

pub fn temp_fixture_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("oulipoly-provider-{label}-{nanos}"))
}

pub fn host_context() -> Value {
    json!({
        "app": "oulipoly-test",
        "app_version": "0.0.0-test",
        "platform": std::env::consts::OS,
        "working_directory": ".",
        "config_root": ".",
        "data_root": ".",
        "env": {}
    })
}

pub fn describe_request() -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "provider_instance_id": PROVIDER_INSTANCE_ID,
        "host": host_context(),
        "params": {}
    })
}

pub fn launch_request() -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "provider_instance_id": PROVIDER_INSTANCE_ID,
        "host": host_context(),
        "params": {
            "settings_id": "example-settings",
            "mode": "default",
            "model": {
                "name": "example-model",
                "provider_args": [],
                "inputs": {
                    "named": {}
                }
            },
            "working_directory": ".",
            "env": {},
            "stdin": {
                "encoding": "base64",
                "data": "cHJvbXB0"
            },
            "session": {}
        }
    })
}

pub fn describe_success_response() -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "ok": true,
        "result": {
            "provider_id": "fake-provider",
            "display_name": "Fake Provider",
            "contract_versions": [oulipoly_provider::generated::CONTRACT_VERSION],
            "preferred_contract": oulipoly_provider::generated::CONTRACT_VERSION,
            "capabilities": {
                "launch": true,
                "policy": false,
                "quota": false,
                "session": false,
                "terminal": false,
                "rotation": false,
                "discovery": false,
                "settings": false,
                "setup_brain": false,
                "setup": false,
                "migration": false
            },
            "concurrency": {
                "safe_for_parallel_invocation": true,
                "state_locking": "none"
            }
        }
    })
}

pub fn describe_error_response(category: &str, code: &str) -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "ok": false,
        "error": {
            "category": category,
            "code": code,
            "retryable": category == "timeout",
            "message": format!("{category} from fake-provider"),
            "details": {
                "source": "example"
            }
        }
    })
}

pub fn launch_stdout_event(seq: u64, data_base64: &str) -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "stdout",
        "data_base64": data_base64
    })
}

pub fn launch_stderr_event(seq: u64, data_base64: &str) -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "stderr",
        "data_base64": data_base64
    })
}

pub fn launch_marker_event(seq: u64) -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "example-marker",
        "value": {
            "phase": "example"
        }
    })
}

pub fn launch_heartbeat_event(seq: u64) -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "heartbeat",
        "detail": "example heartbeat"
    })
}

pub fn launch_exit_event(seq: u64, code: i32) -> Value {
    json!({
        "contract": oulipoly_provider::generated::CONTRACT_VERSION,
        "request_id": REQUEST_ID,
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {
            "kind": "exited",
            "code": code
        },
        "terminal_signal": {
            "kind": if code == 0 { "clean_exit" } else { "nonzero_exit" },
            "evidence": "fake-provider exit event",
            "observed_at_unix_ms": 1000 + seq
        },
        "session": {
            "provider_session_id": "example-session"
        }
    })
}

pub fn json_line(value: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string(value).expect("fixture value should serialize")
    )
}

pub struct RecordedInvocation {
    pub argv: Vec<String>,
    pub stdin: String,
}

pub fn read_recorded_invocation(path: impl AsRef<std::path::Path>) -> RecordedInvocation {
    let recorded = std::fs::read_to_string(path).expect("record should be written");
    let (argv, stdin) = recorded
        .split_once("\nstdin:\n")
        .expect("record should contain stdin section");
    let argv = argv
        .strip_prefix("argv:\n")
        .expect("record should contain argv section")
        .lines()
        .map(str::to_owned)
        .collect();
    RecordedInvocation {
        argv,
        stdin: stdin.to_owned(),
    }
}

pub fn assert_transport_kind(
    error: &oulipoly_provider::error::ProviderClientError,
    expected: &str,
) {
    assert_eq!(
        error.transport_kind(),
        expected,
        "transport error kind should preserve the contract precedence"
    );
}

pub fn assert_provider_error(
    error: &oulipoly_provider::error::ProviderCapabilityError,
    category: oulipoly_provider::generated::ErrorCategory,
    code: &str,
) {
    assert_eq!(error.error().category, category);
    assert_eq!(error.error().code, code);
    assert_eq!(error.request_id(), REQUEST_ID);
}
