pub mod support {
    pub mod contract_matrix;
}

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use support::contract_matrix::{
    CONTRACT_VERSION, EXPECTED_SCHEMA_FILES, LOCKED_ERROR_CATEGORIES, SCHEMA_DRAFT_2020_12,
    contract_v1_dir, load_json, schema_file_for,
};

#[test]
fn schema_file_inventory_matches_expected_contract_v1() {
    let mut actual = fs::read_dir(contract_v1_dir())
        .expect("contract/v1 directory must exist")
        .map(|entry| {
            entry
                .expect("schema directory entry must be readable")
                .file_name()
                .into_string()
                .expect("schema filename must be UTF-8")
        })
        .filter(|name| name.ends_with(".schema.json"))
        .collect::<Vec<_>>();
    actual.sort();

    let mut expected = EXPECTED_SCHEMA_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    expected.sort();

    assert_eq!(actual, expected);
}

#[test]
fn contract_v1_schemas_declare_draft_2020_12() {
    for filename in EXPECTED_SCHEMA_FILES {
        let schema = load_json(&contract_v1_dir().join(filename));
        assert_eq!(
            schema.get("$schema").and_then(Value::as_str),
            Some(SCHEMA_DRAFT_2020_12),
            "{filename} must declare JSON Schema draft 2020-12"
        );
    }
}

#[test]
fn contract_v1_schemas_are_valid_draft_2020_12() {
    for filename in EXPECTED_SCHEMA_FILES {
        let schema = load_json(&contract_v1_dir().join(filename));
        let meta = jsonschema::meta::validator_for(&schema)
            .unwrap_or_else(|err| panic!("{filename} did not select a valid meta-schema: {err}"));
        let errors = meta
            .iter_errors(&schema)
            .map(|err| err.to_string())
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "{filename} is not valid schema: {errors:?}"
        );
    }
}

#[test]
fn contract_v1_refs_resolve_to_checked_in_schema_defs() {
    let schemas = EXPECTED_SCHEMA_FILES
        .iter()
        .map(|filename| {
            (
                (*filename).to_owned(),
                load_json(&contract_v1_dir().join(filename)),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut failures = Vec::new();
    for (filename, schema) in &schemas {
        for reference in collect_refs(schema) {
            if reference.starts_with('#') {
                if schema.pointer(reference.trim_start_matches('#')).is_none() {
                    failures.push(format!("{filename}: unresolved local ref {reference}"));
                }
                continue;
            }

            let Some((target_file, fragment)) = reference.split_once('#') else {
                failures.push(format!(
                    "{filename}: external ref lacks fragment {reference}"
                ));
                continue;
            };
            let Some(target_schema) = schemas.get(target_file) else {
                failures.push(format!("{filename}: missing ref target file {target_file}"));
                continue;
            };
            if target_schema.pointer(fragment).is_none() {
                failures.push(format!("{filename}: unresolved external ref {reference}"));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "schema $ref targets must resolve: {failures:#?}"
    );
}

#[test]
fn common_schema_owns_shared_contract_defs() {
    let common = load_json(&schema_file_for("common"));
    let defs = common
        .get("$defs")
        .and_then(Value::as_object)
        .expect("common.schema.json must define $defs");
    let required = [
        "ContractVersion",
        "RequestId",
        "ProviderInstanceId",
        "HostContext",
        "RequestEnvelope",
        "SuccessResponseEnvelope",
        "ErrorResponseEnvelope",
        "ErrorCategory",
        "ErrorObject",
        "ArtifactRef",
        "FieldDescriptor",
        "Diagnostic",
        "BytePayload",
        "ProcessStatus",
        "TerminalSignal",
        "Timeout",
        "Cancellation",
        "Marker",
        "Artifact",
        "ProviderModelRequest",
        "TranscriptHint",
        "CanonicalTranscriptSource",
    ];

    let missing = required
        .iter()
        .filter(|name| !defs.contains_key(**name))
        .copied()
        .collect::<Vec<_>>();
    assert!(missing.is_empty(), "missing shared $defs: {missing:?}");

    assert_eq!(
        common
            .pointer("/$defs/ContractVersion/const")
            .and_then(Value::as_str),
        Some(CONTRACT_VERSION)
    );
}

#[test]
fn locked_error_categories_match_contract() {
    let common = load_json(&schema_file_for("common"));
    let actual = common
        .pointer("/$defs/ErrorCategory/enum")
        .and_then(Value::as_array)
        .expect("common ErrorCategory enum must exist")
        .iter()
        .map(|value| value.as_str().expect("error category must be a string"))
        .collect::<BTreeSet<_>>();
    let expected = LOCKED_ERROR_CATEGORIES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}

fn collect_refs(value: &Value) -> BTreeSet<String> {
    fn walk(value: &Value, refs: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                    refs.insert(reference.to_owned());
                }
                for child in object.values() {
                    walk(child, refs);
                }
            }
            Value::Array(values) => {
                for child in values {
                    walk(child, refs);
                }
            }
            _ => {}
        }
    }

    let mut refs = BTreeSet::new();
    walk(value, &mut refs);
    refs
}
