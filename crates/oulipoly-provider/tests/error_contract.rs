pub mod support {
    pub mod contract_matrix;
}

use serde_json::Value;
use std::collections::BTreeSet;
use support::contract_matrix::{
    LOCKED_ERROR_CATEGORIES, NON_LAUNCH_ROWS, fixtures, load_json, non_launch_fixture,
    schema_file_for,
};

#[test]
fn error_response_fixtures_use_locked_error_categories() {
    let fixtures = fixtures();
    let allowed = LOCKED_ERROR_CATEGORIES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();

    for row in NON_LAUNCH_ROWS {
        let category = non_launch_fixture(&fixtures, row.subcommand, "error_response")
            .pointer("/error/category")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{} error fixture lacks error.category", row.subcommand));
        assert!(
            allowed.contains(category),
            "{} uses non-contract error category {category}",
            row.subcommand
        );
        seen.insert(category);
    }

    assert!(
        LOCKED_ERROR_CATEGORIES
            .iter()
            .all(|category| seen.contains(category)),
        "fixtures should exercise every locked error category at least once; seen {seen:?}"
    );
}

#[test]
fn shared_error_object_requires_core_fields() {
    let common = load_json(&schema_file_for("common"));
    let required = common
        .pointer("/$defs/ErrorObject/required")
        .and_then(Value::as_array)
        .expect("ErrorObject required array must exist")
        .iter()
        .map(|value| value.as_str().expect("required field must be a string"))
        .collect::<BTreeSet<_>>();

    assert_eq!(
        required,
        ["category", "code", "message", "retryable"]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
}
