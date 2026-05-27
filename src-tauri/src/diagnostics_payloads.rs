//! ## Declared roles
//!
//! `accessor`, `mapper`, `validator`.
//!
//! Account-window + quota diagnostics payload shaping (boundary-map row H10,
//! relocated out of `main.rs` by AGE-198 / slice B4). `read_account_window_state`
//! is the `accessor` (reads quota + windows from the state DB); the `*_payload`
//! functions are `mapper`s that shape state records into the `serde_json::Value`
//! emitted in the `OULIPOLY_UNKNOWN_DIAGNOSTIC=` diagnostics payload. Output-preserving:
//! the JSON field sets, ordering, and rfc3339 timestamp formatting are byte-identical
//! to the pre-relocation `main.rs` implementation.

use oulipoly_state::StateDb;

pub(crate) fn account_window_state_payload(
    state: &StateDb,
    provider_name: &str,
) -> serde_json::Value {
    format_account_window_state_payload(read_account_window_state(state, provider_name))
}

struct AccountWindowStateRead {
    quota: Result<Option<oulipoly_state::QuotaRecord>, String>,
    windows: Result<Vec<oulipoly_state::QuotaWindow>, String>,
}

fn read_account_window_state(state: &StateDb, provider_name: &str) -> AccountWindowStateRead {
    account_window_state_read(
        state.get_quota(provider_name),
        state.get_windows(provider_name),
    )
}

fn account_window_state_read(
    quota: Result<Option<oulipoly_state::QuotaRecord>, String>,
    windows: Result<Vec<oulipoly_state::QuotaWindow>, String>,
) -> AccountWindowStateRead {
    AccountWindowStateRead { quota, windows }
}

fn format_account_window_state_payload(read: AccountWindowStateRead) -> serde_json::Value {
    let (quota, quota_read_error) = match read.quota {
        Ok(quota) => (quota.map(quota_record_payload), None),
        Err(err) => (None, Some(err)),
    };
    let (windows, windows_read_error) = match read.windows {
        Ok(windows) => (
            windows
                .into_iter()
                .map(quota_window_payload)
                .collect::<Vec<_>>(),
            None,
        ),
        Err(err) => (Vec::new(), Some(err)),
    };
    serde_json::json!({
        "quota": quota,
        "quota_read_error": quota_read_error,
        "windows": windows,
        "windows_read_error": windows_read_error,
    })
}

fn quota_record_payload(record: oulipoly_state::QuotaRecord) -> serde_json::Value {
    serde_json::json!({
        "calls_since_refresh": record.calls_since_refresh,
        "refreshed_at": record.refreshed_at.map(|value| value.to_rfc3339()),
        "exhausted_at": record.exhausted_at.map(|value| value.to_rfc3339()),
        "topology_peak_live_window_count": record.topology_peak_live_window_count,
        "last_topology_probe_at": record.last_topology_probe_at.map(|value| value.to_rfc3339()),
        "next_available_at": record.next_available_at.map(|value| value.to_rfc3339()),
        "last_refresh_at": record.last_refresh_at.map(|value| value.to_rfc3339()),
        "failure_class": record.failure_class,
    })
}

fn quota_window_payload(window: oulipoly_state::QuotaWindow) -> serde_json::Value {
    serde_json::json!({
        "window_id": window.window_id,
        "used_percent": window.used_percent,
        "resets_at": window.resets_at.to_rfc3339(),
        "last_delta_percent": window.last_delta_percent,
        "last_delta_calls": window.last_delta_calls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn quota_record_payload_shapes_full_field_set_with_rfc3339_timestamps() {
        let refreshed = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        let record = oulipoly_state::QuotaRecord {
            provider_name: "p".to_string(),
            calls_since_refresh: 7,
            refreshed_at: Some(refreshed),
            exhausted_at: None,
            topology_peak_live_window_count: 3,
            last_topology_probe_at: None,
            next_available_at: None,
            last_refresh_at: None,
            failure_class: Some("quota_exhausted".to_string()),
        };

        let payload = quota_record_payload(record);

        assert_eq!(payload["calls_since_refresh"], serde_json::json!(7));
        assert_eq!(
            payload["refreshed_at"],
            serde_json::json!(refreshed.to_rfc3339())
        );
        assert_eq!(payload["exhausted_at"], serde_json::Value::Null);
        assert_eq!(
            payload["topology_peak_live_window_count"],
            serde_json::json!(3)
        );
        assert_eq!(payload["last_topology_probe_at"], serde_json::Value::Null);
        assert_eq!(payload["next_available_at"], serde_json::Value::Null);
        assert_eq!(payload["last_refresh_at"], serde_json::Value::Null);
        assert_eq!(
            payload["failure_class"],
            serde_json::json!("quota_exhausted")
        );
        // Exact key set (no extra/missing fields).
        let obj = payload.as_object().expect("object");
        assert_eq!(obj.len(), 8);
    }

    #[test]
    fn quota_window_payload_shapes_full_field_set_with_rfc3339_resets_at() {
        let resets = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let window = oulipoly_state::QuotaWindow {
            provider_name: "p".to_string(),
            window_id: 2,
            used_percent: 0.23,
            resets_at: resets,
            last_delta_percent: Some(0.01),
            last_delta_calls: Some(4),
        };

        let payload = quota_window_payload(window);

        assert_eq!(payload["window_id"], serde_json::json!(2));
        assert_eq!(payload["used_percent"], serde_json::json!(0.23));
        assert_eq!(payload["resets_at"], serde_json::json!(resets.to_rfc3339()));
        assert_eq!(payload["last_delta_percent"], serde_json::json!(0.01));
        assert_eq!(payload["last_delta_calls"], serde_json::json!(4));
        let obj = payload.as_object().expect("object");
        assert_eq!(obj.len(), 5);
    }

    #[test]
    fn account_window_envelope_ok_branch_maps_quota_and_windows() {
        let read = AccountWindowStateRead {
            quota: Ok(None),
            windows: Ok(Vec::new()),
        };

        let payload = format_account_window_state_payload(read);

        assert_eq!(payload["quota"], serde_json::Value::Null);
        assert_eq!(payload["quota_read_error"], serde_json::Value::Null);
        assert_eq!(payload["windows"], serde_json::json!([]));
        assert_eq!(payload["windows_read_error"], serde_json::Value::Null);
        let obj = payload.as_object().expect("object");
        assert_eq!(obj.len(), 4);
    }

    #[test]
    fn account_window_envelope_err_branches_capture_read_errors() {
        let read = AccountWindowStateRead {
            quota: Err("quota boom".to_string()),
            windows: Err("windows boom".to_string()),
        };

        let payload = format_account_window_state_payload(read);

        assert_eq!(payload["quota"], serde_json::Value::Null);
        assert_eq!(payload["quota_read_error"], serde_json::json!("quota boom"));
        assert_eq!(payload["windows"], serde_json::json!([]));
        assert_eq!(
            payload["windows_read_error"],
            serde_json::json!("windows boom")
        );
    }
}
