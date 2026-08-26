//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn single_provider_always_zero() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = ModelConfig {
        name: "single".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new("x", vec![])],
        inputs: vec![],
        provider: None,
    };
    assert_eq!(select_provider(&model, &db, None).unwrap(), 0);
}

#[test]
fn round_robin_on_fresh_state() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    let first = select_provider(&model, &db, None).unwrap();
    assert_eq!(first, 0);

    record_invocation_for_test(&db, "test", "a", 0, true);

    let second = select_provider(&model, &db, None).unwrap();
    assert_eq!(second, 1);
}

#[test]
fn avoids_errored_providers() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    for _ in 0..3 {
        record_invocation_for_test(&db, "test", "a", 0, false);
    }

    assert_eq!(select_provider(&model, &db, None).unwrap(), 1);
}

#[test]
fn failing_idle_provider_does_not_monopolize_nested_dispatch_while_healthy_provider_is_live() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    for _ in 0..ERROR_THRESHOLD {
        record_invocation_for_test(&db, &model.name, "a", 0, false);
    }
    db.start_invocation(&invocation_start_for_test(&model.name, "b", 1))
        .unwrap();

    assert_eq!(
        select_provider(&model, &db, None).unwrap(),
        1,
        "recent-error suppression must run before least-live-load restriction"
    );
}

#[test]
fn live_parent_on_every_provider_does_not_create_a_provider_capacity_cap() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    for (provider_index, provider_name) in [(0, "a"), (1, "b")] {
        db.start_invocation(&invocation_start_for_test(
            &model.name,
            provider_name,
            provider_index,
        ))
        .unwrap();
    }

    assert!(select_provider(&model, &db, None).is_ok());
}

// Risk: Balancer recent-error call-site | level: unit
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-alpha-skipped.md §Test-intent track
#[test]
fn fallback_recent_error_scoring_uses_provider_name_not_reused_index() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    for _ in 0..3 {
        record_invocation_for_test(&db, "test", "old-a", 0, false);
    }

    let selected = select_provider(&model, &db, None).unwrap();
    assert_eq!(
        model.providers[selected].name, "a",
        "stale failures for old-a at index 0 must not suppress current provider a"
    );
}
