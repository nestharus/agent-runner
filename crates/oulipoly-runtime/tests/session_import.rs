#![cfg(unix)]

use chrono::{DateTime, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_provider::generated::CONTRACT_VERSION;
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::services::{
    ProductionSessionImportService, SessionImportProviderStatus, SessionImportProviderTarget,
    SessionImportServicePort, SessionImportServiceRequest,
};
use oulipoly_state::StateDb;
use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SETTINGS_A: &str = "settings-a";
const SETTINGS_B: &str = "settings-b";

struct Fixture {
    dir: tempfile::TempDir,
    db: StateDb,
    state_path: PathBuf,
    mode_path: PathBuf,
    provider_path: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let db = StateDb::open(&state_path).unwrap();
        let mode_path = dir.path().join("provider-mode.txt");
        fs::write(&mode_path, mode).unwrap();
        let provider_path =
            write_fake_provider_script(dir.path(), "fake-provider", &mode_path, dir.path());
        Self {
            dir,
            db,
            state_path,
            mode_path,
            provider_path,
        }
    }

    fn set_mode(&self, mode: &str) {
        fs::write(&self.mode_path, mode).unwrap();
    }

    fn registry(&self) -> ProviderRegistry {
        ProviderRegistry::from_model_configs(
            &[model("model-a", "provider-a", &self.provider_path)],
            ProviderRegistryOptions::default()
                .with_config_root(self.dir.path().join("config"))
                .with_data_root(self.dir.path().join("data")),
        )
        .unwrap()
    }

    fn service(&self) -> ProductionSessionImportService {
        ProductionSessionImportService::with_registry_handle(ProviderRegistryHandle::new(Arc::new(
            self.registry(),
        )))
    }

    fn conn(&self) -> Connection {
        Connection::open(&self.state_path).unwrap()
    }
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse::<DateTime<Utc>>().unwrap()
}

fn target(model_name: &str, provider_name: &str, settings_id: &str) -> SessionImportProviderTarget {
    SessionImportProviderTarget {
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_instance_id: Some(format!("{provider_name}-instance")),
        settings_id: settings_id.to_string(),
    }
}

fn request<'a>(
    db: &'a StateDb,
    providers: &'a [SessionImportProviderTarget],
    observed_at: DateTime<Utc>,
) -> SessionImportServiceRequest<'a> {
    SessionImportServiceRequest {
        state: db,
        providers,
        observed_at,
        limit: Some(100),
        since_unix_ms: None,
        effective_cwd: None,
        backfill_turns: false,
    }
}

#[test]
fn first_import_mints_chains_and_reimport_only_refreshes_metadata() {
    let fixture = Fixture::new("two-v1");
    let service = fixture.service();
    let targets = [target("model-a", "provider-a", SETTINGS_A)];

    let first = service
        .import_sessions(request(&fixture.db, &targets, ts("2026-06-01T00:00:00Z")))
        .unwrap()
        .report;

    assert_eq!(first.totals.discovered, 2);
    assert_eq!(first.totals.imported, 2);
    assert_eq!(first.totals.skipped, 0);
    assert_eq!(chain_segment_count(&fixture.conn()), 2);
    let first_metadata = fixture
        .db
        .imported_session_display_metadata("provider-a", "native-one")
        .unwrap()
        .unwrap();
    assert_eq!(first_metadata.first_seen_at, ts("2026-06-01T00:00:00Z"));
    assert_eq!(first_metadata.last_seen_at, ts("2026-06-01T00:00:00Z"));
    assert_eq!(first_metadata.title.as_deref(), Some("Native one"));
    assert_eq!(first_metadata.turn_count, Some(3));

    fixture.set_mode("two-v2");
    let second = service
        .import_sessions(request(&fixture.db, &targets, ts("2026-06-01T00:05:00Z")))
        .unwrap()
        .report;

    assert_eq!(second.totals.discovered, 2);
    assert_eq!(second.totals.imported, 0);
    assert_eq!(second.totals.skipped, 2);
    assert_eq!(chain_segment_count(&fixture.conn()), 2);
    let refreshed = fixture
        .db
        .imported_session_display_metadata("provider-a", "native-one")
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.first_seen_at, ts("2026-06-01T00:00:00Z"));
    assert_eq!(refreshed.last_seen_at, ts("2026-06-01T00:05:00Z"));
    assert_eq!(refreshed.title.as_deref(), Some("Native one refreshed"));
    assert_eq!(refreshed.turn_count, Some(7));
}

#[test]
fn pre_existing_owned_session_is_not_duplicated_or_clobbered() {
    let fixture = Fixture::new("owned");
    seed_owned_segment(
        &fixture.conn(),
        "chain-owned",
        "provider-a",
        "owned-session",
    );
    let service = fixture.service();
    let targets = [target("model-a", "provider-a", SETTINGS_A)];

    let report = service
        .import_sessions(request(&fixture.db, &targets, ts("2026-06-01T00:00:00Z")))
        .unwrap()
        .report;

    assert_eq!(report.totals.imported, 0);
    assert_eq!(report.totals.skipped, 1);
    let conn = fixture.conn();
    assert_eq!(chain_segment_count(&conn), 1);
    let row = conn
        .query_row(
            "SELECT chain_id, transition_reason FROM session_chain_segments WHERE provider_name = 'provider-a' AND session_id = 'owned-session'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("chain-owned".to_string(), "initial".to_string()));
    assert!(
        fixture
            .db
            .imported_session_display_metadata("provider-a", "owned-session")
            .unwrap()
            .is_some(),
        "display metadata is independent of chain ownership"
    );
}

#[test]
fn provider_error_or_missing_capability_does_not_abort_other_provider_import() {
    for (bad_mode, expected_status) in [
        (
            "no-enumerate-capability",
            SessionImportProviderStatus::Skipped {
                reason: "session_enumerate_capability_missing: provider describe did not advertise session.enumerate capability".to_string(),
            },
        ),
        ("enumerate-error", SessionImportProviderStatus::Failed),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let db = StateDb::open(&state_path).unwrap();
        let bad_mode_path = dir.path().join("bad-mode.txt");
        let good_mode_path = dir.path().join("good-mode.txt");
        fs::write(&bad_mode_path, bad_mode).unwrap();
        fs::write(&good_mode_path, "one").unwrap();
        let bad_provider =
            write_fake_provider_script(dir.path(), "bad-provider", &bad_mode_path, dir.path());
        let good_provider =
            write_fake_provider_script(dir.path(), "good-provider", &good_mode_path, dir.path());
        let registry = ProviderRegistry::from_model_configs(
            &[
                model("model-a", "provider-a", &bad_provider),
                model("model-b", "provider-b", &good_provider),
            ],
            ProviderRegistryOptions::default()
                .with_config_root(dir.path().join("config"))
                .with_data_root(dir.path().join("data")),
        )
        .unwrap();
        let service = ProductionSessionImportService::with_registry_handle(
            ProviderRegistryHandle::new(Arc::new(registry)),
        );
        let targets = [
            target("model-a", "provider-a", SETTINGS_A),
            target("model-b", "provider-b", SETTINGS_B),
        ];

        let report = service
            .import_sessions(request(&db, &targets, ts("2026-06-01T00:00:00Z")))
            .unwrap()
            .report;

        assert_eq!(report.providers[0].status, expected_status);
        assert_eq!(report.providers[1].status, SessionImportProviderStatus::Succeeded);
        assert_eq!(report.providers[1].imported, 1);
        assert_eq!(report.totals.imported, 1);
        assert_eq!(chain_segment_count(&Connection::open(&state_path).unwrap()), 1);
    }
}

#[test]
fn fake_provider_import_lists_sessions_and_resume_resolves_provider_native_id() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("state.db");
    let db = StateDb::open(&state_path).unwrap();
    let provider_a_name = guarded_provider_name(&["cla", "ude"]);
    let provider_a_mode_path = dir.path().join("provider-a-mode.txt");
    let opencode_mode_path = dir.path().join("opencode-mode.txt");
    fs::write(&provider_a_mode_path, "provider-a-one").unwrap();
    fs::write(&opencode_mode_path, "opencode-one").unwrap();
    let provider_a_binary = write_fake_provider_script(
        dir.path(),
        "fake-provider-a",
        &provider_a_mode_path,
        dir.path(),
    );
    let opencode_provider =
        write_fake_provider_script(dir.path(), "fake-opencode", &opencode_mode_path, dir.path());
    let registry = ProviderRegistry::from_model_configs(
        &[
            model("provider-a-model", &provider_a_name, &provider_a_binary),
            model("opencode-model", "opencode", &opencode_provider),
        ],
        ProviderRegistryOptions::default()
            .with_config_root(dir.path().join("config"))
            .with_data_root(dir.path().join("data")),
    )
    .unwrap();
    let service = ProductionSessionImportService::with_registry_handle(
        ProviderRegistryHandle::new(Arc::new(registry)),
    );
    let targets = [
        target("provider-a-model", &provider_a_name, SETTINGS_A),
        target("opencode-model", "opencode", SETTINGS_B),
    ];

    let report = service
        .import_sessions(request(&db, &targets, ts("2026-06-01T00:00:00Z")))
        .unwrap()
        .report;

    assert_eq!(report.totals.imported, 2);
    let rows = db.imported_session_list().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| (
                row.active_provider.as_str(),
                row.active_provider_session_id.as_str(),
                row.turn_count,
                row.is_imported,
            ))
            .collect::<Vec<_>>(),
        vec![
            (provider_a_name.as_str(), "provider-a-native", 0, true),
            ("opencode", "opencode-native", 0, true),
        ]
    );
    let models = std::collections::HashMap::new();
    let resolved = db.resolve_resume(&models, "opencode-native", None).unwrap();
    assert_eq!(resolved.active_provider, "opencode");
    assert_eq!(resolved.active_session_id, "opencode-native");
}

#[test]
fn relative_cwd_is_reported_and_never_reaches_state() {
    let fixture = Fixture::new("relative-cwd");
    let service = fixture.service();
    let targets = [target("model-a", "provider-a", SETTINGS_A)];

    let report = service
        .import_sessions(request(&fixture.db, &targets, ts("2026-06-01T00:00:00Z")))
        .unwrap()
        .report;

    assert_eq!(report.totals.imported, 0);
    assert_eq!(report.totals.errors, 1);
    assert_eq!(
        report.providers[0].status,
        SessionImportProviderStatus::Failed
    );
    assert!(report.providers[0].errors[0].contains("session_enumerate_invalid_cwd"));
    assert_eq!(chain_segment_count(&fixture.conn()), 0);
    assert_eq!(metadata_count(&fixture.conn()), 0);
}

fn model(name: &str, provider_name: &str, provider_path: &Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(provider_name, Vec::new())],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(provider_path.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn chain_segment_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM session_chain_segments", [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn metadata_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM imported_session_display_metadata",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

fn seed_owned_segment(conn: &Connection, chain_id: &str, provider_name: &str, session_id: &str) {
    conn.execute(
        "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
         VALUES (?1, '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', 'model-a')",
        params![chain_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_chain_segments (chain_id, provider_name, session_id, started_at, transition_reason)
         VALUES (?1, ?2, ?3, '2026-05-01T00:00:00Z', 'initial')",
        params![chain_id, provider_name, session_id],
    )
    .unwrap();
}

fn write_fake_provider_script(dir: &Path, name: &str, mode_path: &Path, cwd: &Path) -> PathBuf {
    let script = dir.join(name);
    fs::write(&script, fake_provider_script(mode_path, cwd)).unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    script
}

fn fake_provider_script(mode_path: &Path, cwd: &Path) -> String {
    format!(
        r###"#!/usr/bin/env python3
import json
import pathlib
import sys

CONTRACT = "{contract}"
MODE_PATH = pathlib.Path({mode_path})
CWD = {cwd}

request = json.loads(sys.stdin.read() or "{{}}")
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
mode = MODE_PATH.read_text().strip()

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-session-import"),
        "ok": True,
        "result": result,
    }}

def error(code):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-session-import"),
        "ok": False,
        "error": {{
            "category": "failed",
            "code": code,
            "message": code,
            "retryable": False,
        }},
    }}

def describe():
    return envelope({{
        "provider_id": "fake-provider",
        "display_name": "Fake Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "session_enumerate": mode != "no-enumerate-capability",
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
    }})

def source():
    return {{"kind": "provider_native_list", "detail": "fixture"}}

def session(session_id, title, cwd, turn_count):
    return {{
        "provider_session_id": session_id,
        "title": title,
        "cwd": cwd,
        "created_unix_ms": 1782000000000,
        "updated_unix_ms": 1782000010000,
        "turn_count": turn_count,
        "source": source(),
    }}

def enumerate_sessions():
    if mode == "enumerate-error":
        return error("provider_enumerate_failed")
    if mode == "relative-cwd":
        sessions = [session("relative-session", "Relative", "relative/path", 1)]
    elif mode == "owned":
        sessions = [session("owned-session", "Owned", CWD, 2)]
    elif mode == "one":
        sessions = [session("native-one", "Native one", CWD, 3)]
    elif mode == "provider-a-one":
        sessions = [session("provider-a-native", "Provider A native", CWD, 3)]
    elif mode == "opencode-one":
        sessions = [session("opencode-native", "OpenCode native", CWD, 5)]
    elif mode == "two-v2":
        sessions = [
            session("native-one", "Native one refreshed", CWD, 7),
            session("native-two", "Native two refreshed", None, 9),
        ]
    else:
        sessions = [
            session("native-one", "Native one", CWD, 3),
            session("native-two", "Native two", None, 5),
        ]
    return envelope({{
        "sessions": sessions,
        "complete": True,
        "next_cursor": None,
        "warnings": [],
    }})

if subcommand == "describe":
    response = describe()
elif subcommand == "session.enumerate":
    response = enumerate_sessions()
else:
    response = error("unsupported_subcommand")

print(json.dumps(response))
"###,
        contract = CONTRACT_VERSION,
        mode_path = json_string(mode_path),
        cwd = json_string(cwd),
    )
}

fn json_string(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).unwrap()
}

fn guarded_provider_name(parts: &[&str]) -> String {
    parts.concat()
}
