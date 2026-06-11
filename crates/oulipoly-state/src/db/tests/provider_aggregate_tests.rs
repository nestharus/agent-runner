//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn recent_errors() {
    let db = test_db();
    let failed = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "m".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let succeeded = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "m".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let failed_id = db.start_invocation(&failed).unwrap();
    db.finalize_invocation(failed_id, false, 1, None, None)
        .unwrap();
    let succeeded_id = db.start_invocation(&succeeded).unwrap();
    db.finalize_invocation(succeeded_id, true, 0, None, None)
        .unwrap();

    let count = db.recent_error_count("m", "fixture-provider", 60).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn recent_error_count_uses_provider_name_not_reused_index_history() {
    let db = test_db();

    for _ in 0..3 {
        record_provider_invocation(
            &db,
            "routing-model",
            "claude-old",
            0,
            false,
            Some("rate_limit"),
            None,
        );
    }

    assert_eq!(
        db.recent_error_count("routing-model", "claude", 60)
            .unwrap(),
        0,
        "current provider name must not inherit recent failures from a prior occupant of index 0"
    );
    assert_eq!(
        db.recent_error_count("routing-model", "claude-old", 60)
            .unwrap(),
        3,
        "the failed provider name still owns its own recent failures"
    );
}

#[test]
fn provider_aggregate_round_trip_follows_name_after_reorder() {
    let db = test_db();
    record_provider_invocation(&db, "routing-model", "claude2", 0, true, None, None);

    let claude2 = db
        .get_provider("routing-model", "claude2")
        .unwrap()
        .expect("claude2 aggregate should exist by provider name");
    assert_eq!(claude2.provider_name, "claude2");
    assert_eq!(claude2.invocation_count, 1);
    assert!(
        db.get_provider("routing-model", "claude")
            .unwrap()
            .is_none(),
        "claude must not inherit claude2 history after taking index 0"
    );

    assert!(
        db.get_provider("routing-model", "claude")
            .unwrap()
            .is_none(),
        "fallback scoring should treat the current claude provider as unused"
    );
}

#[test]
fn provider_aggregate_round_trip_does_not_inherit_renamed_provider_history() {
    let db = test_db();
    record_provider_invocation(&db, "routing-model", "claude-old", 0, true, None, None);

    let old = db
        .get_provider("routing-model", "claude-old")
        .unwrap()
        .expect("old provider name should retain its aggregate");
    assert_eq!(old.provider_name, "claude-old");
    assert_eq!(old.invocation_count, 1);
    assert!(
        db.get_provider("routing-model", "claude")
            .unwrap()
            .is_none(),
        "renamed provider claude starts without aggregate history unless invocations use that name"
    );
}
