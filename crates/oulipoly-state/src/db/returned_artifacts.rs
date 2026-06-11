use super::{DbError, StateDb, sqlite};
use crate::db::sqlite_adapter::RusqliteOptionalExtension;
use chrono::{DateTime, Utc};
use oulipoly_agent_messenger::ReturnedArtifactRef;
use uuid::Uuid;

struct InvocationIdentity {
    row_id: i64,
    uuid: Uuid,
}

struct ReturnedArtifactRawRow {
    version_id: String,
    name: String,
    workflow_run_id: String,
    artifact_name: String,
    version: i64,
    sha256: String,
    content_len: i64,
    format_hint: Option<String>,
    verdict_line: Option<String>,
    source_json: String,
    returned_at_text: String,
}

struct ReturnedArtifactValidatedInputs {
    version: i64,
    content_len: i64,
}

struct ReturnedArtifactPayloadFields {
    source_json: String,
    returned_at: String,
}

struct ReturnedArtifactRowParams<'a> {
    invocation_row_id: i64,
    ordinal: i64,
    version_id: &'a str,
    name: &'a str,
    workflow_run_id: &'a str,
    artifact_name: &'a str,
    version: i64,
    sha256: &'a str,
    content_len: i64,
    format_hint: &'a Option<String>,
    verdict_line: &'a Option<String>,
    source_kind: &'static str,
    source_json: &'a str,
    returned_at: &'a str,
}

struct ParsedReturnedArtifactFieldValues {
    source: oulipoly_agent_messenger::ReturnedArtifactSource,
    returned_at: DateTime<Utc>,
    producer_invocation_uuid: Uuid,
    version: i64,
    content_len: i64,
}

struct ValidatedReturnedArtifactFieldValues {
    source: oulipoly_agent_messenger::ReturnedArtifactSource,
    returned_at: DateTime<Utc>,
    producer_invocation_uuid: Uuid,
    version: u64,
    content_len: u64,
}

enum ReturnedArtifactFieldError {
    SourceJson(serde_json::Error),
    ReturnedAt {
        raw: String,
        err: chrono::ParseError,
    },
    ProducerUuid(sqlite::Error),
    NegativeInteger {
        field: &'static str,
    },
}

fn returned_source_kind(source: &oulipoly_agent_messenger::ReturnedArtifactSource) -> &'static str {
    match source {
        oulipoly_agent_messenger::ReturnedArtifactSource::Scratchpad { .. } => "scratchpad",
        oulipoly_agent_messenger::ReturnedArtifactSource::InlineBytes => "inline_bytes",
    }
}

fn returned_artifact_producer_uuid(workflow_run_id: &str) -> sqlite::Result<Uuid> {
    let uuid_text = returned_artifact_workflow_uuid_text(workflow_run_id)?;
    parse_returned_artifact_uuid(uuid_text)
}

fn returned_artifact_workflow_uuid_text(workflow_run_id: &str) -> sqlite::Result<&str> {
    workflow_run_id
        .strip_prefix("return:")
        .ok_or_else(returned_artifact_workflow_namespace_error)
}

fn returned_artifact_workflow_namespace_error() -> sqlite::Error {
    sqlite::Error::FromSqlConversionFailure(
        2,
        sqlite::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "returned artifact workflow_run_id is not in return namespace",
        )),
    )
}

fn parse_returned_artifact_uuid(uuid_text: &str) -> sqlite::Result<Uuid> {
    Uuid::parse_str(uuid_text).map_err(|err| {
        sqlite::Error::FromSqlConversionFailure(2, sqlite::Type::Text, Box::new(err))
    })
}

fn returned_artifact_version_id(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> String {
    let encoded_name = returned_artifact_encoded_name(artifact_name);
    format_returned_artifact_version_id(invocation_uuid, &encoded_name, version)
}

fn returned_artifact_encoded_name(artifact_name: &str) -> String {
    let mut encoded_name = String::new();
    for byte in artifact_name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded_name.push(byte as char);
        } else {
            encoded_name.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded_name
}

fn format_returned_artifact_version_id(
    invocation_uuid: Uuid,
    encoded_name: &str,
    version: u64,
) -> String {
    format!("store://return/{invocation_uuid}/{encoded_name}/{version}")
}

fn returned_artifact_sql_integer(value: u64, field: &str) -> Result<i64, DbError> {
    validate_returned_artifact_sql_integer(value, field)?;
    Ok(map_returned_artifact_sql_integer(value))
}

fn validate_returned_artifact_sql_integer(value: u64, field: &str) -> Result<(), DbError> {
    if value > i64::MAX as u64 {
        Err(returned_artifact_sql_integer_overflow(field, value))
    } else {
        Ok(())
    }
}

fn map_returned_artifact_sql_integer(value: u64) -> i64 {
    value as i64
}

fn returned_artifact_sql_integer_overflow(field: &str, value: u64) -> DbError {
    format!("Returned artifact {field} exceeds SQLite INTEGER range: {value}")
}

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

    fn prepare_returned_artifacts_table(conn: &sqlite::Connection) -> Result<(), DbError> {
        conn.execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(|e| format!("Failed to ensure returned-artifacts schema: {e}"))
    }

    fn load_invocation_identity_for_returned_artifacts(
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

    fn load_invocation_uuid_text(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
    ) -> Result<String, DbError> {
        conn.query_row(
            "SELECT invocation_uuid FROM invocations WHERE id = ?1",
            sqlite::params![invocation_row_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to load invocation for returned artifacts: {e}"))?
        .ok_or_else(|| format!("Invocation {invocation_row_id} not found"))
    }

    fn parse_invocation_uuid_for_returned_artifacts(
        invocation_row_id: i64,
        uuid_text: &str,
    ) -> Result<Uuid, DbError> {
        Uuid::parse_str(uuid_text)
            .map_err(|e| format!("Invalid invocation UUID on row {invocation_row_id}: {e}"))
    }

    fn validate_returned_artifact_refs(
        identity: &InvocationIdentity,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        for reference in refs {
            Self::validate_returned_artifact_ref(identity.row_id, identity.uuid, reference)?;
        }
        Ok(())
    }

    fn replace_returned_artifact_rows(
        conn: &sqlite::Connection,
        invocation_row_id: i64,
        refs: &[ReturnedArtifactRef],
    ) -> Result<(), DbError> {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin returned-artifacts tx: {e}"))?;
        tx.execute(
            "DELETE FROM invocation_returned_artifacts WHERE invocation_id = ?1",
            sqlite::params![invocation_row_id],
        )
        .map_err(|e| format!("Failed to reset returned artifacts: {e}"))?;
        for (ordinal, reference) in refs.iter().enumerate() {
            Self::insert_returned_artifact_row(&tx, invocation_row_id, ordinal, reference)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit returned-artifacts tx: {e}"))
    }

    fn validate_returned_artifact_ref(
        invocation_row_id: i64,
        invocation_uuid: Uuid,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        let derived_uuid =
            returned_artifact_producer_uuid(&reference.store_address.workflow_run_id)
                .map_err(|e| format!("Invalid returned-artifact workflow_run_id: {e}"))?;
        Self::validate_returned_artifact_producer_uuid(reference, derived_uuid)?;
        Self::validate_returned_artifact_owner(invocation_row_id, invocation_uuid, reference)?;
        Self::validate_returned_artifact_version_id(reference, derived_uuid)
    }

    fn validate_returned_artifact_producer_uuid(
        reference: &ReturnedArtifactRef,
        derived_uuid: Uuid,
    ) -> Result<(), DbError> {
        if derived_uuid == reference.producer_invocation_uuid {
            Ok(())
        } else {
            Err(format!(
                "Returned artifact producer UUID mismatch: workflow_run_id encodes {derived_uuid}, ref carries {}",
                reference.producer_invocation_uuid
            ))
        }
    }

    fn validate_returned_artifact_owner(
        invocation_row_id: i64,
        invocation_uuid: Uuid,
        reference: &ReturnedArtifactRef,
    ) -> Result<(), DbError> {
        if reference.producer_invocation_uuid == invocation_uuid {
            Ok(())
        } else {
            Err(format!(
                "Returned artifact belongs to {}, but invocation row {invocation_row_id} is {invocation_uuid}",
                reference.producer_invocation_uuid
            ))
        }
    }

    fn validate_returned_artifact_version_id(
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
            Err(format!(
                "Returned artifact version_id mismatch: expected {expected_version_id}, ref carries {}",
                reference.version_id
            ))
        }
    }

    fn insert_returned_artifact_row(
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

    fn validate_returned_artifact_inputs(
        reference: &ReturnedArtifactRef,
    ) -> Result<ReturnedArtifactValidatedInputs, DbError> {
        Ok(ReturnedArtifactValidatedInputs {
            version: returned_artifact_sql_integer(reference.store_address.version, "version")?,
            content_len: returned_artifact_sql_integer(reference.content_len, "content_len")?,
        })
    }

    fn format_returned_artifact_payload_fields(
        reference: &ReturnedArtifactRef,
    ) -> Result<ReturnedArtifactPayloadFields, DbError> {
        Ok(ReturnedArtifactPayloadFields {
            source_json: serde_json::to_string(&reference.source)
                .map_err(|e| format!("Failed to encode returned-artifact source: {e}"))?,
            returned_at: reference.returned_at.to_rfc3339(),
        })
    }

    fn bind_returned_artifact_row_params<'a>(
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

    fn execute_returned_artifact_row_insert(
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
        .map_err(|e| format!("Failed to record returned artifact: {e}"))?;
        Ok(())
    }

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

    fn returned_artifacts_schema_is_readable(conn: &sqlite::Connection) -> Result<bool, DbError> {
        Self::validate_returned_artifacts_object_type(conn)?;
        Self::returned_artifacts_have_version_id(conn)
    }

    fn validate_returned_artifacts_object_type(conn: &sqlite::Connection) -> Result<(), DbError> {
        match Self::returned_artifacts_object_type(conn)?.as_deref() {
            None | Some("table") => Ok(()),
            Some(other) => Err(Self::unexpected_returned_artifacts_object_error(other)),
        }
    }

    fn returned_artifacts_object_type(
        conn: &sqlite::Connection,
    ) -> Result<Option<String>, DbError> {
        conn.query_row(
            "SELECT type
             FROM sqlite_master
             WHERE name = 'invocation_returned_artifacts'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to inspect returned-artifacts schema: {e}"))
    }

    fn unexpected_returned_artifacts_object_error(object_type: &str) -> DbError {
        format!("Unexpected returned-artifacts schema shape: object type={object_type}")
    }

    fn returned_artifacts_have_version_id(conn: &sqlite::Connection) -> Result<bool, DbError> {
        let columns = Self::returned_artifact_columns(conn)?;
        Ok(Self::has_column(&columns, "version_id"))
    }

    fn load_returned_artifact_rows(
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
            .map_err(|e| format!("Failed to prepare returned-artifacts query: {e}"))?;
        let rows = stmt
            .query_map(
                sqlite::params![invocation_row_id],
                Self::map_returned_artifact_raw_row,
            )
            .map_err(|e| format!("Failed to query returned artifacts: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read returned artifact row: {e}"))
    }

    fn map_returned_artifact_raw_row(
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

    fn parse_returned_artifact_rows(
        rows: Vec<ReturnedArtifactRawRow>,
    ) -> Result<Vec<ReturnedArtifactRef>, DbError> {
        rows.into_iter()
            .map(Self::parse_returned_artifact_row)
            .collect()
    }

    fn parse_returned_artifact_row(
        row: ReturnedArtifactRawRow,
    ) -> Result<ReturnedArtifactRef, DbError> {
        let parsed = Self::parse_returned_artifact_field_values(&row)
            .map_err(Self::format_returned_artifact_parse_error)?;
        let validated = Self::validate_returned_artifact_field_values(parsed)
            .map_err(Self::format_returned_artifact_parse_error)?;
        Ok(Self::map_parsed_returned_artifact_to_ref(row, validated))
    }

    fn parse_returned_artifact_field_values(
        row: &ReturnedArtifactRawRow,
    ) -> Result<ParsedReturnedArtifactFieldValues, ReturnedArtifactFieldError> {
        Ok(ParsedReturnedArtifactFieldValues {
            source: serde_json::from_str(&row.source_json)
                .map_err(ReturnedArtifactFieldError::SourceJson)?,
            returned_at: DateTime::parse_from_rfc3339(&row.returned_at_text)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|err| ReturnedArtifactFieldError::ReturnedAt {
                    raw: row.returned_at_text.clone(),
                    err,
                })?,
            producer_invocation_uuid: returned_artifact_producer_uuid(&row.workflow_run_id)
                .map_err(ReturnedArtifactFieldError::ProducerUuid)?,
            version: row.version,
            content_len: row.content_len,
        })
    }

    fn validate_returned_artifact_field_values(
        parsed: ParsedReturnedArtifactFieldValues,
    ) -> Result<ValidatedReturnedArtifactFieldValues, ReturnedArtifactFieldError> {
        Ok(ValidatedReturnedArtifactFieldValues {
            source: parsed.source,
            returned_at: parsed.returned_at,
            producer_invocation_uuid: parsed.producer_invocation_uuid,
            version: Self::validate_returned_artifact_nonnegative_integer(
                parsed.version,
                "version",
            )?,
            content_len: Self::validate_returned_artifact_nonnegative_integer(
                parsed.content_len,
                "content_len",
            )?,
        })
    }

    fn validate_returned_artifact_nonnegative_integer(
        value: i64,
        field: &'static str,
    ) -> Result<u64, ReturnedArtifactFieldError> {
        u64::try_from(value).map_err(|_| ReturnedArtifactFieldError::NegativeInteger { field })
    }

    fn map_parsed_returned_artifact_to_ref(
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

    fn format_returned_artifact_parse_error(err: ReturnedArtifactFieldError) -> DbError {
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

    fn returned_artifact_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "invocation_returned_artifacts",
            "Failed to inspect returned-artifacts schema",
            "Failed to query returned-artifacts columns",
            "Failed to read returned-artifacts column",
        )
    }
}
