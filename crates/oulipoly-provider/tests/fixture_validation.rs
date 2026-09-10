pub mod support {
    pub mod contract_matrix;
}

use serde_json::{Map, Value, json};
use support::contract_matrix::{
    LAUNCH_EVENT_ROWS, LAUNCH_REQUEST_SCHEMA_DEF, NON_LAUNCH_ROWS, SCHEMA_DRAFT_2020_12, fixtures,
    launch_event_fixture, launch_fixture, load_json, non_launch_fixture, schema_file_for,
};

#[test]
fn provider_unavailable_validates_on_launch_and_terminal_classify_without_weakening_enums() {
    let fixtures = fixtures();
    let mut exit = launch_event_fixture(&fixtures, "exit").clone();
    exit["terminal_signal"]["kind"] = json!("provider_unavailable");
    validate_against_def("launch", "LaunchExitEvent", &exit);
    let mut classified =
        non_launch_fixture(&fixtures, "terminal.classify", "success_response").clone();
    classified["result"]["terminal_signal"]["kind"] = json!("provider_unavailable");
    validate_against_def("terminal", "TerminalClassifyResponse", &classified);
    let registry = oulipoly_provider::schemas::SchemaRegistry::new();
    classified["result"]["terminal_signal"]["kind"] = json!("unrecognized_terminal_kind");
    assert!(
        registry
            .validate_response("terminal.classify", &classified)
            .is_err()
    );
}

#[test]
fn schema_validation_fixtures_cover_expected_matrix() {
    let fixtures = fixtures();
    for row in NON_LAUNCH_ROWS {
        non_launch_fixture(&fixtures, row.subcommand, "request");
        non_launch_fixture(&fixtures, row.subcommand, "success_response");
        non_launch_fixture(&fixtures, row.subcommand, "error_response");
    }

    launch_fixture(&fixtures, "request");
    for event in LAUNCH_EVENT_ROWS {
        launch_event_fixture(&fixtures, event.kind);
    }
}

#[test]
fn request_success_and_error_fixtures_validate_against_schema_targets() {
    let fixtures = fixtures();
    for row in NON_LAUNCH_ROWS {
        validate_against_def(
            row.schema_file,
            row.request_schema_def,
            non_launch_fixture(&fixtures, row.subcommand, "request"),
        );
        validate_against_def(
            row.schema_file,
            row.success_response_schema_def,
            non_launch_fixture(&fixtures, row.subcommand, "success_response"),
        );
        validate_against_def(
            row.schema_file,
            row.error_response_schema_def,
            non_launch_fixture(&fixtures, row.subcommand, "error_response"),
        );
    }
}

#[test]
fn launch_request_and_event_fixtures_validate_against_schema_targets() {
    let fixtures = fixtures();
    validate_against_def(
        "launch",
        LAUNCH_REQUEST_SCHEMA_DEF,
        launch_fixture(&fixtures, "request"),
    );
    for row in LAUNCH_EVENT_ROWS {
        validate_against_def(
            "launch",
            row.schema_def,
            launch_event_fixture(&fixtures, row.kind),
        );
    }
}

#[test]
fn session_replace_provider_owned_and_legacy_success_fixtures_validate() {
    let fixtures = fixtures();
    let provider_owned = non_launch_fixture(&fixtures, "session.replace", "success_response");
    let legacy = non_launch_fixture(&fixtures, "session.replace", "legacy_success_response");

    validate_against_def("session", "SessionReplaceResponse", provider_owned);
    validate_against_def("session", "SessionReplaceResponse", legacy);
}

#[test]
fn session_replace_provider_owned_request_fixture_has_no_host_observed_preimage() {
    let fixtures = fixtures();
    let request = non_launch_fixture(&fixtures, "session.replace", "request");
    let params = request.get("params").expect("replace params");

    validate_against_def("session", "SessionReplaceRequest", request);
    assert_eq!(
        params.get("replace_protocol"),
        Some(&json!("oulipoly.provider_owned_replace/v1"))
    );
    assert!(params.get("canonical_transcript").is_some());
    assert!(params.get("preimage_sha256_expected").is_some());
    assert!(
        params.get("preimage_sha256").is_none(),
        "provider-owned request must not include a host-observed preimage hash"
    );
}

#[test]
fn policy_response_schema_permits_absent_empty_argv_and_env() {
    let response = json!({
        "contract": "oulipoly.provider/v1",
        "request_id": "req-policy",
        "ok": true,
        "result": {
            "accepted": true,
            "stdin": null,
            "prompt": null,
            "diagnostics": [],
            "markers": []
        }
    });

    validate_against_def("policy", "PolicyEvaluateResponse", &response);
}

fn validate_against_def(capability: &str, definition: &str, instance: &Value) {
    let mut schema = load_json(&schema_file_for(capability));
    let common = load_json(&schema_file_for("common"));
    merge_common_defs_and_rewrite_refs(&mut schema, &common);

    let defs = schema
        .get("$defs")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let wrapper = json!({
        "$schema": SCHEMA_DRAFT_2020_12,
        "$defs": defs,
        "$ref": format!("#/$defs/{definition}")
    });

    let validator = jsonschema::validator_for(&wrapper)
        .unwrap_or_else(|err| panic!("{capability} {definition} schema did not compile: {err}"));
    let errors = validator
        .iter_errors(instance)
        .map(|err| err.to_string())
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "{capability} {definition} rejected fixture {instance}: {errors:?}"
    );
}

fn merge_common_defs_and_rewrite_refs(schema: &mut Value, common: &Value) {
    rewrite_common_refs(schema);

    let common_defs = common
        .get("$defs")
        .and_then(Value::as_object)
        .expect("common schema must have $defs");
    let schema_defs = schema
        .as_object_mut()
        .expect("schema root must be object")
        .entry("$defs")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("$defs must be object");

    for (name, definition) in common_defs {
        schema_defs
            .entry(name.clone())
            .or_insert_with(|| definition.clone());
    }
}

fn rewrite_common_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object
                .get("$ref")
                .and_then(Value::as_str)
                .map(str::to_owned)
                && let Some(definition) = reference.strip_prefix("common.schema.json#/$defs/")
            {
                object.insert(
                    "$ref".to_owned(),
                    Value::String(format!("#/$defs/{definition}")),
                );
            }
            for child in object.values_mut() {
                rewrite_common_refs(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                rewrite_common_refs(child);
            }
        }
        _ => {}
    }
}
