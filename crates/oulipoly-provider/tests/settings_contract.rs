pub mod support {
    pub mod contract_matrix;
}

use serde_json::Value;
use support::contract_matrix::{fixtures, non_launch_fixture};

#[test]
fn settings_update_and_delete_requests_carry_required_version_preconditions() {
    let fixtures = fixtures();
    for subcommand in ["settings.update", "settings.delete"] {
        let request = non_launch_fixture(&fixtures, subcommand, "request");
        let params = request
            .get("params")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("{subcommand} request params must be an object"));
        assert!(
            params.contains_key("id"),
            "{subcommand} request must include id"
        );
        assert!(
            params.contains_key("version"),
            "{subcommand} request must include version precondition"
        );
    }
}

#[test]
fn settings_create_and_validate_do_not_require_persistence_version() {
    let fixtures = fixtures();
    for subcommand in ["settings.create", "settings.validate"] {
        let request = non_launch_fixture(&fixtures, subcommand, "request");
        let params = request
            .get("params")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("{subcommand} request params must be an object"));
        assert!(
            !params.contains_key("version"),
            "{subcommand} is a draft operation and must not require version"
        );
    }
}

#[test]
fn settings_version_conflict_error_fixture_uses_conflict_category() {
    let fixtures = fixtures();
    for subcommand in ["settings.update", "settings.delete"] {
        let category = non_launch_fixture(&fixtures, subcommand, "error_response")
            .pointer("/error/category")
            .and_then(Value::as_str);
        assert_eq!(
            category,
            Some("conflict"),
            "{subcommand} stale version errors"
        );
    }
}
