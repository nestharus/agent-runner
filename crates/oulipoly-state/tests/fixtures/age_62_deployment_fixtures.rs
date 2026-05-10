use oulipoly_state::deployment::DbRole;
use oulipoly_state::deployment::metadata::store::rows::{
    DeploymentId, DeploymentPhase, DeploymentRow, DeploymentSnapshot, HookOutboxRow,
    ImportWatermarkRow, PrimaryPointer, QueueStateRow, RetentionStateRow,
};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use uuid::Uuid;

pub const DEPLOYMENT_UUID: Uuid = Uuid::from_u128(0x06200000000000000000000000000062);
pub const HOOK_UUID: Uuid = Uuid::from_u128(0x06200000000000000000000000000162);
pub const FIXTURE_TIME: &str = "2026-05-10T12:00:00Z";

pub struct Age62DeploymentFixture {
    _tempdir: TempDir,
    pub data_root: PathBuf,
}

impl Age62DeploymentFixture {
    pub fn new(schema_version: u32, role: DbRole) -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let data_root = tempdir.path().to_path_buf();
        create_versioned_state_db(&data_root.join("state.db.v5"), 5);
        create_versioned_state_db(&data_root.join("state.db.v6"), 6);
        seed_coordinator(&data_root, Some((schema_version, role)));
        Self {
            _tempdir: tempdir,
            data_root,
        }
    }

    pub fn without_primary_pointer() -> Self {
        let tempdir = tempfile::tempdir().unwrap();
        let data_root = tempdir.path().to_path_buf();
        create_versioned_state_db(&data_root.join("state.db.v5"), 5);
        seed_coordinator(&data_root, None);
        Self {
            _tempdir: tempdir,
            data_root,
        }
    }

    pub fn coordinator_path(&self) -> PathBuf {
        self.data_root.join("state-deploy.db")
    }

    pub fn versioned_path(&self, schema_version: u32) -> PathBuf {
        self.data_root.join(format!("state.db.v{schema_version}"))
    }
}

pub fn pre_cutover_fixture() -> Age62DeploymentFixture {
    Age62DeploymentFixture::new(5, DbRole::PreCutoverPrimary)
}

pub fn post_cutover_fixture() -> Age62DeploymentFixture {
    Age62DeploymentFixture::new(6, DbRole::PostCutoverPrimary)
}

pub fn deployment_snapshot(schema_version: u32, role: DbRole) -> DeploymentSnapshot {
    let deployment_id = DeploymentId(DEPLOYMENT_UUID);
    DeploymentSnapshot {
        primary: PrimaryPointer {
            schema_version,
            deployment_id: Some(deployment_id),
            role,
            updated_at: FIXTURE_TIME.parse().unwrap(),
        },
        active_deployment: Some(DeploymentRow {
            deployment_id,
            from_schema_version: 5,
            to_schema_version: 6,
            phase: DeploymentPhase::Importing,
            started_at: FIXTURE_TIME.parse().unwrap(),
            updated_at: FIXTURE_TIME.parse().unwrap(),
            notes: Some("age-62 fixture".to_string()),
        }),
        queue_states: vec![QueueStateRow {
            deployment_id,
            direction: "ForwardToB".to_string(),
            activation_state: "active".to_string(),
            last_sequence: 12,
            last_acked_sequence: 7,
            updated_at: FIXTURE_TIME.parse().unwrap(),
        }],
        retention: Some(RetentionStateRow {
            deployment_id,
            retention_started_at: Some(FIXTURE_TIME.parse().unwrap()),
            retention_completed_at: None,
            reverse_dual_write_active: true,
            updated_at: FIXTURE_TIME.parse().unwrap(),
        }),
    }
}

pub fn missing_primary_snapshot() -> DeploymentSnapshot {
    DeploymentSnapshot {
        primary: PrimaryPointer {
            schema_version: 0,
            deployment_id: None,
            role: DbRole::Steady,
            updated_at: FIXTURE_TIME.parse().unwrap(),
        },
        active_deployment: None,
        queue_states: Vec::new(),
        retention: None,
    }
}

pub fn coordinator_tables_sql() -> &'static str {
    "
    PRAGMA user_version = 1;

    CREATE TABLE IF NOT EXISTS deployments (
        deployment_id BLOB PRIMARY KEY NOT NULL,
        from_schema_version INTEGER NOT NULL,
        to_schema_version INTEGER NOT NULL,
        phase TEXT NOT NULL,
        started_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        notes TEXT
    );

    CREATE TABLE IF NOT EXISTS primary_pointer (
        pointer_id INTEGER PRIMARY KEY CHECK (pointer_id = 1),
        schema_version INTEGER NOT NULL,
        deployment_id BLOB,
        role TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS queue_state (
        deployment_id BLOB PRIMARY KEY NOT NULL,
        direction TEXT NOT NULL,
        activation_state TEXT NOT NULL,
        last_sequence INTEGER NOT NULL DEFAULT 0,
        last_acked_sequence INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS import_watermarks (
        deployment_id BLOB NOT NULL,
        table_name TEXT NOT NULL,
        last_pk_json TEXT,
        last_seen_row_version INTEGER NOT NULL DEFAULT 0,
        completed_pass INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (deployment_id, table_name)
    );

    CREATE TABLE IF NOT EXISTS retention_state (
        deployment_id BLOB PRIMARY KEY NOT NULL,
        retention_started_at TEXT,
        retention_completed_at TEXT,
        reverse_dual_write_active INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS hook_outbox (
        hook_id BLOB PRIMARY KEY NOT NULL,
        deployment_id BLOB,
        hook_kind TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        enqueued_at TEXT NOT NULL,
        delivered_at TEXT
    );

    CREATE INDEX IF NOT EXISTS idx_hook_outbox_undelivered
        ON hook_outbox(delivered_at) WHERE delivered_at IS NULL;
    "
}

pub fn seed_coordinator(data_root: &Path, primary: Option<(u32, DbRole)>) {
    fs::create_dir_all(data_root).unwrap();
    let conn = Connection::open(data_root.join("state-deploy.db")).unwrap();
    conn.execute_batch(coordinator_tables_sql()).unwrap();
    conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
    seed_all_row_families(&conn, primary);
}

pub fn create_versioned_state_db(path: &Path, user_version: u32) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let conn = Connection::open(path).unwrap();
    conn.pragma_update(None, "user_version", user_version)
        .unwrap();
}

pub fn data_root_content_snapshot(data_root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    collect_files(data_root, data_root, &mut snapshot);
    snapshot
}

pub fn representative_rows() -> (
    DeploymentRow,
    PrimaryPointer,
    QueueStateRow,
    ImportWatermarkRow,
    RetentionStateRow,
    HookOutboxRow,
) {
    let deployment_id = DeploymentId(DEPLOYMENT_UUID);
    let now = FIXTURE_TIME.parse().unwrap();
    (
        DeploymentRow {
            deployment_id,
            from_schema_version: 5,
            to_schema_version: 6,
            phase: DeploymentPhase::Importing,
            started_at: now,
            updated_at: now,
            notes: Some("age-62 fixture".to_string()),
        },
        PrimaryPointer {
            schema_version: 6,
            deployment_id: Some(deployment_id),
            role: DbRole::PostCutoverPrimary,
            updated_at: now,
        },
        QueueStateRow {
            deployment_id,
            direction: "ForwardToB".to_string(),
            activation_state: "active".to_string(),
            last_sequence: 12,
            last_acked_sequence: 7,
            updated_at: now,
        },
        ImportWatermarkRow {
            deployment_id,
            table_name: "invocations".to_string(),
            last_pk_json: Some(r#"{"id":1}"#.to_string()),
            last_seen_row_version: 99,
            completed_pass: true,
            updated_at: now,
        },
        RetentionStateRow {
            deployment_id,
            retention_started_at: Some(now),
            retention_completed_at: None,
            reverse_dual_write_active: true,
            updated_at: now,
        },
        HookOutboxRow {
            hook_id: HOOK_UUID,
            deployment_id: Some(deployment_id),
            hook_kind: "event_log".to_string(),
            payload_json: r#"{"event":"fixture"}"#.to_string(),
            enqueued_at: now,
            delivered_at: None,
        },
    )
}

fn seed_all_row_families(conn: &Connection, primary: Option<(u32, DbRole)>) {
    let (deployment, mut pointer, queue, watermark, retention, hook) = representative_rows();
    if let Some((schema_version, role)) = primary {
        pointer.schema_version = schema_version;
        pointer.role = role;
    }

    insert_deployment(conn, &deployment);
    if primary.is_some() {
        insert_primary_pointer(conn, &pointer);
    }
    insert_queue_state(conn, &queue);
    insert_import_watermark(conn, &watermark);
    insert_retention_state(conn, &retention);
    insert_hook_outbox(conn, &hook);
}

fn insert_deployment(conn: &Connection, deployment: &DeploymentRow) {
    conn.execute(
        "INSERT INTO deployments
            (deployment_id, from_schema_version, to_schema_version, phase, started_at, updated_at, notes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params_from_iter(deployment_values(deployment)),
    )
    .unwrap();
}

fn insert_primary_pointer(conn: &Connection, pointer: &PrimaryPointer) {
    conn.execute(
        "INSERT INTO primary_pointer
                (pointer_id, schema_version, deployment_id, role, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4)",
        params_from_iter(primary_pointer_values(pointer)),
    )
    .unwrap();
}

fn insert_queue_state(conn: &Connection, queue: &QueueStateRow) {
    conn.execute(
        "INSERT INTO queue_state
            (deployment_id, direction, activation_state, last_sequence, last_acked_sequence, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params_from_iter(queue_state_values(queue)),
    )
    .unwrap();
}

fn insert_import_watermark(conn: &Connection, watermark: &ImportWatermarkRow) {
    conn.execute(
        "INSERT INTO import_watermarks
            (deployment_id, table_name, last_pk_json, last_seen_row_version, completed_pass, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params_from_iter(import_watermark_values(watermark)),
    )
    .unwrap();
}

fn insert_retention_state(conn: &Connection, retention: &RetentionStateRow) {
    conn.execute(
        "INSERT INTO retention_state
            (deployment_id, retention_started_at, retention_completed_at, reverse_dual_write_active, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params_from_iter(retention_state_values(retention)),
    )
    .unwrap();
}

fn insert_hook_outbox(conn: &Connection, hook: &HookOutboxRow) {
    conn.execute(
        "INSERT INTO hook_outbox
            (hook_id, deployment_id, hook_kind, payload_json, enqueued_at, delivered_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params_from_iter(hook_outbox_values(hook)),
    )
    .unwrap();
}

fn deployment_values(deployment: &DeploymentRow) -> Vec<Value> {
    vec![
        blob_value(deployment.deployment_id.0.as_bytes()),
        integer_value(deployment.from_schema_version),
        integer_value(deployment.to_schema_version),
        text_value(format!("{:?}", deployment.phase)),
        text_value(deployment.started_at.to_rfc3339()),
        text_value(deployment.updated_at.to_rfc3339()),
        optional_text_value(deployment.notes.clone()),
    ]
}

fn primary_pointer_values(pointer: &PrimaryPointer) -> Vec<Value> {
    vec![
        integer_value(pointer.schema_version),
        blob_value(pointer.deployment_id.unwrap().0.as_bytes()),
        text_value(format!("{:?}", pointer.role)),
        text_value(pointer.updated_at.to_rfc3339()),
    ]
}

fn queue_state_values(queue: &QueueStateRow) -> Vec<Value> {
    vec![
        blob_value(queue.deployment_id.0.as_bytes()),
        text_value(queue.direction.clone()),
        text_value(queue.activation_state.clone()),
        Value::Integer(queue.last_sequence as i64),
        Value::Integer(queue.last_acked_sequence as i64),
        text_value(queue.updated_at.to_rfc3339()),
    ]
}

fn import_watermark_values(watermark: &ImportWatermarkRow) -> Vec<Value> {
    vec![
        blob_value(watermark.deployment_id.0.as_bytes()),
        text_value(watermark.table_name.clone()),
        optional_text_value(watermark.last_pk_json.clone()),
        Value::Integer(watermark.last_seen_row_version as i64),
        Value::Integer(i64::from(watermark.completed_pass)),
        text_value(watermark.updated_at.to_rfc3339()),
    ]
}

fn retention_state_values(retention: &RetentionStateRow) -> Vec<Value> {
    vec![
        blob_value(retention.deployment_id.0.as_bytes()),
        optional_text_value(
            retention
                .retention_started_at
                .map(|value| value.to_rfc3339()),
        ),
        optional_text_value(
            retention
                .retention_completed_at
                .map(|value| value.to_rfc3339()),
        ),
        Value::Integer(i64::from(retention.reverse_dual_write_active)),
        text_value(retention.updated_at.to_rfc3339()),
    ]
}

fn hook_outbox_values(hook: &HookOutboxRow) -> Vec<Value> {
    vec![
        blob_value(hook.hook_id.as_bytes()),
        blob_value(hook.deployment_id.unwrap().0.as_bytes()),
        text_value(hook.hook_kind.clone()),
        text_value(hook.payload_json.clone()),
        text_value(hook.enqueued_at.to_rfc3339()),
        optional_text_value(hook.delivered_at.map(|value| value.to_rfc3339())),
    ]
}

fn blob_value(bytes: &[u8]) -> Value {
    Value::Blob(bytes.to_vec())
}

fn integer_value(value: u32) -> Value {
    Value::Integer(i64::from(value))
}

fn text_value(value: String) -> Value {
    Value::Text(value)
}

fn optional_text_value(value: Option<String>) -> Value {
    value.map(Value::Text).unwrap_or(Value::Null)
}

fn collect_files(root: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for path in sorted_child_paths(current) {
        collect_path(root, &path, snapshot);
    }
}

fn sorted_child_paths(current: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(current)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn collect_path(root: &Path, path: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
    if is_directory(path) {
        collect_files(root, path, snapshot);
    } else {
        snapshot.insert(relative_path(root, path), file_bytes(path));
    }
}

fn is_directory(path: &Path) -> bool {
    path.is_dir()
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap().to_path_buf()
}

fn file_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}
