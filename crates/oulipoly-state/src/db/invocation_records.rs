//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - parser
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, parser, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_records.rs
//!     role: intrinsic-surface
//!     Domain: invocation-records-persistence
//!     Owns:
//!       - the StateDb invocation-records persistence surface this concern extends, split
//!         from the StateDb facade with the public API preserved
//!       - intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: StateDb, sqlite, chrono DateTime/Utc, and the InvocationRecord/Status DTOs this concern maps
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: DateTime, StateDb, Utc, sqlite
//! ```

use super::{StateDb, sqlite};
use chrono::{DateTime, Utc};
use oulipoly_core::CancellationToken;

const INVOCATION_QUERY_PROGRESS_OPS: i32 = 100;
// Inspect a finite window beyond the rendered node budget so terminal ancestors of
// active descendants are preferred without returning to a global running-row seed.
const RUNNING_DESCENDANT_CANDIDATE_FACTOR: usize = 4;
const RUNNING_DESCENDANT_SCAN_FACTOR: usize = 8;
const RUNNING_DESCENDANT_CANDIDATES_SQL: &str = "SELECT id
     FROM invocations INDEXED BY idx_invocations_parent_running_created
     WHERE parent_invocation_id = ?1
     ORDER BY (status = 'running') DESC, created_at, id
     LIMIT ?2";
const RUNNING_DESCENDANT_EXISTS_SQL: &str = "WITH RECURSIVE descendants(id, status) AS (
         SELECT id, status
         FROM invocations
         WHERE id = ?1
         UNION
         SELECT child.id, child.status
         FROM invocations AS child INDEXED BY idx_invocations_parent
         JOIN descendants AS parent ON child.parent_invocation_id = parent.id
         WHERE parent.status != 'running'
         LIMIT ?2
     )
     SELECT EXISTS(SELECT 1 FROM descendants WHERE status = 'running')";

#[cfg(test)]
std::thread_local! {
    static INVOCATION_ROW_MAPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(any(test, feature = "test-support"))]
struct InvocationQueryProgressPauseState {
    entered: std::sync::Mutex<bool>,
    release: std::sync::atomic::AtomicBool,
    wake: std::sync::Condvar,
}

#[cfg(any(test, feature = "test-support"))]
impl InvocationQueryProgressPauseState {
    fn new() -> Self {
        Self {
            entered: std::sync::Mutex::new(false),
            release: std::sync::atomic::AtomicBool::new(false),
            wake: std::sync::Condvar::new(),
        }
    }

    fn wait_for_release_or_cancellation(&self, cancellation: &CancellationToken) {
        let mut entered = self
            .entered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *entered = true;
        self.wake.notify_all();
        while !self.release.load(std::sync::atomic::Ordering::SeqCst)
            && !cancellation.is_cancelled()
        {
            entered = self
                .wake
                .wait_timeout(entered, std::time::Duration::from_millis(5))
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
type InvocationQueryProgressPauseRegistry = std::sync::Mutex<
    Option<(
        std::path::PathBuf,
        std::sync::Arc<InvocationQueryProgressPauseState>,
    )>,
>;

#[cfg(any(test, feature = "test-support"))]
fn invocation_query_progress_pause_registry() -> &'static InvocationQueryProgressPauseRegistry {
    static REGISTRY: std::sync::OnceLock<InvocationQueryProgressPauseRegistry> =
        std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(any(test, feature = "test-support"))]
fn invocation_query_progress_pause(
    path: &std::path::Path,
) -> Option<std::sync::Arc<InvocationQueryProgressPauseState>> {
    invocation_query_progress_pause_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|(configured_path, _)| configured_path == path)
        .map(|(_, pause)| std::sync::Arc::clone(pause))
}

#[cfg(any(test, feature = "test-support"))]
pub struct InvocationQueryProgressPause {
    path: std::path::PathBuf,
    state: std::sync::Arc<InvocationQueryProgressPauseState>,
}

#[cfg(any(test, feature = "test-support"))]
impl InvocationQueryProgressPause {
    pub fn wait_until_entered(&self, timeout: std::time::Duration) -> bool {
        let entered = self
            .state
            .entered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (entered, _) = self
            .state
            .wake
            .wait_timeout_while(entered, timeout, |entered| !*entered)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *entered
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for InvocationQueryProgressPause {
    fn drop(&mut self) {
        self.state
            .release
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.state.wake.notify_all();
        let mut registry = invocation_query_progress_pause_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if registry.as_ref().is_some_and(|(path, state)| {
            path == &self.path && std::sync::Arc::ptr_eq(state, &self.state)
        }) {
            *registry = None;
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InvocationRecord {
    pub id: i64,
    pub invocation_uuid: String,
    pub model_name: String,
    pub provider_name: Option<String>,
    pub provider_index: usize,
    pub parent_invocation_id: Option<i64>,
    pub status: InvocationStatus,
    pub success: Option<bool>,
    pub exit_code: Option<i32>,
    pub error_category: Option<String>,
    pub terminal_reason: Option<String>,
    pub session_id: Option<String>,
    pub session_capture_method: Option<String>,
    pub provider_session_id: Option<String>,
    pub resume_input_id: Option<String>,
    pub provider_session_capture_method: Option<String>,
    pub provider_session_resolved_account: Option<String>,
    pub resume_acceptance_status: Option<String>,
    pub resume_acceptance_evidence: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct InvocationStart {
    pub invocation_uuid: String,
    pub model_name: String,
    pub provider_name: String,
    pub provider_index: usize,
    pub parent_invocation_id: Option<i64>,
}

struct InvocationRecordRawFields {
    id: i64,
    invocation_uuid: String,
    model_name: String,
    provider_name: Option<String>,
    provider_index: i64,
    parent_invocation_id: Option<i64>,
    status_raw: String,
    success: Option<i64>,
    exit_code: Option<i32>,
    error_category: Option<String>,
    terminal_reason: Option<String>,
    session_id: Option<String>,
    session_capture_method: Option<String>,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
    provider_session_capture_method: Option<String>,
    provider_session_resolved_account: Option<String>,
    resume_acceptance_status: Option<String>,
    resume_acceptance_evidence: Option<String>,
    created_at_raw: String,
    finished_at_raw: Option<String>,
}

struct InvocationRecordParsedFields {
    status: InvocationStatus,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationStatus {
    Running,
    Succeeded,
    Failed,
    Legacy,
}

impl InvocationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            InvocationStatus::Running => "running",
            InvocationStatus::Succeeded => "succeeded",
            InvocationStatus::Failed => "failed",
            InvocationStatus::Legacy => "legacy",
        }
    }

    /// Inherent `from_str` returning `Option<Self>` per the PR-A contract
    /// (`tmp/01-pr-a-contract.md` §"Struct contract"). The `FromStr` trait
    /// impl below provides the `Result`-returning idiomatic Rust surface;
    /// this inherent method is the contracted API caller-facing surface.
    /// Clippy's `should_implement_trait` lint flags the name collision —
    /// allowed here because both surfaces are intentional and the contract
    /// pins this specific shape.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        s.parse().ok()
    }
}

impl std::str::FromStr for InvocationStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(InvocationStatus::Running),
            "succeeded" => Ok(InvocationStatus::Succeeded),
            "failed" => Ok(InvocationStatus::Failed),
            "legacy" => Ok(InvocationStatus::Legacy),
            _ => Err(format_unknown_invocation_status(s)),
        }
    }
}

fn format_unknown_invocation_status(raw: &str) -> String {
    format!("Unknown invocation status: {raw}")
}

impl StateDb {
    #[cfg(any(test, feature = "test-support"))]
    pub fn pause_invocation_query_progress_for_test(&self) -> InvocationQueryProgressPause {
        let state = std::sync::Arc::new(InvocationQueryProgressPauseState::new());
        let mut registry = invocation_query_progress_pause_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            registry.is_none(),
            "only one invocation query progress pause may be active"
        );
        *registry = Some((self.db_path.clone(), std::sync::Arc::clone(&state)));
        InvocationQueryProgressPause {
            path: self.db_path.clone(),
            state,
        }
    }

    #[cfg(test)]
    pub(super) fn reset_invocation_row_map_count() {
        INVOCATION_ROW_MAPS.with(|maps| maps.set(0));
    }

    #[cfg(test)]
    pub(super) fn invocation_row_map_count() -> usize {
        INVOCATION_ROW_MAPS.with(std::cell::Cell::get)
    }

    pub fn record_legacy_resume_input_session_id(
        &self,
        id: i64,
        resume_input_id: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE invocations
                 SET session_id = ?1
                 WHERE id = ?2 AND session_capture_method = 'resumed'",
                sqlite::params![resume_input_id, id],
            )
            .map_err(|err| Self::format_legacy_resume_session_update_error(id, err))?;
        Ok(())
    }

    fn format_legacy_resume_session_update_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to update legacy resume session_id for invocation {id}: {err}")
    }

    pub fn update_resume_acceptance(
        &self,
        id: i64,
        status: &str,
        evidence: Option<&str>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE invocations
                 SET resume_acceptance_status = ?1,
                     resume_acceptance_evidence = ?2
                 WHERE id = ?3",
                sqlite::params![status, evidence, id],
            )
            .map_err(|err| Self::format_resume_acceptance_update_error(id, err))?;
        Ok(())
    }

    pub(super) fn format_resume_acceptance_update_error(id: i64, err: sqlite::Error) -> String {
        format!("Failed to update resume acceptance for invocation {id}: {err}")
    }

    pub fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String> {
        let sql = Self::invocation_record_select_sql(&self.conn, "WHERE invocation_uuid = ?1")?;
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(Self::format_invocation_lookup_prepare_error)?;

        let result = stmt.query_row(sqlite::params![uuid], Self::map_invocation_row);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(Self::format_invocation_lookup_query_error(err)),
        }
    }

    fn format_invocation_lookup_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare invocation lookup: {err}")
    }

    fn format_invocation_lookup_query_error(err: sqlite::Error) -> String {
        format!("Failed to query invocation: {err}")
    }

    pub fn list_invocation_children(
        &self,
        parent_id: i64,
    ) -> Result<Vec<InvocationRecord>, String> {
        let sql = Self::invocation_record_select_sql(
            &self.conn,
            "WHERE parent_invocation_id = ?1
             ORDER BY created_at, id",
        )?;
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(Self::format_invocation_child_lookup_prepare_error)?;

        let rows = stmt
            .query_map(sqlite::params![parent_id], Self::map_invocation_row)
            .map_err(Self::format_invocation_children_query_error)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_invocation_children_map_error)
    }

    pub fn list_invocation_children_bounded(
        &self,
        parent_id: i64,
        limit: usize,
        prioritize_running: bool,
    ) -> Result<Vec<InvocationRecord>, String> {
        self.list_invocation_children_bounded_inner(parent_id, limit, prioritize_running)
    }

    pub fn list_invocation_children_bounded_with_cancel(
        &self,
        parent_id: i64,
        limit: usize,
        prioritize_running: bool,
        cancellation: &CancellationToken,
    ) -> Result<Vec<InvocationRecord>, String> {
        self.with_invocation_query_cancellation(cancellation, || {
            self.list_invocation_children_bounded_inner(parent_id, limit, prioritize_running)
        })
    }

    fn list_invocation_children_bounded_inner(
        &self,
        parent_id: i64,
        limit: usize,
        prioritize_running: bool,
    ) -> Result<Vec<InvocationRecord>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let clause = if prioritize_running {
            "WHERE parent_invocation_id = ?1
             ORDER BY (status = 'running') DESC, created_at, id
             LIMIT ?2"
        } else {
            "WHERE parent_invocation_id = ?1
             ORDER BY created_at, id
             LIMIT ?2"
        };
        let sql = Self::invocation_record_select_sql(&self.conn, clause)?;
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(Self::format_invocation_child_lookup_prepare_error)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt
            .query_map(sqlite::params![parent_id, limit], Self::map_invocation_row)
            .map_err(Self::format_invocation_children_query_error)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_invocation_children_map_error)
    }

    pub fn list_invocation_children_with_running_descendants_bounded(
        &self,
        parent_id: i64,
        limit: usize,
    ) -> Result<Vec<InvocationRecord>, String> {
        self.list_invocation_children_with_running_descendants_bounded_inner(
            parent_id,
            limit,
            &CancellationToken::new(),
        )
    }

    pub fn list_invocation_children_with_running_descendants_bounded_with_cancel(
        &self,
        parent_id: i64,
        limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<InvocationRecord>, String> {
        self.with_invocation_query_cancellation(cancellation, || {
            self.list_invocation_children_with_running_descendants_bounded_inner(
                parent_id,
                limit,
                cancellation,
            )
        })
    }

    fn list_invocation_children_with_running_descendants_bounded_inner(
        &self,
        parent_id: i64,
        limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<InvocationRecord>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let live_child_ids = self.list_live_subtree_child_ids(parent_id, limit, cancellation)?;
        let mut children = Vec::with_capacity(limit);
        for id in &live_child_ids {
            if let Some(record) = self.get_invocation_by_id(*id)? {
                children.push(record);
            }
        }
        if children.len() >= limit {
            return Ok(children);
        }

        let candidates = self.list_invocation_children_excluding_ids_bounded(
            parent_id,
            limit - children.len(),
            &live_child_ids,
        )?;
        children.extend(candidates);
        Ok(children)
    }

    fn list_invocation_children_excluding_ids_bounded(
        &self,
        parent_id: i64,
        limit: usize,
        excluded_ids: &[i64],
    ) -> Result<Vec<InvocationRecord>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        if excluded_ids.is_empty() {
            return self.list_invocation_children_bounded_inner(parent_id, limit, true);
        }
        let excluded_ids = excluded_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let clause = format!(
            "WHERE parent_invocation_id = ?1
               AND id NOT IN ({excluded_ids})
             ORDER BY (status = 'running') DESC, created_at, id
             LIMIT ?2"
        );
        let sql = Self::invocation_record_select_sql(&self.conn, &clause)?;
        let mut statement = self
            .conn
            .prepare(&sql)
            .map_err(Self::format_invocation_child_lookup_prepare_error)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement
            .query_map(sqlite::params![parent_id, limit], Self::map_invocation_row)
            .map_err(Self::format_invocation_children_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_invocation_children_map_error)
    }

    fn list_live_subtree_child_ids(
        &self,
        parent_id: i64,
        limit: usize,
        cancellation: &CancellationToken,
    ) -> Result<Vec<i64>, String> {
        let candidate_limit =
            Self::scaled_invocation_query_limit(limit, RUNNING_DESCENDANT_CANDIDATE_FACTOR);
        let mut statement = self
            .conn
            .prepare(RUNNING_DESCENDANT_CANDIDATES_SQL)
            .map_err(Self::format_invocation_child_lookup_prepare_error)?;
        let candidate_ids = statement
            .query_map(sqlite::params![parent_id, candidate_limit], |row| {
                row.get(0)
            })
            .map_err(Self::format_invocation_children_query_error)?;
        let candidate_ids = candidate_ids
            .collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_invocation_children_map_error)?;
        drop(statement);

        let descendant_limit =
            Self::scaled_invocation_query_limit(limit, RUNNING_DESCENDANT_SCAN_FACTOR);
        let mut statement = self
            .conn
            .prepare(RUNNING_DESCENDANT_EXISTS_SQL)
            .map_err(Self::format_invocation_child_lookup_prepare_error)?;
        let mut live_child_ids = Vec::with_capacity(limit);
        for candidate_id in candidate_ids {
            if cancellation.is_cancelled() {
                return Err("Invocation child lookup cancelled".to_string());
            }
            let has_running_descendant = statement
                .query_row(sqlite::params![candidate_id, descendant_limit], |row| {
                    row.get::<_, bool>(0)
                })
                .map_err(Self::format_invocation_children_query_error)?;
            if has_running_descendant {
                live_child_ids.push(candidate_id);
                if live_child_ids.len() == limit {
                    break;
                }
            }
        }
        Ok(live_child_ids)
    }

    fn scaled_invocation_query_limit(limit: usize, factor: usize) -> i64 {
        i64::try_from(limit.saturating_mul(factor)).unwrap_or(i64::MAX)
    }

    fn with_invocation_query_cancellation<T>(
        &self,
        cancellation: &CancellationToken,
        query: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if cancellation.is_cancelled() {
            return Err("Invocation child lookup cancelled".to_string());
        }
        let handler_cancellation = cancellation.clone();
        let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handler_interrupted = std::sync::Arc::clone(&interrupted);
        #[cfg(any(test, feature = "test-support"))]
        let progress_pause = invocation_query_progress_pause(&self.db_path);
        self.conn
            .progress_handler(
                INVOCATION_QUERY_PROGRESS_OPS,
                Some(move || {
                    #[cfg(any(test, feature = "test-support"))]
                    if let Some(pause) = progress_pause.as_ref() {
                        pause.wait_for_release_or_cancellation(&handler_cancellation);
                    }
                    let cancelled = handler_cancellation.is_cancelled();
                    if cancelled {
                        handler_interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    cancelled
                }),
            )
            .map_err(|error| format!("Failed to install invocation query cancellation: {error}"))?;
        let result = query();
        let reset = self.conn.progress_handler(0, None::<fn() -> bool>);
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Invocation child lookup cancelled".to_string());
        }
        reset.map_err(|error| format!("Failed to clear invocation query cancellation: {error}"))?;
        result
    }

    #[cfg(test)]
    pub(super) fn running_descendant_candidates_sql() -> &'static str {
        RUNNING_DESCENDANT_CANDIDATES_SQL
    }

    #[cfg(test)]
    pub(super) fn running_descendant_exists_sql() -> &'static str {
        RUNNING_DESCENDANT_EXISTS_SQL
    }

    fn get_invocation_by_id(&self, id: i64) -> Result<Option<InvocationRecord>, String> {
        let sql = Self::invocation_record_select_sql(&self.conn, "WHERE id = ?1")?;
        let mut statement = self
            .conn
            .prepare(&sql)
            .map_err(Self::format_invocation_lookup_prepare_error)?;
        match statement.query_row(sqlite::params![id], Self::map_invocation_row) {
            Ok(record) => Ok(Some(record)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(Self::format_invocation_lookup_query_error(error)),
        }
    }

    fn format_invocation_child_lookup_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare invocation child lookup: {err}")
    }

    fn format_invocation_children_query_error(err: sqlite::Error) -> String {
        format!("Failed to query invocation children: {err}")
    }

    fn format_invocation_children_map_error(err: sqlite::Error) -> String {
        format!("Failed to map invocation children: {err}")
    }

    fn map_invocation_row(row: &sqlite::Row<'_>) -> sqlite::Result<InvocationRecord> {
        #[cfg(test)]
        INVOCATION_ROW_MAPS.with(|maps| maps.set(maps.get() + 1));
        let raw = Self::read_invocation_record_raw_fields(row)?;
        let parsed = Self::parse_invocation_record_raw_fields(&raw)?;
        Ok(Self::map_invocation_record(raw, parsed))
    }

    fn read_invocation_record_raw_fields(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<InvocationRecordRawFields> {
        Ok(InvocationRecordRawFields {
            id: row.get(0)?,
            invocation_uuid: row.get(1)?,
            model_name: row.get(2)?,
            provider_name: row.get(3)?,
            provider_index: row.get(4)?,
            parent_invocation_id: row.get(5)?,
            status_raw: row.get(6)?,
            success: row.get(7)?,
            exit_code: row.get(8)?,
            error_category: row.get(9)?,
            terminal_reason: row.get(10)?,
            session_id: row.get(11)?,
            session_capture_method: row.get(12)?,
            provider_session_id: row.get(13)?,
            resume_input_id: row.get(14)?,
            provider_session_capture_method: row.get(15)?,
            provider_session_resolved_account: row.get(16)?,
            resume_acceptance_status: row.get(17)?,
            resume_acceptance_evidence: row.get(18)?,
            created_at_raw: row.get(19)?,
            finished_at_raw: row.get(20)?,
        })
    }

    fn parse_invocation_record_raw_fields(
        raw: &InvocationRecordRawFields,
    ) -> sqlite::Result<InvocationRecordParsedFields> {
        Ok(InvocationRecordParsedFields {
            status: Self::parse_invocation_status_at(&raw.status_raw, 6)?,
            created_at: Self::strict_rfc3339_at(&raw.created_at_raw, 18)?,
            finished_at: Self::optional_strict_rfc3339_at(raw.finished_at_raw.clone(), 19)?,
        })
    }

    fn map_invocation_record(
        raw: InvocationRecordRawFields,
        parsed: InvocationRecordParsedFields,
    ) -> InvocationRecord {
        InvocationRecord {
            id: raw.id,
            invocation_uuid: raw.invocation_uuid,
            model_name: raw.model_name,
            provider_name: raw.provider_name,
            provider_index: raw.provider_index as usize,
            parent_invocation_id: raw.parent_invocation_id,
            status: parsed.status,
            success: raw.success.map(|value| value != 0),
            exit_code: raw.exit_code,
            error_category: raw.error_category,
            terminal_reason: raw.terminal_reason,
            session_id: raw.session_id,
            session_capture_method: raw.session_capture_method,
            provider_session_id: raw.provider_session_id,
            resume_input_id: raw.resume_input_id,
            provider_session_capture_method: raw.provider_session_capture_method,
            provider_session_resolved_account: raw.provider_session_resolved_account,
            resume_acceptance_status: raw.resume_acceptance_status,
            resume_acceptance_evidence: raw.resume_acceptance_evidence,
            created_at: parsed.created_at,
            finished_at: parsed.finished_at,
        }
    }

    fn optional_strict_rfc3339_at(
        raw: Option<String>,
        column_index: usize,
    ) -> sqlite::Result<Option<DateTime<Utc>>> {
        raw.map(|s| Self::strict_rfc3339_at(&s, column_index))
            .transpose()
    }

    fn parse_invocation_status_at(
        raw: &str,
        column_index: usize,
    ) -> sqlite::Result<InvocationStatus> {
        raw.parse::<InvocationStatus>()
            .map_err(|_| Self::invocation_status_conversion_failure(raw, column_index))
    }

    fn invocation_status_conversion_failure(raw: &str, column_index: usize) -> sqlite::Error {
        sqlite::Error::FromSqlConversionFailure(
            column_index,
            sqlite::Type::Text,
            format_unknown_invocation_status(raw).into(),
        )
    }
}
