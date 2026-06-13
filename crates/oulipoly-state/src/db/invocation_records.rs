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
//! ```

use super::{StateDb, sqlite};
use chrono::{DateTime, Utc};

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

    fn format_resume_acceptance_update_error(id: i64, err: sqlite::Error) -> String {
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
