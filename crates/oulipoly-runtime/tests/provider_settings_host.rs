use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_provider::generated::{CONTRACT_VERSION, SettingsValues};
use oulipoly_runtime::provider_settings::{ProviderSettingsHost, ProviderSettingsHostOptions};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

// risk: Provider diagnostics loss; level: runtime settings target; source: contract "Settings-capable and unsupported describe/target tests"
#[test]
fn describe_settings_target_reports_capability_and_schema_id() {
    let fixture = SettingsHostFixture::new();
    let target = fixture
        .host()
        .describe_settings_target("example-model")
        .expect("settings-capable describe should produce a target");

    assert_eq!(target.model_name, "example-model");
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
        .describe_settings_target("example-model")
        .expect("describe should still return a target for providers without settings");

    assert!(!target.settings_supported);
    assert!(target.schema_id.is_none());

    let error = fixture
        .host()
        .settings_list("example-model")
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

    let schema = host
        .settings_schema("example-model", "example.settings/v1")
        .expect("schema should invoke provider schema subcommand");
    let records = host
        .settings_list("example-model")
        .expect("list should invoke provider settings.list subcommand");
    let record = host
        .settings_get("example-model", "record")
        .expect("get should invoke provider settings.get subcommand");
    let created = host
        .settings_create(
            "example-model",
            Some("Record".to_owned()),
            settings_values(),
        )
        .expect("create should invoke provider settings.create subcommand");
    let updated = host
        .settings_update(
            "example-model",
            "record",
            "opaque-version",
            settings_values(),
        )
        .expect("update should invoke provider settings.update subcommand");
    let deleted = host
        .settings_delete("example-model", "record", "opaque-version")
        .expect("delete should invoke provider settings.delete subcommand");
    let validation = host
        .settings_validate("example-model", settings_values())
        .expect("validate should invoke provider settings.validate subcommand");
    let migration = host
        .settings_migrate(
            "example-model",
            true,
            json!({"models": {"example-model": {"provider": {"script": "opaque"}}}}),
        )
        .expect("migrate should invoke provider settings.migrate subcommand");

    assert_eq!(schema.schema_id, "example.settings/v1");
    assert_eq!(records.records[0].version, "opaque-version");
    assert_eq!(record.record.version, "opaque-version");
    assert_eq!(created.record.version, "provider-created-version");
    assert_eq!(updated.record.version, "provider-updated-version");
    assert!(deleted.deleted);
    assert!(validation.valid);
    assert_eq!(
        migration.actions,
        vec![json!({"kind": "would-write", "target": "record"})]
    );

    let calls = fixture.recorded_calls();
    let subcommands = calls
        .iter()
        .map(|call| call.subcommand.as_str())
        .collect::<Vec<_>>();
    for subcommand in &subcommands {
        assert!(
            matches!(
                *subcommand,
                "describe"
                    | "schema"
                    | "settings.list"
                    | "settings.get"
                    | "settings.create"
                    | "settings.update"
                    | "settings.delete"
                    | "settings.validate"
                    | "settings.migrate"
            ),
            "settings host must not invoke broad or unrelated provider subcommands: {subcommand}"
        );
    }
    assert_eq!(
        subcommands
            .into_iter()
            .filter(|subcommand| *subcommand != "describe")
            .collect::<Vec<_>>(),
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
    );

    for call in calls {
        assert_eq!(call.request["contract"], CONTRACT_VERSION);
        assert!(
            call.request["request_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
            "request ids must be present and non-empty for {subcommand}",
            subcommand = call.subcommand
        );
        assert_eq!(call.request["provider_instance_id"], "provider-settings");
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

    assert_exact_settings_params(
        &fixture.recorded_calls(),
        &[
            ("schema", json!({"schema_id": "example.settings/v1"})),
            ("settings.list", json!({})),
            ("settings.get", json!({"id": "record"})),
            (
                "settings.create",
                json!({
                    "display_name": "Record",
                    "values": {
                        "endpoint": "https://example.test",
                        "enabled": true,
                        "limit": 3,
                    },
                }),
            ),
            (
                "settings.update",
                json!({
                    "id": "record",
                    "version": "opaque-version",
                    "values": {
                        "endpoint": "https://example.test",
                        "enabled": true,
                        "limit": 3,
                    },
                }),
            ),
            (
                "settings.delete",
                json!({"id": "record", "version": "opaque-version"}),
            ),
            (
                "settings.validate",
                json!({
                    "values": {
                        "endpoint": "https://example.test",
                        "enabled": true,
                        "limit": 3,
                    },
                }),
            ),
            (
                "settings.migrate",
                json!({
                    "dry_run": true,
                    "legacy": {
                        "models": {
                            "example-model": {
                                "provider": {"script": "opaque"},
                            },
                        },
                    },
                }),
            ),
        ],
    );
}

// risk: Registry/model reload staleness; level: runtime settings host; source: contract "Registry refresh discards stale describe/settings cache by replacing the registry/service instance"
#[test]
fn rebuild_replaces_registry_reflects_configured_artifact_changes_and_discards_settings_cache() {
    let fixture = RebuildFixture::new();
    let mut host = ProviderSettingsHost::from_model_configs(
        &[model_with_provider_name(
            "example-model",
            "provider-a",
            &fixture.provider_a,
        )],
        fixture.options(),
    )
    .expect("initial settings host should build from configured provider artifact");

    let first_target = host
        .describe_settings_target("example-model")
        .expect("initial provider should describe");
    let first_schema = host
        .settings_schema("example-model", "example.settings/a")
        .expect("initial provider should return schema");
    assert_eq!(first_target.provider_id, "provider-a");
    assert_eq!(first_schema.schema_id, "example.settings/a");

    host.rebuild_from_model_configs(&[model_with_provider_name(
        "example-model",
        "provider-b",
        &fixture.provider_b,
    )])
    .expect("rebuild should replace configured registry/service");

    let rebuilt_target = host
        .describe_settings_target("example-model")
        .expect("rebuilt provider should describe");
    let rebuilt_schema = host
        .settings_schema("example-model", "example.settings/b")
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
        .settings_update(
            "example-model",
            "record",
            "stale-version",
            settings_values(),
        )
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

    host.settings_migrate("example-model", true, legacy.clone())
        .expect("migration dry-run should invoke provider settings.migrate");

    let call = fixture
        .recorded_calls()
        .into_iter()
        .find(|call| call.subcommand == "settings.migrate")
        .expect("settings.migrate call should be recorded");
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

fn model_with_provider_name(name: &str, provider_name: &str, path: &Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(provider_name, Vec::new())],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(path.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn assert_exact_settings_params(calls: &[RecordedCall], expected: &[(&str, Value)]) {
    for (subcommand, expected_params) in expected {
        let call = calls
            .iter()
            .find(|call| call.subcommand == *subcommand)
            .unwrap_or_else(|| panic!("missing recorded {subcommand} request"));
        assert_eq!(
            &call.request["params"], expected_params,
            "{subcommand} params must match the generated provider contract exactly"
        );
    }
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
        fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<RecordedCall>(line).unwrap())
            .filter(|call| call.request["provider_instance_id"] == provider_id)
            .map(|call| call.subcommand)
            .collect()
    }
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
        ProviderSettingsHost::from_model_configs(
            &[model(&self.script)],
            ProviderSettingsHostOptions::default()
                .with_config_root(self.config_root.clone())
                .with_data_root(self.data_root.clone()),
        )
        .expect("settings host should build from configured model refs")
    }

    fn recorded_calls(&self) -> Vec<RecordedCall> {
        fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("recorded call should parse"))
            .collect()
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
