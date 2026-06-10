//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn select_provider_filters_exhausted_accounts() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.60, 24 * 7, 0.01, 22)]);
    db.mark_exhausted("a").unwrap();

    assert_eq!(selected_provider_index(&model, &db), 1);
}

#[test]
fn select_provider_readmits_exhausted_account_when_all_windows_elapsed() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(1.0, -1, 0.01, 22), (0.80, -2, 0.30, 22)]);
    db.mark_exhausted("a").unwrap();

    assert_eq!(selected_provider_index(&model, &db), 0);
    assert_eq!(db.get_quota("a").unwrap().unwrap().exhausted_at, None);
}

#[test]
fn select_provider_keeps_exhausted_account_excluded_while_a_window_is_live() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.20, -1, 0.01, 22), (1.0, 5, 0.30, 22)]);
    db.mark_exhausted("a").unwrap();

    assert_eq!(selected_provider_index(&model, &db), 1);
    assert!(db.get_quota("a").unwrap().unwrap().exhausted_at.is_some());
}

#[test]
fn select_provider_keeps_zero_window_exhausted_account_excluded() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    db.upsert_quota_refresh("a", &[]).unwrap();
    db.mark_exhausted("a").unwrap();

    assert_eq!(selected_provider_index(&model, &db), 1);
    let err = select_provider(&single_provider_model(), &db, None).unwrap_err();
    assert_eq!(
        err,
        RoutingError::AllProvidersQuotaExhausted {
            model_name: "single".to_string(),
            provider_names: vec!["a".to_string()],
        },
        "zero-window exhausted provider must be excluded from the eligible set"
    );
    assert!(db.get_quota("a").unwrap().unwrap().exhausted_at.is_some());
}

#[test]
fn select_provider_hard_excludes_accounts_at_or_over_live_window_quota() {
    for target_window in [TestWindow::SevenDay, TestWindow::FiveHour] {
        for used in [0.0, 0.99] {
            let db = StateDb::open(Path::new(":memory:")).unwrap();
            let model = two_provider_model();
            let (seven_day_used, five_hour_used) = match target_window {
                TestWindow::SevenDay => (used, 0.20),
                TestWindow::FiveHour => (0.20, used),
            };
            seed_two_window_used(&db, "a", seven_day_used, five_hour_used);
            seed_two_window_used(&db, "b", 0.995, 0.995);

            assert_eq!(
                selected_provider_index(&model, &db),
                0,
                "used={used} should stay eligible below 100%"
            );
        }

        for used in [1.0, 1.5] {
            let db = StateDb::open(Path::new(":memory:")).unwrap();
            let model = two_provider_model();
            let (seven_day_used, five_hour_used) = match target_window {
                TestWindow::SevenDay => (used, 0.20),
                TestWindow::FiveHour => (0.20, used),
            };
            seed_two_window_used(&db, "a", seven_day_used, five_hour_used);
            seed_two_window_used(&db, "b", 0.50, 0.50);

            assert_eq!(
                selected_provider_index(&model, &db),
                1,
                "used={used} must be excluded at or over 100%"
            );
        }
    }
}

#[test]
fn select_provider_past_reset_window_at_quota_does_not_hard_exclude() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = single_provider_model();

    seed_windows_with_deltas(&db, "a", &[(1.0, -1, 0.01, 22)]);

    assert_eq!(selected_provider_index(&model, &db), 0);
}

#[test]
fn select_provider_errors_when_single_account_is_at_or_over_quota() {
    for target_window in [TestWindow::SevenDay, TestWindow::FiveHour] {
        for used in [1.0, 1.5] {
            let db = StateDb::open(Path::new(":memory:")).unwrap();
            let model = single_provider_model();
            let (seven_day_used, five_hour_used) = match target_window {
                TestWindow::SevenDay => (used, 0.20),
                TestWindow::FiveHour => (0.20, used),
            };
            seed_two_window_used(&db, "a", seven_day_used, five_hour_used);

            let err = select_provider(&model, &db, None).unwrap_err();
            assert_eq!(
                err,
                RoutingError::AllProvidersQuotaExhausted {
                    model_name: "single".to_string(),
                    provider_names: vec!["a".to_string()],
                }
            );
            assert!(
                err.to_string()
                    .contains("all providers in pool single are quota-exhausted"),
                "{err}"
            );
        }
    }
}

#[test]
fn all_providers_exhausted_returns_clean_error() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    db.upsert_quota_refresh("a", &[]).unwrap();
    db.upsert_quota_refresh("b", &[]).unwrap();

    db.mark_exhausted("b").unwrap();
    db.mark_exhausted("a").unwrap();

    let err = select_provider(&model, &db, None).unwrap_err();
    assert_eq!(
        err,
        RoutingError::AllProvidersQuotaExhausted {
            model_name: "test".to_string(),
            provider_names: vec!["a".to_string(), "b".to_string()],
        }
    );
}

#[test]
fn all_provider_windows_at_or_over_quota_returns_clean_error() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_two_window_used(&db, "a", 1.0, 0.20);
    seed_two_window_used(&db, "b", 0.20, 1.5);

    let err = select_provider(&model, &db, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("all providers in pool test are quota-exhausted"),
        "{err}"
    );
}

#[test]
fn empty_model_reports_all_providers_exhausted_with_empty_display_list() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = ModelConfig {
        name: "empty".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![],
        inputs: vec![],
        provider: None,
    };

    let err = select_provider(&model, &db, None).unwrap_err();

    assert_eq!(
        err,
        RoutingError::AllProvidersQuotaExhausted {
            model_name: "empty".to_string(),
            provider_names: vec![],
        }
    );
    assert_eq!(
        err.to_string(),
        "all providers in pool empty are quota-exhausted: <empty>"
    );
}
