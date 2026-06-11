//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - parser
//! - validator
//!
//! Role set: { accessor, formatter, mapper, parser, validator }
//!
//! Provider quota read APIs and row-to-DTO mapping helpers.

use super::provider_quotas::{QuotaRecord, QuotaWindow, RawQuotaRecordRow, RawQuotaWindowRow};
use super::*;

struct QuotaRecordTimestamps {
    refreshed_at: Option<DateTime<Utc>>,
    exhausted_at: Option<DateTime<Utc>>,
    last_topology_probe_at: Option<DateTime<Utc>>,
    next_available_at: Option<DateTime<Utc>>,
    last_refresh_at: Option<DateTime<Utc>>,
}

struct DecodedQuotaRecordFields {
    calls_since_refresh: u64,
    timestamps: QuotaRecordTimestamps,
    topology_peak_live_window_count: usize,
    failure_class: Option<String>,
}

impl StateDb {
    /// Fetch provider-level quota metadata. Windows live in a separate
    /// table — use `get_windows` to get the actual quota numbers.
    pub fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        let Some(raw) = self.raw_quota_record(provider_name)? else {
            return Ok(None);
        };
        Self::quota_record_from_raw(provider_name, raw)
            .map(Some)
            .map_err(Self::format_quota_query_error)
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
            .map_err(Self::format_quota_prepare_error)?;

        let result = stmt.query_row(sqlite::params![provider_name], Self::raw_quota_record_row);

        match result {
            Ok(row) => Ok(Some(row)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Self::format_quota_query_error(e)),
        }
    }

    fn format_quota_prepare_error(e: sqlite::Error) -> String {
        format!("Failed to prepare quota query: {e}")
    }

    fn format_quota_query_error(e: sqlite::Error) -> String {
        format!("Failed to query quota: {e}")
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
        let fields = Self::decode_quota_record_fields(raw)?;
        Ok(Self::map_quota_record_fields(provider_name, fields))
    }

    fn decode_quota_record_fields(
        raw: RawQuotaRecordRow,
    ) -> sqlite::Result<DecodedQuotaRecordFields> {
        let calls_since_refresh = Self::map_quota_calls_since_refresh(raw.calls_since_refresh);
        let timestamps = Self::parse_quota_record_timestamps(&raw);
        let topology_peak_live_window_count = Self::validate_quota_topology_peak_live_window_count(
            raw.topology_peak_live_window_count,
        )?;
        Ok(Self::decoded_quota_record_fields(
            calls_since_refresh,
            timestamps,
            topology_peak_live_window_count,
            raw.failure_class,
        ))
    }

    fn map_quota_calls_since_refresh(value: i64) -> u64 {
        value as u64
    }

    fn parse_quota_record_timestamps(raw: &RawQuotaRecordRow) -> QuotaRecordTimestamps {
        QuotaRecordTimestamps {
            refreshed_at: Self::parse_quota_timestamp(raw.refreshed_at.clone()),
            exhausted_at: Self::parse_quota_timestamp(raw.exhausted_at.clone()),
            last_topology_probe_at: Self::parse_quota_timestamp(raw.last_topology_probe_at.clone()),
            next_available_at: Self::parse_quota_timestamp(raw.next_available_at.clone()),
            last_refresh_at: Self::parse_quota_timestamp(raw.last_refresh_at.clone()),
        }
    }

    fn decoded_quota_record_fields(
        calls_since_refresh: u64,
        timestamps: QuotaRecordTimestamps,
        topology_peak_live_window_count: usize,
        failure_class: Option<String>,
    ) -> DecodedQuotaRecordFields {
        DecodedQuotaRecordFields {
            calls_since_refresh,
            timestamps,
            topology_peak_live_window_count,
            failure_class,
        }
    }

    fn map_quota_record_fields(
        provider_name: &str,
        fields: DecodedQuotaRecordFields,
    ) -> QuotaRecord {
        QuotaRecord {
            provider_name: provider_name.to_string(),
            calls_since_refresh: fields.calls_since_refresh,
            refreshed_at: fields.timestamps.refreshed_at,
            exhausted_at: fields.timestamps.exhausted_at,
            topology_peak_live_window_count: fields.topology_peak_live_window_count,
            last_topology_probe_at: fields.timestamps.last_topology_probe_at,
            next_available_at: fields.timestamps.next_available_at,
            last_refresh_at: fields.timestamps.last_refresh_at,
            failure_class: fields.failure_class,
        }
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
        let rows = self.read_raw_quota_windows(provider_name)?;
        Self::quota_windows_from_raw(provider_name, rows)
    }

    fn read_raw_quota_windows(
        &self,
        provider_name: &str,
    ) -> Result<Vec<RawQuotaWindowRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT window_id, used_percent, resets_at, last_delta_percent, last_delta_calls
                 FROM provider_quota_windows
                 WHERE provider_name = ?1
                 ORDER BY window_id",
            )
            .map_err(Self::format_windows_prepare_error)?;
        let rows = stmt
            .query_map(sqlite::params![provider_name], Self::raw_quota_window_row)
            .map_err(Self::format_windows_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_window_row_error)
    }

    fn quota_windows_from_raw(
        provider_name: &str,
        rows: Vec<RawQuotaWindowRow>,
    ) -> Result<Vec<QuotaWindow>, String> {
        let mut out = Vec::new();
        for r in rows {
            out.push(
                Self::quota_window_from_raw(provider_name, r)
                    .map_err(Self::format_window_row_error)?,
            );
        }
        Ok(out)
    }

    fn format_windows_prepare_error(e: sqlite::Error) -> String {
        format!("Failed to prepare windows query: {e}")
    }

    fn format_windows_query_error(e: sqlite::Error) -> String {
        format!("Failed to query windows: {e}")
    }

    fn format_window_row_error(e: sqlite::Error) -> String {
        format!("Row error: {e}")
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
        let resets_at = Self::parse_quota_window_resets_at(&raw.resets_at)?;
        Ok(Self::map_quota_window_from_raw(
            provider_name,
            raw,
            resets_at,
        ))
    }

    fn map_quota_window_from_raw(
        provider_name: &str,
        raw: RawQuotaWindowRow,
        resets_at: DateTime<Utc>,
    ) -> QuotaWindow {
        QuotaWindow {
            provider_name: provider_name.to_string(),
            window_id: raw.window_id as u32,
            used_percent: raw.used_percent,
            resets_at,
            last_delta_percent: raw.last_delta_percent,
            last_delta_calls: raw.last_delta_calls.map(|v| v as u64),
        }
    }

    fn parse_quota_window_resets_at(raw: &str) -> sqlite::Result<DateTime<Utc>> {
        Self::strict_rfc3339_at(raw, 2)
    }
}
