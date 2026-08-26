pub mod support {
    pub mod contract_matrix;
}

use oulipoly_provider::schemas::SchemaRegistry;
use support::contract_matrix::{
    EXPECTED_SCHEMA_FILES, LAUNCH_EVENT_ROWS, NON_LAUNCH_ROWS, fixtures, launch_event_fixture,
    launch_fixture, non_launch_fixture,
};

#[test]
fn schema_registry_lookup_and_validation_helpers_cover_s2_matrix() {
    let registry = SchemaRegistry::new();
    let fixtures = fixtures();

    for filename in EXPECTED_SCHEMA_FILES {
        assert!(
            registry.schema_by_file(filename).is_some(),
            "schema registry must embed or name {filename}"
        );
    }

    for row in NON_LAUNCH_ROWS {
        let schema = registry
            .schema_for_subcommand(row.subcommand)
            .unwrap_or_else(|| panic!("missing registry row for {}", row.subcommand));
        assert_eq!(
            schema.schema_file,
            format!("{}.schema.json", row.schema_file)
        );
        assert_eq!(schema.request_def, row.request_schema_def);
        assert_eq!(schema.response_def, Some(row.success_response_schema_def));
        assert_eq!(
            schema.error_response_def,
            Some(row.error_response_schema_def)
        );

        registry
            .validate_request(
                row.subcommand,
                non_launch_fixture(&fixtures, row.subcommand, "request"),
            )
            .unwrap_or_else(|err| {
                panic!("request validation failed for {}: {err}", row.subcommand)
            });
        registry
            .validate_response(
                row.subcommand,
                non_launch_fixture(&fixtures, row.subcommand, "success_response"),
            )
            .unwrap_or_else(|err| {
                panic!("response validation failed for {}: {err}", row.subcommand)
            });
        registry
            .validate_error_response(
                row.subcommand,
                non_launch_fixture(&fixtures, row.subcommand, "error_response"),
            )
            .unwrap_or_else(|err| {
                panic!(
                    "error response validation failed for {}: {err}",
                    row.subcommand
                )
            });
    }

    registry
        .validate_request(
            "describe",
            non_launch_fixture(&fixtures, "describe", "legacy_request"),
        )
        .expect("legacy describe request must remain valid");
    registry
        .validate_response(
            "describe",
            non_launch_fixture(&fixtures, "describe", "legacy_success_response"),
        )
        .expect("legacy describe response must omit the negotiated capability and remain valid");

    let launch = registry
        .schema_for_subcommand("launch")
        .expect("missing launch request registry row");
    assert_eq!(launch.schema_file, "launch.schema.json");
    assert_eq!(launch.request_def, "LaunchRequest");
    assert!(launch.response_def.is_none());
    assert!(launch.error_response_def.is_none());
    registry
        .validate_request("launch", launch_fixture(&fixtures, "request"))
        .unwrap_or_else(|err| panic!("launch request validation failed: {err}"));

    for row in LAUNCH_EVENT_ROWS {
        let schema = registry
            .schema_for_launch_event(row.kind)
            .unwrap_or_else(|| panic!("missing launch event registry row for {}", row.kind));
        assert_eq!(schema.schema_file, "launch.schema.json");
        assert_eq!(schema.event_def, row.schema_def);
        registry
            .validate_launch_event(row.kind, launch_event_fixture(&fixtures, row.kind))
            .unwrap_or_else(|err| panic!("launch event validation failed for {}: {err}", row.kind));
    }
}
