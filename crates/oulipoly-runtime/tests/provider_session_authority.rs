#![cfg(unix)]

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry,
    ProvidersConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::executor::RuntimeExecutorService;
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ACCOUNT: &str = "selected-account";
const SESSION: &str = "provider-session";

struct Fixture {
    _temp: tempfile::TempDir,
    endpoint_path: PathBuf,
    order_path: PathBuf,
}

impl Fixture {
    fn new(observed_session: Option<&str>, launch_output_v1: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let endpoint_path = temp.path().join("provider-endpoint.py");
        let order_path = temp.path().join("order.txt");
        fs::write(
            &endpoint_path,
            provider_body(&order_path, observed_session, launch_output_v1),
        )
        .unwrap();
        let mut permissions = fs::metadata(&endpoint_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&endpoint_path, permissions).unwrap();
        Self {
            _temp: temp,
            endpoint_path,
            order_path,
        }
    }

    fn service(&self) -> RuntimeExecutorService {
        RuntimeExecutorService::new(Arc::new(self.registry()))
    }

    fn registry(&self) -> ProviderRegistry {
        let model = model_with_competing_reference();
        let providers = ProvidersConfig {
            entries: HashMap::from([(
                ACCOUNT.to_string(),
                ProviderEntry {
                    implementation: Some(ProviderEndpointConfig {
                        family: "selected-family".to_string(),
                        executable: self.endpoint_path.display().to_string(),
                    }),
                    settings_id: Some("selected-account-settings".to_string()),
                    ..Default::default()
                },
            )]),
        };
        ProviderRegistry::from_configs(
            std::slice::from_ref(&model),
            &providers,
            ProviderRegistryOptions::default(),
        )
        .unwrap()
    }

    fn order(&self) -> Vec<String> {
        fs::read_to_string(&self.order_path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn policy_and_launch_reuse_the_preflighted_executable_identity() {
    let fixture = Fixture::new(Some(SESSION), true);
    let registry = fixture.registry();
    registry.preflight_account(ACCOUNT).unwrap();
    let replacement = fixture._temp.path().join("replacement.py");
    fs::write(
        &replacement,
        format!(
            "#!/usr/bin/env python3\nwith open({}, 'a', encoding='utf-8') as handle:\n    handle.write('replacement\\n')\nraise SystemExit(64)\n",
            json_string(&fixture.order_path.display().to_string())
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&replacement).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&replacement, permissions).unwrap();
    fs::rename(&replacement, &fixture.endpoint_path).unwrap();

    let output = RuntimeExecutorService::new(Arc::new(registry))
        .execute(request(None))
        .expect("dispatch must retain the executable pinned during preflight")
        .result;

    assert_eq!(output.complete_stdout_bytes().unwrap(), b"ok\n");
    assert_eq!(fixture.order(), ["describe", "policy.evaluate", "launch"]);
}

#[test]
fn selected_account_endpoint_ignores_competing_model_reference() {
    let fixture = Fixture::new(Some(SESSION), true);

    let output = fixture
        .service()
        .execute(request(None))
        .expect("selected account endpoint should launch")
        .result;

    assert_eq!(output.complete_stdout_bytes().unwrap(), b"ok\n");
    assert_eq!(output.session_capture.session_id.as_deref(), Some(SESSION));
    assert_eq!(
        output.session_capture.method.db_value(),
        "external_provider_launch"
    );
    assert_eq!(fixture.order(), ["describe", "policy.evaluate", "launch"]);
}

#[test]
fn expected_session_mismatch_and_missing_observation_fail_closed() {
    for (observed, expected_kind) in [
        (Some("different-session"), "session_identity_mismatch"),
        (None, "session_identity_observation_missing"),
    ] {
        let fixture = Fixture::new(observed, true);

        let error = fixture
            .service()
            .execute(request(Some(SESSION)))
            .err()
            .expect("unverified provider session must fail");

        assert!(error.to_string().contains(expected_kind), "{error}");
        assert_eq!(fixture.order(), ["describe", "policy.evaluate", "launch"]);
    }
}

#[test]
fn launch_output_v1_rejection_still_precedes_policy_and_launch() {
    let fixture = Fixture::new(Some(SESSION), false);

    let error = fixture
        .service()
        .execute(request(None))
        .err()
        .expect("missing complete output capability must fail");

    assert!(
        error
            .to_string()
            .contains("complete_launch_output_unsupported"),
        "{error}"
    );
    assert_eq!(fixture.order(), ["describe"]);
}

fn request(expected_session: Option<&str>) -> ExecutorServiceRequest {
    let model = model_with_competing_reference();
    let provider = model.providers[0].clone();
    match expected_session {
        Some(provider_session_id) => {
            ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
                model,
                provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "resume".to_string(),
                working_dir: None,
                models_dir: None,
                extra_inputs: HashMap::new(),
                parent_invocation_env: None,
                start_known_provider_session_id: provider_session_id.to_string(),
                mailbox_delivery_correlation: None,
            }
        }
        None => ExecutorServiceRequest::Effective {
            model,
            provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "create".to_string(),
            working_dir: None,
            models_dir: None,
            extra_inputs: HashMap::new(),
            parent_invocation_env: None,
        },
    }
}

fn model_with_competing_reference() -> ModelConfig {
    ModelConfig {
        name: "model-a".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(ACCOUNT, Vec::new())],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some("/competing/model/provider-must-not-run".to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn provider_body(
    order_path: &Path,
    observed_session: Option<&str>,
    launch_output_v1: bool,
) -> String {
    let session_argument = observed_session
        .map(|session| {
            format!(
                r#", session={{"provider_session_id":{}}}"#,
                json_string(session)
            )
        })
        .unwrap_or_default();
    format!(
        r#"#!/usr/bin/env python3
import base64
import json
import pathlib
import sys

ORDER = pathlib.Path({order_path})
SUBCOMMAND = sys.argv[1]
REQUEST = json.loads(sys.stdin.read())
with ORDER.open("a", encoding="utf-8") as handle:
    handle.write(SUBCOMMAND + "\n")

contract = "oulipoly.provider/v1"
request_id = REQUEST["request_id"]

def envelope(result):
    print(json.dumps({{"contract": contract, "request_id": request_id, "ok": True, "result": result}}, separators=(",", ":")))

if SUBCOMMAND == "describe":
    envelope({{
        "provider_id": "selected-endpoint",
        "display_name": "Selected Endpoint",
        "contract_versions": [contract],
        "preferred_contract": contract,
        "capabilities": {{
            "launch": True,
            "launch_output_v1": {launch_output_v1},
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
elif SUBCOMMAND == "policy.evaluate":
    envelope({{"accepted": True, "env": {{}}, "stdin": None, "prompt": None, "diagnostics": [], "markers": []}})
elif SUBCOMMAND == "launch":
    def event(seq, kind, **values):
        value = {{"contract": contract, "request_id": request_id, "seq": seq, "time_unix_ms": 1000 + seq, "kind": kind}}
        value.update(values)
        print(json.dumps(value, separators=(",", ":")))
    event(1, "stdout", data_base64=base64.b64encode(b"ok\n").decode("ascii"))
    event(2, "marker", name="oulipoly.launch_output_complete/v1", value={{
        "protocol": "oulipoly.launch_output/v1",
        "stdout": {{"bytes": 3, "sha256": "dc51b8c96c2d745df3bd5590d990230a482fd247123599548e0632fdbf97fc22"}},
        "stderr": {{"bytes": 0, "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}},
        "data_event_count": 1
    }})
    event(3, "exit", status={{"kind": "exited", "code": 0}}, terminal_signal={{
        "kind": "clean_exit", "evidence": "fixture", "observed_at_unix_ms": 1003
    }}{session_argument})
else:
    raise SystemExit(64)
"#,
        order_path = json_string(&order_path.display().to_string()),
        launch_output_v1 = if launch_output_v1 { "True" } else { "False" },
    )
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}
