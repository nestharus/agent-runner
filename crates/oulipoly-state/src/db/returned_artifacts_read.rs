//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - parser
//! - predicate
//! - validator
//!
//! Role set: { accessor, formatter, mapper, parser, predicate, validator }
//!
//! Returned-artifact list query and row decoding.

use super::*;
use chrono::{DateTime, Utc};
use oulipoly_agent_messenger::ReturnedArtifactRef;

impl StateDb {
    pub fn list_returned_artifacts(
        &self,
        invocation_row_id: i64,
    ) -> Result<Vec<ReturnedArtifactRef>, DbError> {
        if !Self::returned_artifacts_schema_is_readable(&self.conn)? {
            return Ok(Vec::new());
        }
        let rows = Self::load_returned_artifact_rows(&self.conn, invocation_row_id)?;
        Self::parse_returned_artifact_rows(rows)
    }

    pub(super) fn returned_artifacts_schema_is_readable(
        conn: &sqlite::Connection,
    ) -> Result<bool, DbError> {
        Self::validate_returned_artifacts_object_type(conn)?;
        Self::returned_artifacts_have_version_id(conn)
    }

    pub(super) fn validate_returned_artifacts_object_type(
        conn: &sqlite::Connection,
    ) -> Result<(), DbError> {
        match Self::returned_artifacts_object_type(conn)?.as_deref() {
            None | Some("table") => Ok(()),
            Some(other) => Err(Self::unexpected_returned_artifacts_object_error(other)),
        }
    }

    pub(super) fn returned_artifacts_object_type(
        conn: &sqlite::Connection,
    ) -> Result<Option<String>, DbError> {
        conn.query_row(
            "SELECT type
             FROM sqlite_master
             WHERE name = 'invocation_returned_artifacts'",
            [],
            Self::map_returned_artifacts_object_type_row,
        )
        .optional()
        .map_err(Self::format_returned_artifacts_schema_inspect_error)
    }

    fn map_returned_artifacts_object_type_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn format_returned_artifacts_schema_inspect_error(err: sqlite::Error) -> DbError {
        format!("Failed to inspect returned-artifacts schema: {err}")
    }

    pub(super) fn unexpected_returned_artifacts_object_error(object_type: &str) -> DbError {
        format!("Unexpected returned-artifacts schema shape: object type={object_type}")
    }

    pub(super) fn returned_artifacts_have_version_id(
        conn: &sqlite::Connection,
    ) -> Result<bool, DbError> {
        let columns = Self::returned_artifact_columns(conn)?;
        Ok(Self::has_column(&columns, "version_id"))
    }

    pub(super) fn load_returned_artifact_rows(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<Vec<ReturnedArtifactRawRow>, DbError> {
        let mut stmt = conn
            .prepare(
                "SELECT
                    version_id,
                    name,
                    workflow_run_id,
                    artifact_name,
                    version,
                    sha256,
                    content_len,
                    format_hint,
                    verdict_line,
                    source_json,
                    returned_at
                 FROM invocation_returned_artifacts
                 WHERE invocation_id = ?1
                 ORDER BY ordinal ASC",
            )
            .map_err(Self::format_returned_artifacts_query_prepare_error)?;
        let rows = stmt
            .query_map(
                sqlite::params![invocation_row_id],
                Self::map_returned_artifact_raw_row,
            )
            .map_err(Self::format_returned_artifacts_query_error)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_returned_artifact_row_read_error)
    }

    fn format_returned_artifacts_query_prepare_error(err: sqlite::Error) -> DbError {
        format!("Failed to prepare returned-artifacts query: {err}")
    }

    fn format_returned_artifacts_query_error(err: sqlite::Error) -> DbError {
        format!("Failed to query returned artifacts: {err}")
    }

    fn format_returned_artifact_row_read_error(err: sqlite::Error) -> DbError {
        format!("Failed to read returned artifact row: {err}")
    }

    pub(super) fn map_returned_artifact_raw_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<ReturnedArtifactRawRow> {
        Ok(ReturnedArtifactRawRow {
            version_id: row.get(0)?,
            name: row.get(1)?,
            workflow_run_id: row.get(2)?,
            artifact_name: row.get(3)?,
            version: row.get(4)?,
            sha256: row.get(5)?,
            content_len: row.get(6)?,
            format_hint: row.get(7)?,
            verdict_line: row.get(8)?,
            source_json: row.get(9)?,
            returned_at_text: row.get(10)?,
        })
    }

    pub(super) fn parse_returned_artifact_rows(
        rows: Vec<ReturnedArtifactRawRow>,
    ) -> Result<Vec<ReturnedArtifactRef>, DbError> {
        rows.into_iter()
            .map(Self::parse_returned_artifact_row)
            .collect()
    }

    pub(super) fn parse_returned_artifact_row(
        row: ReturnedArtifactRawRow,
    ) -> Result<ReturnedArtifactRef, DbError> {
        let parsed = Self::parse_returned_artifact_field_values(&row)
            .map_err(Self::format_returned_artifact_parse_error)?;
        let validated = Self::validate_returned_artifact_field_values(parsed)
            .map_err(Self::format_returned_artifact_parse_error)?;
        Ok(Self::map_parsed_returned_artifact_to_ref(row, validated))
    }

    pub(super) fn parse_returned_artifact_field_values(
        row: &ReturnedArtifactRawRow,
    ) -> Result<ParsedReturnedArtifactFieldValues, ReturnedArtifactFieldError> {
        let source = Self::parse_returned_artifact_source(&row.source_json)?;
        let returned_at = Self::parse_returned_artifact_returned_at(&row.returned_at_text)?;
        let producer_invocation_uuid =
            Self::parse_returned_artifact_producer_uuid(&row.workflow_run_id)?;
        Ok(Self::map_returned_artifact_field_values(
            row,
            source,
            returned_at,
            producer_invocation_uuid,
        ))
    }

    fn parse_returned_artifact_source(
        raw: &str,
    ) -> Result<oulipoly_agent_messenger::ReturnedArtifactSource, ReturnedArtifactFieldError> {
        serde_json::from_str(raw).map_err(ReturnedArtifactFieldError::SourceJson)
    }

    fn parse_returned_artifact_returned_at(
        raw: &str,
    ) -> Result<DateTime<Utc>, ReturnedArtifactFieldError> {
        DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|err| Self::returned_artifact_returned_at_error(raw, err))
    }

    fn returned_artifact_returned_at_error(
        raw: &str,
        err: chrono::ParseError,
    ) -> ReturnedArtifactFieldError {
        ReturnedArtifactFieldError::ReturnedAt {
            raw: raw.to_string(),
            err,
        }
    }

    fn parse_returned_artifact_producer_uuid(
        workflow_run_id: &str,
    ) -> Result<uuid::Uuid, ReturnedArtifactFieldError> {
        returned_artifact_producer_uuid(workflow_run_id)
            .map_err(ReturnedArtifactFieldError::ProducerUuid)
    }

    fn map_returned_artifact_field_values(
        row: &ReturnedArtifactRawRow,
        source: oulipoly_agent_messenger::ReturnedArtifactSource,
        returned_at: DateTime<Utc>,
        producer_invocation_uuid: uuid::Uuid,
    ) -> ParsedReturnedArtifactFieldValues {
        ParsedReturnedArtifactFieldValues {
            source,
            returned_at,
            producer_invocation_uuid,
            version: row.version,
            content_len: row.content_len,
        }
    }

    pub(super) fn validate_returned_artifact_field_values(
        parsed: ParsedReturnedArtifactFieldValues,
    ) -> Result<ValidatedReturnedArtifactFieldValues, ReturnedArtifactFieldError> {
        let version =
            Self::validate_returned_artifact_nonnegative_integer(parsed.version, "version")?;
        let content_len = Self::validate_returned_artifact_nonnegative_integer(
            parsed.content_len,
            "content_len",
        )?;
        Ok(Self::map_validated_returned_artifact_field_values(
            parsed,
            version,
            content_len,
        ))
    }

    fn map_validated_returned_artifact_field_values(
        parsed: ParsedReturnedArtifactFieldValues,
        version: u64,
        content_len: u64,
    ) -> ValidatedReturnedArtifactFieldValues {
        ValidatedReturnedArtifactFieldValues {
            source: parsed.source,
            returned_at: parsed.returned_at,
            producer_invocation_uuid: parsed.producer_invocation_uuid,
            version,
            content_len,
        }
    }

    pub(super) fn validate_returned_artifact_nonnegative_integer(
        value: i64,
        field: &'static str,
    ) -> Result<u64, ReturnedArtifactFieldError> {
        u64::try_from(value).map_err(|_| ReturnedArtifactFieldError::NegativeInteger { field })
    }

    pub(super) fn map_parsed_returned_artifact_to_ref(
        row: ReturnedArtifactRawRow,
        parsed: ValidatedReturnedArtifactFieldValues,
    ) -> ReturnedArtifactRef {
        ReturnedArtifactRef {
            version_id: row.version_id,
            name: row.name,
            store_address: oulipoly_agent_messenger::StoreAddress {
                workflow_run_id: row.workflow_run_id,
                artifact_name: row.artifact_name,
                version: parsed.version,
            },
            sha256: row.sha256,
            content_len: parsed.content_len,
            format_hint: row.format_hint,
            verdict_line: row.verdict_line,
            source: parsed.source,
            producer_invocation_uuid: parsed.producer_invocation_uuid,
            returned_at: parsed.returned_at,
        }
    }

    pub(super) fn format_returned_artifact_parse_error(err: ReturnedArtifactFieldError) -> DbError {
        match err {
            ReturnedArtifactFieldError::SourceJson(err) => {
                format!("Failed to parse returned artifact source JSON: {err}")
            }
            ReturnedArtifactFieldError::ReturnedAt { raw, err } => {
                format!("Bad returned artifact returned_at {raw}: {err}")
            }
            ReturnedArtifactFieldError::ProducerUuid(err) => {
                format!("Failed to parse returned artifact producer UUID: {err}")
            }
            ReturnedArtifactFieldError::NegativeInteger { field } => {
                format!("negative returned artifact {field}")
            }
        }
    }

    pub(super) fn returned_artifact_columns(
        conn: &sqlite::Connection,
    ) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "invocation_returned_artifacts",
            "Failed to inspect returned-artifacts schema",
            "Failed to query returned-artifacts columns",
            "Failed to read returned-artifacts column",
        )
    }
}
