//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn live_load_prefers_lower_load_only_when_quota_equivalent() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.40, 24, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.40, 24, 0.01, 22)]);
    record_running_invocations_for_test(&db, &model.name, "a", 0, 3);

    let selected = select_provider(&model, &db, None).unwrap();

    assert_eq!(
        selected, 1,
        "quota-equivalent candidates must prefer the lower-live-load provider"
    );
}

#[test]
fn live_load_does_not_override_clear_quota_winner() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.20, 24, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.60, 24, 0.01, 22)]);
    record_running_invocations_for_test(&db, &model.name, "a", 0, 5);

    let selected = select_provider(&model, &db, None).unwrap();

    assert_eq!(
        selected, 0,
        "busier provider a remains selected because it is the clear quota/density winner"
    );
}

#[test]
fn live_load_ignores_stale_running_rows() {
    let (_dir, path, db) = file_backed_state("stale-live-load");
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.40, 24, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.40, 24, 0.01, 22)]);
    record_running_invocations_for_test(&db, &model.name, "a", 0, 3);
    age_running_invocations_for_test(&path, "a", Utc::now() - Duration::hours(2));

    let selected = select_provider(&model, &db, None).unwrap();

    assert_eq!(
        selected, 0,
        "stale running rows should age out and leave the original index tie-break intact"
    );
}

fn age_running_invocations_for_test(
    path: &Path,
    provider_name: &str,
    created_at: chrono::DateTime<Utc>,
) {
    Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE invocations SET created_at = ?1 WHERE provider_name = ?2 AND status = 'running'",
            rusqlite::params![created_at.to_rfc3339(), provider_name],
        )
        .unwrap();
}
