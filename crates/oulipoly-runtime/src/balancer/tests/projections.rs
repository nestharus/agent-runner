//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn high_weekly_account_stops_winning_after_cumulative_turns() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    let long_delta = 0.01;
    let long_calls = 22;

    seed_windows_with_deltas(
        &db,
        "a",
        &[
            (0.80, 24 * 7, long_delta, long_calls),
            (0.04, 5, 0.30, long_calls),
        ],
    );
    seed_windows_with_deltas(
        &db,
        "b",
        &[
            (0.10, 24 * 7, long_delta, long_calls),
            (0.85, 5, 0.30, long_calls),
        ],
    );
    seed_assistant_turns_since_refresh(&db, "a", 500);

    assert_eq!(selected_provider_index(&model, &db), 1);
}

#[test]
fn compute_projections_with_context_refreshes_stale_quota_and_scans_sessions() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    let resets = (Utc::now() + Duration::hours(48)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let quota_script = format!(
        "printf '%s' '{{\"windows\":[{{\"used_percent\":25,\"resets_at\":\"{resets}\"}}]}}'"
    );
    let providers_cfg = providers_config_with_scripts(&[("a", quota_script.as_str())]);
    let sessions_cfg = sessions_config_with_scripts(&[(
        "a",
        "printf '%s\n' '{\"session_id\":\"a-session\",\"turn_id\":\"turn-1\",\"timestamp\":\"2099-01-01T00:00:00Z\",\"role\":\"assistant\"}'",
    )]);
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let projections = compute_projections(&model, &db, Some(&ctx));

    assert_eq!(db.get_windows("a").unwrap().len(), 1);
    assert_eq!(db.count_assistant_turns_since("a", None).unwrap(), 1);
    assert_eq!(
        projections
            .iter()
            .find(|projection| projection.provider_index == 0)
            .unwrap()
            .projections_per_window
            .len(),
        0,
        "newly refreshed windows have no learned burn rate yet"
    );
}

#[test]
fn compute_projections_with_context_swallows_refresh_and_scan_failures() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.20, 24, 0.01, 10)]);
    db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::days(30)))
        .unwrap();
    let providers_cfg = providers_config_with_scripts(&[("a", "exit 1")]);
    let sessions_cfg = sessions_config_with_scripts(&[("a", "exit 1")]);
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let projections = compute_projections(&model, &db, Some(&ctx));

    let projection = projections
        .iter()
        .find(|projection| projection.provider_index == 0)
        .unwrap();
    assert_eq!(projection.projections_per_window.len(), 1);
    assert!(projection.binding_score.is_some());
    assert_eq!(db.count_assistant_turns_since("a", None).unwrap(), 0);
    assert_approx(db.get_windows("a").unwrap()[0].used_percent, 0.20, 1e-12);
}

#[test]
fn compute_projections_suppresses_recent_error_provider_with_windows() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.10, 24, 0.01, 10)]);
    seed_windows_with_deltas(&db, "b", &[(0.20, 24, 0.01, 10)]);
    for _ in 0..3 {
        record_invocation_for_test(&db, &model.name, "a", 0, false);
    }

    let projections = compute_projections(&model, &db, None);
    let suppressed = projections
        .iter()
        .find(|projection| projection.provider_index == 0)
        .unwrap();
    let healthy = projections
        .iter()
        .find(|projection| projection.provider_index == 1)
        .unwrap();

    assert_eq!(suppressed.recent_error_count, 3);
    assert_eq!(suppressed.projections_per_window, Vec::new());
    assert_eq!(suppressed.binding_score, None);
    assert!(healthy.binding_score.is_some());
}

#[test]
fn compute_projections_treats_turn_count_read_errors_as_zero_turns() {
    let (_dir, path, db) = file_backed_state("turn-count-errors");
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.10, 24, 0.01, 1)]);
    seed_assistant_turns_since_refresh(&db, "a", 10);
    drop_table(&path, "session_turns");

    let projections = compute_projections(&model, &db, None);

    let projection = projections
        .iter()
        .find(|projection| projection.provider_index == 0)
        .unwrap();
    assert_approx(
        projection.projections_per_window[0].projected_used,
        0.10,
        1e-12,
    );
}

// risk: compute_projections refactor equivalence; level: particular-integration; source: proposal §11.1 compute_projections refactor equivalence / A4.
#[test]
fn compute_projections_exposes_window_projection_used_by_selection() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.40, 5, 0.01, 22)]);
    seed_assistant_turns_since_refresh(&db, "alpha", 10);

    let projections = compute_projections(&model, &db, None);

    let active = projections
        .iter()
        .find(|projection| projection.provider_index == 0)
        .expect("active provider projection");
    assert_eq!(active.projections_per_window.len(), 1);
    assert!(active.projections_per_window[0].projected_used >= 0.40);
    assert!(active.binding_score.is_some());
}
