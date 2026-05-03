#![allow(dead_code)]

use agent_runner_lib::state::{InvocationRepository, InvocationStart, StateDb};
use chrono::{Duration, Utc};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

pub const MODEL_NAME: &str = "fixture-model";
pub const CLAUDE_PROVIDER: &str = "claude";
pub const CLAUDE_ALT_PROVIDER: &str = "claude2";
pub const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";

pub struct StateRepoFixture {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
}

impl StateRepoFixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("data").join("state.db");
        Self { _dir: dir, db_path }
    }

    pub fn root(&self) -> &Path {
        self._dir.path()
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path).unwrap()
    }

    pub fn conn(&self) -> Connection {
        let _ = self.open_db();
        Connection::open(&self.db_path).unwrap()
    }

    pub fn start_invocation(
        &self,
        repo: &dyn InvocationRepository,
        uuid: &str,
        provider_name: &str,
        provider_index: usize,
    ) -> i64 {
        repo.start_invocation(&InvocationStart {
            invocation_uuid: uuid.to_string(),
            model_name: MODEL_NAME.to_string(),
            provider_name: provider_name.to_string(),
            provider_index,
            parent_invocation_id: None,
        })
        .unwrap()
    }

    pub fn record_failed_invocation(
        &self,
        repo: &dyn InvocationRepository,
        uuid: &str,
        provider_name: &str,
        provider_index: usize,
    ) {
        let id = self.start_invocation(repo, uuid, provider_name, provider_index);
        repo.finalize_invocation(id, false, 42, Some("fixture_error"), Some("stderr"))
            .unwrap();
    }

    pub fn seed_quota_window(&self, provider: &str) {
        let conn = self.conn();
        let refreshed_at = Utc::now().to_rfc3339();
        let resets_at = (Utc::now() + Duration::hours(24)).to_rfc3339();
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at, exhausted_at)
             VALUES (?1, 0.75, ?2, 0, ?3, ?3)",
            params![provider, resets_at, refreshed_at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at, last_delta_percent, last_delta_calls)
             VALUES (?1, 0, 0.75, ?2, 0.01, 22)",
            params![provider, resets_at],
        )
        .unwrap();
    }

    pub fn quota_window_count(&self, provider: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM provider_quota_windows WHERE provider_name = ?1",
                params![provider],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn calls_since_refresh(&self, provider: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT calls_since_refresh FROM provider_quotas WHERE provider_name = ?1",
                params![provider],
                |row| row.get(0),
            )
            .unwrap()
    }

    pub fn exhausted_at_is_set(&self, provider: &str) -> bool {
        self.conn()
            .query_row(
                "SELECT exhausted_at IS NOT NULL FROM provider_quotas WHERE provider_name = ?1",
                params![provider],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            == 1
    }

    pub fn seed_active_chain(&self, chain_id: &str, provider: &str, session_id: &str) -> i64 {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![chain_id, MODEL_NAME],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![chain_id, provider, session_id],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    pub fn missing_read_only_path(&self) -> PathBuf {
        self.root().join("missing-parent").join("state.db")
    }
}

pub fn ensure_parent_missing(path: &Path) {
    if let Some(parent) = path.parent() {
        assert!(
            !parent.exists(),
            "fixture expected absent parent: {parent:?}"
        );
    }
    assert!(!path.exists(), "fixture expected absent path: {path:?}");
}
