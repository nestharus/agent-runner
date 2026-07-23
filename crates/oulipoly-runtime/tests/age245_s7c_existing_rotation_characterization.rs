//! Declared roles: accessor, mapper, validator, orchestration.

use chrono::{DateTime, Duration, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, ScriptSessionStorageType,
    SessionStorage, SessionsConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::balancer::TransitionReason;
use oulipoly_runtime::migration::{MigrationError, migrate_chain_segment};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest,
    ProductionMigrationService,
};
use oulipoly_state::{
    InvocationStart, QuotaWindowInput, ResolvedResume, SessionTurnIngest, StateDb,
};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MODEL: &str = "age245-model";
const SOURCE_PROVIDER: &str = "source-provider";
const TARGET_PROVIDER: &str = "target-provider";
const EXTERNAL_MODEL: &str = "model-alpha";
const EXTERNAL_PROVIDER: &str = "fake-provider";
const SESSION_ID: &str = "77777777-7777-4777-8777-777777777777";
const TURN_ONE: &str = "turn-one";
const TURN_TWO: &str = "turn-two";

struct Fixture {
    dir: tempfile::TempDir,
    state: StateDb,
    source_root: PathBuf,
    target_root: PathBuf,
    workspace: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainRow {
    chain_id: String,
    created_at: String,
    last_used_at: String,
    model_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SegmentRow {
    id: i64,
    chain_id: String,
    provider_name: String,
    session_id: String,
    started_at: String,
    ended_at: Option<String>,
    last_turn_id: Option<String>,
    transition_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceMigrationSnapshot {
    segment: ComparableSegment,
    stderr: String,
    target_bytes: Vec<u8>,
    source_bytes: Vec<u8>,
    chains: Vec<ComparableChainRow>,
    segments: Vec<ComparableSegmentRow>,
    full_chain_rows: Vec<TableRow>,
    full_segment_rows: Vec<TableRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComparableSegment {
    chain_id: String,
    source_provider: String,
    source_session_id: String,
    target_provider: String,
    target_provider_index: usize,
    target_session_id: String,
    target_relative_path: PathBuf,
    reason: TransitionReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComparableChainRow {
    chain_id: String,
    created_at: String,
    last_used_at: String,
    model_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComparableSegmentRow {
    id: i64,
    chain_id: String,
    provider_name: String,
    session_id: String,
    started_at: String,
    ended_at: Option<String>,
    last_turn_id: Option<String>,
    transition_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableRow {
    fields: Vec<(String, String)>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = StateDb::open(&dir.path().join("state.db")).expect("state db");
        let source_root = dir.path().join("source-root");
        let target_root = dir.path().join("target-root");
        let workspace = dir.path().join("workspace").join("project");
        std::fs::create_dir_all(&workspace).expect("workspace");
        Self {
            dir,
            state,
            source_root,
            target_root,
            workspace,
        }
    }

    fn conn(&self) -> Connection {
        Connection::open(self.dir.path().join("state.db")).expect("sqlite")
    }

    fn model(&self) -> ModelConfig {
        ModelConfig {
            name: MODEL.to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                provider(SOURCE_PROVIDER, &self.source_root),
                provider(TARGET_PROVIDER, &self.target_root),
            ],
            inputs: Vec::new(),
            provider: None,
        }
    }

    fn seed_resolved(&self, model: &ModelConfig) -> ResolvedResume {
        let invocation_id = self
            .state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid::Uuid::new_v4().to_string(),
                model_name: MODEL.to_string(),
                provider_name: SOURCE_PROVIDER.to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .expect("start invocation");
        self.state
            .update_session_capture(invocation_id, Some(SESSION_ID), "fixture")
            .expect("capture");
        self.state
            .mint_chain_for_invocation_session(invocation_id)
            .expect("mint chain");
        let chain_id = self
            .state
            .chain_id_for_segment(SOURCE_PROVIDER, SESSION_ID)
            .expect("chain lookup")
            .expect("chain id");
        ResolvedResume {
            chain_id,
            model_name: Some(model.name.clone()),
            model: Some(model.clone()),
            active_provider: SOURCE_PROVIDER.to_string(),
            active_session_id: SESSION_ID.to_string(),
        }
    }

    fn seed_turns(&self, boundary: bool) {
        self.state
            .ingest_session_turns_batch(
                SOURCE_PROVIDER,
                &[
                    turn(TURN_ONE, "2026-05-01T00:00:00Z", false),
                    turn(TURN_TWO, "2026-05-01T00:00:10Z", boundary),
                ],
            )
            .expect("turn ingest");
    }

    fn write_source_transcript(&self, body: &str) -> PathBuf {
        let dir = self
            .source_root
            .join(storage_workspace_dir(&self.workspace));
        std::fs::create_dir_all(&dir).expect("source transcript dir");
        let path = dir.join(format!("{SESSION_ID}.jsonl"));
        std::fs::write(&path, body).expect("source transcript");
        path
    }

    fn marker_script(&self) -> (PathBuf, PathBuf) {
        let marker = self.dir.path().join("provider-call-count.txt");
        std::fs::write(&marker, "0").expect("marker seed");
        let script = self.dir.path().join("fake-provider");
        std::fs::write(
            &script,
            format!(
                "#!/usr/bin/env bash\nset -euo pipefail\ncount=$(cat {marker}); count=$((count + 1)); printf '%s' \"$count\" > {marker}; printf '{{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"request-example-001\",\"ok\":true,\"result\":{{}}}}\\n'\n",
                marker = shell_quote(&marker)
            ),
        )
        .expect("script");
        make_executable(&script);
        (script, marker)
    }
}

fn provider(name: &str, root: &Path) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: Some(ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }),
        session_capture: None,
        resume_acceptance: None,
        session_storage: Some(script_backed_storage(root)),
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn script_backed_storage(root: &Path) -> SessionStorage {
    let storage_type = serde_json::from_value::<ScriptSessionStorageType>(
        serde_json::Value::String(["cla", "ude_code"].concat()),
    )
    .expect("storage type");

    SessionStorage::Script {
        cwd_script: adapter_command("ude-code-cwd", root),
        transcript_script: Some(adapter_command("ude-code-locate-transcript", root)),
        storage_type: Some(storage_type),
    }
}

fn adapter_command(adapter_suffix: &str, root: &Path) -> String {
    format!("{}{} {}", "cla", adapter_suffix, double_quote(root))
}

fn external_model(provider_path: &Path) -> ModelConfig {
    ModelConfig {
        name: EXTERNAL_MODEL.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            EXTERNAL_PROVIDER,
            Vec::new(),
        )],
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

fn populated_registry_handle(provider_path: &Path) -> ProviderRegistryHandle {
    let registry = ProviderRegistry::from_model_configs(
        &[external_model(provider_path)],
        ProviderRegistryOptions::default(),
    )
    .expect("populated registry");
    ProviderRegistryHandle::new(Arc::new(registry))
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn double_quote(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('"', "\\\""))
}

fn turn(turn_id: &str, timestamp: &str, boundary: bool) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: SESSION_ID.to_string(),
        turn_id: turn_id.to_string(),
        timestamp: DateTime::parse_from_rfc3339(timestamp)
            .expect("timestamp")
            .with_timezone(&Utc),
        role: "assistant".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: boundary,
        body: Some(format!(r#"{{"id":"{turn_id}"}}"#)),
    }
}

fn quota_window(used_percent: f64, hours_until_reset: i64) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent,
        resets_at: Utc::now() + Duration::hours(hours_until_reset),
    }
}

fn seed_quota_threshold_pressure(fixture: &Fixture) {
    fixture
        .state
        .upsert_quota_refresh(SOURCE_PROVIDER, &[quota_window(0.83, 50)])
        .expect("source quota refresh");
    fixture
        .state
        .set_window_delta_for_test(SOURCE_PROVIDER, 0, 0.01, 22)
        .expect("source quota delta");
    fixture
        .state
        .upsert_quota_refresh(
            TARGET_PROVIDER,
            &[quota_window(0.19, 24 * 7), quota_window(0.09, 3)],
        )
        .expect("target quota refresh");
    fixture
        .state
        .set_window_delta_for_test(TARGET_PROVIDER, 0, 0.01, 22)
        .expect("target weekly quota delta");
    fixture
        .state
        .set_window_delta_for_test(TARGET_PROVIDER, 1, 0.01, 22)
        .expect("target short quota delta");
}

fn storage_workspace_dir(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' => '-',
            ch if (ch.is_ascii() && ch.is_alphanumeric()) || ch == '-' => ch,
            _ => '-',
        })
        .collect()
}

fn chain_rows(conn: &Connection) -> Vec<ChainRow> {
    let mut stmt = conn
        .prepare(
            "SELECT chain_id, created_at, last_used_at, model_name
             FROM session_chains
             ORDER BY chain_id",
        )
        .expect("prepare chain rows");
    stmt.query_map([], |row| {
        Ok(ChainRow {
            chain_id: row.get(0)?,
            created_at: row.get(1)?,
            last_used_at: row.get(2)?,
            model_name: row.get(3)?,
        })
    })
    .expect("query chain rows")
    .collect::<Result<Vec<_>, _>>()
    .expect("read chain rows")
}

fn segment_rows(conn: &Connection) -> Vec<SegmentRow> {
    let mut stmt = conn
        .prepare(
            "SELECT id, chain_id, provider_name, session_id, started_at, ended_at,
                    last_turn_id, transition_reason
             FROM session_chain_segments
             ORDER BY id",
        )
        .expect("prepare segment rows");
    stmt.query_map([], |row| {
        Ok(SegmentRow {
            id: row.get(0)?,
            chain_id: row.get(1)?,
            provider_name: row.get(2)?,
            session_id: row.get(3)?,
            started_at: row.get(4)?,
            ended_at: row.get(5)?,
            last_turn_id: row.get(6)?,
            transition_reason: row.get(7)?,
        })
    })
    .expect("query segment rows")
    .collect::<Result<Vec<_>, _>>()
    .expect("read segment rows")
}

fn set_chain_times(conn: &Connection, chain_id: &str) {
    conn.execute(
        "UPDATE session_chains
         SET created_at = '2026-05-01T00:00:00Z',
             last_used_at = '2026-05-01T00:00:00Z'
         WHERE chain_id = ?1",
        params![chain_id],
    )
    .expect("normalize chain times");
    conn.execute(
        "UPDATE session_chain_segments
         SET started_at = '2026-05-01T00:00:00Z'
         WHERE chain_id = ?1",
        params![chain_id],
    )
    .expect("normalize segment time");
}

fn migrate_direct_snapshot(
    fixture: &Fixture,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    reason: TransitionReason,
) -> ServiceMigrationSnapshot {
    let source_path = source_path_for(fixture);
    let mut stderr = Vec::new();
    let segment = migrate_chain_segment(
        &fixture.state,
        &SessionsConfig::default(),
        model,
        resolved,
        &fixture.workspace,
        1,
        reason,
        &mut stderr,
    )
    .expect("direct migration");
    migration_snapshot(fixture, segment, source_path, stderr)
}

fn migrate_service_snapshot(
    fixture: &Fixture,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    reason: TransitionReason,
) -> ServiceMigrationSnapshot {
    let source_path = source_path_for(fixture);
    let mut stderr = Vec::new();
    let (provider_path, marker_path) = fixture.marker_script();
    let service =
        ProductionMigrationService::with_registry_handle(populated_registry_handle(&provider_path));
    let output = service
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved,
            manual_target: service_manual_target(reason),
            active_exhausted: false,
            migration_model: model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .expect("service migration");
    let segment = match output {
        MigrationServiceOutput::Migrated { segment } => segment,
        other => panic!("expected migrated service output, got {other:?}"),
    };
    assert_eq!(
        std::fs::read_to_string(marker_path).expect("marker"),
        "0",
        "built-in provider == None service path must not invoke populated provider registry"
    );
    migration_snapshot(fixture, segment, source_path, stderr)
}

fn service_manual_target(reason: TransitionReason) -> Option<&'static str> {
    match reason {
        TransitionReason::Manual => Some(TARGET_PROVIDER),
        TransitionReason::QuotaThreshold
        | TransitionReason::Exhausted
        | TransitionReason::Initial
        | TransitionReason::Imported => None,
    }
}

fn migration_snapshot(
    fixture: &Fixture,
    segment: oulipoly_runtime::migration::MigratedSegment,
    source_path: PathBuf,
    stderr: Vec<u8>,
) -> ServiceMigrationSnapshot {
    ServiceMigrationSnapshot {
        target_bytes: std::fs::read(&segment.target_jsonl_path).expect("target bytes"),
        source_bytes: std::fs::read(source_path).expect("source bytes"),
        stderr: String::from_utf8(stderr).expect("stderr utf8"),
        segment: comparable_segment(segment),
        chains: comparable_chain_rows(chain_rows(&fixture.conn())),
        segments: comparable_segment_rows(segment_rows(&fixture.conn())),
        full_chain_rows: comparable_full_chain_rows(full_table_rows(
            &fixture.conn(),
            "session_chains",
            "chain_id",
        )),
        full_segment_rows: comparable_full_segment_rows(full_table_rows(
            &fixture.conn(),
            "session_chain_segments",
            "id",
        )),
    }
}

fn comparable_segment(segment: oulipoly_runtime::migration::MigratedSegment) -> ComparableSegment {
    ComparableSegment {
        chain_id: "<chain-id>".to_string(),
        source_provider: segment.source_provider,
        source_session_id: segment.source_session_id,
        target_provider: segment.target_provider,
        target_provider_index: segment.target_provider_index,
        target_session_id: segment.target_session_id,
        target_relative_path: segment
            .target_jsonl_path
            .file_name()
            .expect("target file name")
            .to_owned()
            .into(),
        reason: segment.reason,
    }
}

fn comparable_chain_rows(rows: Vec<ChainRow>) -> Vec<ComparableChainRow> {
    rows.into_iter()
        .map(|row| ComparableChainRow {
            chain_id: "<chain-id>".to_string(),
            created_at: row.created_at,
            last_used_at: row.last_used_at,
            model_name: row.model_name,
        })
        .collect()
}

fn comparable_segment_rows(rows: Vec<SegmentRow>) -> Vec<ComparableSegmentRow> {
    let close_time = rows
        .iter()
        .find(|row| row.ended_at.is_some())
        .and_then(|row| row.ended_at.as_deref())
        .map(str::to_string);
    rows.into_iter()
        .map(|row| ComparableSegmentRow {
            id: row.id,
            chain_id: "<chain-id>".to_string(),
            provider_name: row.provider_name,
            session_id: row.session_id,
            started_at: normalize_dynamic_time(&row.started_at, close_time.as_deref()),
            ended_at: row
                .ended_at
                .as_deref()
                .map(|time| normalize_dynamic_time(time, close_time.as_deref())),
            last_turn_id: row.last_turn_id,
            transition_reason: row.transition_reason,
        })
        .collect()
}

fn comparable_full_chain_rows(rows: Vec<TableRow>) -> Vec<TableRow> {
    rows.into_iter()
        .map(|row| TableRow {
            fields: row
                .fields
                .into_iter()
                .map(|(name, value)| {
                    if name == "chain_id" {
                        (name, "<chain-id>".to_string())
                    } else {
                        (name, value)
                    }
                })
                .collect(),
        })
        .collect()
}

fn comparable_full_segment_rows(rows: Vec<TableRow>) -> Vec<TableRow> {
    let close_time = rows
        .iter()
        .flat_map(|row| row.fields.iter())
        .find(|(name, value)| name == "ended_at" && value != "<NULL>")
        .map(|(_, value)| value.clone());
    rows.into_iter()
        .map(|row| TableRow {
            fields: row
                .fields
                .into_iter()
                .map(|(name, value)| match name.as_str() {
                    "chain_id" => (name, "<chain-id>".to_string()),
                    "started_at" | "ended_at" => {
                        (name, normalize_dynamic_time(&value, close_time.as_deref()))
                    }
                    _ => (name, value),
                })
                .collect(),
        })
        .collect()
}

fn full_table_rows(conn: &Connection, table: &str, order_by: &str) -> Vec<TableRow> {
    let sql = format!("SELECT * FROM {table} ORDER BY {order_by}");
    let mut stmt = conn.prepare(&sql).expect("prepare full table rows");
    let column_names = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let column_count = stmt.column_count();
    stmt.query_map([], |row| {
        let mut fields = Vec::new();
        for (index, name) in column_names.iter().enumerate().take(column_count) {
            fields.push((name.clone(), row_value(row, index)?));
        }
        Ok(TableRow { fields })
    })
    .expect("query full table rows")
    .collect::<Result<Vec<_>, _>>()
    .expect("read full table rows")
}

fn normalize_dynamic_time(value: &str, close_time: Option<&str>) -> String {
    if close_time == Some(value) {
        "<migration-time>".to_string()
    } else {
        value.to_string()
    }
}

fn row_value(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    let value = row.get_ref(index)?;
    Ok(match value {
        rusqlite::types::ValueRef::Null => "<NULL>".to_string(),
        rusqlite::types::ValueRef::Integer(value) => value.to_string(),
        rusqlite::types::ValueRef::Real(value) => value.to_string(),
        rusqlite::types::ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
        rusqlite::types::ValueRef::Blob(value) => format!("{value:?}"),
    })
}

fn source_path_for(fixture: &Fixture) -> PathBuf {
    fixture
        .source_root
        .join(storage_workspace_dir(&fixture.workspace))
        .join(format!("{SESSION_ID}.jsonl"))
}

fn seeded_fixture_for_ab(
    source_body: &str,
    boundary: bool,
) -> (Fixture, ModelConfig, ResolvedResume) {
    let fixture = Fixture::new();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    fixture.seed_turns(boundary);
    set_chain_times(&fixture.conn(), &resolved.chain_id);
    fixture.write_source_transcript(source_body);
    (fixture, model, resolved)
}

#[test]
fn built_in_service_matches_direct_manual_materialization_identity() {
    let source_body = format!(
        "{{\"uuid\":\"{TURN_ONE}\",\"sessionId\":\"{SESSION_ID}\"}}\n{{\"uuid\":\"{TURN_TWO}\",\"sessionId\":\"{SESSION_ID}\"}}\n"
    );
    let (direct_fixture, direct_model, direct_resolved) =
        seeded_fixture_for_ab(&source_body, false);
    let (service_fixture, service_model, service_resolved) =
        seeded_fixture_for_ab(&source_body, false);

    assert_eq!(
        migrate_direct_snapshot(
            &direct_fixture,
            &direct_model,
            &direct_resolved,
            TransitionReason::Manual,
        ),
        migrate_service_snapshot(
            &service_fixture,
            &service_model,
            &service_resolved,
            TransitionReason::Manual,
        )
    );
}

#[test]
fn built_in_service_matches_direct_compaction_boundary_materialization_identity() {
    let source_body = format!(
        "{{\"uuid\":\"{TURN_ONE}\"}}\n{{\"uuid\":\"{TURN_TWO}\"}}\n{{\"uuid\":\"turn-three\"}}\n"
    );
    let (direct_fixture, direct_model, direct_resolved) = seeded_fixture_for_ab(&source_body, true);
    let (service_fixture, service_model, service_resolved) =
        seeded_fixture_for_ab(&source_body, true);

    assert_eq!(
        migrate_direct_snapshot(
            &direct_fixture,
            &direct_model,
            &direct_resolved,
            TransitionReason::Manual,
        ),
        migrate_service_snapshot(
            &service_fixture,
            &service_model,
            &service_resolved,
            TransitionReason::Manual,
        )
    );
}

#[test]
fn built_in_service_matches_direct_active_exhausted_materialization_identity() {
    let source_body = format!(
        "{{\"uuid\":\"{TURN_ONE}\",\"sessionId\":\"{SESSION_ID}\"}}\n{{\"uuid\":\"{TURN_TWO}\",\"sessionId\":\"{SESSION_ID}\"}}\n"
    );
    let (direct_fixture, direct_model, direct_resolved) =
        seeded_fixture_for_ab(&source_body, false);
    let (service_fixture, service_model, service_resolved) =
        seeded_fixture_for_ab(&source_body, false);
    service_fixture
        .state
        .mark_exhausted(SOURCE_PROVIDER)
        .expect("mark source exhausted");

    assert_eq!(
        migrate_direct_snapshot(
            &direct_fixture,
            &direct_model,
            &direct_resolved,
            TransitionReason::Exhausted,
        ),
        migrate_service_snapshot(
            &service_fixture,
            &service_model,
            &service_resolved,
            TransitionReason::Exhausted,
        )
    );
}

#[test]
fn built_in_service_matches_direct_quota_threshold_materialization_identity() {
    let source_body = format!(
        "{{\"uuid\":\"{TURN_ONE}\",\"sessionId\":\"{SESSION_ID}\"}}\n{{\"uuid\":\"{TURN_TWO}\",\"sessionId\":\"{SESSION_ID}\"}}\n"
    );
    let (direct_fixture, direct_model, direct_resolved) =
        seeded_fixture_for_ab(&source_body, false);
    let (service_fixture, service_model, service_resolved) =
        seeded_fixture_for_ab(&source_body, false);
    seed_quota_threshold_pressure(&service_fixture);

    assert_eq!(
        migrate_direct_snapshot(
            &direct_fixture,
            &direct_model,
            &direct_resolved,
            TransitionReason::QuotaThreshold,
        ),
        migrate_service_snapshot(
            &service_fixture,
            &service_model,
            &service_resolved,
            TransitionReason::QuotaThreshold,
        )
    );
}

#[test]
fn built_in_service_missing_source_leaves_storage_and_chain_rows_unchanged() {
    let fixture = Fixture::new();
    let (provider_path, marker_path) = fixture.marker_script();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    fixture.seed_turns(false);
    set_chain_times(&fixture.conn(), &resolved.chain_id);
    let before_chains = chain_rows(&fixture.conn());
    let before_segments = segment_rows(&fixture.conn());
    let service =
        ProductionMigrationService::with_registry_handle(populated_registry_handle(&provider_path));
    let mut stderr = Vec::new();
    let error = service
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some(TARGET_PROVIDER),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .expect_err("missing source should fail before service mutation");

    assert!(matches!(
        error,
        oulipoly_runtime::services::ServiceError::Dependency { .. }
    ));
    assert!(stderr.is_empty());
    assert!(!fixture.target_root.exists());
    assert_eq!(chain_rows(&fixture.conn()), before_chains);
    assert_eq!(segment_rows(&fixture.conn()), before_segments);
    assert_eq!(
        std::fs::read_to_string(marker_path).expect("marker"),
        "0",
        "built-in provider == None missing-source service path must not invoke provider registry"
    );
}

#[test]
fn built_in_dispatch_contract_requires_populated_registry_zero_provider_call_surface() {
    let fixture = Fixture::new();
    let (provider_path, marker_path) = fixture.marker_script();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    fixture.seed_turns(false);
    fixture.write_source_transcript(&format!(
        "{{\"uuid\":\"{TURN_ONE}\",\"sessionId\":\"{SESSION_ID}\"}}\n"
    ));
    let service =
        ProductionMigrationService::with_registry_handle(populated_registry_handle(&provider_path));
    let mut stderr = Vec::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some(TARGET_PROVIDER),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .expect("built-in service migration with populated registry");
    assert!(matches!(output, MigrationServiceOutput::Migrated { .. }));
    assert_eq!(
        std::fs::read_to_string(marker_path).expect("marker"),
        "0",
        "built-in provider == None dispatch fixture must observe zero provider calls"
    );
}

#[test]
fn built_in_materialize_preserves_source_storage_and_opens_exact_chain_rows() {
    let fixture = Fixture::new();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    fixture.seed_turns(false);
    set_chain_times(&fixture.conn(), &resolved.chain_id);
    let source_body = format!(
        "{{\"uuid\":\"{TURN_ONE}\",\"sessionId\":\"{SESSION_ID}\"}}\n{{\"uuid\":\"{TURN_TWO}\",\"sessionId\":\"{SESSION_ID}\"}}\n"
    );
    let source_path = fixture.write_source_transcript(&source_body);

    let mut stderr = Vec::new();
    let migrated = migrate_chain_segment(
        &fixture.state,
        &SessionsConfig::default(),
        &model,
        &resolved,
        &fixture.workspace,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .expect("built-in migration");

    assert_eq!(
        std::fs::read_to_string(&source_path).expect("source"),
        source_body
    );
    assert_eq!(
        std::fs::read_to_string(&migrated.target_jsonl_path).expect("target"),
        source_body
    );
    assert_eq!(
        migrated.target_jsonl_path,
        fixture
            .target_root
            .join(storage_workspace_dir(&fixture.workspace))
            .join(format!("{SESSION_ID}.jsonl"))
    );
    assert_eq!(
        String::from_utf8(stderr).expect("stderr"),
        format!("[migrate] {SOURCE_PROVIDER} -> {TARGET_PROVIDER} reason=manual\n")
    );

    let chains = chain_rows(&fixture.conn());
    assert_eq!(chains.len(), 1);
    let chain = &chains[0];
    assert_eq!(chain.chain_id, resolved.chain_id);
    assert_eq!(chain.created_at, "2026-05-01T00:00:00Z");
    assert_eq!(chain.model_name, MODEL);

    let segments = segment_rows(&fixture.conn());
    assert_eq!(segments.len(), 2);
    assert_eq!(
        segments[0],
        SegmentRow {
            id: 1,
            chain_id: resolved.chain_id.clone(),
            provider_name: SOURCE_PROVIDER.to_string(),
            session_id: SESSION_ID.to_string(),
            started_at: "2026-05-01T00:00:00Z".to_string(),
            ended_at: segments[0].ended_at.clone(),
            last_turn_id: Some(TURN_TWO.to_string()),
            transition_reason: "initial".to_string(),
        }
    );
    let close_time = segments[0]
        .ended_at
        .as_deref()
        .expect("source segment should be closed");
    assert_eq!(chain.last_used_at, "2026-05-01T00:00:00Z");
    assert_eq!(
        segments[1],
        SegmentRow {
            id: 2,
            chain_id: resolved.chain_id,
            provider_name: TARGET_PROVIDER.to_string(),
            session_id: SESSION_ID.to_string(),
            started_at: close_time.to_string(),
            ended_at: None,
            last_turn_id: None,
            transition_reason: "manual".to_string(),
        }
    );
}

#[test]
fn built_in_materialize_slices_provider_storage_from_recorded_boundary() {
    let fixture = Fixture::new();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    fixture.seed_turns(true);
    let pre_boundary = format!("{{\"uuid\":\"{TURN_ONE}\"}}\n");
    let post_boundary = format!("{{\"uuid\":\"{TURN_TWO}\"}}\n{{\"uuid\":\"turn-three\"}}\n");
    fixture.write_source_transcript(&(pre_boundary + &post_boundary));

    let mut stderr = Vec::new();
    let migrated = migrate_chain_segment(
        &fixture.state,
        &SessionsConfig::default(),
        &model,
        &resolved,
        &fixture.workspace,
        1,
        TransitionReason::QuotaThreshold,
        &mut stderr,
    )
    .expect("built-in migration");

    assert_eq!(
        std::fs::read_to_string(&migrated.target_jsonl_path).expect("target"),
        post_boundary
    );
    let segments = segment_rows(&fixture.conn());
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].last_turn_id.as_deref(), Some(TURN_TWO));
    assert_eq!(segments[1].transition_reason, "quota_threshold");
}

#[test]
fn built_in_materialize_missing_source_leaves_storage_and_chain_rows_unchanged() {
    let fixture = Fixture::new();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    fixture.seed_turns(false);
    set_chain_times(&fixture.conn(), &resolved.chain_id);
    let before_chains = chain_rows(&fixture.conn());
    let before_segments = segment_rows(&fixture.conn());

    let mut stderr = Vec::new();
    let error = migrate_chain_segment(
        &fixture.state,
        &SessionsConfig::default(),
        &model,
        &resolved,
        &fixture.workspace,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
    .expect_err("missing source should fail before mutation");

    assert!(matches!(
        error,
        MigrationError::SourceMissingStorage { provider } if provider == SOURCE_PROVIDER
    ));
    assert!(stderr.is_empty());
    assert!(!fixture.target_root.exists());
    assert_eq!(chain_rows(&fixture.conn()), before_chains);
    assert_eq!(segment_rows(&fixture.conn()), before_segments);
}
