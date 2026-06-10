//! ## Declared roles
//!
//! `orchestration`.

use super::*;

// risk: Codex/non-Claude resume migration abort; level: particular-integration; source: AGE-48 contract §Test plan #1 / proposal A1, A2, A3.
#[test]
fn decide_migration_stays_for_omega_source_in_omega_pool() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("omega", "omega"), ("sigma", "omega")]);
    seed_windows_with_deltas(&db, "omega", &[(0.99, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "sigma", &[(0.30, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Codex/non-Claude resume migration abort; level: particular-integration; source: AGE-48 contract §Test plan #2 / proposal A1, A2, A3.
#[test]
fn decide_migration_stays_for_omega_source_with_no_storage_pool() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("omega", "none"), ("sigma", "none")]);
    seed_windows_with_deltas(&db, "omega", &[(0.99, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "sigma", &[(0.30, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Codex/non-Claude resume migration abort; level: particular-integration; source: AGE-48 contract §Test plan #3 / proposal A1, A2, A3.
#[test]
fn decide_migration_stays_for_omega_source_with_alpha_target() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("omega", "omega"), ("alpha", "project_storage")]);
    seed_windows_with_deltas(&db, "omega", &[(0.99, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "alpha", &[(0.30, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Manual migration target eligibility; level: particular-integration; source: AGE-48 contract §Test plan #4 / proposal A1, A2, A3.
#[test]
fn decide_migration_stays_for_manual_migrate_to_omega_target() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("omega", "omega")]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), Some("omega")).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

#[test]
fn decide_migration_stays_for_manual_omega_source_to_named_alpha_target() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("omega", "omega"), ("alpha", "project_storage")]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), Some("alpha")).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Exhausted branch reaches non-migratable source; level: particular-integration; source: AGE-48 contract §Test plan #5 / proposal A1, A2, A3.
#[test]
fn decide_migration_stays_for_omega_source_when_exhausted() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("omega", "omega"), ("alpha", "project_storage")]);
    seed_windows_with_deltas(&db, "omega", &[(0.99, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "alpha", &[(0.30, 5, 0.01, 22)]);
    db.mark_exhausted("omega").unwrap();

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

// risk: Target filtering preserves load ordering after Codex exclusion; level: particular-integration; source: AGE-48 contract §Test plan #6 / proposal A1, A2, A4.
#[test]
fn decide_migration_picks_eligible_target_skipping_omega_target() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[
        ("alpha", "project_storage"),
        ("omega", "omega"),
        ("beta", "project_storage"),
    ]);
    seed_windows_with_deltas(&db, "alpha", &[(0.90, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "omega", &[(0.10, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.20, 5, 0.01, 22)]);

    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 2,
            reason: TransitionReason::QuotaThreshold
        }
    );
}

// risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
#[test]
fn decide_migration_reports_projection_state_errors() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    drop_quota_table(&db);

    let err = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap_err();

    assert!(matches!(err, MigrationError::Db { .. }));
}

#[test]
fn decide_migration_stays_when_projection_window_reads_fail_after_active_quota_lookup() {
    let (_dir, path, db) = file_backed_state("migration-projection-degrades");
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.80, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.20, 5, 0.01, 22)]);
    drop_table(&path, "provider_quota_windows");

    let decision = decide_migration(&db, &model, &resolved_for(&model, 1), None).unwrap();

    assert_eq!(
        decision,
        MigrationDecision::Stay,
        "active quota lookup succeeds, then projection window reads degrade to zero-load strict-better handling"
    );
}

#[test]
fn decide_migration_stays_when_manual_target_unknown() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);

    // characterization: AGE-48 may edit the manual branch, and unknown manual targets were uncovered.
    let decision =
        decide_migration(&db, &model, &resolved_for(&model, 0), Some("alpha-missing")).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

#[test]
fn decide_migration_stays_when_manual_target_has_no_session_storage() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "none")]);

    // characterization: AGE-48 may edit the manual branch, and no-storage manual targets were uncovered.
    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), Some("beta")).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

#[test]
fn decide_migration_stays_when_exhausted_active_has_no_eligible_sibling() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "none")]);
    seed_windows_with_deltas(&db, "alpha", &[(0.99, 5, 0.01, 22)]);
    seed_windows_with_deltas(&db, "beta", &[(0.30, 5, 0.01, 22)]);
    db.mark_exhausted("alpha").unwrap();

    // characterization: AGE-48 may change target eligibility, and exhausted no-target behavior was uncovered.
    let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}

#[test]
fn decide_migration_stays_when_active_provider_missing_from_migration_model() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = migratable_model(&[("alpha", "project_storage"), ("beta", "project_storage")]);
    let mut resolved = resolved_for(&model, 0);
    resolved.active_provider = "archived-alpha".to_string();

    // characterization: AGE-48 will read active-provider resolution, and missing-active behavior was uncovered.
    let decision = decide_migration(&db, &model, &resolved, None).unwrap();

    assert_eq!(decision, MigrationDecision::Stay);
}
