pub mod support {
    pub mod contract_matrix;
}

use oulipoly_provider::generated as dto;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use support::contract_matrix::{
    LAUNCH_EVENT_ROWS, NON_LAUNCH_ROWS, fixtures, launch_event_fixture, launch_fixture,
    non_launch_fixture,
};

#[test]
fn dto_roundtrip_covers_every_s2_contract_type() {
    let fixtures = fixtures();

    assert_eq!(NON_LAUNCH_ROWS.len(), 29);
    assert_non_launch_round_trip::<
        dto::DescribeRequest,
        dto::DescribeResult,
        dto::DescribeResponse,
        dto::DescribeErrorResponse,
    >(&fixtures, "describe");
    assert_non_launch_round_trip::<
        dto::SchemaRequest,
        dto::SchemaResult,
        dto::SchemaResponse,
        dto::SchemaErrorResponse,
    >(&fixtures, "schema");
    assert_non_launch_round_trip::<
        dto::SettingsListRequest,
        dto::SettingsListResult,
        dto::SettingsListResponse,
        dto::SettingsListErrorResponse,
    >(&fixtures, "settings.list");
    assert_non_launch_round_trip::<
        dto::SettingsGetRequest,
        dto::SettingsGetResult,
        dto::SettingsGetResponse,
        dto::SettingsGetErrorResponse,
    >(&fixtures, "settings.get");
    assert_non_launch_round_trip::<
        dto::SettingsCreateRequest,
        dto::SettingsCreateResult,
        dto::SettingsCreateResponse,
        dto::SettingsCreateErrorResponse,
    >(&fixtures, "settings.create");
    assert_non_launch_round_trip::<
        dto::SettingsUpdateRequest,
        dto::SettingsUpdateResult,
        dto::SettingsUpdateResponse,
        dto::SettingsUpdateErrorResponse,
    >(&fixtures, "settings.update");
    assert_non_launch_round_trip::<
        dto::SettingsDeleteRequest,
        dto::SettingsDeleteResult,
        dto::SettingsDeleteResponse,
        dto::SettingsDeleteErrorResponse,
    >(&fixtures, "settings.delete");
    assert_non_launch_round_trip::<
        dto::SettingsValidateRequest,
        dto::SettingsValidateResult,
        dto::SettingsValidateResponse,
        dto::SettingsValidateErrorResponse,
    >(&fixtures, "settings.validate");
    assert_non_launch_round_trip::<
        dto::SettingsMigrateRequest,
        dto::SettingsMigrateResult,
        dto::SettingsMigrateResponse,
        dto::SettingsMigrateErrorResponse,
    >(&fixtures, "settings.migrate");
    assert_non_launch_round_trip::<
        dto::PolicyEvaluateRequest,
        dto::PolicyEvaluateResult,
        dto::PolicyEvaluateResponse,
        dto::PolicyEvaluateErrorResponse,
    >(&fixtures, "policy.evaluate");
    assert_non_launch_round_trip::<
        dto::TerminalClassifyRequest,
        dto::TerminalClassifyResult,
        dto::TerminalClassifyResponse,
        dto::TerminalClassifyErrorResponse,
    >(&fixtures, "terminal.classify");
    assert_non_launch_round_trip::<
        dto::QuotaSourceRequest,
        dto::QuotaSourceResult,
        dto::QuotaSourceResponse,
        dto::QuotaSourceErrorResponse,
    >(&fixtures, "quota.source");
    assert_non_launch_round_trip::<
        dto::QuotaProbeRequest,
        dto::QuotaProbeResult,
        dto::QuotaProbeResponse,
        dto::QuotaProbeErrorResponse,
    >(&fixtures, "quota.probe");
    assert_non_launch_round_trip::<
        dto::QuotaRefreshAuthRequest,
        dto::QuotaRefreshAuthResult,
        dto::QuotaRefreshAuthResponse,
        dto::QuotaRefreshAuthErrorResponse,
    >(&fixtures, "quota.refresh_auth");
    assert_non_launch_round_trip::<
        dto::SessionLocateTranscriptRequest,
        dto::SessionLocateTranscriptResult,
        dto::SessionLocateTranscriptResponse,
        dto::SessionLocateTranscriptErrorResponse,
    >(&fixtures, "session.locate_transcript");
    assert_non_launch_round_trip::<
        dto::SessionReadTurnsRequest,
        dto::SessionReadTurnsResult,
        dto::SessionReadTurnsResponse,
        dto::SessionReadTurnsErrorResponse,
    >(&fixtures, "session.read_turns");
    assert_non_launch_round_trip::<
        dto::SessionCaptureRequest,
        dto::SessionCaptureResult,
        dto::SessionCaptureResponse,
        dto::SessionCaptureErrorResponse,
    >(&fixtures, "session.capture");
    assert_non_launch_round_trip::<
        dto::SessionExportRequest,
        dto::SessionExportResult,
        dto::SessionExportResponse,
        dto::SessionExportErrorResponse,
    >(&fixtures, "session.export");
    assert_non_launch_round_trip::<
        dto::SessionReplaceRequest,
        dto::SessionReplaceResult,
        dto::SessionReplaceResponse,
        dto::SessionReplaceErrorResponse,
    >(&fixtures, "session.replace");
    assert_non_launch_round_trip::<
        dto::RotationAssessRequest,
        dto::RotationAssessResult,
        dto::RotationAssessResponse,
        dto::RotationAssessErrorResponse,
    >(&fixtures, "rotation.assess");
    assert_non_launch_round_trip::<
        dto::RotationMaterializeRequest,
        dto::RotationMaterializeResult,
        dto::RotationMaterializeResponse,
        dto::RotationMaterializeErrorResponse,
    >(&fixtures, "rotation.materialize");
    assert_non_launch_round_trip::<
        dto::DiscoveryModelsRequest,
        dto::DiscoveryModelsResult,
        dto::DiscoveryModelsResponse,
        dto::DiscoveryModelsErrorResponse,
    >(&fixtures, "discovery.models");
    assert_non_launch_round_trip::<
        dto::DiscoveryAccountsRequest,
        dto::DiscoveryAccountsResult,
        dto::DiscoveryAccountsResponse,
        dto::DiscoveryAccountsErrorResponse,
    >(&fixtures, "discovery.accounts");
    assert_non_launch_round_trip::<
        dto::SetupDetectRequest,
        dto::SetupDetectResult,
        dto::SetupDetectResponse,
        dto::SetupDetectErrorResponse,
    >(&fixtures, "setup.detect");
    assert_non_launch_round_trip::<
        dto::SetupInstallPlanRequest,
        dto::SetupInstallPlanResult,
        dto::SetupInstallPlanResponse,
        dto::SetupInstallPlanErrorResponse,
    >(&fixtures, "setup.install_plan");
    assert_non_launch_round_trip::<
        dto::SetupSyncPlanRequest,
        dto::SetupSyncPlanResult,
        dto::SetupSyncPlanResponse,
        dto::SetupSyncPlanErrorResponse,
    >(&fixtures, "setup.sync_plan");
    assert_non_launch_round_trip::<
        dto::SetupBrainTurnRequest,
        dto::SetupBrainTurnResult,
        dto::SetupBrainTurnResponse,
        dto::SetupBrainTurnErrorResponse,
    >(&fixtures, "setup_brain.turn");
    assert_non_launch_round_trip::<
        dto::MigrationPlanRequest,
        dto::MigrationPlanResult,
        dto::MigrationPlanResponse,
        dto::MigrationPlanErrorResponse,
    >(&fixtures, "migration.plan");
    assert_non_launch_round_trip::<
        dto::MigrationApplyRequest,
        dto::MigrationApplyResult,
        dto::MigrationApplyResponse,
        dto::MigrationApplyErrorResponse,
    >(&fixtures, "migration.apply");

    assert_eq!(LAUNCH_EVENT_ROWS.len(), 5);
    assert_json_round_trip::<dto::LaunchRequest>(launch_fixture(&fixtures, "request"));
    assert_json_round_trip::<dto::LaunchStdoutEvent>(launch_event_fixture(&fixtures, "stdout"));
    assert_json_round_trip::<dto::LaunchStderrEvent>(launch_event_fixture(&fixtures, "stderr"));
    assert_json_round_trip::<dto::LaunchMarkerEvent>(launch_event_fixture(&fixtures, "marker"));
    assert_json_round_trip::<dto::LaunchHeartbeatEvent>(launch_event_fixture(
        &fixtures,
        "heartbeat",
    ));
    assert_json_round_trip::<dto::LaunchExitEvent>(launch_event_fixture(&fixtures, "exit"));
}

#[test]
fn dto_discriminants_reject_schema_invalid_values() {
    let fixtures = fixtures();

    let mut stdout = launch_event_fixture(&fixtures, "stdout").clone();
    stdout["kind"] = json!("stderr");
    assert_deserialize_error::<dto::LaunchStdoutEvent>(&stdout);

    let mut terminal = launch_event_fixture(&fixtures, "exit").clone();
    terminal["terminal_signal"]["kind"] = json!("not_a_terminal_signal");
    assert_deserialize_error::<dto::LaunchExitEvent>(&terminal);

    let mut status = launch_event_fixture(&fixtures, "exit").clone();
    status["status"] = json!({"kind": "exited"});
    assert_deserialize_error::<dto::LaunchExitEvent>(&status);

    let mut success = non_launch_fixture(&fixtures, "describe", "success_response").clone();
    success["ok"] = json!(false);
    assert_deserialize_error::<dto::DescribeResponse>(&success);

    let mut error = non_launch_fixture(&fixtures, "describe", "error_response").clone();
    error["ok"] = json!(true);
    assert_deserialize_error::<dto::DescribeErrorResponse>(&error);

    let mut describe = non_launch_fixture(&fixtures, "describe", "success_response").clone();
    describe["result"]
        .as_object_mut()
        .expect("describe result must be object")
        .remove("provider_id");
    assert_deserialize_error::<dto::DescribeResponse>(&describe);

    let mut schema = non_launch_fixture(&fixtures, "schema", "request").clone();
    schema["params"]
        .as_object_mut()
        .expect("schema params must be object")
        .remove("schema_id");
    assert_deserialize_error::<dto::SchemaRequest>(&schema);
}

fn assert_non_launch_round_trip<Request, Result, Response, ErrorResponse>(
    fixtures: &Value,
    subcommand: &str,
) where
    Request: DeserializeOwned + Serialize,
    Result: DeserializeOwned + Serialize,
    Response: DeserializeOwned + Serialize,
    ErrorResponse: DeserializeOwned + Serialize,
{
    assert_json_round_trip::<Request>(non_launch_fixture(fixtures, subcommand, "request"));
    let success = non_launch_fixture(fixtures, subcommand, "success_response");
    assert_json_round_trip::<Result>(
        success
            .get("result")
            .unwrap_or_else(|| panic!("missing result fixture for {subcommand}")),
    );
    assert_json_round_trip::<Response>(success);
    assert_json_round_trip::<ErrorResponse>(non_launch_fixture(
        fixtures,
        subcommand,
        "error_response",
    ));
}

fn assert_json_round_trip<T>(value: &Value)
where
    T: DeserializeOwned + Serialize,
{
    let encoded = serde_json::to_string(value).expect("fixture must serialize");
    let typed: T = serde_json::from_str(&encoded).expect("fixture must deserialize through DTO");
    let reencoded = serde_json::to_value(typed).expect("DTO must serialize");
    assert_eq!(reencoded, *value);
}

fn assert_deserialize_error<T>(value: &Value)
where
    T: DeserializeOwned,
{
    let encoded = serde_json::to_string(value).expect("fixture must serialize");
    assert!(
        serde_json::from_str::<T>(&encoded).is_err(),
        "DTO accepted schema-invalid JSON: {value}"
    );
}
