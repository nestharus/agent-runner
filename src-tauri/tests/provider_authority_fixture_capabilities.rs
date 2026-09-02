#![cfg(unix)]

mod provider_authority_fixture;

use oulipoly_provider::client::{ProviderClient, ProviderClientOptions};
use oulipoly_provider::generated::{
    DescribeResult, LAUNCH_OUTPUT_COMPLETE_MARKER_V1, PROMPT_ACCEPTED_MARKER_V1, ProcessStatus,
};
use oulipoly_provider::resolver::ProviderArtifactRef;
use serde_json::{Value, json};
use std::path::PathBuf;

fn contract_fixtures() -> Value {
    serde_json::from_str(include_str!(
        "../../crates/oulipoly-provider/tests/fixtures/contract_v1/fixtures.json"
    ))
    .unwrap()
}

fn generated_endpoint(contents: &str, account: &str) -> PathBuf {
    generated_endpoint_with_prompt_acceptance(contents, account, &[])
}

fn generated_endpoint_with_prompt_acceptance(
    contents: &str,
    account: &str,
    prompt_acceptance_accounts: &[&str],
) -> PathBuf {
    let migrated =
        provider_authority_fixture::with_explicit_provider_authority_for_prompt_acceptance(
            contents,
            prompt_acceptance_accounts,
        );
    let providers = migrated.parse::<toml::Table>().unwrap();
    providers[account]["implementation"]["executable"]
        .as_str()
        .map(PathBuf::from)
        .unwrap()
}

#[test]
fn selected_prompt_acceptance_capability_emits_a_correlated_marker() {
    let endpoint = generated_endpoint_with_prompt_acceptance(
        "[fixture]\ncommand = '/bin/sh'\n\n[fixture.resume_acceptance]\naccepted_output_patterns = ['fixture accepted']\n",
        "fixture",
        &["fixture"],
    );
    let client = client_for(endpoint);
    let fixtures = contract_fixtures();
    let describe: DescribeResult = client
        .invoke_typed("describe", request_for(&fixtures, "describe").clone(), [])
        .unwrap();
    assert!(describe.capabilities.prompt_acceptance_v1);

    let mut launch_request = fixtures["launch"]["request"].clone();
    launch_request["params"]["argv"] =
        json!(["/bin/sh", "-c", "printf 'fixture accepted\\n' >&2",]);
    launch_request["params"]["working_directory"] =
        json!(std::env::current_dir().unwrap().display().to_string());
    launch_request["params"]
        .as_object_mut()
        .unwrap()
        .remove("stdin");

    let launch = client.launch(launch_request, []).unwrap();
    assert!(
        launch
            .retained_marker_value(PROMPT_ACCEPTED_MARKER_V1)
            .is_some()
    );
}

fn client_for(path: PathBuf) -> ProviderClient {
    ProviderClient::new(
        ProviderArtifactRef::Path { path },
        ProviderClientOptions::default(),
    )
}

fn request_for<'a>(fixtures: &'a Value, operation: &str) -> &'a Value {
    &fixtures["non_launch"][operation]["request"]
}

#[test]
fn generated_profiles_advertise_only_scenario_capabilities() {
    let fixtures = contract_fixtures();
    let describe_request = request_for(&fixtures, "describe");
    let cases = [
        (
            "none",
            "[none]\n",
            [false, false, false, false, false, false],
        ),
        (
            "launch",
            "[launch]\ncommand = '/bin/true'\n",
            [true, true, false, false, false, false],
        ),
        (
            "policy",
            "[policy]\nsystem_prompt_override = 'fixture'\n",
            [false, true, false, false, false, false],
        ),
        (
            "quota",
            "[quota]\nquota_script = 'fixture'\n",
            [false, false, true, false, false, false],
        ),
        (
            "session",
            "[session]\ninteractive_args = ['fixture']\n",
            [false, false, false, true, false, false],
        ),
        (
            "enumeration",
            "[enumeration.session_storage]\nkind = 'script'\ncwd_script = 'fixture'\n",
            [false, false, false, true, true, false],
        ),
        (
            "terminal",
            "[terminal.resume_acceptance]\naccepted_output_patterns = ['fixture']\n",
            [false, false, false, true, false, true],
        ),
    ];

    for (account, contents, expected) in cases {
        let endpoint = generated_endpoint(contents, account);
        let describe: DescribeResult = client_for(endpoint)
            .invoke_typed("describe", describe_request.clone(), [])
            .unwrap();
        let capabilities = describe.capabilities;
        assert_eq!(
            [
                capabilities.launch,
                capabilities.policy,
                capabilities.quota,
                capabilities.session,
                capabilities.session_enumerate,
                capabilities.terminal,
            ],
            expected,
            "profile for {account}"
        );
        assert!(!capabilities.prompt_acceptance_v1);
        assert_eq!(capabilities.launch_output_v1, capabilities.launch);
        assert_eq!(capabilities.session_turn_pages_v1, capabilities.session);
    }
}

#[test]
fn generated_endpoint_directly_implements_every_advertised_operation() {
    let transcript = tempfile::NamedTempFile::new().unwrap();
    let transcript_locator = format!(
        "printf '%s\\n' {:?}; :",
        transcript.path().display().to_string()
    );
    let storage_type = ["cla", "ude_code"].concat();
    let endpoint = generated_endpoint(
        &format!(
            r#"
[fixture]
command = "/bin/sh"
system_prompt_override = "fixture policy"
quota_script = '''printf '%s\n' '{{"used_percent":0,"resets_at":"2026-09-03T00:00:00Z"}}' '''
interactive_args = ["--interactive"]

[fixture.session_storage]
kind = "script"
cwd_script = "fixture sessions"
transcript_script = {transcript_locator:?}
storage_type = {storage_type:?}

[fixture.resume_acceptance]
accepted_output_patterns = ["fixture accepted"]
"#
        ),
        "fixture",
    );
    let client = client_for(endpoint);
    let fixtures = contract_fixtures();

    let describe_envelope = client
        .invoke_json("describe", request_for(&fixtures, "describe").clone(), [])
        .unwrap();
    let describe: DescribeResult =
        serde_json::from_value(describe_envelope["result"].clone()).unwrap();
    assert!(describe.capabilities.launch);
    assert!(!describe.capabilities.prompt_acceptance_v1);
    assert!(describe.capabilities.launch_output_v1);
    assert!(describe.capabilities.policy);
    assert!(describe.capabilities.quota);
    assert!(describe.capabilities.session);
    assert!(describe.capabilities.session_turn_pages_v1);
    assert!(describe.capabilities.session_enumerate);
    assert!(describe.capabilities.terminal);

    for operation in [
        "policy.evaluate",
        "quota.source",
        "quota.probe",
        "quota.refresh_auth",
        "session.locate_transcript",
        "session.enumerate",
        "session.read_turns",
        "session.capture",
        "session.export",
        "session.replace",
        "terminal.classify",
    ] {
        let mut request = request_for(&fixtures, operation).clone();
        if operation == "session.replace" {
            request["params"]["canonical_transcript"]["data_base64"] = json!("");
            request["params"]["canonical_transcript"]["sha256"] =
                json!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
            request["params"]["canonical_transcript"]["turn_count"] = json!(0);
            request["params"]["preimage_sha256_expected"] =
                json!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        }
        client
            .invoke_json(operation, request, [])
            .unwrap_or_else(|error| panic!("{operation} direct probe failed: {error}"));
    }

    let mut launch_request = fixtures["launch"]["request"].clone();
    launch_request["params"]["argv"] = json!([
        "/bin/sh",
        "-c",
        "printf fixture-stdout; printf fixture-stderr >&2"
    ]);
    launch_request["params"]["working_directory"] =
        json!(std::env::current_dir().unwrap().display().to_string());
    launch_request["params"]
        .as_object_mut()
        .unwrap()
        .remove("stdin");

    let launch = client.launch(launch_request, []).unwrap();
    assert_eq!(launch.stdout_bytes(), b"fixture-stdout");
    assert_eq!(launch.stderr_bytes(), b"fixture-stderr");
    assert!(
        launch
            .retained_marker_value(PROMPT_ACCEPTED_MARKER_V1)
            .is_none()
    );
    assert!(
        launch
            .retained_marker_value(LAUNCH_OUTPUT_COMPLETE_MARKER_V1)
            .is_some()
    );
    assert_eq!(launch.exit.status, ProcessStatus::Exited { code: 0 });
}

#[test]
fn describe_omits_unselected_extension_capabilities() {
    let endpoint = generated_endpoint("[fixture]\ncommand = '/bin/true'\n", "fixture");
    let client = client_for(endpoint);
    let mut request = request_for(&contract_fixtures(), "describe").clone();
    request["host"].as_object_mut().unwrap().remove("env");

    let describe = client.invoke_json("describe", request, []).unwrap();
    let capabilities = describe["result"]["capabilities"].as_object().unwrap();
    assert!(!capabilities.contains_key("prompt_acceptance_v1"));
    assert!(!capabilities.contains_key("launch_output_v1"));
    assert!(!capabilities.contains_key("session_turn_pages_v1"));
}
