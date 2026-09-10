pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::client::{ProviderClient, ProviderClientOptions, ProviderOutputLimits};
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{DescribeResult, SchemaResult, SettingsListResult};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_provider::schemas::SchemaRegistry;
use serde_json::json;
use std::ffi::OsString;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use support::provider_client::{
    REQUEST_ID, describe_request, describe_success_response, executable_script,
    fake_provider_source, read_recorded_invocation, schema_request, settings_list_request,
    temp_fixture_dir,
    testkit::{FakeProvider, FakeProviderMode},
};

#[test]
fn invoke_success_returns_typed_result_and_keeps_stderr_diagnostics() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let result: DescribeResult = client
        .invoke_typed(
            "describe",
            describe_request(),
            FakeProviderMode::SuccessStderr.env(),
        )
        .expect("valid success envelope should return typed result");

    assert_eq!(result.provider_id, "fake-provider");
    assert!(
        client
            .last_diagnostics()
            .stderr_text()
            .contains("diagnostic")
    );
}

#[test]
fn invoke_describe_accepts_schema_valid_freeform_concurrency_metadata() {
    let provider_id = ["cla", "ude"].concat();
    let response = json!({
        "contract": "oulipoly.provider/v1",
        "ok": true,
        "request_id": "provider-registry-f7eff50a-09d6-4cc7-968e-5f0e0c567104",
        "result": {
            "capabilities": {
                "discovery": true,
                "launch": true,
                "migration": true,
                "policy": true,
                "quota": true,
                "rotation": true,
                "session": true,
                "settings": true,
                "setup": true,
                "setup_brain": true,
                "terminal": true
            },
            "concurrency": {
                "launch_streams": "one_per_process",
                "process_model": "one_shot_cli",
                "state_serialization": "provider_advisory_locks"
            },
            "contract_versions": ["oulipoly.provider/v1"],
            "display_name": "Example Provider",
            "preferred_contract": "oulipoly.provider/v1",
            "provider_id": provider_id.clone(),
            "settings_schema_id": format!("{}.settings/v1", provider_id)
        }
    });
    SchemaRegistry::new()
        .validate_response("describe", &response)
        .expect("captured provider describe response must validate against frozen schema");

    let result = serde_json::from_value::<DescribeResult>(response["result"].clone());

    assert!(
        result.is_ok(),
        "schema-valid describe result should deserialize for host consumers: {:?}",
        result.err()
    );
    let concurrency = serde_json::to_value(result.unwrap().concurrency.unwrap())
        .expect("concurrency metadata should serialize back to JSON");
    assert_eq!(concurrency["launch_streams"], "one_per_process");
}

#[test]
fn invoke_writes_request_on_stdin_not_argv() {
    let fake = FakeProvider::compile(fake_provider_source());
    let record = temp_fixture_dir("argv-record").join("record.txt");
    let client = client_for(fake.path());

    client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::RecordArgvStdin.env_with_record(&record),
        )
        .expect("recording invocation should succeed");

    let recorded = read_recorded_invocation(record);
    assert_eq!(
        client.last_invocation_argv(),
        vec![fake.path().into_os_string(), "describe".into()]
    );
    assert_eq!(recorded.argv.len(), 2);
    assert_eq!(recorded.argv[1], "describe");
    assert!(!recorded.argv.iter().any(|arg| arg.contains(REQUEST_ID)));
    assert!(
        recorded
            .stdin
            .contains("\"request_id\":\"request-example-001\"")
    );
}

#[test]
fn invoke_schema_subcommand_returns_typed_result_through_one_shot_provider() {
    let fake = FakeProvider::compile(fake_provider_source());
    let record = temp_fixture_dir("schema-record").join("record.txt");
    let client = client_for(fake.path());

    let result: SchemaResult = client
        .invoke_typed("schema", schema_request(), s5_record_env(&record))
        .expect("registered schema subcommand should return a typed schema result");

    assert_eq!(result.schema_id, "example.settings/v1");
    assert_eq!(result.schema["type"], "object");

    let recorded = read_recorded_invocation(record);
    assert_eq!(
        client.last_invocation_argv(),
        vec![fake.path().into_os_string(), "schema".into()]
    );
    assert_eq!(recorded.argv.len(), 2);
    assert_eq!(recorded.argv[1], "schema");
    assert!(
        recorded
            .stdin
            .contains("\"schema_id\":\"example.settings/v1\"")
    );
}

#[test]
fn invoke_settings_list_subcommand_returns_typed_result_through_one_shot_provider() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());

    let result: SettingsListResult = client
        .invoke_typed("settings.list", settings_list_request(), s5_success_env())
        .expect("registered settings.list subcommand should return a typed result");

    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].id, "example-settings");
    assert_eq!(result.records[0].version, "7");
    assert_eq!(
        client.last_invocation_argv(),
        vec![fake.path().into_os_string(), "settings.list".into()]
    );
}

#[test]
fn invoke_rejects_unknown_subcommand_before_spawn() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let error = client
        .invoke_json(
            "unknown.command",
            describe_request(),
            FakeProviderMode::Success.env(),
        )
        .expect_err("unknown subcommand should reject before spawn");

    assert_eq!(error.transport_kind(), "unknown_subcommand");
    assert!(!fake.was_spawned());
}

#[test]
fn invoke_rejects_schema_invalid_request_before_spawn() {
    let fake = FakeProvider::compile(fake_provider_source());
    let mut request = describe_request();
    request
        .as_object_mut()
        .expect("request should be object")
        .remove("contract");
    let client = client_for(fake.path());

    let error = client
        .invoke_json("describe", request, FakeProviderMode::Success.env())
        .expect_err("schema-invalid request should reject before spawn");

    assert_eq!(error.transport_kind(), "schema_invalid_request");
    assert!(!fake.was_spawned());
}

#[test]
fn invoke_rejects_invalid_stdout_protocol_shapes() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let cases = [
        (FakeProviderMode::EmptyStdout, "empty_stdout"),
        (FakeProviderMode::InvalidUtf8, "invalid_utf8"),
        (FakeProviderMode::NonObjectArray, "non_object_json"),
        (FakeProviderMode::NonObjectString, "non_object_json"),
        (FakeProviderMode::NonObjectNumber, "non_object_json"),
        (FakeProviderMode::MissingOk, "schema_invalid_response"),
        (FakeProviderMode::InvalidJson, "invalid_json"),
        (FakeProviderMode::MultipleJson, "multiple_json_objects"),
        (FakeProviderMode::LeadingLog, "leading_stdout_text"),
        (FakeProviderMode::TrailingJunk, "trailing_non_whitespace"),
        (FakeProviderMode::StderrEnvelopeOnly, "empty_stdout"),
    ];

    for (mode, expected_kind) in cases {
        let error = client
            .invoke_json("describe", describe_request(), mode.env())
            .expect_err("invalid stdout protocol shape should fail");
        assert_eq!(error.transport_kind(), expected_kind, "mode {mode:?}");
    }
}

#[test]
fn invoke_rejects_mismatched_contract_and_request_id() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    for mode in [
        FakeProviderMode::MismatchedContract,
        FakeProviderMode::MismatchedRequestId,
    ] {
        let error = client
            .invoke_json("describe", describe_request(), mode.env())
            .expect_err("correlation mismatch should be protocol error");
        assert!(matches!(
            error,
            ProviderClientError::Protocol { .. } | ProviderClientError::Transport { .. }
        ));
        assert!(matches!(
            error.transport_kind(),
            "mismatched_contract" | "mismatched_request_id"
        ));
    }
}

#[test]
fn invoke_early_stdin_close_valid_ok_true_wins() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let result = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::EarlyStdinSuccess.env(),
        )
        .expect("valid success envelope should win over broken stdin");

    assert_eq!(result["ok"], true);
    assert!(client.last_diagnostics().stdin_closed_early);
}

#[test]
fn invoke_early_stdin_close_valid_ok_false_wins_as_capability_error() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::EarlyStdinError.env(),
        )
        .expect_err("valid provider error should win over broken stdin");

    assert!(error.is_provider_capability());
    assert_eq!(error.provider_error_code(), Some("example_early_stdin"));
}

#[test]
fn invoke_early_stdin_close_without_valid_envelope_is_transport_error() {
    let fake = FakeProvider::compile(fake_provider_source());
    let client = client_for(fake.path());
    let error = client
        .invoke_json(
            "describe",
            describe_request(),
            FakeProviderMode::EarlyStdinEmpty.env(),
        )
        .expect_err("early stdin close without envelope should be transport error");

    assert_eq!(error.transport_kind(), "provider_closed_stdin_early");
}

#[test]
fn invoke_script_uses_artifact_then_single_subcommand_arg() {
    let client = ProviderClient::new(
        ProviderArtifactRef::Script {
            path: executable_script(),
        },
        ProviderClientOptions::default(),
    );
    let result = client
        .invoke_json("describe", describe_request(), [])
        .expect("direct executable script should be invokable");

    assert_eq!(result["ok"], true);
    assert_eq!(client.last_invocation_argv().len(), 2);
    assert_eq!(client.last_invocation_argv()[1], "describe");
}

#[cfg(unix)]
#[test]
fn invoke_accepts_execute_only_native_provider() {
    let fake = FakeProvider::compile(fake_provider_source());
    let native_provider = fake.native_path();
    let mut permissions = fs::metadata(&native_provider)
        .expect("native provider metadata")
        .permissions();
    permissions.set_mode(0o111);
    fs::set_permissions(&native_provider, permissions).expect("chmod native provider");
    let client = client_for(native_provider);

    let result: DescribeResult = client
        .invoke_typed(
            "describe",
            describe_request(),
            FakeProviderMode::Success.env(),
        )
        .expect("execute permission alone should keep a native provider available");

    assert_eq!(result.provider_id, "fake-provider");
    fake.cleanup();
}

#[cfg(unix)]
#[test]
fn invoke_script_preserves_configured_path_for_sibling_resources() {
    let directory = temp_fixture_dir("script-location");
    fs::create_dir_all(&directory).expect("create fixture directory");
    let configured = directory.join("provider.sh");
    let observed_path = directory.join("observed-path.txt");
    let response_path = directory.join("response.json");
    fs::write(
        &response_path,
        serde_json::to_vec(&describe_success_response()).expect("serialize describe response"),
    )
    .expect("write sibling response");
    write_location_dependent_provider_script(&configured);
    let client = ProviderClient::new(
        ProviderArtifactRef::Script {
            path: configured.clone(),
        },
        ProviderClientOptions::default(),
    );

    let result: DescribeResult = client
        .invoke_typed(
            "describe",
            describe_request(),
            vec![(
                "PROVIDER_OBSERVED_PATH".to_owned(),
                observed_path.as_os_str().to_os_string(),
            )],
        )
        .expect("pinned script should load resources beside its configured path");

    assert_eq!(result.provider_id, "fake-provider");
    assert_eq!(
        fs::read_to_string(&observed_path).expect("read observed script path"),
        configured.to_string_lossy()
    );
    fs::remove_dir_all(directory).expect("remove fixture directory");
}

#[cfg(unix)]
#[test]
fn client_pins_executable_that_advertised_capabilities_across_invocations() {
    let directory = temp_fixture_dir("pinned-provider-identity");
    fs::create_dir_all(&directory).expect("create fixture directory");
    let configured = directory.join("provider.sh");
    let replacement = directory.join("replacement.sh");
    write_describe_provider_script(&configured, "selected-provider");
    write_describe_provider_script(&replacement, "replacement-provider");
    let client = ProviderClient::new(
        ProviderArtifactRef::Script {
            path: configured.clone(),
        },
        ProviderClientOptions::default(),
    );

    let selected: DescribeResult = client
        .invoke_typed("describe", describe_request(), [])
        .expect("selected artifact should describe");
    fs::rename(&replacement, &configured).expect("replace configured artifact");
    let after_replacement: DescribeResult = client
        .invoke_typed("describe", describe_request(), [])
        .expect("pinned selected artifact should remain executable");

    assert_eq!(selected.provider_id, "selected-provider");
    assert_eq!(after_replacement.provider_id, "selected-provider");
    fs::remove_dir_all(directory).expect("remove fixture directory");
}

#[cfg(unix)]
fn write_describe_provider_script(path: &Path, provider_id: &str) {
    let mut response = describe_success_response();
    response["result"]["provider_id"] = json!(provider_id);
    response["result"]["display_name"] = json!(provider_id);
    fs::write(
        path,
        format!(
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
            serde_json::to_string(&response).expect("serialize describe response")
        ),
    )
    .expect("write provider script");
    let mut permissions = fs::metadata(path).expect("provider metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod provider script");
}

#[cfg(unix)]
fn write_location_dependent_provider_script(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\nprintf '%s' \"$0\" > \"$PROVIDER_OBSERVED_PATH\"\n[ \"$1\" = describe ] || exit 2\ncat >/dev/null\nscript_dir=$(CDPATH= cd \"$(dirname \"$0\")\" && pwd)\ncat \"$script_dir/response.json\"\n",
    )
    .expect("write location-dependent provider script");
    let mut permissions = fs::metadata(path).expect("provider metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod provider script");
}

fn client_for(path: impl Into<std::path::PathBuf>) -> ProviderClient {
    ProviderClient::new(
        ProviderArtifactRef::Path { path: path.into() },
        ProviderClientOptions {
            output_limits: ProviderOutputLimits {
                stdout_bytes: 256 * 1024,
                stderr_bytes: 64 * 1024,
            },
            timeout: Duration::from_secs(3),
            ..ProviderClientOptions::default()
        },
    )
}

fn s5_success_env() -> Vec<(String, String)> {
    vec![("FAKE_PROVIDER_MODE".to_owned(), "s5-success".to_owned())]
}

fn s5_record_env(record: &Path) -> Vec<(String, OsString)> {
    vec![
        (
            "FAKE_PROVIDER_MODE".to_owned(),
            OsString::from("s5-record-argv-stdin"),
        ),
        (
            "FAKE_PROVIDER_RECORD_PATH".to_owned(),
            record.as_os_str().to_os_string(),
        ),
    ]
}
