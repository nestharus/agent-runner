mod common;

use oulipoly_agent_store::PutRequest;
use uuid::Uuid;

// proposal-test-intent-row 2: metadata - producer invocation UUID
// assumption-register: A9
// named risk: producer identity metadata could be dropped between receipt, point read, and history/list surfaces
// selected level: particular-integration
#[test]
fn producer_uuid_round_trips_through_receipt_meta_and_list() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-meta", "producer.md");
    let producer_uuid = Uuid::new_v4();

    let receipt = store
        .put(PutRequest {
            producer_invocation_uuid: Some(producer_uuid),
            ..common::put_request(key.clone(), b"producer".to_vec())
        })
        .expect("put");
    let meta = store
        .get_meta(&key, Some(receipt.version))
        .expect("get meta");
    let listed = store
        .list(common::list_filter(
            Some("R-meta"),
            Some("producer.md"),
            false,
        ))
        .expect("list");

    assert_eq!(receipt.producer_invocation_uuid, Some(producer_uuid));
    assert_eq!(meta.producer_invocation_uuid, Some(producer_uuid));
    assert!(!listed.is_empty(), "list returned no producer.md entries");
    assert_eq!(listed[0].producer_invocation_uuid, Some(producer_uuid));
}

// proposal-test-intent-row 3: metadata - SHA-256 exact bytes
// assumption-register: A8
// named risk: content identity could be computed from transformed text or stored inconsistently with the byte payload
// selected level: particular-integration
#[test]
fn sha256_is_computed_from_exact_written_bytes_and_matches_read_content() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-meta", "binary.bin");
    let content = vec![0, 255, 192, b'\n', b'a', 0, 42];
    let expected_hash = common::sha256_hex(&content);

    let receipt = store
        .put(common::put_request(key.clone(), content.clone()))
        .expect("put");
    let record = store.get(&key, Some(receipt.version)).expect("get");
    let meta = store
        .get_meta(&key, Some(receipt.version))
        .expect("get meta");

    assert_eq!(receipt.sha256, expected_hash);
    assert_eq!(meta.sha256, expected_hash);
    assert_eq!(common::sha256_hex(&record.content), expected_hash);
    assert_eq!(record.content, content);
}

// proposal-test-intent-row 4: metadata - format hint
// assumption-register: A9
// named risk: format metadata could be omitted from one read path or normalized unexpectedly
// selected level: particular-integration
#[test]
fn format_hint_persists_unchanged_on_meta_and_list() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-meta", "formatted.md");

    let receipt = store
        .put(PutRequest {
            format_hint: Some("text/markdown".to_string()),
            ..common::put_request(key.clone(), b"# Title\n".to_vec())
        })
        .expect("put");
    let meta = store
        .get_meta(&key, Some(receipt.version))
        .expect("get meta");
    let listed = store
        .list(common::list_filter(
            Some("R-meta"),
            Some("formatted.md"),
            false,
        ))
        .expect("list");

    assert_eq!(meta.format_hint.as_deref(), Some("text/markdown"));
    assert!(!listed.is_empty(), "list returned no formatted.md entries");
    assert_eq!(listed[0].format_hint.as_deref(), Some("text/markdown"));
}

// proposal-test-intent-row 5: metadata - verdict line nullable state
// assumption-register: A9
// named risk: optional verdict metadata could collapse absent and present states or lose text
// selected level: particular-integration
#[test]
fn verdict_line_persists_when_supplied_and_stays_none_when_omitted() {
    let (_db, store) = common::init_temp_store();
    let key_with = common::key("R-meta", "with-verdict.md");
    let key_without = common::key("R-meta", "without-verdict.md");

    let with_receipt = store
        .put(PutRequest {
            verdict_line: Some("APPROVED: durable enough".to_string()),
            ..common::put_request(key_with.clone(), b"with".to_vec())
        })
        .expect("put with verdict");
    let without_receipt = store
        .put(common::put_request(
            key_without.clone(),
            b"without".to_vec(),
        ))
        .expect("put without verdict");

    let with_meta = store
        .get_meta(&key_with, Some(with_receipt.version))
        .expect("get with");
    let without_meta = store
        .get_meta(&key_without, Some(without_receipt.version))
        .expect("get without");

    assert_eq!(
        with_meta.verdict_line.as_deref(),
        Some("APPROVED: durable enough")
    );
    assert_eq!(without_meta.verdict_line, None);
}

// proposal-test-intent-row 6: metadata - predecessor version
// assumption-register: A7
// named risk: version ancestry metadata could be lost or accidentally backfilled onto older rows
// selected level: particular-integration
#[test]
fn predecessor_version_persists_for_v2_and_remains_none_for_v1() {
    let (_db, store) = common::init_temp_store();
    let key = common::key("R-meta", "lineage.md");

    let v1 = store
        .put(common::put_request(key.clone(), b"v1".to_vec()))
        .expect("put v1");
    let v2 = store
        .put(PutRequest {
            predecessor_version: Some(v1.version),
            ..common::put_request(key.clone(), b"v2".to_vec())
        })
        .expect("put v2");

    let v1_meta = store.get_meta(&key, Some(v1.version)).expect("get v1");
    let v2_meta = store.get_meta(&key, Some(v2.version)).expect("get v2");

    assert_eq!(v1_meta.predecessor_version, None);
    assert_eq!(v2_meta.predecessor_version, Some(v1.version));
}
