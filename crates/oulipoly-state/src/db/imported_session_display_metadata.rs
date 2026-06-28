//! Imported session display metadata persistence.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSessionDisplayMetadataUpsert {
    pub provider_name: String,
    pub provider_session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub turn_count: Option<u64>,
    pub provider_updated_at: Option<DateTime<Utc>>,
    pub seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSessionDisplayMetadata {
    pub provider_name: String,
    pub provider_session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub turn_count: Option<u64>,
    pub provider_updated_at: Option<DateTime<Utc>>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

impl StateDb {
    pub fn upsert_imported_session_display_metadata(
        &self,
        input: &ImportedSessionDisplayMetadataUpsert,
    ) -> Result<(), DbError> {
        let turn_count = optional_u64_to_i64(input.turn_count)?;
        let provider_updated_at = input
            .provider_updated_at
            .map(|timestamp| timestamp.to_rfc3339());
        let seen_at = input.seen_at.to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO imported_session_display_metadata (
                    provider_name,
                    provider_session_id,
                    title,
                    cwd,
                    turn_count,
                    provider_updated_at,
                    first_seen_at,
                    last_seen_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
                ON CONFLICT(provider_name, provider_session_id) DO UPDATE SET
                    title = excluded.title,
                    cwd = excluded.cwd,
                    turn_count = excluded.turn_count,
                    provider_updated_at = excluded.provider_updated_at,
                    last_seen_at = excluded.last_seen_at",
                params![
                    input.provider_name,
                    input.provider_session_id,
                    input.title,
                    input.cwd,
                    turn_count,
                    provider_updated_at,
                    seen_at,
                ],
            )
            .map(|_| ())
            .map_err(format_imported_session_metadata_upsert_error)
    }

    pub fn imported_session_display_metadata(
        &self,
        provider_name: &str,
        provider_session_id: &str,
    ) -> Result<Option<ImportedSessionDisplayMetadata>, DbError> {
        self.conn
            .query_row(
                "SELECT provider_name,
                        provider_session_id,
                        title,
                        cwd,
                        turn_count,
                        provider_updated_at,
                        first_seen_at,
                        last_seen_at
                 FROM imported_session_display_metadata
                 WHERE provider_name = ?1 AND provider_session_id = ?2",
                params![provider_name, provider_session_id],
                map_imported_session_display_metadata_row,
            )
            .optional()
            .map_err(format_imported_session_metadata_read_error)
    }
}

fn optional_u64_to_i64(value: Option<u64>) -> Result<Option<i64>, DbError> {
    value
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| "Imported session turn_count exceeds SQLite INTEGER".to_string())
        })
        .transpose()
}

fn map_imported_session_display_metadata_row(
    row: &sqlite::Row<'_>,
) -> sqlite::Result<ImportedSessionDisplayMetadata> {
    let provider_updated_at = optional_rfc3339(row.get(5)?)?;
    Ok(ImportedSessionDisplayMetadata {
        provider_name: row.get(0)?,
        provider_session_id: row.get(1)?,
        title: row.get(2)?,
        cwd: row.get(3)?,
        turn_count: optional_i64_to_u64(row.get(4)?)?,
        provider_updated_at,
        first_seen_at: rfc3339(row.get(6)?)?,
        last_seen_at: rfc3339(row.get(7)?)?,
    })
}

fn optional_i64_to_u64(value: Option<i64>) -> sqlite::Result<Option<u64>> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                sqlite::Error::FromSqlConversionFailure(
                    4,
                    sqlite::Type::Integer,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "negative imported session turn_count",
                    )),
                )
            })
        })
        .transpose()
}

fn optional_rfc3339(value: Option<String>) -> sqlite::Result<Option<DateTime<Utc>>> {
    value.map(rfc3339).transpose()
}

fn rfc3339(value: String) -> sqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| {
            sqlite::Error::FromSqlConversionFailure(0, sqlite::Type::Text, Box::new(error))
        })
}

fn format_imported_session_metadata_upsert_error(error: sqlite::Error) -> DbError {
    format!("Failed to upsert imported session display metadata: {error}")
}

fn format_imported_session_metadata_read_error(error: sqlite::Error) -> DbError {
    format!("Failed to read imported session display metadata: {error}")
}
