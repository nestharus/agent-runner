pub mod support {
    pub mod contract_matrix;
}

use std::collections::BTreeSet;
use support::contract_matrix::{
    EXPECTED_LAUNCH_EVENT_KINDS, EXPECTED_SUBCOMMANDS, LAUNCH_EVENT_ROWS, LAUNCH_REQUEST_DTO,
    LAUNCH_REQUEST_SCHEMA_DEF, NON_LAUNCH_ROWS, assert_no_duplicates, fixtures,
    launch_event_fixture, launch_fixture, non_launch_fixture, schema_file_for,
};

#[test]
fn s2_completeness_matrix_covers_every_design_subcommand() {
    assert_no_duplicates("expected subcommands", EXPECTED_SUBCOMMANDS);

    let matrix = NON_LAUNCH_ROWS
        .iter()
        .map(|row| row.subcommand)
        .chain(std::iter::once("launch"))
        .collect::<BTreeSet<_>>();
    let expected = EXPECTED_SUBCOMMANDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    assert_eq!(matrix, expected);

    let event_matrix = LAUNCH_EVENT_ROWS
        .iter()
        .map(|row| row.kind)
        .collect::<BTreeSet<_>>();
    let event_expected = EXPECTED_LAUNCH_EVENT_KINDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(event_matrix, event_expected);
}

#[test]
fn completeness_matrix_declares_schema_dto_fixture_and_registry_targets() {
    let fixtures = fixtures();

    for row in NON_LAUNCH_ROWS {
        assert!(
            schema_file_for(row.schema_file).exists(),
            "{} schema file must exist for {}",
            row.schema_file,
            row.subcommand
        );
        assert!(!row.request_schema_def.is_empty());
        assert!(!row.result_schema_def.is_empty());
        assert!(!row.success_response_schema_def.is_empty());
        assert!(!row.error_response_schema_def.is_empty());
        assert!(!row.request_dto.is_empty());
        assert!(!row.result_dto.is_empty());
        assert!(!row.response_dto.is_empty());

        non_launch_fixture(&fixtures, row.subcommand, "request");
        non_launch_fixture(&fixtures, row.subcommand, "success_response");
        non_launch_fixture(&fixtures, row.subcommand, "error_response");
    }

    assert!(
        schema_file_for("launch").exists(),
        "launch schema file must exist"
    );
    assert_eq!(LAUNCH_REQUEST_DTO, "LaunchRequest");
    assert_eq!(LAUNCH_REQUEST_SCHEMA_DEF, "LaunchRequest");
    launch_fixture(&fixtures, "request");
    for event in LAUNCH_EVENT_ROWS {
        assert!(!event.schema_def.is_empty());
        assert!(!event.dto.is_empty());
        launch_event_fixture(&fixtures, event.kind);
    }
}
