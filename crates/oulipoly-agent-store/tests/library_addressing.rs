mod common;

use oulipoly_agent_store::StoreError;

// proposal-test-intent-row 1: addressing - version assignment
// assumption-register: A7
// named risk: version assignment could overwrite or skip the per-key sequence for normal serial writes
// selected level: particular-integration
#[test]
fn first_put_creates_version_1_then_2_for_same_key() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R1", "artifact.md");

    let first = store
        .put(common::put_request(key.clone(), b"first".to_vec()))
        .expect("first put");
    let second = store
        .put(common::put_request(key.clone(), b"second".to_vec()))
        .expect("second put");

    assert_eq!(first.version, 1);
    assert_eq!(second.version, 2);
    assert_eq!(first.key, key);
    assert_eq!(second.key, key);
}

// proposal-test-intent-row 8: explicit reads - requested version
// assumption-register: A7
// named risk: explicit reads could ignore the requested version and always resolve latest
// selected level: particular-integration
#[test]
fn explicit_version_read_returns_requested_version_when_newer_exists() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R1", "artifact.md");
    let v1_content = b"version-one".to_vec();
    let v2_content = b"version-two".to_vec();

    store
        .put(common::put_request(key.clone(), v1_content.clone()))
        .expect("put v1");
    store
        .put(common::put_request(key.clone(), v2_content))
        .expect("put v2");

    let explicit = store.get(&key, Some(1)).expect("get explicit v1");

    assert_eq!(explicit.meta.version, 1);
    assert_eq!(explicit.content, v1_content);
}

// proposal-test-intent-row 8: explicit reads - missing requested version
// assumption-register: A7
// named risk: explicit reads could ignore an absent requested version and silently return latest
// selected level: particular-integration
#[test]
fn explicit_missing_version_is_not_found() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R1", "artifact.md");
    store
        .put(common::put_request(key.clone(), b"version-one".to_vec()))
        .expect("put v1");

    let err = store
        .get(&key, Some(2))
        .expect_err("missing explicit version");

    assert!(matches!(err, StoreError::NotFound));
}
