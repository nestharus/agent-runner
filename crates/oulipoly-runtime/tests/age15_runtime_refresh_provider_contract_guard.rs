use oulipoly_config::{ProviderEntry, ProvidersConfig};
use oulipoly_runtime::quota::{self, InFlight, QuotaScriptWindow, RefreshOutcome};
use oulipoly_state::StateDb;
use rusqlite::Connection;
use std::path::Path;

// Guard for AGE-15 supported surface: the user-facing --usage path may add its
// own row-state outcome, but routing/Tauri refresh_provider must keep exposing
// cache-write failures through the existing RefreshOutcome::Failed contract.
#[test]
fn usage_does_not_change_refresh_provider_contract_for_cache_write_failure_in_routing_or_tauri_paths()
 {
    let _rich_window_contract = QuotaScriptWindow {
        window_id: 0,
        used_percent: 42.0,
        resets_at: "2099-01-01T00:00:00Z".to_string(),
        label: Some("weekly".to_string()),
        limit: Some(100),
        remaining: Some(58),
        unit: Some("requests".to_string()),
    };

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let state = StateDb::open(&db_path).unwrap();
    make_quota_window_cache_unwritable(&db_path);

    let mut providers = ProvidersConfig::default();
    providers.entries.insert(
        "quota-provider".to_string(),
        ProviderEntry {
            quota_script: Some(
                "printf '{\"windows\":[{\"used_percent\":42,\"resets_at\":\"2099-01-01T00:00:00Z\"}]}'"
                    .to_string(),
            ),
            ..ProviderEntry::default()
        },
    );

    let outcome = quota::refresh_provider("quota-provider", &providers, &InFlight::new(), &state);

    match outcome {
        RefreshOutcome::Failed(message) => {
            assert!(
                message.contains("Failed")
                    || message.contains("cache")
                    || message.contains("view")
                    || message.contains("provider_quota_windows"),
                "cache-write failure should preserve an actionable failed message: {message}"
            );
        }
        RefreshOutcome::Updated { .. } => panic!("cache-write failure must not be Updated"),
        RefreshOutcome::NoScript => panic!("configured quota_script must not be NoScript"),
        RefreshOutcome::AlreadyInFlight => panic!("no caller-owned in-flight claim was present"),
    }
}

fn make_quota_window_cache_unwritable(db_path: &Path) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        "DROP TABLE provider_quota_windows;
         CREATE VIEW provider_quota_windows AS
         SELECT
            CAST(NULL AS TEXT) AS provider_name,
            CAST(NULL AS INTEGER) AS window_id,
            CAST(NULL AS REAL) AS used_percent,
            CAST(NULL AS TEXT) AS resets_at,
            CAST(NULL AS REAL) AS last_delta_percent,
            CAST(NULL AS INTEGER) AS last_delta_calls
         WHERE 0;",
    )
    .unwrap();
}
