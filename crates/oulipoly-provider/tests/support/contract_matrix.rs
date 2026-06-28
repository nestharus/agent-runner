use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub const CONTRACT_VERSION: &str = "oulipoly.provider/v1";
pub const SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

pub const EXPECTED_SCHEMA_FILES: &[&str] = &[
    "common.schema.json",
    "describe.schema.json",
    "schema.schema.json",
    "settings.schema.json",
    "launch.schema.json",
    "policy.schema.json",
    "quota.schema.json",
    "terminal.schema.json",
    "session.schema.json",
    "rotation.schema.json",
    "discovery.schema.json",
    "setup.schema.json",
    "migration.schema.json",
];

pub const EXPECTED_SUBCOMMANDS: &[&str] = &[
    "describe",
    "schema",
    "settings.list",
    "settings.get",
    "settings.create",
    "settings.update",
    "settings.delete",
    "settings.validate",
    "settings.migrate",
    "policy.evaluate",
    "launch",
    "terminal.classify",
    "quota.source",
    "quota.probe",
    "quota.refresh_auth",
    "session.enumerate",
    "session.locate_transcript",
    "session.read_turns",
    "session.capture",
    "session.export",
    "session.replace",
    "rotation.assess",
    "rotation.materialize",
    "discovery.models",
    "discovery.accounts",
    "setup.detect",
    "setup.install_plan",
    "setup.sync_plan",
    "setup_brain.turn",
    "migration.plan",
    "migration.apply",
];

pub const EXPECTED_LAUNCH_EVENT_KINDS: &[&str] =
    &["stdout", "stderr", "marker", "heartbeat", "exit"];

pub const LOCKED_ERROR_CATEGORIES: &[&str] = &[
    "unsupported",
    "invalid_request",
    "invalid_settings",
    "unavailable",
    "timeout",
    "conflict",
    "failed",
];

#[derive(Debug, Clone, Copy)]
pub struct ContractRow {
    pub subcommand: &'static str,
    pub schema_file: &'static str,
    pub request_schema_def: &'static str,
    pub result_schema_def: &'static str,
    pub success_response_schema_def: &'static str,
    pub error_response_schema_def: &'static str,
    pub request_dto: &'static str,
    pub result_dto: &'static str,
    pub response_dto: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct LaunchEventRow {
    pub kind: &'static str,
    pub schema_def: &'static str,
    pub dto: &'static str,
}

macro_rules! row {
    ($subcommand:literal, $capability:literal, $stem:literal) => {
        ContractRow {
            subcommand: $subcommand,
            schema_file: $capability,
            request_schema_def: concat!($stem, "Request"),
            result_schema_def: concat!($stem, "Result"),
            success_response_schema_def: concat!($stem, "Response"),
            error_response_schema_def: concat!($stem, "ErrorResponse"),
            request_dto: concat!($stem, "Request"),
            result_dto: concat!($stem, "Result"),
            response_dto: concat!($stem, "Response"),
        }
    };
}

pub const NON_LAUNCH_ROWS: &[ContractRow] = &[
    row!("describe", "describe", "Describe"),
    row!("schema", "schema", "Schema"),
    row!("settings.list", "settings", "SettingsList"),
    row!("settings.get", "settings", "SettingsGet"),
    row!("settings.create", "settings", "SettingsCreate"),
    row!("settings.update", "settings", "SettingsUpdate"),
    row!("settings.delete", "settings", "SettingsDelete"),
    row!("settings.validate", "settings", "SettingsValidate"),
    row!("settings.migrate", "settings", "SettingsMigrate"),
    row!("policy.evaluate", "policy", "PolicyEvaluate"),
    row!("terminal.classify", "terminal", "TerminalClassify"),
    row!("quota.source", "quota", "QuotaSource"),
    row!("quota.probe", "quota", "QuotaProbe"),
    row!("quota.refresh_auth", "quota", "QuotaRefreshAuth"),
    row!(
        "session.locate_transcript",
        "session",
        "SessionLocateTranscript"
    ),
    row!("session.enumerate", "session", "SessionEnumerate"),
    row!("session.read_turns", "session", "SessionReadTurns"),
    row!("session.capture", "session", "SessionCapture"),
    row!("session.export", "session", "SessionExport"),
    row!("session.replace", "session", "SessionReplace"),
    row!("rotation.assess", "rotation", "RotationAssess"),
    row!("rotation.materialize", "rotation", "RotationMaterialize"),
    row!("discovery.models", "discovery", "DiscoveryModels"),
    row!("discovery.accounts", "discovery", "DiscoveryAccounts"),
    row!("setup.detect", "setup", "SetupDetect"),
    row!("setup.install_plan", "setup", "SetupInstallPlan"),
    row!("setup.sync_plan", "setup", "SetupSyncPlan"),
    row!("setup_brain.turn", "setup", "SetupBrainTurn"),
    row!("migration.plan", "migration", "MigrationPlan"),
    row!("migration.apply", "migration", "MigrationApply"),
];

pub const LAUNCH_REQUEST_DTO: &str = "LaunchRequest";
pub const LAUNCH_REQUEST_SCHEMA_DEF: &str = "LaunchRequest";
pub const LAUNCH_EVENT_ROWS: &[LaunchEventRow] = &[
    LaunchEventRow {
        kind: "stdout",
        schema_def: "LaunchStdoutEvent",
        dto: "LaunchStdoutEvent",
    },
    LaunchEventRow {
        kind: "stderr",
        schema_def: "LaunchStderrEvent",
        dto: "LaunchStderrEvent",
    },
    LaunchEventRow {
        kind: "marker",
        schema_def: "LaunchMarkerEvent",
        dto: "LaunchMarkerEvent",
    },
    LaunchEventRow {
        kind: "heartbeat",
        schema_def: "LaunchHeartbeatEvent",
        dto: "LaunchHeartbeatEvent",
    },
    LaunchEventRow {
        kind: "exit",
        schema_def: "LaunchExitEvent",
        dto: "LaunchExitEvent",
    },
];

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("provider crate is under crates/")
        .to_path_buf()
}

pub fn contract_v1_dir() -> PathBuf {
    repo_root().join("contract/v1")
}

pub fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/contract_v1/fixtures.json")
}

pub fn generated_rs_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated.rs")
}

pub fn schemas_rs_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/schemas.rs")
}

pub fn load_json(path: &Path) -> Value {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed reading {}: {err}", path.display()));
    serde_json::from_str(&contents)
        .unwrap_or_else(|err| panic!("failed parsing {} as JSON: {err}", path.display()))
}

pub fn fixtures() -> Value {
    load_json(&fixture_path())
}

pub fn non_launch_fixture<'a>(fixtures: &'a Value, subcommand: &str, name: &str) -> &'a Value {
    fixtures
        .pointer(&format!("/non_launch/{subcommand}/{name}"))
        .unwrap_or_else(|| panic!("missing fixture for {subcommand} {name}"))
}

pub fn launch_fixture<'a>(fixtures: &'a Value, name: &str) -> &'a Value {
    fixtures
        .pointer(&format!("/launch/{name}"))
        .unwrap_or_else(|| panic!("missing launch fixture {name}"))
}

pub fn launch_event_fixture<'a>(fixtures: &'a Value, kind: &str) -> &'a Value {
    fixtures
        .pointer(&format!("/launch/events/{kind}"))
        .unwrap_or_else(|| panic!("missing launch event fixture {kind}"))
}

pub fn assert_no_duplicates(label: &str, values: &[&str]) {
    let mut seen = BTreeSet::new();
    let duplicates = values
        .iter()
        .filter(|value| !seen.insert(**value))
        .copied()
        .collect::<Vec<_>>();
    assert!(
        duplicates.is_empty(),
        "{label} has duplicates: {duplicates:?}"
    );
}

pub fn schema_file_for(capability: &str) -> PathBuf {
    contract_v1_dir().join(format!("{capability}.schema.json"))
}

pub fn schema_def_pointer(definition: &str) -> String {
    format!("/$defs/{definition}")
}
