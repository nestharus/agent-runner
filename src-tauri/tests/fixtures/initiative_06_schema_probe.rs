#![cfg(unix)]
#![allow(dead_code)]

use oulipoly_state::StateDb;
use oulipoly_state::schema;
use oulipoly_state::schema_probe::{BinaryInfo, FeatureMap, SchemaProbeReport, StateDbReport};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const CURRENT_SCHEMA_VERSION: u32 = schema::CURRENT_SCHEMA_VERSION as u32;
pub const MINIMUM_SUPPORTED_SCHEMA_VERSION: u32 = schema::MINIMUM_SUPPORTED_SCHEMA_VERSION as u32;

pub struct SchemaProbeFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbPhysicalSnapshot {
    pub parent_exists: bool,
    pub db_exists: bool,
    pub db_len: Option<u64>,
    pub db_modified: Option<std::time::SystemTime>,
    pub wal_exists: bool,
    pub shm_exists: bool,
}

impl SchemaProbeFixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            config_home: dir.path().join("config"),
            data_home: dir.path().join("data"),
            dir,
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn data_home(&self) -> &Path {
        &self.data_home
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    pub fn missing_parent_db_path(&self) -> PathBuf {
        self.root().join("missing-parent").join("state.db")
    }

    pub fn run_schema_probe(&self, extra_args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("session").arg("schema-probe");
        cmd.args(extra_args);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }

    pub fn create_current_schema_db(&self) -> PathBuf {
        create_current_schema_db_at(&self.db_path())
    }

    pub fn create_incompatible_user_version_db(&self) -> PathBuf {
        let path = self.db_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        drop(conn);
        path
    }

    pub fn create_future_user_version_db(&self) -> PathBuf {
        let path = self.create_current_schema_db();
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", schema::CURRENT_SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        path
    }

    pub fn create_wrong_index_definition_db(&self) -> PathBuf {
        let path = self.create_current_schema_db();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "
            DROP INDEX idx_session_turns_session_lookup;
            CREATE INDEX idx_session_turns_session_lookup
                ON session_turns(provider_name, session_id, timestamp);
            ",
        )
        .unwrap();
        drop(conn);
        path
    }

    pub fn create_invalid_database_file(&self) -> PathBuf {
        let path = self.db_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"this is not sqlite").unwrap();
        path
    }

    pub fn create_directory_at_db_path(&self) -> PathBuf {
        let path = self.db_path();
        fs::create_dir_all(&path).unwrap();
        path
    }

    pub fn create_unreadable_db(&self) -> PathBuf {
        let path = self.create_current_schema_db();
        chmod(&path, 0o000);
        path
    }

    pub fn restore_db_permissions(&self) {
        let path = self.db_path();
        if path.exists() {
            chmod(&path, 0o600);
        }
        for sidecar in [wal_path(&path), shm_path(&path)] {
            if sidecar.exists() {
                chmod(&sidecar, 0o600);
            }
        }
    }

    pub fn create_wal_sidecar_permission_error(&self) -> PathBuf {
        let path = self.db_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE t (id INTEGER PRIMARY KEY, value TEXT);
            INSERT INTO t (value) VALUES ('kept in wal');
            ",
        )
        .unwrap();
        let wal = wal_path(&path);
        let shm = shm_path(&path);
        assert!(wal.exists(), "fixture must leave a WAL sidecar at {wal:?}");
        assert!(shm.exists(), "fixture must leave a SHM sidecar at {shm:?}");
        chmod(&wal, 0o000);
        chmod(&shm, 0o000);
        std::mem::forget(conn);
        path
    }

    pub fn physical_snapshot(&self) -> DbPhysicalSnapshot {
        physical_snapshot_for(&self.db_path())
    }

    pub fn physical_snapshot_for(&self, path: &Path) -> DbPhysicalSnapshot {
        physical_snapshot_for(path)
    }
}

impl Drop for SchemaProbeFixture {
    fn drop(&mut self) {
        self.restore_db_permissions();
    }
}

pub fn missing_db_fixture() -> SchemaProbeFixture {
    SchemaProbeFixture::new()
}

pub fn current_schema_db_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_current_schema_db();
    fixture
}

pub fn incompatible_schema_db_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_incompatible_user_version_db();
    fixture
}

pub fn future_schema_db_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_future_user_version_db();
    fixture
}

pub fn wrong_index_definition_db_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_wrong_index_definition_db();
    fixture
}

pub fn unreadable_db_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_unreadable_db();
    fixture
}

pub fn invalid_database_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_invalid_database_file();
    fixture
}

pub fn directory_database_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_directory_at_db_path();
    fixture
}

pub fn wal_sidecar_error_fixture() -> SchemaProbeFixture {
    let fixture = SchemaProbeFixture::new();
    fixture.create_wal_sidecar_permission_error();
    fixture
}

pub fn create_current_schema_db_at(path: &Path) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    StateDb::open(path).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode = DELETE;").unwrap();
    drop(conn);
    path.to_path_buf()
}

pub fn parse_stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

pub fn parse_stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap()
}

pub fn assert_json_error(output: &Output, code: &str) -> Value {
    assert!(output.stdout.is_empty(), "{output:?}");
    let json = parse_stderr_json(output);
    assert_eq!(json["error"]["code"], code, "{json}");
    json
}

pub fn required_tables() -> [&'static str; 4] {
    [
        "invocations",
        "session_turns",
        "session_chains",
        "session_chain_segments",
    ]
}

pub fn required_columns() -> BTreeMap<String, Vec<&'static str>> {
    BTreeMap::from([
        (
            "invocations".to_string(),
            vec![
                "session_id",
                "session_capture_method",
                "resume_acceptance_status",
                "resume_acceptance_evidence",
            ],
        ),
        (
            "session_turns".to_string(),
            vec!["parent_turn_id", "is_sidechain", "is_compaction_boundary"],
        ),
        (
            "session_chains".to_string(),
            vec!["chain_id", "created_at", "last_used_at", "model_name"],
        ),
        (
            "session_chain_segments".to_string(),
            vec![
                "chain_id",
                "provider_name",
                "session_id",
                "started_at",
                "ended_at",
                "last_turn_id",
                "transition_reason",
            ],
        ),
    ])
}

pub fn required_indexes() -> BTreeMap<String, Vec<&'static str>> {
    BTreeMap::from([
        (
            "invocations".to_string(),
            vec!["idx_invocations_provider_session"],
        ),
        (
            "session_turns".to_string(),
            vec!["idx_session_turns_session_lookup"],
        ),
        (
            "session_chain_segments".to_string(),
            vec!["idx_segments_session", "idx_segments_chain_active"],
        ),
    ])
}

pub fn assert_structural_maps(json: &Value, expected: bool) {
    for table in required_tables() {
        assert_eq!(json["state_db"]["tables"][table], expected, "{json}");
    }
    for (table, columns) in required_columns() {
        for column in columns {
            assert_eq!(
                json["state_db"]["required_columns"][table.as_str()][column],
                expected,
                "{json}"
            );
        }
    }
    for (table, indexes) in required_indexes() {
        for index in indexes {
            assert_eq!(
                json["state_db"]["required_indexes"][table.as_str()][index],
                expected,
                "{json}"
            );
        }
    }
}

pub fn assert_no_dotted_compatibility_keys(json: &Value) {
    let state_db = json["state_db"].as_object().unwrap();
    for map_name in ["tables", "required_columns", "required_indexes"] {
        for key in state_db[map_name].as_object().unwrap().keys() {
            assert!(
                !key.contains('.'),
                "{map_name} used dotted compatibility key {key:?}: {json}"
            );
        }
    }
    for map_name in ["required_columns", "required_indexes"] {
        for nested in state_db[map_name].as_object().unwrap().values() {
            for key in nested.as_object().unwrap().keys() {
                assert!(
                    !key.contains('.'),
                    "{map_name} used dotted nested key {key:?}: {json}"
                );
            }
        }
    }
}

pub fn report_for_predicate(
    exists: bool,
    compatible: bool,
    session_import_replace: bool,
    session_pause_handshake: bool,
    supported_storage_types: Vec<String>,
) -> SchemaProbeReport {
    let features = features_with(session_import_replace, session_pause_handshake);
    let safe_for_import_replace = exists
        && compatible
        && session_import_replace
        && session_pause_handshake
        && supported_storage_types == self::supported_storage_types();
    SchemaProbeReport {
        binary: BinaryInfo {
            name: "oulipoly-agent-runner".to_string(),
            version: "0.1.0".to_string(),
            commit: "unknown".to_string(),
        },
        state_db: StateDbReport {
            path: PathBuf::from("/tmp/state.db"),
            exists,
            schema_version: CURRENT_SCHEMA_VERSION,
            user_version: CURRENT_SCHEMA_VERSION,
            current_schema_version: CURRENT_SCHEMA_VERSION,
            minimum_supported_schema_version: MINIMUM_SUPPORTED_SCHEMA_VERSION,
            compatible,
            migratable: false,
            tables: bool_table_map(true),
            required_columns: bool_nested_map(required_columns(), true),
            required_indexes: bool_nested_map(required_indexes(), true),
        },
        features,
        supported_storage_types,
        safe_for_import_replace,
    }
}

pub fn report_for_json_shape() -> SchemaProbeReport {
    report_for_predicate(true, true, true, true, supported_storage_types())
}

pub fn supported_storage_types() -> Vec<String> {
    ["claude_code", "codex_session", "other"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn features_with(session_import_replace: bool, session_pause_handshake: bool) -> FeatureMap {
    BTreeMap::from([
        ("session_locate".to_string(), false),
        ("session_export".to_string(), false),
        ("session_import_replace".to_string(), session_import_replace),
        (
            "session_pause_handshake".to_string(),
            session_pause_handshake,
        ),
        ("session_schema_probe".to_string(), true),
    ])
}

fn bool_table_map(value: bool) -> BTreeMap<String, bool> {
    required_tables()
        .into_iter()
        .map(|table| (table.to_string(), value))
        .collect()
}

fn bool_nested_map(
    source: BTreeMap<String, Vec<&'static str>>,
    value: bool,
) -> BTreeMap<String, BTreeMap<String, bool>> {
    source
        .into_iter()
        .map(|(table, keys)| {
            (
                table,
                keys.into_iter()
                    .map(|key| (key.to_string(), value))
                    .collect(),
            )
        })
        .collect()
}

fn physical_snapshot_for(path: &Path) -> DbPhysicalSnapshot {
    let parent = path.parent().unwrap();
    let metadata = fs::metadata(path).ok();
    DbPhysicalSnapshot {
        parent_exists: parent.exists(),
        db_exists: path.exists(),
        db_len: metadata.as_ref().map(|m| m.len()),
        db_modified: metadata.and_then(|m| m.modified().ok()),
        wal_exists: wal_path(path).exists(),
        shm_exists: shm_path(path).exists(),
    }
}

fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

fn chmod(path: &Path, mode: u32) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).unwrap();
}
