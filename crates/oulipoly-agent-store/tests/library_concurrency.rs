mod common;

use std::sync::{Arc, Barrier};
use std::thread;

use oulipoly_agent_store::Store;

// proposal-test-intent-row 15: concurrency - serialized version assignment and content integrity
// assumption-register: A2, A7
// named risk: concurrent writers could torn-write content, collide on the same version, or silently lose one writer
// selected level: particular-integration
#[test]
fn concurrent_puts_same_key_create_distinct_versions_and_intact_content() {
    let (db, _) = common::init_temp_store();
    let db_path = db.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(2));
    let key = common::key("R-concurrent", "artifact.bin");

    let worker = |content: Vec<u8>, barrier: Arc<Barrier>| {
        let db_path = db_path.clone();
        let key = key.clone();
        thread::spawn(move || {
            let store = Store::open(&db_path).expect("open store in thread");
            barrier.wait();
            let receipt = store
                .put(common::put_request(key.clone(), content.clone()))
                .expect("thread put");
            (receipt.version, receipt.sha256, content)
        })
    };

    let a = worker(b"thread-a-content".to_vec(), barrier.clone());
    let b = worker(b"thread-b-content".to_vec(), barrier);
    let mut writes = vec![a.join().expect("thread a"), b.join().expect("thread b")];
    writes.sort_by_key(|(version, _, _)| *version);

    assert_eq!(writes[0].0, 1);
    assert_eq!(writes[1].0, 2);
    assert_ne!(writes[0].2, writes[1].2);
    assert_ne!(
        writes[0].1, writes[1].1,
        "receipt sha256 must differ for distinct contents"
    );

    let reader = common::open_store(&db);
    for (version, receipt_hash, content) in writes {
        let record = reader.get(&key, Some(version)).expect("read version");
        let expected_hash = common::sha256_hex(&content);
        assert_eq!(receipt_hash, expected_hash);
        assert_eq!(record.meta.sha256, expected_hash);
        assert_eq!(record.content, content);
    }
}
