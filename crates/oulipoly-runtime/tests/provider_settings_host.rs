use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry, ProvidersConfig,
};
use oulipoly_provider::generated::{
    CONTRACT_VERSION, SchemaResult, SettingsDeleteResult, SettingsGetResult, SettingsListResult,
    SettingsMigrateResult, SettingsValidateResult, SettingsValues, SettingsWriteResult,
};
use oulipoly_runtime::provider_settings::{ProviderSettingsHost, ProviderSettingsHostOptions};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

const ACCOUNT: &str = "provider-a";

// risk: Provider diagnostics loss; level: runtime settings target; source: contract "Settings-capable and unsupported describe/target tests"
#[test]
fn describe_settings_target_reports_capability_and_schema_id() {
    let fixture = SettingsHostFixture::new();
    let target = fixture
        .host()
        .describe_settings_target(ACCOUNT)
        .expect("settings-capable describe should produce a target");

    assert_eq!(target.model_name, ACCOUNT);
    assert_eq!(target.provider_id, "provider-a");
    assert!(target.settings_supported);
    assert_eq!(target.schema_id.as_deref(), Some("example.settings/v1"));
}

// risk: Provider diagnostics loss; level: runtime settings target; source: contract "Unsupported settings capability maps to structured unsupported/unavailable error"
#[test]
fn unsupported_settings_target_returns_structured_unsupported_error() {
    let fixture = SettingsHostFixture::unsupported();
    let target = fixture
        .host()
        .describe_settings_target(ACCOUNT)
        .expect("describe should still return a target for providers without settings");

    assert!(!target.settings_supported);
    assert!(target.schema_id.is_none());

    let error = fixture
        .host()
        .settings_list(ACCOUNT)
        .expect_err("settings.list should reject unsupported settings capability");
    assert_eq!(error.category(), "unsupported");
    assert_eq!(error.code(), Some("settings_unsupported"));
    assert_eq!(error.retryable(), Some(false));
}

// risk: Broad provider dispatch shortcut; level: runtime host; source: contract "Runtime request-envelope signals"
#[test]
fn settings_host_invokes_only_schema_and_settings_subcommands_with_typed_envelopes() {
    let fixture = SettingsHostFixture::new();
    let host = fixture.host();

    let schema = invoke_settings_schema(&host);
    let records = invoke_settings_list(&host);
    let record = invoke_settings_get(&host);
    let created = invoke_settings_create(&host);
    let updated = invoke_settings_update(&host);
    let deleted = invoke_settings_delete(&host);
    let validation = invoke_settings_validate(&host);
    let migration = invoke_settings_migrate(&host);

    assert_schema_result(&schema);
    assert_list_result(&records);
    assert_get_result(&record);
    assert_create_result(&created);
    assert_update_result(&updated);
    assert_delete_result(&deleted);
    assert_validate_result(&validation);
    assert_migrate_result(&migration);

    let calls = fixture.recorded_calls();
    assert_settings_call_trace(&calls, &fixture);
}

fn invoke_settings_schema(host: &ProviderSettingsHost) -> SchemaResult {
    host.settings_schema(ACCOUNT, "example.settings/v1")
        .expect("schema should invoke provider schema subcommand")
}

fn invoke_settings_list(host: &ProviderSettingsHost) -> SettingsListResult {
    host.settings_list(ACCOUNT)
        .expect("list should invoke provider settings.list subcommand")
}

fn invoke_settings_get(host: &ProviderSettingsHost) -> SettingsGetResult {
    host.settings_get(ACCOUNT, "record")
        .expect("get should invoke provider settings.get subcommand")
}

fn invoke_settings_create(host: &ProviderSettingsHost) -> SettingsWriteResult {
    host.settings_create(ACCOUNT, Some("Record".to_owned()), settings_values())
        .expect("create should invoke provider settings.create subcommand")
}

fn invoke_settings_update(host: &ProviderSettingsHost) -> SettingsWriteResult {
    host.settings_update(ACCOUNT, "record", "opaque-version", settings_values())
        .expect("update should invoke provider settings.update subcommand")
}

fn invoke_settings_delete(host: &ProviderSettingsHost) -> SettingsDeleteResult {
    host.settings_delete(ACCOUNT, "record", "opaque-version")
        .expect("delete should invoke provider settings.delete subcommand")
}

fn invoke_settings_validate(host: &ProviderSettingsHost) -> SettingsValidateResult {
    host.settings_validate(ACCOUNT, settings_values())
        .expect("validate should invoke provider settings.validate subcommand")
}

fn invoke_settings_migrate(host: &ProviderSettingsHost) -> SettingsMigrateResult {
    host.settings_migrate(ACCOUNT, true, legacy_settings_config())
        .expect("migrate should invoke provider settings.migrate subcommand")
}

fn assert_schema_result(schema: &SchemaResult) {
    assert_eq!(schema.schema_id, "example.settings/v1");
}

fn assert_list_result(records: &SettingsListResult) {
    assert_eq!(records.records[0].version, "opaque-version");
}

fn assert_get_result(record: &SettingsGetResult) {
    assert_eq!(record.record.version, "opaque-version");
}

fn assert_create_result(created: &SettingsWriteResult) {
    assert_eq!(created.record.version, "provider-created-version");
}

fn assert_update_result(updated: &SettingsWriteResult) {
    assert_eq!(updated.record.version, "provider-updated-version");
}

fn assert_delete_result(deleted: &SettingsDeleteResult) {
    assert!(deleted.deleted);
}

fn assert_validate_result(validation: &SettingsValidateResult) {
    assert!(validation.valid);
}

fn assert_migrate_result(migration: &SettingsMigrateResult) {
    assert_eq!(migration.actions, expected_migration_actions());
}

fn expected_migration_actions() -> Vec<Value> {
    vec![json!({"kind": "would-write", "target": "record"})]
}

fn legacy_settings_config() -> Value {
    json!({"models": {"example-model": {"provider": {"script": "opaque"}}}})
}

fn assert_settings_call_trace(calls: &[RecordedCall], fixture: &SettingsHostFixture) {
    let subcommands = call_subcommands(calls);
    assert_settings_host_subcommands_are_allowed(&subcommands);
    assert_non_describe_settings_subcommands(&subcommands);
    assert_common_settings_call_envelopes(calls, fixture);
    assert_exact_settings_params(calls, &expected_settings_params());
}

fn assert_non_describe_settings_subcommands(subcommands: &[&str]) {
    assert_eq!(
        non_describe_subcommands(subcommands),
        expected_settings_subcommands()
    );
}

fn expected_settings_subcommands() -> Vec<&'static str> {
    vec![
        "schema",
        "settings.list",
        "settings.get",
        "settings.create",
        "settings.update",
        "settings.delete",
        "settings.validate",
        "settings.migrate",
    ]
}

fn expected_settings_params() -> Vec<(&'static str, Value)> {
    vec![
        ("schema", json!({"schema_id": "example.settings/v1"})),
        ("settings.list", json!({})),
        ("settings.get", json!({"id": "record"})),
        ("settings.create", expected_create_params()),
        ("settings.update", expected_update_params()),
        (
            "settings.delete",
            json!({"id": "record", "version": "opaque-version"}),
        ),
        ("settings.validate", expected_validate_params()),
        ("settings.migrate", expected_migrate_params()),
    ]
}

fn expected_create_params() -> Value {
    json!({
        "display_name": "Record",
        "values": expected_settings_values_json(),
    })
}

fn expected_update_params() -> Value {
    json!({
        "id": "record",
        "version": "opaque-version",
        "values": expected_settings_values_json(),
    })
}

fn expected_validate_params() -> Value {
    json!({"values": expected_settings_values_json()})
}

fn expected_migrate_params() -> Value {
    json!({"dry_run": true, "legacy": legacy_settings_config()})
}

fn expected_settings_values_json() -> Value {
    json!({
        "endpoint": "https://example.test",
        "enabled": true,
        "limit": 3,
    })
}

// risk: Registry/model reload staleness; level: runtime settings host; source: contract "Registry refresh discards stale describe/settings cache by replacing the registry/service instance"
#[test]
fn rebuild_replaces_registry_reflects_configured_artifact_changes_and_discards_settings_cache() {
    let fixture = RebuildFixture::new();
    let host = ProviderSettingsHost::from_configs(
        &[model_with_provider_name(
            "example-model",
            "provider-a",
            &fixture.provider_a,
        )],
        &providers("provider-a", &fixture.provider_a),
        fixture.options(),
    )
    .expect("initial settings host should build from configured provider artifact");

    let first_target = host
        .describe_settings_target("provider-a")
        .expect("initial provider should describe");
    let first_schema = host
        .settings_schema("provider-a", "example.settings/a")
        .expect("initial provider should return schema");
    assert_eq!(first_target.provider_id, "provider-a");
    assert_eq!(first_schema.schema_id, "example.settings/a");

    host.rebuild_from_configs(
        &[model_with_provider_name(
            "example-model",
            "provider-b",
            &fixture.provider_b,
        )],
        &providers("provider-b", &fixture.provider_b),
        fixture.options(),
    )
    .expect("rebuild should replace configured registry/service");

    let rebuilt_target = host
        .describe_settings_target("provider-b")
        .expect("rebuilt provider should describe");
    let rebuilt_schema = host
        .settings_schema("provider-b", "example.settings/b")
        .expect("rebuilt provider should return schema from the new artifact");

    assert_eq!(rebuilt_target.provider_id, "provider-b");
    assert_eq!(rebuilt_schema.schema_id, "example.settings/b");
    assert_eq!(
        fixture.subcommands_for("provider-a"),
        vec!["describe", "schema"],
        "stale provider-a cache must not service calls after rebuild"
    );
    assert_eq!(
        fixture.subcommands_for("provider-b"),
        vec!["describe", "schema"],
        "rebuild must invoke the configured replacement artifact"
    );
}

// risk: Broad provider dispatch shortcut; level: runtime host API; source: contract "No public method may accept an arbitrary subcommand string"
#[test]
fn settings_host_does_not_expose_public_arbitrary_subcommand_dispatch() {
    let source = include_str!("../src/provider_settings/mod.rs");

    assert!(!source.contains("pub fn invoke("));
    assert!(!source.contains("pub async fn invoke("));
    assert!(!source.contains("subcommand: &str"));
    assert!(!source.contains("subcommand: String"));
}

// risk: Provider diagnostics loss and opaque version data loss; level: runtime host; source: contract "Runtime error signals"
#[test]
fn settings_host_preserves_conflict_error_details_and_diagnostics() {
    let fixture = SettingsHostFixture::new();
    let host = fixture.host();

    let error = host
        .settings_update(ACCOUNT, "record", "stale-version", settings_values())
        .expect_err("stale update should preserve provider conflict response");

    assert_eq!(error.category(), "conflict");
    assert_eq!(error.code(), Some("settings_conflict"));
    assert_eq!(error.message(), "record changed");
    assert_eq!(error.retryable(), Some(false));
    assert_eq!(
        error.details()["remote_version"],
        "provider-updated-version"
    );
    assert_eq!(error.diagnostics()[0].path.as_deref(), Some("/endpoint"));
    assert_eq!(error.diagnostics()[0].message, "Reload before saving");
    assert_eq!(
        error.process_status().and_then(|status| status.exit_code),
        Some(17)
    );
}

// risk: Migration mutating or interpreting central config; level: runtime host; source: contract "Migration Contract"
#[test]
fn settings_migrate_forwards_legacy_payload_opaquely() {
    let fixture = SettingsHostFixture::new();
    let host = fixture.host();
    let legacy = json!({
        "providers": {
            "provider-a": {
                "command": "example",
                "args": ["--setting", "opaque"],
                "nested": {"provider_owned": true}
            }
        },
        "models": {
            "example-model": {
                "providers": [{"name": "provider-a", "args": ["--profile", "record"]}]
            }
        }
    });

    host.settings_migrate(ACCOUNT, true, legacy.clone())
        .expect("migration dry-run should invoke provider settings.migrate");

    let calls = fixture.recorded_calls();
    let call = recorded_call_for_subcommand(&calls, "settings.migrate");
    assert_eq!(call.request["params"]["dry_run"], true);
    assert_eq!(call.request["params"]["legacy"], legacy);
}

fn settings_values() -> SettingsValues {
    BTreeMap::from([
        ("endpoint".to_owned(), json!("https://example.test")),
        ("enabled".to_owned(), json!(true)),
        ("limit".to_owned(), json!(3)),
    ])
}

fn model(path: &Path) -> ModelConfig {
    model_with_provider_name("example-model", "provider-a", path)
}

fn model_with_provider_name(name: &str, provider_name: &str, _path: &Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(provider_name, Vec::new())],
        inputs: Vec::new(),
        provider: None,
    }
}

fn providers(provider_name: &str, path: &Path) -> ProvidersConfig {
    ProvidersConfig {
        entries: HashMap::from([(
            provider_name.to_string(),
            ProviderEntry {
                implementation: Some(ProviderEndpointConfig {
                    family: "settings-fixture".to_string(),
                    executable: path.display().to_string(),
                }),
                ..Default::default()
            },
        )]),
    }
}

fn call_subcommands(calls: &[RecordedCall]) -> Vec<&str> {
    calls.iter().map(call_subcommand).collect()
}

fn call_subcommand(call: &RecordedCall) -> &str {
    call.subcommand.as_str()
}

fn assert_settings_host_subcommands_are_allowed(subcommands: &[&str]) {
    for subcommand in subcommands {
        assert!(
            settings_host_subcommand_is_allowed(subcommand),
            "settings host must not invoke broad or unrelated provider subcommands: {subcommand}"
        );
    }
}

fn settings_host_subcommand_is_allowed(subcommand: &str) -> bool {
    matches!(
        subcommand,
        "describe"
            | "schema"
            | "settings.list"
            | "settings.get"
            | "settings.create"
            | "settings.update"
            | "settings.delete"
            | "settings.validate"
            | "settings.migrate"
    )
}

fn non_describe_subcommands<'a>(subcommands: &[&'a str]) -> Vec<&'a str> {
    subcommands
        .iter()
        .copied()
        .filter(|subcommand| *subcommand != "describe")
        .collect()
}

fn assert_common_settings_call_envelopes(calls: &[RecordedCall], fixture: &SettingsHostFixture) {
    for call in calls {
        assert_eq!(call.request["contract"], CONTRACT_VERSION);
        assert!(
            call.request["request_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "request ids must be present and non-empty for {subcommand}",
            subcommand = call.subcommand
        );
        let expected_instance = if call.subcommand == "describe" {
            "provider-registry"
        } else {
            "provider-a-instance"
        };
        assert_eq!(call.request["provider_instance_id"], expected_instance);
        if call.subcommand == "describe" {
            assert!(call.request["host"]["config_root"].is_null());
            assert!(call.request["host"]["data_root"].is_null());
            assert_eq!(
                call.request["host"]["env"]["OULIPOLY_HOST_PROMPT_ACCEPTANCE_V1"],
                "1"
            );
        } else {
            assert_eq!(
                call.request["host"]["config_root"],
                fixture.config_root.display().to_string()
            );
            assert_eq!(
                call.request["host"]["data_root"],
                fixture.data_root.display().to_string()
            );
            assert_eq!(call.request["host"]["env"], json!({}));
        }
    }
}

fn assert_exact_settings_params(calls: &[RecordedCall], expected: &[(&str, Value)]) {
    for (subcommand, expected_params) in expected {
        let call = recorded_call_for_subcommand(calls, subcommand);
        assert_recorded_call_params(call, subcommand, expected_params);
    }
}

fn recorded_call_for_subcommand<'a>(
    calls: &'a [RecordedCall],
    subcommand: &str,
) -> &'a RecordedCall {
    calls
        .iter()
        .find(|call| call.subcommand == subcommand)
        .unwrap_or_else(|| panic!("missing recorded {subcommand} request"))
}

fn assert_recorded_call_params(call: &RecordedCall, subcommand: &str, expected_params: &Value) {
    assert_eq!(
        &call.request["params"], expected_params,
        "{subcommand} params must match the generated provider contract exactly"
    );
}

struct SettingsHostFixture {
    _temp: tempfile::TempDir,
    script: PathBuf,
    record: PathBuf,
    config_root: PathBuf,
    data_root: PathBuf,
}

struct RebuildFixture {
    _temp: tempfile::TempDir,
    provider_a: PathBuf,
    provider_b: PathBuf,
    record: PathBuf,
    config_root: PathBuf,
    data_root: PathBuf,
}

impl RebuildFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let record = temp.path().join("rebuild-calls.jsonl");
        let provider_a = temp.path().join("provider-a-settings.py");
        let provider_b = temp.path().join("provider-b-settings.py");
        let config_root = temp.path().join("config-root");
        let data_root = temp.path().join("data-root");
        fs::create_dir_all(&config_root).unwrap();
        fs::create_dir_all(&data_root).unwrap();
        fs::write(
            &provider_a,
            fake_rebuild_provider_script(&record, "provider-a", "example.settings/a"),
        )
        .unwrap();
        fs::write(
            &provider_b,
            fake_rebuild_provider_script(&record, "provider-b", "example.settings/b"),
        )
        .unwrap();
        make_executable(&provider_a);
        make_executable(&provider_b);
        Self {
            _temp: temp,
            provider_a,
            provider_b,
            record,
            config_root,
            data_root,
        }
    }

    fn options(&self) -> ProviderSettingsHostOptions {
        ProviderSettingsHostOptions::default()
            .with_config_root(self.config_root.clone())
            .with_data_root(self.data_root.clone())
    }

    fn subcommands_for(&self, provider_id: &str) -> Vec<String> {
        let calls = recorded_calls(&self.record);
        let provider_calls = recorded_calls_for_provider(calls, provider_id);
        recorded_call_subcommands(provider_calls)
    }
}

fn recorded_calls(path: &Path) -> Vec<RecordedCall> {
    parse_recorded_calls(&recorded_calls_text(path))
}

fn recorded_calls_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn parse_recorded_calls(text: &str) -> Vec<RecordedCall> {
    parse_recorded_call_lines(recorded_call_lines(text))
}

fn recorded_call_lines(text: &str) -> std::str::Lines<'_> {
    text.lines()
}

fn parse_recorded_call_lines(lines: std::str::Lines<'_>) -> Vec<RecordedCall> {
    lines.map(parse_recorded_call).collect()
}

fn parse_recorded_call(line: &str) -> RecordedCall {
    serde_json::from_str::<RecordedCall>(line).unwrap()
}

fn recorded_calls_for_provider(calls: Vec<RecordedCall>, provider_id: &str) -> Vec<RecordedCall> {
    calls
        .into_iter()
        .filter(|call| recorded_call_is_for_provider(call, provider_id))
        .collect()
}

fn recorded_call_is_for_provider(call: &RecordedCall, provider_id: &str) -> bool {
    call.request["provider_instance_id"] == provider_id
}

fn recorded_call_subcommands(calls: Vec<RecordedCall>) -> Vec<String> {
    calls.into_iter().map(recorded_call_subcommand).collect()
}

fn recorded_call_subcommand(call: RecordedCall) -> String {
    call.subcommand
}

impl SettingsHostFixture {
    fn new() -> Self {
        Self::with_settings_capability(true)
    }

    fn unsupported() -> Self {
        Self::with_settings_capability(false)
    }

    fn with_settings_capability(settings_supported: bool) -> Self {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let script = temp.path().join("provider-a-settings.py");
        let record = temp.path().join("calls.jsonl");
        let config_root = temp.path().join("config-root");
        let data_root = temp.path().join("data-root");
        fs::create_dir_all(&config_root).unwrap();
        fs::create_dir_all(&data_root).unwrap();
        fs::write(&script, fake_provider_script(&record, settings_supported)).unwrap();
        make_executable(&script);
        Self {
            _temp: temp,
            script,
            record,
            config_root,
            data_root,
        }
    }

    fn host(&self) -> ProviderSettingsHost {
        ProviderSettingsHost::from_configs(
            &[model(&self.script)],
            &providers("provider-a", &self.script),
            ProviderSettingsHostOptions::default()
                .with_config_root(self.config_root.clone())
                .with_data_root(self.data_root.clone()),
        )
        .expect("settings host should build from configured account endpoint")
    }

    fn recorded_calls(&self) -> Vec<RecordedCall> {
        recorded_calls(&self.record)
    }
}

#[derive(Debug, serde::Deserialize)]
struct RecordedCall {
    subcommand: String,
    request: Value,
}

fn fake_provider_script(record: &Path, settings_supported: bool) -> String {
    let record = serde_json::to_string(&record.display().to_string()).unwrap();
    let settings_supported = if settings_supported { "True" } else { "False" };
    let schema_id = if settings_supported == "True" {
        r#""settings_schema_id": "example.settings/v1","#
    } else {
        ""
    };
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

record_path = pathlib.Path({record})
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
with record_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{"contract": request.get("contract"), "request_id": request.get("request_id"), "ok": True, "result": result}}

def error():
    return {{
        "contract": request.get("contract"),
        "request_id": request.get("request_id"),
        "ok": False,
        "error": {{
            "category": "conflict",
            "code": "settings_conflict",
            "message": "record changed",
            "retryable": False,
            "details": {{"remote_version": "provider-updated-version"}},
            "diagnostics": [{{"severity": "warning", "message": "Reload before saving", "path": "/endpoint", "code": "stale"}}],
        }},
        "process_status": {{"kind": "exited", "code": 17}},
    }}

if subcommand == "schema":
    response = envelope({{"schema_id": "example.settings/v1", "schema": {{"type": "object"}}, "ui": {{"order": ["endpoint"]}}}})
elif subcommand == "describe":
    response = envelope({{"provider_id": "provider-a", "display_name": "Provider A", "contract_versions": ["{contract}"], "preferred_contract": "{contract}", "capabilities": {{"launch": True, "policy": False, "quota": False, "session": False, "terminal": False, "rotation": False, "discovery": False, "settings": {settings_supported}, "setup_brain": False, "setup": False, "migration": False}}, {schema_id} "concurrency": {{"safe_for_parallel_invocation": True, "state_locking": "none"}}}})
elif subcommand == "settings.list":
    response = envelope({{"records": [{{"id": "record", "display_name": "Record", "version": "opaque-version", "summary": {{"endpoint": "https://example.test"}}}}]}})
elif subcommand == "settings.get":
    response = envelope({{"record": {{"id": "record", "display_name": "Record", "version": "opaque-version", "values": {{"endpoint": "https://example.test"}}}}}})
elif subcommand == "settings.create":
    response = envelope({{"record": {{"id": "record", "display_name": request["params"].get("display_name") or "Record", "version": "provider-created-version", "values": request["params"]["values"]}}, "diagnostics": []}})
elif subcommand == "settings.update":
    response = error() if request["params"]["version"] == "stale-version" else envelope({{"record": {{"id": request["params"]["id"], "display_name": "Record", "version": "provider-updated-version", "values": request["params"]["values"]}}, "diagnostics": []}})
elif subcommand == "settings.delete":
    response = envelope({{"deleted": True, "id": request["params"]["id"]}})
elif subcommand == "settings.validate":
    response = envelope({{"valid": True, "diagnostics": []}})
elif subcommand == "settings.migrate":
    response = envelope({{"actions": [{{"kind": "would-write", "target": "record"}}], "warnings": ["dry-run"], "requires_user_input": False, "diagnostics": []}})
else:
    response = {{"contract": request.get("contract", "{contract}"), "request_id": request.get("request_id", "request-example-001"), "ok": False, "error": {{"category": "unsupported", "code": "unsupported_subcommand", "message": "unsupported", "retryable": False}}}}
print(json.dumps(response))
"#,
        record = record,
        contract = CONTRACT_VERSION,
        settings_supported = settings_supported,
        schema_id = schema_id
    )
}

fn fake_rebuild_provider_script(record: &Path, provider_id: &str, schema_id: &str) -> String {
    let record = serde_json::to_string(&record.display().to_string()).unwrap();
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

record_path = pathlib.Path({record})
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
request["provider_instance_id"] = "{provider_id}"
with record_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{"contract": request.get("contract"), "request_id": request.get("request_id"), "ok": True, "result": result}}

if subcommand == "describe":
    response = envelope({{"provider_id": "{provider_id}", "display_name": "Provider", "contract_versions": ["{contract}"], "preferred_contract": "{contract}", "capabilities": {{"launch": True, "policy": False, "quota": False, "session": False, "terminal": False, "rotation": False, "discovery": False, "settings": True, "setup_brain": False, "setup": False, "migration": False}}, "settings_schema_id": "{schema_id}"}})
elif subcommand == "schema":
    response = envelope({{"schema_id": "{schema_id}", "schema": {{"type": "object", "title": "{provider_id}"}}, "ui": {{}}}})
else:
    response = {{"contract": request.get("contract", "{contract}"), "request_id": request.get("request_id", "request-example-001"), "ok": False, "error": {{"category": "unsupported", "code": "unsupported_subcommand", "message": "unsupported", "retryable": False}}}}
print(json.dumps(response))
"#,
        record = record,
        provider_id = provider_id,
        schema_id = schema_id,
        contract = CONTRACT_VERSION
    )
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}
