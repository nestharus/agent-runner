//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn pinned_fresh_route_selects_requested_eligible_provider() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    let selected = select_provider_with_pin(&model, &db, None, Some("b")).unwrap();

    assert_eq!(selected, 1);
}

#[test]
fn pinned_fresh_route_errors_for_provider_not_in_model() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    let err = select_provider_with_pin(&model, &db, None, Some("missing")).unwrap_err();

    assert!(
        matches!(err, RoutingError::PinnedProviderNotInModel { .. }),
        "expected not-in-model pin error, got {err:?}"
    );
}

#[test]
fn pinned_fresh_route_errors_for_ineligible_provider() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    seed_windows_with_deltas(&db, "a", &[(0.20, 24, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(1.00, 24, 0.01, 22)]);

    let err = select_provider_with_pin(&model, &db, None, Some("b")).unwrap_err();

    assert!(
        matches!(err, RoutingError::PinnedProviderIneligible { .. }),
        "expected ineligible pin error, got {err:?}"
    );
}
