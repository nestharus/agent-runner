use chrono::{DateTime, Utc};
use oulipoly_state::QuotaWindowInput;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct QuotaScriptWindow {
    #[serde(default)]
    pub window_id: u32,
    /// Percent on a 0..100 scale. Values outside that range are rejected.
    /// Scripts wrapping APIs that report a 0..1 fraction must multiply by 100
    /// before emitting.
    pub used_percent: f64,
    pub resets_at: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub remaining: Option<u64>,
    #[serde(default)]
    pub unit: Option<String>,
}

impl QuotaScriptWindow {
    /// Converts to `QuotaWindowInput` for routing.
    ///
    /// # Panics
    /// Panics if `resets_at` is not a valid RFC3339 timestamp. Callers must
    /// ensure this struct was produced by `parse_output`, which validates the
    /// field; constructing a `QuotaScriptWindow` directly bypasses that check.
    pub fn to_quota_window_input(&self) -> QuotaWindowInput {
        quota_window_input(self.used_percent, parse_resets_at(&self.resets_at))
    }
}

fn parse_resets_at(resets_at: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(resets_at)
        .expect("QuotaScriptWindow is validated by parse_output")
        .with_timezone(&Utc)
}

fn quota_window_input(used_percent: f64, resets_at: DateTime<Utc>) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent: used_percent / 100.0,
        resets_at,
    }
}

#[derive(Debug)]
pub enum RefreshOutcome {
    Updated {
        windows: Vec<QuotaWindowInput>,
    },
    /// Provider is missing a `quota_script` entry in providers.toml — caller
    /// should fall back to invocation-count.
    NoScript,
    /// Another caller is already refreshing — use the existing cached value.
    AlreadyInFlight,
    /// Script ran but the output didn't parse / exit was non-zero.
    Failed(String),
}
