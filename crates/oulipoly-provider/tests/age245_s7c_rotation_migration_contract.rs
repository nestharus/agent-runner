//! Declared roles: accessor, parser, validator, predicate, mapper, formatter.

use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const FIXTURE: &str = "tests/fixtures/age245_s7c_rotation_migration_matrix.json";
const SCHEMA_DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

#[test]
fn s7c_fixture_matrix_covers_required_rotation_and_migration_cases() {
    let matrix = load_matrix();
    assert_case_ids(
        &matrix,
        "rotation_assess",
        &[
            "assess-success",
            "assess-denied",
            "assess-provider-error",
            "assess-protocol-invalid",
            "assess-capability-missing",
            "assess-transport-failure",
        ],
    );
    assert_case_ids(
        &matrix,
        "rotation_materialize",
        &[
            "materialize-success",
            "materialize-missing-source",
            "materialize-dry-run",
            "materialize-no-change",
            "materialize-no-change-wrong-chain",
            "materialize-compaction-boundary",
            "materialize-no-mutation-on-failure",
            "materialize-provider-error",
            "materialize-protocol-invalid",
            "materialize-crash-after-artifact",
            "materialize-crash-during-apply",
        ],
    );
    assert_case_ids(
        &matrix,
        "migration",
        &[
            "migration-plan-success",
            "migration-plan-capability-missing",
            "migration-plan-error",
            "migration-plan-protocol-invalid",
            "migration-apply-success",
            "migration-apply-capability-missing",
            "migration-apply-error",
            "migration-apply-protocol-invalid",
        ],
    );
    assert_case_ids(
        &matrix,
        "host_state_plan_rejections",
        &[
            "reject-additional-property",
            "reject-wrong-chain-id",
            "reject-mismatched-provider",
            "reject-wrong-session-id",
            "reject-invalid-transition-reason",
            "reject-stale-snapshot",
            "reject-conflicting-active-target",
            "reject-missing-artifact",
            "reject-hash-mismatch",
            "reject-unsupported-version",
        ],
    );
}

#[test]
fn s7c_fake_provider_fixture_declares_every_matrix_mode() {
    let matrix = load_matrix();
    let fixture_source = std::fs::read_to_string(
        manifest_dir().join("tests/fixtures/provider_client/fake_provider.rs"),
    )
    .expect("fake provider fixture source");
    for section in ["rotation_assess", "rotation_materialize", "migration"] {
        for case in matrix
            .get(section)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{section} cases"))
        {
            let Some(mode) = case.get("mode").and_then(Value::as_str) else {
                continue;
            };
            assert!(
                fixture_source.contains(mode),
                "fake-provider fixture must declare mode {mode:?} for {section}"
            );
        }
    }
}

#[test]
fn s7c_rotation_materialize_success_fixture_validates_against_current_schema() {
    validate_against_def(
        "rotation",
        "RotationMaterializeResponse",
        &rotation_materialize_response(host_state_plan_success()),
    );
}

#[test]
fn s7c_rotation_host_state_plan_schema_is_strict_and_declared() {
    let schema = load_json(&schema_file("rotation"));
    let materialize_result = pointer_object(
        &schema,
        "/$defs/RotationMaterializeResult/properties/host_state_plan",
    );
    assert!(
        materialize_result.get("$ref").is_some(),
        "rotation.materialize host_state_plan must reference a declared strict schema, not a loose object"
    );

    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("rotation schema defs");
    let strict_plan = defs
        .values()
        .find(|definition| {
            definition
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key("chain_id"))
                && definition.get("additionalProperties") == Some(&Value::Bool(false))
        })
        .expect("strict host_state_plan definition with chain_id");
    assert!(
        strict_plan
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.iter().any(|field| field == "chain_id")),
        "strict host_state_plan schema must require chain identity"
    );
}

#[test]
fn s7c_rotation_host_state_plan_schema_rejects_fixture_negative_cases() {
    let matrix = load_matrix();
    let cases = matrix
        .get("host_state_plan_rejections")
        .and_then(Value::as_array)
        .expect("rejection cases");
    for case in cases {
        let id = case.get("id").and_then(Value::as_str).expect("case id");
        let plan = case.get("plan").expect("plan");
        let response = rotation_materialize_response(plan.clone());
        let errors = validation_errors("rotation", "RotationMaterializeResponse", &response);
        assert!(
            !errors.is_empty(),
            "{id} must be rejected by strict rotation materialize host_state_plan schema"
        );
    }
}

fn load_matrix() -> Value {
    load_json(&manifest_dir().join(FIXTURE))
}

fn assert_case_ids(matrix: &Value, key: &str, expected: &[&str]) {
    let actual = matrix
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{key} cases"))
        .iter()
        .map(|case| {
            case.get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{key} case id"))
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    let expected = expected
        .iter()
        .map(|id| id.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "{key} fixture coverage drifted");
}

fn host_state_plan_success() -> Value {
    json!({
        "schema_version": 1,
        "operation": "rotation.materialize",
        "chain_id": "chain-alpha",
        "source_provider": "source-provider",
        "target_provider": "target-provider",
        "source_session_id": "session-source",
        "target_session_id": "session-target",
        "transition_reason": "quota_threshold",
        "segments": [
            {
                "provider": "source-provider",
                "session_id": "session-source",
                "ended_at": "2026-05-01T00:00:00Z"
            },
            {
                "provider": "target-provider",
                "session_id": "session-target",
                "started_at": "2026-05-01T00:00:00Z"
            }
        ],
        "artifacts": [
            {
                "kind": "file",
                "path": "/tmp/oulipoly/session-target.jsonl",
                "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }
        ]
    })
}

fn rotation_materialize_response(plan: Value) -> Value {
    json!({
        "contract": "oulipoly.provider/v1",
        "request_id": "request-example-001",
        "ok": true,
        "result": {
            "changed": true,
            "target_provider_session_id": "session-target",
            "artifacts": [
                {
                    "kind": "file",
                    "path": "/tmp/oulipoly/session-target.jsonl",
                    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                }
            ],
            "host_state_plan": plan
        }
    })
}

fn validate_against_def(capability: &str, definition: &str, instance: &Value) {
    let errors = validation_errors(capability, definition, instance);
    assert!(
        errors.is_empty(),
        "{capability} {definition} rejected fixture {instance}: {errors:?}"
    );
}

fn validation_errors(capability: &str, definition: &str, instance: &Value) -> Vec<String> {
    let mut schema = load_json(&schema_file(capability));
    let common = load_json(&schema_file("common"));
    merge_common_defs_and_rewrite_refs(&mut schema, &common);

    let defs = schema
        .get("$defs")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let wrapper = json!({
        "$schema": SCHEMA_DRAFT,
        "$defs": defs,
        "$ref": format!("#/$defs/{definition}")
    });

    let validator = jsonschema::validator_for(&wrapper)
        .unwrap_or_else(|err| panic!("{capability} {definition} schema did not compile: {err}"));
    validator
        .iter_errors(instance)
        .map(|err| err.to_string())
        .collect::<Vec<_>>()
}

fn merge_common_defs_and_rewrite_refs(schema: &mut Value, common: &Value) {
    rewrite_common_refs(schema);
    let common_defs = common
        .get("$defs")
        .and_then(Value::as_object)
        .expect("common schema defs");
    let schema_defs = schema
        .as_object_mut()
        .expect("schema object")
        .entry("$defs")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .expect("schema defs object");
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

fn pointer_object<'a>(value: &'a Value, pointer: &str) -> &'a Map<String, Value> {
    value
        .pointer(pointer)
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("missing object at {pointer}"))
}

fn schema_file(capability: &str) -> PathBuf {
    manifest_dir()
        .parent()
        .expect("workspace root")
        .parent()
        .expect("repo root")
        .join("contract/v1")
        .join(format!("{capability}.schema.json"))
}

fn load_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
