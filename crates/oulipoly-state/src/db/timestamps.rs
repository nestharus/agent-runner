//! ## Declared roles
//!
//! - parser
//! - formatter
//!
//! Role set: { parser, formatter }
//!
//! RFC3339 timestamp parsing and formatting helpers for persisted rows.

use super::{StateDb, sqlite};
use chrono::{DateTime, Utc};

impl StateDb {
    pub(super) fn strict_rfc3339_at(
        raw: &str,
        column_index: usize,
    ) -> sqlite::Result<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                sqlite::Error::FromSqlConversionFailure(
                    column_index,
                    sqlite::Type::Text,
                    Box::new(e),
                )
            })
    }

    pub(super) fn strict_rfc3339_message(
        raw: &str,
        field_name: &str,
    ) -> Result<DateTime<Utc>, String> {
        DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| Self::format_bad_rfc3339_message(raw, field_name, e))
    }

    fn format_bad_rfc3339_message(raw: &str, field_name: &str, err: chrono::ParseError) -> String {
        format!("Bad {field_name} {raw}: {err}")
    }

    pub(super) fn optional_forgiving_rfc3339(raw: Option<String>) -> Option<DateTime<Utc>> {
        raw.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    pub(super) fn fallback_now_rfc3339(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now())
    }

    pub(super) fn current_rfc3339_timestamp() -> String {
        Utc::now().to_rfc3339()
    }
}
