//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - validator
//!
//! Role set: { accessor, formatter, mapper, orchestration, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/provider_quotas.rs
//!     role: intrinsic-surface
//!     Domain: provider-quotas-persistence
//!     Owns:
//!       - StateDb provider-quotas persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: ColumnRepair, Connection, DateTime, DropColumnRepair, StateDb, Utc, sqlite
//! ```
//!
//! Provider quota schema validation, repair orchestration, and row mapping types.

use super::*;

impl StateDb {
    pub(super) fn ensure_provider_quotas_schema(conn: &sqlite::Connection) -> Result<(), String> {
        let columns = Self::provider_quotas_columns(conn)?;
        Self::execute_column_repairs(
            conn,
            &columns,
            Self::provider_quotas_column_repairs().as_slice(),
        )?;
        Self::execute_drop_column_repairs(
            conn,
            &columns,
            Self::provider_quotas_drop_column_repairs().as_slice(),
        )
    }

    fn provider_quotas_column_repairs() -> [ColumnRepair; 5] {
        [
            ColumnRepair {
                column_name: "last_empty_refresh_at",
                sql: "ALTER TABLE provider_quotas ADD COLUMN last_empty_refresh_at TEXT",
                error_context: "Failed to add provider_quotas.last_empty_refresh_at",
            },
            ColumnRepair {
                column_name: "exhausted_at",
                sql: "ALTER TABLE provider_quotas ADD COLUMN exhausted_at TEXT NULL",
                error_context: "Failed to add provider_quotas.exhausted_at",
            },
            ColumnRepair {
                column_name: "next_available_at",
                sql: "ALTER TABLE provider_quotas ADD COLUMN next_available_at TEXT NULL",
                error_context: "Failed to add provider_quotas.next_available_at",
            },
            ColumnRepair {
                column_name: "last_refresh_at",
                sql: "ALTER TABLE provider_quotas ADD COLUMN last_refresh_at TEXT NULL",
                error_context: "Failed to add provider_quotas.last_refresh_at",
            },
            ColumnRepair {
                column_name: "failure_class",
                sql: "ALTER TABLE provider_quotas ADD COLUMN failure_class TEXT NULL",
                error_context: "Failed to add provider_quotas.failure_class",
            },
        ]
    }

    fn provider_quotas_drop_column_repairs() -> [DropColumnRepair; 2] {
        [
            DropColumnRepair {
                column_name: "last_delta_percent",
                sql: "ALTER TABLE provider_quotas DROP COLUMN last_delta_percent",
                error_context: "Failed to drop provider_quotas.last_delta_percent",
            },
            DropColumnRepair {
                column_name: "last_delta_calls",
                sql: "ALTER TABLE provider_quotas DROP COLUMN last_delta_calls",
                error_context: "Failed to drop provider_quotas.last_delta_calls",
            },
        ]
    }

    pub(super) fn ensure_provider_quotas_topology_schema(
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
        let columns = Self::provider_quotas_columns(conn)?;
        Self::execute_column_repairs(
            conn,
            &columns,
            Self::provider_quotas_topology_column_repairs().as_slice(),
        )?;
        Self::backfill_provider_quotas_topology_peak_counts(conn)
    }

    fn provider_quotas_topology_column_repairs() -> [ColumnRepair; 2] {
        [
            ColumnRepair {
                column_name: "topology_peak_live_window_count",
                sql: "ALTER TABLE provider_quotas ADD COLUMN topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0",
                error_context: "Failed to add provider_quotas.topology_peak_live_window_count",
            },
            ColumnRepair {
                column_name: "last_topology_probe_at",
                sql: "ALTER TABLE provider_quotas ADD COLUMN last_topology_probe_at TEXT",
                error_context: "Failed to add provider_quotas.last_topology_probe_at",
            },
        ]
    }

    fn backfill_provider_quotas_topology_peak_counts(
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
        if !Self::provider_quotas_topology_backfill_is_needed(conn)? {
            return Ok(());
        }
        conn.execute(Self::provider_quotas_topology_backfill_sql(), [])
            .map_err(Self::format_provider_quotas_topology_backfill_error)?;
        Ok(())
    }

    fn provider_quotas_topology_backfill_is_needed(
        conn: &sqlite::Connection,
    ) -> Result<bool, String> {
        conn.query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM provider_quotas
                 WHERE topology_peak_live_window_count < (
                     SELECT COUNT(*)
                     FROM provider_quota_windows
                     WHERE provider_quota_windows.provider_name = provider_quotas.provider_name
                 )
             )",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|needed| needed != 0)
        .map_err(|e| format!("Failed to inspect provider_quotas topology peak counts: {e}"))
    }

    fn format_provider_quotas_topology_backfill_error(e: sqlite::Error) -> String {
        format!("Failed to backfill provider_quotas topology peak counts: {e}")
    }

    fn provider_quotas_topology_backfill_sql() -> &'static str {
        "UPDATE provider_quotas
         SET topology_peak_live_window_count = MAX(
            topology_peak_live_window_count,
            (
                SELECT COUNT(*)
                FROM provider_quota_windows
                WHERE provider_quota_windows.provider_name = provider_quotas.provider_name
            )
         )
         WHERE topology_peak_live_window_count < (
             SELECT COUNT(*)
             FROM provider_quota_windows
             WHERE provider_quota_windows.provider_name = provider_quotas.provider_name
         )"
    }

    pub(super) fn ensure_provider_quota_windows_schema(
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
        let columns = Self::provider_quota_windows_columns(conn)?;
        Self::execute_column_repairs(
            conn,
            &columns,
            Self::provider_quota_windows_column_repairs().as_slice(),
        )
    }

    fn provider_quota_windows_column_repairs() -> [ColumnRepair; 2] {
        [
            ColumnRepair {
                column_name: "last_delta_percent",
                sql: "ALTER TABLE provider_quota_windows ADD COLUMN last_delta_percent REAL NULL",
                error_context: "Failed to add provider_quota_windows.last_delta_percent",
            },
            ColumnRepair {
                column_name: "last_delta_calls",
                sql: "ALTER TABLE provider_quota_windows ADD COLUMN last_delta_calls INTEGER NULL",
                error_context: "Failed to add provider_quota_windows.last_delta_calls",
            },
        ]
    }

    fn provider_quotas_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "provider_quotas",
            "Failed to inspect provider_quotas schema",
            "Failed to inspect provider_quotas columns",
            "Failed to read provider_quotas column",
        )
    }

    fn provider_quota_windows_columns(conn: &sqlite::Connection) -> Result<Vec<String>, String> {
        Self::table_column_names(
            conn,
            "provider_quota_windows",
            "Failed to inspect provider_quota_windows schema",
            "Failed to inspect provider_quota_windows columns",
            "Failed to read provider_quota_windows column",
        )
    }
}

/// Ceiling on the per-turn burn rate that `upsert_quota_refresh` is willing
/// to learn from a single refresh-to-refresh sample. A transient upstream
/// spike (observed on the ChatGPT usage endpoint: `used_percent` briefly
/// reported as 1.0 before the window reset) paired with a small turn count
/// produced a learned rate of ~0.05/turn that then got carried forward across
/// subsequent no-change refreshes, projecting every provider near the ceiling
/// and making the whole pool look unusable. The highest plausible real
/// rate observed in live data is ~5e-4/turn on a 5h provider-a window; 0.1/turn
/// is a 200x safety margin that still filters the spike case.
pub(super) const MAX_LEARNABLE_BURN_RATE: f64 = 0.1;

/// Minimum assistant-turn sample size before a refresh-to-refresh delta is
/// accepted as a burn-rate learn. Below this, a 1%-on-6-turns observation
/// extrapolates to rates that are dominated by sample noise -- when the
/// rate is then multiplied by `turns_since_refresh` at scoring time, a
/// 65%-used window can project to 97% on nothing but measurement error,
/// making the provider look nearly exhausted. Live-caught 2026-04-21 on provider A with
/// `last_delta_percent=0.01 / last_delta_calls=6` -> projected
/// 0.65 + 193x0.00167 = 0.972, blocking the whole high-quota pool. 20
/// turns is the empirical floor where per-turn rates stabilize to within
/// ~2x of the long-run mean across observed provider-a/provider-b samples.
pub(super) const MIN_LEARN_SAMPLE_CALLS: u64 = 20;

/// Refuse to learn a burn rate from a sample where the window's
/// `used_percent` is already near its ceiling. A 100%-reading window did
/// not fill at a natural rate during the prior inter-refresh interval -- it
/// hit a wall at some unknown point during that window and stayed pinned.
/// The dp/dc ratio from that interval is an artifact of the cap, not a
/// physical rate. Live-caught 2026-04-21 on provider B after a transient
/// ChatGPT upstream spike reported `used_percent=1.0` on the 7-day
/// window: learned rate became 1.0/34 ~= 0.029/turn on WEEKLY (where real
/// rates live near 6e-5/turn), projecting every subsequent invocation
/// near the ceiling on nothing but a bad sample. User intuition:
/// "turns barely budge weekly" -- so any single sample imputing a weekly
/// move > 1 point is suspect, and the cleanest marker of "suspect" is
/// "the sample is at the rail." Matching ceiling from score_by_density.
pub(super) const NEAR_EXHAUSTED_USED_PERCENT: f64 = 0.99;

/// Per-provider (account) metadata. Keyed on provider name (e.g. `provider-a`,
/// `provider-b`), which spans every model routed through that account.
/// The actual quota numbers live in `provider_quota_windows` -- one row per
/// rolling window the CLI exposes (e.g. 5-hour + 7-day).
#[derive(Debug, Clone)]
pub struct QuotaRecord {
    pub provider_name: String,
    /// Calls recorded against this provider since the last refresh.
    pub calls_since_refresh: u64,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub exhausted_at: Option<DateTime<Utc>>,
    pub topology_peak_live_window_count: usize,
    pub last_topology_probe_at: Option<DateTime<Utc>>,
    pub next_available_at: Option<DateTime<Utc>>,
    pub last_refresh_at: Option<DateTime<Utc>>,
    pub failure_class: Option<String>,
}

/// One rolling-quota window reported by a provider's quota script.
/// `window_id` is a stable per-provider position index (window 0, 1, ...)
/// so the same window survives across refreshes for delta-learning.
#[derive(Debug, Clone)]
pub struct QuotaWindow {
    pub provider_name: String,
    pub window_id: u32,
    /// 0..1 ratio. 0.23 = 23% of this window's budget consumed.
    pub used_percent: f64,
    pub resets_at: DateTime<Utc>,
    pub last_delta_percent: Option<f64>,
    pub last_delta_calls: Option<u64>,
}

/// Input to `upsert_quota_refresh` -- one window's freshly-fetched values.
#[derive(Debug, Clone)]
pub struct QuotaWindowInput {
    pub used_percent: f64,
    pub resets_at: DateTime<Utc>,
}

pub(super) struct RawQuotaRecordRow {
    pub(super) calls_since_refresh: i64,
    pub(super) refreshed_at: Option<String>,
    pub(super) exhausted_at: Option<String>,
    pub(super) topology_peak_live_window_count: i64,
    pub(super) last_topology_probe_at: Option<String>,
    pub(super) next_available_at: Option<String>,
    pub(super) last_refresh_at: Option<String>,
    pub(super) failure_class: Option<String>,
}

pub(super) struct RawQuotaWindowRow {
    pub(super) window_id: i64,
    pub(super) used_percent: f64,
    pub(super) resets_at: String,
    pub(super) last_delta_percent: Option<f64>,
    pub(super) last_delta_calls: Option<i64>,
}

pub(super) struct QuotaAggregateProjection {
    pub(super) legacy_used: f64,
    pub(super) legacy_resets: Option<String>,
    pub(super) topology_peak_live_window_count: i64,
}

pub(super) struct QuotaWindowDelta {
    pub(super) last_delta_percent: Option<f64>,
    pub(super) last_delta_calls: Option<u64>,
}
