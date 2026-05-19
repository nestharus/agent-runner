use super::{in_memory_state, seed_learned_windows};

#[test]
fn age158_seed_learned_windows_empty_input_writes_empty_quota_refresh() {
    let db = in_memory_state();

    seed_learned_windows(&db, "empty-provider", &[]);

    let quota = db.get_quota("empty-provider").unwrap().unwrap();
    assert_eq!(quota.provider_name, "empty-provider");
    assert_eq!(quota.calls_since_refresh, 0);
    assert!(quota.refreshed_at.is_some());
    assert!(quota.exhausted_at.is_none());
    assert!(db.get_windows("empty-provider").unwrap().is_empty());
}
