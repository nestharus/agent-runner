//! ## Declared roles
//!
//! `orchestration`.

use super::*;

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_picks_best_scored_sibling_on_resume() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.83, 50, 0.01, 22)]);
    seed_windows_with_deltas(
        &db,
        "beta",
        &[(0.19, 24 * 7, 0.01, 22), (0.09, 3, 0.01, 22)],
    );

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::QuotaThreshold
        }
    );
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_stays_when_active_is_least_loaded() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.30, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.80, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_ignores_short_window_pressure_on_siblings() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("gamma", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.83, 50, 0.01, 22)]);
    seed_windows_with_deltas(
        &db,
        "gamma",
        &[(0.19, 24 * 7, 0.01, 22), (0.09, 3, 0.01, 22)],
    );

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::QuotaThreshold
        }
    );
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_breaks_ties_by_provider_index() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[
        ("alpha", "project_storage"),
        ("beta", "project_storage"),
        ("gamma", "project_storage"),
    ]);
    seed_windows_with_deltas(&db, "alpha", &[(0.30, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.30, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "gamma", &[(0.90, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 2), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 0,
            reason: TransitionReason::QuotaThreshold
        }
    );
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_migrates_when_exhausted_flag_set() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.20, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.30, 5, 0.01, 22)]);
    db.mark_exhausted("alpha").unwrap();

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::Exhausted
        }
    );
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_stays_when_single_provider_pool() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.99, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_stays_when_no_sibling_has_session_storage() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "none")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.99, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.30, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_manual_overrides_best_score() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.50, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.60, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), Some("beta")).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::Manual
        }
    );
}

#[test]
fn decide_migration_manual_allows_script_project_storage_storage() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "script_storage"), ("beta", "script_storage")]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), Some("beta")).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::Manual
        }
    );
}

#[test]
fn decide_migration_picks_script_project_storage_sibling_on_resume() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "script_storage"), ("beta", "script_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.90, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.20, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::QuotaThreshold
        }
    );
}

#[test]
fn decide_migration_skips_unparseable_script_storage_sibling_on_resume() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[
        ("alpha", "project_storage"),
        ("beta", "custom_script"),
        ("gamma", "project_storage"),
    ]);
    seed_windows_with_deltas(&db, "alpha", &[(0.90, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.10, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "gamma", &[(0.20, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 2,
            reason: TransitionReason::QuotaThreshold
        }
    );
}
