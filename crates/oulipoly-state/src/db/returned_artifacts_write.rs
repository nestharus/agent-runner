//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - orchestration
//! - parser
//! - validator
//! - mapper
//!
//! Role set: { accessor, formatter, orchestration, parser, validator, mapper }
//!
//! Returned-artifact replacement and row insertion.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/db/returned_artifacts_write.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly_agent_messenger::ReturnedArtifactRef returned-artifact-ref contract
//!       - ReturnedArtifactRef.store_address workflow_run_id/artifact_name/version field contract
//!       - StateDb invocation_returned_artifacts persistence row and SQLite mutation contract
//!       - Returned-artifact UUID identity, source JSON encoding, and returned_at timestamp persistence contract
//! ```

use super::*;
use oulipoly_agent_messenger::ReturnedArtifactRef;
use uuid::Uuid;

impl StateDb {
    pub fn record_returned_artifacts(
        &self,
        invocation_row_id: i64,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        Self::prepare_returned_artifacts_table(&self.conn)?;
        let identity =
            Self::load_invocation_identity_for_returned_artifacts(&self.conn, invocation_row_id)?;
        Self::validate_returned_artifact_refs(&identity, refs)?;
        Self::replace_returned_artifact_rows(&self.conn, invocation_row_id, refs)
    }

    pub(super) fn prepare_returned_artifacts_table(
        conn: &sqlite::Connection,
    ) -> Result<(), DbError> {
        conn.execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(Self::format_returned_artifacts_schema_ensure_error)
    }

    fn format_returned_artifacts_schema_ensure_error(err: sqlite::Error) -> DbError {
        format!("Failed to ensure returned-artifacts schema: {err}")
    }

    pub(super) fn load_invocation_identity_for_returned_artifacts(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<InvocationIdentity, DbError> {
        let uuid_text = Self::load_invocation_uuid_text(conn, invocation_row_id)?;
        let uuid =
            Self::parse_invocation_uuid_for_returned_artifacts(invocation_row_id, &uuid_text)?;
        Ok(InvocationIdentity {
            row_id: invocation_row_id,
            uuid,
        })
    }

    pub(super) fn load_invocation_uuid_text(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<String, DbError> {
        let uuid_text = Self::query_invocation_uuid_text(conn, invocation_row_id)
            .map_err(Self::format_returned_artifact_invocation_load_error)?;
        Self::require_invocation_uuid_text(invocation_row_id, uuid_text)
    }

    fn query_invocation_uuid_text(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> sqlite::Result<Option<String>> {
        conn.query_row(
            "SELECT invocation_uuid FROM invocations WHERE id = ?1",
            sqlite::params![invocation_row_id],
            Self::map_invocation_uuid_text_row,
        )
        .optional()
    }

    fn map_invocation_uuid_text_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn require_invocation_uuid_text(
        invocation_row_id: i64,
        uuid_text: Option<String>,
    ) -> Result<String, DbError> {
        uuid_text.ok_or_else(|| {
            Self::format_returned_artifact_invocation_not_found_error(invocation_row_id)
        })
    }

    fn format_returned_artifact_invocation_load_error(err: sqlite::Error) -> DbError {
        format!("Failed to load invocation for returned artifacts: {err}")
    }

    fn format_returned_artifact_invocation_not_found_error(invocation_row_id: i64) -> DbError {
        format!("Invocation {invocation_row_id} not found")
    }

    pub(super) fn parse_invocation_uuid_for_returned_artifacts(
        invocation_row_id: i64,
        uuid_text: &str,
    ) -> Result<Uuid, DbError> {
        Uuid::parse_str(uuid_text)
            .map_err(|e| Self::format_invalid_invocation_uuid(invocation_row_id, e))
    }

    fn format_invalid_invocation_uuid(invocation_row_id: i64, err: uuid::Error) -> DbError {
        format!("Invalid invocation UUID on row {invocation_row_id}: {err}")
    }

    pub(super) fn validate_returned_artifact_refs(
        identity: &InvocationIdentity,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        for reference in refs {
            Self::validate_returned_artifact_ref(identity.row_id, identity.uuid, reference)?;
        }
        Ok(())
    }

    pub(super) fn replace_returned_artifact_rows(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        let tx = conn
            .unchecked_transaction()
            .map_err(Self::format_begin_returned_artifacts_tx_error)?;
        tx.execute(
            "DELETE FROM invocation_returned_artifacts WHERE invocation_id = ?1",
            sqlite::params![invocation_row_id],
        )
        .map_err(Self::format_reset_returned_artifacts_error)?;
        for (ordinal, reference) in refs.iter().enumerate() {
            Self::insert_returned_artifact_row(&tx, invocation_row_id, ordinal, reference)?;
        }
        tx.commit()
            .map_err(Self::format_commit_returned_artifacts_tx_error)
    }

    fn format_begin_returned_artifacts_tx_error(err: sqlite::Error) -> DbError {
        format!("Failed to begin returned-artifacts tx: {err}")
    }

    fn format_reset_returned_artifacts_error(err: sqlite::Error) -> DbError {
        format!("Failed to reset returned artifacts: {err}")
    }

    fn format_commit_returned_artifacts_tx_error(err: sqlite::Error) -> DbError {
        format!("Failed to commit returned-artifacts tx: {err}")
    }

    pub(super) fn validate_returned_artifact_ref(
        invocation_row_id: i64,
        invocation_uuid: Uuid,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        let derived_uuid = Self::parse_returned_artifact_workflow_run_id(
            &reference.store_address.workflow_run_id,
        )?;
        Self::validate_returned_artifact_producer_uuid(reference, derived_uuid)?;
        Self::validate_returned_artifact_owner(invocation_row_id, invocation_uuid, reference)?;
        Self::validate_returned_artifact_version_id(reference, derived_uuid)
    }

    fn parse_returned_artifact_workflow_run_id(workflow_run_id: &str) -> Result<Uuid, DbError> {
        returned_artifact_producer_uuid(workflow_run_id)
            .map_err(Self::format_invalid_returned_artifact_workflow_run_id)
    }

    fn format_invalid_returned_artifact_workflow_run_id(err: sqlite::Error) -> DbError {
        format!("Invalid returned-artifact workflow_run_id: {err}")
    }

    pub(super) fn validate_returned_artifact_producer_uuid(
        reference: &ReturnedArtifactRef,
        derived_uuid: Uuid,
    ) -> Result<(), DbError> {
        if derived_uuid == reference.producer_invocation_uuid {
            Ok(())
        } else {
            Err(Self::format_returned_artifact_producer_uuid_mismatch(
                derived_uuid,
                reference.producer_invocation_uuid,
            ))
        }
    }

    fn format_returned_artifact_producer_uuid_mismatch(
        derived_uuid: Uuid,
        producer_invocation_uuid: Uuid,
    ) -> DbError {
        format!(
            "Returned artifact producer UUID mismatch: workflow_run_id encodes {derived_uuid}, ref carries {producer_invocation_uuid}"
        )
    }

    pub(super) fn validate_returned_artifact_owner(
        invocation_row_id: i64,
        invocation_uuid: Uuid,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        if reference.producer_invocation_uuid == invocation_uuid {
            Ok(())
        } else {
            Err(Self::format_returned_artifact_owner_mismatch(
                invocation_row_id,
                invocation_uuid,
                reference.producer_invocation_uuid,
            ))
        }
    }

    fn format_returned_artifact_owner_mismatch(
        invocation_row_id: i64,
        invocation_uuid: Uuid,
        producer_invocation_uuid: Uuid,
    ) -> DbError {
        format!(
            "Returned artifact belongs to {producer_invocation_uuid}, but invocation row {invocation_row_id} is {invocation_uuid}"
        )
    }

    pub(super) fn validate_returned_artifact_version_id(
        reference: &ReturnedArtifactRef,
        derived_uuid: Uuid,
    ) -> Result<(), DbError> {
        let expected_version_id = returned_artifact_version_id(
            derived_uuid,
            &reference.store_address.artifact_name,
            reference.store_address.version,
        );
        if reference.version_id == expected_version_id {
            Ok(())
        } else {
            Err(Self::format_returned_artifact_version_id_mismatch(
                &expected_version_id,
                &reference.version_id,
            ))
        }
    }

    fn format_returned_artifact_version_id_mismatch(
        expected_version_id: &str,
        actual_version_id: &str,
    ) -> DbError {
        format!(
            "Returned artifact version_id mismatch: expected {expected_version_id}, ref carries {actual_version_id}"
        )
    }

    pub(super) fn insert_returned_artifact_row(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        ordinal: usize,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        let validated = Self::validate_returned_artifact_inputs(reference)?;
        let payload = Self::format_returned_artifact_payload_fields(reference)?;
        let params = Self::bind_returned_artifact_row_params(
            invocation_row_id,
            ordinal,
            reference,
            &validated,
            &payload,
        );
        Self::execute_returned_artifact_row_insert(conn, &params)
    }

    pub(super) fn validate_returned_artifact_inputs(
        reference: &ReturnedArtifactRef,
    ) -> Result<ReturnedArtifactValidatedInputs, DbError> {
        Ok(ReturnedArtifactValidatedInputs {
            version: returned_artifact_sql_integer(reference.store_address.version, "version")?,
            content_len: returned_artifact_sql_integer(reference.content_len, "content_len")?,
        })
    }

    pub(super) fn format_returned_artifact_payload_fields(
        reference: &ReturnedArtifactRef,
    ) -> Result<ReturnedArtifactPayloadFields, DbError> {
        Ok(ReturnedArtifactPayloadFields {
            source_json: Self::encode_returned_artifact_source(&reference.source)?,
            returned_at: Self::format_returned_at(reference.returned_at),
        })
    }

    fn encode_returned_artifact_source(
        source: &oulipoly_agent_messenger::ReturnedArtifactSource,
    ) -> Result<String, DbError> {
        serde_json::to_string(source)
            .map_err(|e| format!("Failed to encode returned-artifact source: {e}"))
    }

    fn format_returned_at(returned_at: DateTime<Utc>) -> String {
        returned_at.to_rfc3339()
    }

    pub(super) fn bind_returned_artifact_row_params<'a>(
        invocation_row_id: i64,
        ordinal: usize,
        reference: &'a ReturnedArtifactRef,
        validated: &'a ReturnedArtifactValidatedInputs,
        payload: &'a ReturnedArtifactPayloadFields,
    ) -> ReturnedArtifactRowParams<'a> {
        ReturnedArtifactRowParams {
            invocation_row_id,
            ordinal: ordinal as i64,
            version_id: &reference.version_id,
            name: &reference.name,
            workflow_run_id: &reference.store_address.workflow_run_id,
            artifact_name: &reference.store_address.artifact_name,
            version: validated.version,
            sha256: &reference.sha256,
            content_len: validated.content_len,
            format_hint: &reference.format_hint,
            verdict_line: &reference.verdict_line,
            source_kind: returned_source_kind(&reference.source),
            source_json: &payload.source_json,
            returned_at: &payload.returned_at,
        }
    }

    pub(super) fn execute_returned_artifact_row_insert(
        conn: &sqlite::Connection,
        params: &ReturnedArtifactRowParams<'_>,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO invocation_returned_artifacts (
                invocation_id,
                ordinal,
                version_id,
                name,
                workflow_run_id,
                artifact_name,
                version,
                sha256,
                content_len,
                format_hint,
                verdict_line,
                source_kind,
                source_json,
                returned_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            sqlite::params![
                params.invocation_row_id,
                params.ordinal,
                params.version_id,
                params.name,
                params.workflow_run_id,
                params.artifact_name,
                params.version,
                params.sha256,
                params.content_len,
                params.format_hint,
                params.verdict_line,
                params.source_kind,
                params.source_json,
                params.returned_at,
            ],
        )
        .map_err(Self::format_returned_artifact_record_error)?;
        Ok(())
    }

    fn format_returned_artifact_record_error(err: sqlite::Error) -> DbError {
        format!("Failed to record returned artifact: {err}")
    }
}
