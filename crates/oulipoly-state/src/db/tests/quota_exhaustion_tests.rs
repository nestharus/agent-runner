//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn mark_exhausted_writes_timestamp_on_existing_quota_row() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();

    let before = Utc::now();
    db.mark_exhausted(provider).unwrap();
    let after = Utc::now();

    let exhausted = exhausted_at(&db, provider).expect("exhausted_at should be set");
    assert!(
        exhausted >= before - chrono::Duration::seconds(1)
            && exhausted <= after + chrono::Duration::seconds(1),
        "exhausted_at {exhausted} should be near mark_exhausted call"
    );
}

#[test]
fn mark_exhausted_creates_row_when_missing() {
    // CodeRabbit pass 1 finding: a plain UPDATE silently dropped the
    // write when a provider had no quota row yet (e.g. misconfigured
    // quota_script that only ever fails, or first-call quota rejection
    // before any refresh succeeded). mark_exhausted must upsert so the
    // flag always lands — otherwise the balancer routes to a known-bad
    // account on the next invocation and we get a guaranteed
    // re-failure that the reactive model is meant to prevent.
    let db = test_db();
    let provider = "never-refreshed";

    let before = Utc::now();
    db.mark_exhausted(provider).unwrap();
    let after = Utc::now();

    let row_count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM provider_quotas WHERE provider_name = ?1",
            sqlite::params![provider],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(row_count, 1, "mark_exhausted must upsert the quota row");

    let exhausted = exhausted_at(&db, provider).expect("exhausted_at set");
    assert!(
        exhausted >= before - chrono::Duration::seconds(1)
            && exhausted <= after + chrono::Duration::seconds(1)
    );
}

#[test]
fn clear_exhausted_nulls_the_flag() {
    let db = test_db();
    let provider = "a";

    db.mark_exhausted(provider).unwrap();
    assert!(exhausted_at_raw(&db, provider).is_some());

    db.clear_exhausted(provider).unwrap();
    assert_eq!(exhausted_at_raw(&db, provider), None);

    db.clear_exhausted(provider).unwrap();
    assert_eq!(exhausted_at_raw(&db, provider), None);

    db.clear_exhausted("nonexistent-provider").unwrap();
}

#[test]
fn record_provider_unavailable_writes_and_round_trips_next_available_at() {
    let db = test_db();
    let provider = "wu-a1-record";
    let ts = chrono::DateTime::parse_from_rfc3339("2026-05-21T01:23:45Z")
        .unwrap()
        .with_timezone(&Utc);

    db.record_provider_unavailable(provider, Some(ts), "RollingWindow5h")
        .unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row written");
    assert_eq!(quota.next_available_at, Some(ts));
    assert_eq!(quota.failure_class.as_deref(), Some("RollingWindow5h"));
}

#[test]
fn record_provider_unavailable_idempotent_under_repeat_calls() {
    let db = test_db();
    let provider = "wu-a1-repeat";
    let ts1 = chrono::DateTime::parse_from_rfc3339("2026-05-21T01:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let ts2 = chrono::DateTime::parse_from_rfc3339("2026-05-21T02:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    db.record_provider_unavailable(provider, Some(ts1), "RollingWindow5h")
        .unwrap();
    db.record_provider_unavailable(provider, Some(ts2), "WeeklyOrLonger")
        .unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row written");
    assert_eq!(quota.next_available_at, Some(ts2));
    assert_eq!(quota.failure_class.as_deref(), Some("WeeklyOrLonger"));
}

#[test]
fn touch_provider_refresh_updates_last_refresh_at_only() {
    let db = test_db();
    let provider = "wu-a1-touch";
    let now = chrono::DateTime::parse_from_rfc3339("2026-05-21T03:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    db.touch_provider_refresh(provider, now).unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row written");
    assert_eq!(quota.last_refresh_at, Some(now));
    assert_eq!(quota.next_available_at, None);
    assert_eq!(quota.failure_class, None);
}

#[test]
fn next_round_robin_index_for_model_returns_none_on_unknown_model() {
    let db = test_db();
    assert_eq!(db.next_round_robin_index_for_model("nope").unwrap(), None);
}

#[test]
fn advance_round_robin_index_persists_across_db_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let now = Utc::now();
    {
        let db = StateDb::open(&path).unwrap();
        db.advance_round_robin_index("claude-opus", 2, now).unwrap();
    }
    let db = StateDb::open(&path).unwrap();
    assert_eq!(
        db.next_round_robin_index_for_model("claude-opus").unwrap(),
        Some(2)
    );

    db.advance_round_robin_index("claude-opus", 5, now).unwrap();
    assert_eq!(
        db.next_round_robin_index_for_model("claude-opus").unwrap(),
        Some(5)
    );
}

#[test]
fn clear_provider_unavailable_nulls_next_available_at_and_failure_class() {
    let db = test_db();
    let provider = "wu-a1-clear";
    let ts = chrono::DateTime::parse_from_rfc3339("2026-05-21T04:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    db.record_provider_unavailable(provider, Some(ts), "UpstreamApiDown")
        .unwrap();
    db.clear_provider_unavailable(provider).unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row exists");
    assert_eq!(quota.next_available_at, None);
    assert_eq!(quota.failure_class, None);
}

#[test]
fn upsert_quota_refresh_clears_exhausted_at_on_nonempty_refresh() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();
    db.mark_exhausted(provider).unwrap();
    assert!(exhausted_at_raw(&db, provider).is_some());

    db.upsert_quota_refresh(provider, &[quota_input(0.20, "2026-04-23T00:00:00Z")])
        .unwrap();

    assert_eq!(exhausted_at_raw(&db, provider), None);
}

#[test]
fn upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();
    db.mark_exhausted(provider).unwrap();
    let exhausted_before = exhausted_at_raw(&db, provider).expect("exhausted_at should be set");

    db.upsert_quota_refresh(provider, &[]).unwrap();

    assert_eq!(
        exhausted_at_raw(&db, provider).as_deref(),
        Some(exhausted_before.as_str())
    );
}
