//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn select_provider_ignores_session_scan_errors_and_uses_stale_turn_counts() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.10, 24, 0.50, 1)]);
    seed_windows_with_deltas(&db, "b", &[(0.40, 24, 0.01, 1)]);
    let providers_cfg = ProvidersConfig::default();
    let sessions_cfg = sessions_config_with_scripts(&[(
        "a",
        "printf '%s\n' '{\"session_id\":\"a-session\",\"turn_id\":\"turn-1\",\"timestamp\":\"2099-01-01T00:00:00Z\",\"role\":\"assistant\"}'; exit 1",
    )]);
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

    assert_eq!(
        selected, 0,
        "failed session scans should leave provider a's stale zero-turn projection in place"
    );
    assert_eq!(db.count_assistant_turns_since("a", None).unwrap(), 0);
}

/// Stale `next_available_at` on a provider whose live `--usage` shows
/// healthy windows must be cleared by `select_provider` so the provider
/// becomes routable. Verifies the verify-before-honour path runs as
/// part of `refresh_routing_inputs` and clears markers that survived a
/// `write_quota_aggregate` (which clears `exhausted_at` but not
/// `next_available_at`).
#[test]
fn select_provider_clears_stale_next_available_at_when_refresh_shows_healthy() {
    let _lock_dir = tempfile::tempdir().unwrap();
    let _env_guard = crate::quota::marker_verification::test_support::EnvGuard::set_many(vec![
        (
            "OULIPOLY_DATA_HOME",
            Some(_lock_dir.path().as_os_str().to_os_string()),
        ),
        ("OULIPOLY_DATA_DIR", None),
    ]);
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    // Provider "a" has a stuck UpstreamApiDown marker far in the future;
    // provider "b" is fine. Without verify-before-honour, "a" would be
    // filtered out and "b" would always win.
    db.record_provider_unavailable(
        "a",
        Some(Utc::now() + Duration::hours(2)),
        "UpstreamApiDown",
    )
    .unwrap();
    let providers_cfg = providers_config_with_scripts(&[
        (
            "a",
            r#"echo '{"windows":[{"used_percent":3,"resets_at":"2099-01-01T00:00:00Z"}]}'"#,
        ),
        (
            "b",
            r#"echo '{"windows":[{"used_percent":4,"resets_at":"2099-01-01T00:00:00Z"}]}'"#,
        ),
    ]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let _ = select_provider(&model, &db, Some(&ctx)).unwrap();

    let quota = db.get_quota("a").unwrap().expect("row written by refresh");
    assert!(
        quota.next_available_at.is_none(),
        "verify-before-honour must clear stale marker after a healthy refresh"
    );
    assert!(quota.failure_class.is_none());
}

/// Stale `next_available_at` on a provider whose refreshed live windows
/// show >=100% used must NOT be cleared. The marker still represents
/// real exhaustion; clearing it would route into a known-bad provider.
#[test]
fn select_provider_keeps_marker_when_refresh_shows_exhausted_window() {
    let _lock_dir = tempfile::tempdir().unwrap();
    let _env_guard = crate::quota::marker_verification::test_support::EnvGuard::set_many(vec![
        (
            "OULIPOLY_DATA_HOME",
            Some(_lock_dir.path().as_os_str().to_os_string()),
        ),
        ("OULIPOLY_DATA_DIR", None),
    ]);
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    let marker_at = Utc::now() + Duration::hours(2);
    db.record_provider_unavailable("a", Some(marker_at), "RollingWindow5h")
        .unwrap();
    let providers_cfg = providers_config_with_scripts(&[
        (
            "a",
            r#"echo '{"windows":[{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}]}'"#,
        ),
        (
            "b",
            r#"echo '{"windows":[{"used_percent":2,"resets_at":"2099-01-01T00:00:00Z"}]}'"#,
        ),
    ]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

    assert_eq!(
        selected, 1,
        "a still exhausted (refresh confirms 100%) — routing must pick b"
    );
    let quota = db.get_quota("a").unwrap().expect("row exists");
    assert_eq!(
        quota.next_available_at,
        Some(marker_at),
        "marker must survive a refresh that confirms exhaustion"
    );
}

/// `provider_is_quota_exhausted` must honour the release-time slack —
/// markers within `MARKER_RELEASE_SLACK_SECS` of their stated release
/// are treated as expired even in cached-only mode. This is the
/// speculative-retry behaviour the routing layer relies on when the
/// verify path is not taken (e.g. async handlers passing `ctx=None`).
#[test]
fn provider_is_quota_exhausted_respects_release_slack() {
    let now = Utc::now();
    let quota_within_slack = QuotaRecord {
        provider_name: "a".to_string(),
        calls_since_refresh: 0,
        refreshed_at: None,
        exhausted_at: None,
        topology_peak_live_window_count: 0,
        last_topology_probe_at: None,
        next_available_at: Some(now + Duration::seconds(30)),
        last_refresh_at: None,
        failure_class: Some("UpstreamApiDown".to_string()),
    };
    assert!(
        !eligibility::provider_is_quota_exhausted(Some(&quota_within_slack), &[], now),
        "marker within slack window must not pin the provider as exhausted"
    );

    let quota_beyond_slack = QuotaRecord {
        next_available_at: Some(now + Duration::hours(1)),
        ..quota_within_slack
    };
    assert!(
        eligibility::provider_is_quota_exhausted(Some(&quota_beyond_slack), &[], now),
        "marker well beyond slack window must keep provider exhausted"
    );
}

#[test]
fn topology_probe_skips_providers_without_refresh_source() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.02, 24 * 7, 0.01, 40)]);
    seed_windows_with_deltas(&db, "b", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);
    let providers_cfg = providers_config_with_scripts(&[("b", "printf '%s' '{\"windows\":[]}'")]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

    assert_eq!(selected, 0);
    assert_eq!(
        db.get_windows("a").unwrap().len(),
        1,
        "provider a has incomplete topology but no refresh source, so the probe must not refresh it"
    );
    assert!(
        db.get_quota("a")
            .unwrap()
            .unwrap()
            .last_topology_probe_at
            .is_none(),
        "skipped topology probes must not stamp last_topology_probe_at"
    );
}

#[test]
fn select_provider_treats_quota_and_window_read_errors_as_empty_cache() {
    let (_dir, path, db) = file_backed_state("select-read-errors");
    let model = two_provider_model();
    record_invocation_for_test(&db, &model.name, "a", 0, true);
    drop_table(&path, "provider_quotas");
    drop_table(&path, "provider_quota_windows");

    let selected = select_provider(&model, &db, None).unwrap();

    assert_eq!(
        selected, 1,
        "quota/window read failures should degrade to empty cache and use invocation-count fallback"
    );
}
