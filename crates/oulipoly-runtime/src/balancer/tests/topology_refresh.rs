//! ## Declared roles
//!
//! `orchestration`.

use super::*;

/// Risk: Topology probe might run too late, after density already chose the stale single-window provider.
/// Level: component.
/// Source: proposal §Test-intent track row 6; Assumptions A2, A6.
#[test]
fn topology_probe_refreshes_incomplete_cached_provider_before_density() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.02, 24 * 7, 0.01, 40)]);
    seed_windows_with_deltas(&db, "b", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);

    let long_resets = (Utc::now() + Duration::hours(80)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let short_resets = (Utc::now() + Duration::hours(5)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let repaired_a_script = format!(
        r#"printf '%s' '{{"windows":[{{"used_percent":4,"resets_at":"{long_resets}"}},{{"used_percent":90,"resets_at":"{short_resets}"}}]}}'"#
    );
    let providers_cfg = providers_config_with_scripts(&[
        ("a", repaired_a_script.as_str()),
        ("b", "printf '%s' '{\"windows\":[]}'"),
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
        db.get_windows("a").unwrap().len(),
        2,
        "topology probe should refresh the incomplete cached provider before scoring"
    );
    assert_eq!(
        selected, 1,
        "after the repaired short-window constraint is visible, provider b should win"
    );
}

/// Risk: Persistently one-window providers could run quota scripts every invocation.
/// Level: component.
/// Source: proposal §Test-intent track row 7; Assumptions A2, A6.
#[test]
fn topology_probe_respects_cooldown_for_persistent_short_topology() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.02, 24 * 7, 0.01, 40)]);
    seed_windows_with_deltas(&db, "b", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);
    db.record_topology_probe("a").unwrap();

    let would_repair_a_script = r#"printf '%s' '{"windows":[{"used_percent":4,"resets_at":"2036-05-09T14:00:00Z"},{"used_percent":90,"resets_at":"2036-05-03T03:50:00Z"}]}'"#;
    let providers_cfg = providers_config_with_scripts(&[
        ("a", would_repair_a_script),
        ("b", "printf '%s' '{\"windows\":[]}'"),
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
        db.get_windows("a").unwrap().len(),
        1,
        "recent topology probe timestamp should suppress a repeat probe"
    );
    assert_eq!(
        selected, 0,
        "cooldown preserves cached routing rather than repeatedly running quota scripts"
    );
}

#[test]
fn routing_refreshes_stale_quota_after_thirty_seconds_before_scoring() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.02, 48, 0.01, 40)]);
    seed_windows_with_deltas(&db, "b", &[(0.40, 48, 0.01, 40)]);
    db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::seconds(31)))
        .unwrap();

    let resets = (Utc::now() + Duration::hours(48)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let exhausted_a_script =
        format!(r#"printf '%s' '{{"windows":[{{"used_percent":100,"resets_at":"{resets}"}}]}}'"#);
    let providers_cfg = providers_config_with_scripts(&[("a", exhausted_a_script.as_str())]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

    assert_eq!(selected, 1);
    assert!(
        db.get_windows("a")
            .unwrap()
            .iter()
            .any(|window| window.used_percent >= 1.0),
        "stale provider a should be refreshed to exhausted before routing"
    );
}

#[test]
fn routing_uses_cached_quota_inside_thirty_second_ttl() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.02, 48, 0.01, 40)]);
    seed_windows_with_deltas(&db, "b", &[(0.40, 48, 0.01, 40)]);
    db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::seconds(10)))
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("quota-ran");
    let script = dir.path().join("quota.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ntouch {}\nprintf '%s' '{{\"windows\":[{{\"used_percent\":100,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'\n",
            marker.display()
        ),
    )
    .unwrap();
    let script_cmd = format!("sh {}", script.display());
    let providers_cfg = providers_config_with_scripts(&[("a", script_cmd.as_str())]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

    assert_eq!(selected, 0);
    assert!(
        !marker.exists(),
        "fresh cached quota should suppress the quota script inside the routing TTL"
    );
}

#[test]
fn routing_refresh_failure_falls_back_to_cached_quota() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.02, 48, 0.01, 40)]);
    seed_windows_with_deltas(&db, "b", &[(0.40, 48, 0.01, 40)]);
    db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::seconds(31)))
        .unwrap();

    let providers_cfg = providers_config_with_scripts(&[("a", "exit 1")]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

    assert_eq!(
        selected, 0,
        "refresh failures should leave cached state usable instead of aborting routing"
    );
    assert!(
        db.get_windows("a")
            .unwrap()
            .iter()
            .all(|window| window.used_percent < 1.0),
        "failed refresh must preserve prior cached windows"
    );
}
