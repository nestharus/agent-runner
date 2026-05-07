mod common;

use chrono::{TimeDelta, Utc};
use oulipoly_agent_scratchpad::ScratchpadError;
use uuid::Uuid;

// proposal § Test-Intent Track row 9
// contract § Expected observable signals row gc-invocation-canonical-safe
// named risk: Scratchpad Domain Layer HIGH - invocation GC could tombstone canonical rows or other scopes
// selected level: library_integration
#[test]
fn invocation_gc_tombstones_only_selected_scratchpad_scope() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let canonical = common::put_canonical_row(&store, "canonical-run", "artifact.md");
    common::put_scratchpad_row(&store, first, "a.md", b"a".to_vec());
    common::put_scratchpad_row(&store, first, "b.md", b"b".to_vec());
    common::put_scratchpad_row(&store, second, "a.md", b"other".to_vec());

    let report = scratchpad
        .gc(common::gc_invocation_request(first, false))
        .expect("gc invocation");

    assert!(!report.dry_run);
    assert_eq!(report.tombstoned_rows.len(), 2);
    assert!(
        report
            .tombstoned_rows
            .iter()
            .all(|row| row.invocation_uuid == first)
    );
    common::assert_no_canonical_rows_tombstoned(&store);
    let canonical_record = store
        .get(&canonical.key, Some(canonical.version))
        .expect("canonical still readable");
    assert_eq!(canonical_record.content, b"canonical bytes".to_vec());
    let other_scope = scratchpad
        .read(common::read_request(second, "a.md", None))
        .expect("other scope still readable");
    assert_eq!(other_scope.content, b"other".to_vec());
}

// proposal § Test-Intent Track row 10
// contract § Expected observable signals row gc-expired-before-past-noop
// named risk: Scratchpad Domain Layer HIGH - TTL cutoff math could delete fresh rows
// selected level: library_integration
#[test]
fn expired_before_past_noops_and_keeps_fresh_rows_readable() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "fresh.md", b"fresh".to_vec());

    let report = scratchpad
        .gc(common::gc_expired_request(
            Utc::now() - TimeDelta::days(1),
            false,
        ))
        .expect("gc past cutoff");

    assert!(report.tombstoned_rows.is_empty());
    assert!(report.already_tombstoned_rows.is_empty());
    let fresh = scratchpad
        .read(common::read_request(invocation, "fresh.md", None))
        .expect("fresh still readable");
    assert_eq!(fresh.content, b"fresh".to_vec());
}

// proposal § Test-Intent Track row 10 and Assumption Register A2
// named risk: Scratchpad Domain Layer HIGH - TTL sweep could ignore derived 7-day expiry or canonical filtering
// selected level: library_integration
#[test]
fn expired_before_tombstones_only_rows_whose_created_at_plus_seven_days_has_elapsed() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let old_invocation = Uuid::new_v4();
    let fresh_invocation = Uuid::new_v4();
    let old_created_at = Utc::now() - TimeDelta::days(8);
    let fresh_created_at = Utc::now() - TimeDelta::days(2);

    common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        old_invocation,
        "old.md",
        b"old".to_vec(),
        old_created_at,
    );
    common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        fresh_invocation,
        "fresh.md",
        b"fresh".to_vec(),
        fresh_created_at,
    );
    common::put_canonical_row(&store, "canonical-run", "artifact.md");

    let report = scratchpad
        .gc(common::gc_expired_request(Utc::now(), false))
        .expect("gc expired");

    assert_eq!(
        report.tombstoned_rows,
        vec![common::address(old_invocation, "old.md")]
    );
    let expired_read = scratchpad
        .read(common::read_request(old_invocation, "old.md", None))
        .expect_err("expired row is tombstoned");
    assert!(matches!(expired_read, ScratchpadError::NotFound));
    let fresh = scratchpad
        .read(common::read_request(fresh_invocation, "fresh.md", None))
        .expect("fresh not expired");
    assert_eq!(fresh.content, b"fresh".to_vec());
    common::assert_no_canonical_rows_tombstoned(&store);
}

// proposal § Test-Intent Track rows 9, 10
// named risk: Scratchpad Domain Layer HIGH - dry-run GC could mutate instead of only enumerating
// selected level: library_integration
#[test]
fn gc_dry_run_enumerates_without_tombstoning() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "candidate.md", b"candidate".to_vec());

    let report = scratchpad
        .gc(common::gc_invocation_request(invocation, true))
        .expect("dry run");

    assert!(report.dry_run);
    assert_eq!(
        report.tombstoned_rows,
        vec![common::address(invocation, "candidate.md")]
    );
    let still_readable = scratchpad
        .read(common::read_request(invocation, "candidate.md", None))
        .expect("dry-run row still readable");
    assert_eq!(still_readable.content, b"candidate".to_vec());
}
