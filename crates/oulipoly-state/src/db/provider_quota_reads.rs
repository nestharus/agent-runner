//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - parser
//! - validator
//!
//! Role set: { accessor, mapper, parser, validator }
//!
//! Provider quota read APIs and row-to-DTO mapping helpers.

use super::provider_quotas::{QuotaRecord, QuotaWindow, RawQuotaRecordRow, RawQuotaWindowRow};
use super::*;

impl StateDb {
    /// Fetch provider-level quota metadata. Windows live in a separate
    /// table — use `get_windows` to get the actual quota numbers.
    pub fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        let Some(raw) = self.raw_quota_record(provider_name)? else {
            return Ok(None);
        };
        Self::quota_record_from_raw(provider_name, raw)
            .map(Some)
            .map_err(|e| format!("Failed to query quota: {e}"))
    }

    fn raw_quota_record(&self, provider_name: &str) -> Result<Option<RawQuotaRecordRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT calls_since_refresh, refreshed_at, exhausted_at,
                        topology_peak_live_window_count, last_topology_probe_at,
                        next_available_at, last_refresh_at, failure_class
                 FROM provider_quotas WHERE provider_name = ?1",
            )
            .map_err(|e| format!("Failed to prepare quota query: {e}"))?;

        let result = stmt.query_row(sqlite::params![provider_name], Self::raw_quota_record_row);

        match result {
            Ok(row) => Ok(Some(row)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query quota: {e}")),
        }
    }

    fn raw_quota_record_row(row: &sqlite::Row<'_>) -> sqlite::Result<RawQuotaRecordRow> {
        Ok(RawQuotaRecordRow {
            calls_since_refresh: row.get(0)?,
            refreshed_at: row.get(1)?,
            exhausted_at: row.get(2)?,
            topology_peak_live_window_count: row.get(3)?,
            last_topology_probe_at: row.get(4)?,
            next_available_at: row.get(5)?,
            last_refresh_at: row.get(6)?,
            failure_class: row.get(7)?,
        })
    }

    fn quota_record_from_raw(
        provider_name: &str,
        raw: RawQuotaRecordRow,
    ) -> sqlite::Result<QuotaRecord> {
        Ok(QuotaRecord {
            provider_name: provider_name.to_string(),
            calls_since_refresh: raw.calls_since_refresh as u64,
            refreshed_at: Self::parse_quota_timestamp(raw.refreshed_at),
            exhausted_at: Self::parse_quota_timestamp(raw.exhausted_at),
            topology_peak_live_window_count: Self::validate_quota_topology_peak_live_window_count(
                raw.topology_peak_live_window_count,
            )?,
            last_topology_probe_at: Self::parse_quota_timestamp(raw.last_topology_probe_at),
            next_available_at: Self::parse_quota_timestamp(raw.next_available_at),
            last_refresh_at: Self::parse_quota_timestamp(raw.last_refresh_at),
            failure_class: raw.failure_class,
        })
    }

    fn parse_quota_timestamp(raw: Option<String>) -> Option<DateTime<Utc>> {
        Self::optional_forgiving_rfc3339(raw)
    }

    fn validate_quota_topology_peak_live_window_count(value: i64) -> sqlite::Result<usize> {
        usize::try_from(value).map_err(|_| {
            sqlite::Error::FromSqlConversionFailure(
                3,
                sqlite::Type::Integer,
                "negative topology_peak_live_window_count".into(),
            )
        })
    }

    /// Fetch every rolling-quota window a provider has reported, ordered by
    /// `window_id`. Empty vec if the provider has never been refreshed.
    pub fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT window_id, used_percent, resets_at, last_delta_percent, last_delta_calls
                 FROM provider_quota_windows
                 WHERE provider_name = ?1
                 ORDER BY window_id",
            )
            .map_err(|e| format!("Failed to prepare windows query: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![provider_name], |row| {
                let raw = Self::raw_quota_window_row(row)?;
                Self::quota_window_from_raw(provider_name, raw)
            })
            .map_err(|e| format!("Failed to query windows: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("Row error: {e}"))?);
        }
        Ok(out)
    }

    fn raw_quota_window_row(row: &sqlite::Row<'_>) -> sqlite::Result<RawQuotaWindowRow> {
        Ok(RawQuotaWindowRow {
            window_id: row.get(0)?,
            used_percent: row.get(1)?,
            resets_at: row.get(2)?,
            last_delta_percent: row.get(3)?,
            last_delta_calls: row.get(4)?,
        })
    }

    fn quota_window_from_raw(
        provider_name: &str,
        raw: RawQuotaWindowRow,
    ) -> sqlite::Result<QuotaWindow> {
        Ok(QuotaWindow {
            provider_name: provider_name.to_string(),
            window_id: raw.window_id as u32,
            used_percent: raw.used_percent,
            resets_at: Self::parse_quota_window_resets_at(&raw.resets_at)?,
            last_delta_percent: raw.last_delta_percent,
            last_delta_calls: raw.last_delta_calls.map(|v| v as u64),
        })
    }

    fn parse_quota_window_resets_at(raw: &str) -> sqlite::Result<DateTime<Utc>> {
        Self::strict_rfc3339_at(raw, 2)
    }
}
