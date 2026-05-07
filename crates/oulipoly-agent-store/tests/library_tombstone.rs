mod common;

use oulipoly_agent_store::{StoreError, TombstoneStatus};

// proposal-test-intent-row 7: tombstone-aware latest reads
// assumption-register: A6, A7
// named risk: latest-read selection could return tombstoned content or the wrong active version after deletion
// selected level: particular-integration
#[test]
fn latest_read_returns_highest_non_tombstoned_version() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-tomb", "artifact.md");
    store
        .put(common::put_request(key.clone(), b"v1".to_vec()))
        .expect("put v1");
    let v2 = store
        .put(common::put_request(key.clone(), b"v2".to_vec()))
        .expect("put v2");
    let v3 = store
        .put(common::put_request(key.clone(), b"v3".to_vec()))
        .expect("put v3");
    store
        .tombstone(&key, v3.version, "tester", "superseded")
        .expect("tombstone v3");

    let latest = store.get(&key, None).expect("latest");

    assert_eq!(latest.meta.version, v2.version);
    assert_eq!(latest.content, b"v2".to_vec());
}

// proposal-test-intent-row 9: list tombstone defaults and order
// assumption-register: A6
// named risk: history/list defaults could leak tombstoned rows or order rows nondeterministically
// selected level: particular-integration
#[test]
fn list_omits_tombstoned_by_default_and_orders_active_rows() {
    let (_db, store) = common::init_temp_store();
    let key_b = common::key("R-list", "b.md");
    let key_a = common::key("R-list", "a.md");
    let a1 = store
        .put(common::put_request(key_a.clone(), b"a1".to_vec()))
        .expect("put a1");
    let a2 = store
        .put(common::put_request(key_a.clone(), b"a2".to_vec()))
        .expect("put a2");
    let b1 = store
        .put(common::put_request(key_b.clone(), b"b1".to_vec()))
        .expect("put b1");
    store
        .tombstone(&key_a, a2.version, "tester", "hide a2")
        .expect("tombstone");

    let listed = store
        .list(common::list_filter(Some("R-list"), None, false))
        .expect("list");

    let observed: Vec<_> = listed
        .iter()
        .map(|m| (m.key.artifact_name.as_str(), m.version))
        .collect();
    assert_eq!(observed, vec![("a.md", a1.version), ("b.md", b1.version)]);
    assert!(listed.iter().all(|m| m.tombstone.is_none()));
}

// proposal-test-intent-row 11: tombstone audit metadata
// assumption-register: A6
// named risk: tombstone implementation could physically delete rows or lose audit metadata
// selected level: particular-integration
#[test]
fn tombstone_records_audit_metadata_and_preserves_meta_row() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-tomb", "audit.md");
    let receipt = store
        .put(common::put_request(
            key.clone(),
            b"audited content".to_vec(),
        ))
        .expect("put");

    let tombstone = store
        .tombstone(&key, receipt.version, "operator", "no longer canonical")
        .expect("tombstone");
    let explicit_meta = store
        .get_meta(&key, Some(receipt.version))
        .expect("explicit meta");
    let with_tombstones = store
        .list(common::list_filter(Some("R-tomb"), Some("audit.md"), true))
        .expect("list");

    assert_eq!(tombstone.status, TombstoneStatus::Tombstoned);
    assert_eq!(tombstone.tombstone.actor, "operator");
    assert_eq!(tombstone.tombstone.reason, "no longer canonical");
    assert_eq!(explicit_meta.tombstone, Some(tombstone.tombstone.clone()));
    assert_eq!(
        with_tombstones.len(),
        1,
        "expected exactly one tombstoned row"
    );
    assert_eq!(with_tombstones[0].tombstone, Some(tombstone.tombstone));
}

// proposal-test-intent-row 11: tombstone semantics - explicit content remains hidden
// assumption-register: A6
// named risk: tombstone implementation could preserve metadata but still leak tombstoned content through explicit get
// selected level: particular-integration
#[test]
fn explicit_get_of_tombstoned_version_returns_not_found_while_meta_remains_visible() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-tomb", "hidden-content.md");
    let receipt = store
        .put(common::put_request(key.clone(), b"hidden".to_vec()))
        .expect("put");
    store
        .tombstone(&key, receipt.version, "operator", "hide content")
        .expect("tombstone");

    let content_err = store
        .get(&key, Some(receipt.version))
        .expect_err("tombstoned content hidden");
    let meta = store
        .get_meta(&key, Some(receipt.version))
        .expect("tombstoned meta visible");

    assert!(matches!(content_err, StoreError::NotFound));
    assert!(meta.tombstone.is_some());
}

// proposal-test-intent-row 12: tombstone idempotency
// assumption-register: A10
// named risk: retry-safe delete behavior could mutate the original tombstone audit identity
// selected level: particular-integration
#[test]
fn tombstoning_already_tombstoned_version_preserves_original_metadata() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-tomb", "idempotent.md");
    let receipt = store
        .put(common::put_request(key.clone(), b"content".to_vec()))
        .expect("put");

    let first = store
        .tombstone(&key, receipt.version, "first-actor", "first reason")
        .expect("first tombstone");
    let second = store
        .tombstone(&key, receipt.version, "second-actor", "second reason")
        .expect("second tombstone");

    assert_eq!(second.status, TombstoneStatus::AlreadyTombstoned);
    assert_eq!(second.tombstone, first.tombstone);
}

// proposal-test-intent-row 13: latest fallback and all-tombstoned not-found
// assumption-register: A6
// named risk: latest resolution after deletes could fail to fall back to prior active content or could hide the all-deleted not-found case
// selected level: particular-integration
#[test]
fn latest_falls_back_to_prior_active_then_not_found_when_all_tombstoned() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-tomb", "fallback.md");
    let v1 = store
        .put(common::put_request(key.clone(), b"v1".to_vec()))
        .expect("put v1");
    let v2 = store
        .put(common::put_request(key.clone(), b"v2".to_vec()))
        .expect("put v2");

    store
        .tombstone(&key, v2.version, "tester", "hide latest")
        .expect("tombstone latest");
    let latest = store.get(&key, None).expect("fallback latest");
    assert_eq!(latest.meta.version, v1.version);
    assert_eq!(latest.content, b"v1".to_vec());

    store
        .tombstone(&key, v1.version, "tester", "hide remaining")
        .expect("tombstone v1");
    let err = store.get(&key, None).expect_err("all tombstoned");
    assert!(matches!(err, StoreError::NotFound));
}
