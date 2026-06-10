//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn score_by_invocation_count_all_error_suppressed_candidates_uses_round_robin() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    for _ in 0..3 {
        record_invocation_for_test(&db, &model.name, "a", 0, false);
        record_invocation_for_test(&db, &model.name, "b", 1, false);
    }
    for _ in 0..2 {
        record_invocation_for_test(&db, &model.name, "a", 0, true);
    }

    let selected = select_provider(&model, &db, None).unwrap();

    assert_eq!(
        selected, 1,
        "when all fallback candidates are error-suppressed, round-robin falls back to lower invocation count"
    );
}

#[test]
fn fresh_pool_falls_through_to_invocation_count_round_robin() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    record_invocation_for_test(&db, "test", "a", 0, true);

    assert_eq!(selected_provider_index(&model, &db), 1);
}
