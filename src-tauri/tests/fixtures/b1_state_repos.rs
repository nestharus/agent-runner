#![allow(dead_code)]

use agent_runner_lib::state::{
    AccountRecord, AuthMethod, AuthStatus, CliProviderRecord, DiscoveredModel, InvocationStart,
    ModelParameter, QuotaWindowInput, StateDb,
};
use chrono::{Duration, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
pub const CHILD_UUID: &str = "22222222-2222-4222-8222-222222222222";
pub const MODEL: &str = "fixture-model";
pub const OTHER_MODEL: &str = "other-model";
pub const PROVIDER: &str = "fixture-provider";
pub const OTHER_PROVIDER: &str = "other-provider";
pub const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
pub const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";

pub struct StateRepoFixture {
    dir: tempfile::TempDir,
    db_path: PathBuf,
}

impl StateRepoFixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("state.db");
        Self { dir, db_path }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn missing_db_path(&self) -> PathBuf {
        self.dir.path().join("missing").join("state.db")
    }

    pub fn corrupt_db_path(&self) -> PathBuf {
        let path = self.dir.path().join("corrupt.db");
        fs::write(&path, "not sqlite").unwrap();
        path
    }

    pub fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path).unwrap()
    }

    pub fn conn(&self) -> Connection {
        let _ = self.open_db();
        Connection::open(&self.db_path).unwrap()
    }

    pub fn count(&self, table: &str) -> i64 {
        self.conn()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    pub fn one_string(&self, sql: &str) -> String {
        self.conn().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    pub fn one_i64(&self, sql: &str) -> i64 {
        self.conn().query_row(sql, [], |row| row.get(0)).unwrap()
    }

    pub fn optional_string(&self, sql: &str) -> Option<String> {
        self.conn()
            .query_row(sql, [], |row| row.get(0))
            .optional()
            .unwrap()
    }

    pub fn seed_quota_row(
        &self,
        provider: &str,
        used_percent: f64,
        calls_since_refresh: i64,
        exhausted: bool,
    ) {
        let exhausted_at = exhausted.then_some("2026-05-02T09:00:00Z");
        self.conn()
            .execute(
                "INSERT INTO provider_quotas
                    (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at, exhausted_at)
                 VALUES (?1, ?2, '2026-05-03T09:00:00Z', ?3, '2026-05-02T09:00:00Z', ?4)",
                params![provider, used_percent, calls_since_refresh, exhausted_at],
            )
            .unwrap();
    }

    pub fn seed_quota_window(&self, provider: &str, window_id: i64, used_percent: f64) {
        self.conn()
            .execute(
                "INSERT INTO provider_quota_windows
                    (provider_name, window_id, used_percent, resets_at, last_delta_percent, last_delta_calls)
                 VALUES (?1, ?2, ?3, '2026-05-03T09:00:00Z', 0.05, 3)",
                params![provider, window_id, used_percent],
            )
            .unwrap();
    }

    pub fn seed_active_chain(
        &self,
        chain_id: &str,
        provider: &str,
        session_id: &str,
        model: &str,
        started_at: &str,
    ) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            params![chain_id, started_at, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'initial')",
            params![chain_id, provider, session_id, started_at],
        )
        .unwrap();
    }

    pub fn seed_turn(&self, provider: &str, session_id: &str, turn_id: &str, timestamp: &str) {
        self.conn()
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_compaction_boundary)
                 VALUES (?1, ?2, ?3, ?4, 'assistant', '', ?4, 0)",
                params![provider, session_id, turn_id, timestamp],
            )
            .unwrap();
    }

    pub fn seed_discovered_model_row(&self, canonical: &str, provider: &str, cli_version: &str) {
        self.conn()
            .execute(
                "INSERT INTO discovered_models
                    (canonical_name, provider, discovered_at, cli_version)
                 VALUES (?1, ?2, '2026-05-02T09:00:00Z', ?3)",
                params![canonical, provider, cli_version],
            )
            .unwrap();
    }
}

pub fn root_invocation() -> InvocationStart {
    InvocationStart {
        invocation_uuid: ROOT_UUID.to_string(),
        model_name: MODEL.to_string(),
        provider_name: PROVIDER.to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

pub fn child_invocation(parent_id: i64) -> InvocationStart {
    InvocationStart {
        invocation_uuid: CHILD_UUID.to_string(),
        model_name: MODEL.to_string(),
        provider_name: PROVIDER.to_string(),
        provider_index: 0,
        parent_invocation_id: Some(parent_id),
    }
}

pub fn quota_window_input(used_percent: f64) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent,
        resets_at: Utc.with_ymd_and_hms(2026, 5, 3, 9, 0, 0).unwrap(),
    }
}

pub fn cli_provider(cli_name: &str, version: &str) -> CliProviderRecord {
    CliProviderRecord {
        cli_name: cli_name.to_string(),
        display_name: format!("{cli_name} display"),
        installed: true,
        version: Some(version.to_string()),
        config_dir: Some(format!("/tmp/{cli_name}")),
        last_synced: Some("2026-05-02T09:00:00Z".to_string()),
    }
}

pub fn account(id: &str, provider: &str) -> AccountRecord {
    AccountRecord {
        id: id.to_string(),
        provider: provider.to_string(),
        profile_name: format!("{id}-profile"),
        auth_method: AuthMethod::OAuth,
        auth_status: AuthStatus::Unknown,
        created_at: "2026-05-02T09:00:00Z".to_string(),
    }
}

pub fn discovered_model(canonical: &str, provider: &str, cli_version: &str) -> DiscoveredModel {
    serde_json::from_value(json!({
        "canonical_name": canonical,
        "provider": provider,
        "parameters": [],
        "discovered_at": "2026-05-02T09:00:00Z",
        "cli_version": cli_version
    }))
    .unwrap()
}

pub fn model_parameter(name: &str) -> ModelParameter {
    serde_json::from_value(json!({
        "name": name,
        "param_type": "string",
        "description": "fixture parameter",
        "cli_mapping": {
            "flag": format!("--{name}"),
            "value_template": "{value}"
        }
    }))
    .unwrap()
}

pub fn recent_time() -> chrono::DateTime<Utc> {
    Utc::now() - Duration::minutes(5)
}

pub fn isolated_xdg_data_home<F: FnOnce()>(body: F) {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let previous = std::env::var_os("XDG_DATA_HOME");
    unsafe {
        std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
    }
    let result = catch_unwind(AssertUnwindSafe(body));
    unsafe {
        match previous {
            Some(value) => std::env::set_var("XDG_DATA_HOME", value),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
