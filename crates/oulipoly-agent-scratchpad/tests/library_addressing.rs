mod common;

use oulipoly_agent_scratchpad::{DeleteSelector, ScratchpadError};
use uuid::Uuid;

// proposal § Test-Intent Track rows 1, 2, 3, 4, 5, 6
// contract § Expected observable signals rows write-version, isolation
// named risk: Scratchpad Domain Layer HIGH - private address derivation can leak or corrupt rows
// selected level: library_integration
#[test]
fn write_maps_private_address_and_versions_with_producer_uuid() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::new_v4();

    let first = scratchpad
        .write(common::write_request(
            invocation,
            "notes.md",
            common::text_bytes(),
        ))
        .expect("write v1");
    let second = scratchpad
        .write(common::write_request(
            invocation,
            "notes.md",
            common::replacement_text_bytes(),
        ))
        .expect("write v2");

    assert_eq!(first.version, 1);
    assert_eq!(second.version, 2);
    assert_eq!(first.address, common::address(invocation, "notes.md"));
    assert_eq!(second.address, common::address(invocation, "notes.md"));
    assert_eq!(first.producer_invocation_uuid, Some(invocation));

    let backing_key = common::store_key(&common::scratchpad_workflow(invocation), "notes.md");
    let backing_meta = store
        .get_meta(&backing_key, Some(first.version))
        .expect("backing metadata");
    assert_eq!(backing_meta.producer_invocation_uuid, Some(invocation));
    assert_eq!(
        backing_meta.key.workflow_run_id,
        common::scratchpad_workflow(invocation)
    );
    let backing_meta_v2 = store
        .get_meta(&backing_key, Some(second.version))
        .expect("backing metadata v2");
    assert_eq!(backing_meta_v2.producer_invocation_uuid, Some(invocation));
    assert_eq!(
        backing_meta_v2.key.workflow_run_id,
        common::scratchpad_workflow(invocation)
    );
}

// proposal § Test-Intent Track rows 2, 4
// contract § Expected observable signals row "Two invocation UUIDs are isolated"
// named risk: Scratchpad Domain Layer HIGH - artifact names alone could cross invocation scopes
// selected level: library_integration
#[test]
fn two_invocations_can_reuse_names_without_list_or_read_leakage() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let first_invocation = Uuid::new_v4();
    let second_invocation = Uuid::new_v4();

    scratchpad
        .write(common::write_request(
            first_invocation,
            "notes.md",
            b"first private".to_vec(),
        ))
        .expect("write first");
    scratchpad
        .write(common::write_request(
            second_invocation,
            "notes.md",
            b"second private".to_vec(),
        ))
        .expect("write second");

    let first_list = scratchpad
        .list(common::list_request(first_invocation, None, false))
        .expect("list first");
    let second_list = scratchpad
        .list(common::list_request(second_invocation, None, false))
        .expect("list second");

    assert_eq!(first_list.len(), 1);
    assert_eq!(second_list.len(), 1);
    assert_eq!(first_list[0].invocation_uuid, first_invocation);
    assert_eq!(second_list[0].invocation_uuid, second_invocation);

    let first_read = scratchpad
        .read(common::read_request(first_invocation, "notes.md", None))
        .expect("read first");
    let second_read = scratchpad
        .read(common::read_request(second_invocation, "notes.md", None))
        .expect("read second");
    assert_eq!(first_read.content, b"first private".to_vec());
    assert_eq!(second_read.content, b"second private".to_vec());
}

// proposal § Test-Intent Track row 4
// named risk: Scratchpad Domain Layer HIGH - delete by name could tombstone another invocation's row
// selected level: library_integration
#[test]
fn cross_scope_read_delete_and_publish_report_not_found() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let owner = Uuid::new_v4();
    let stranger = Uuid::new_v4();

    scratchpad
        .write(common::write_request(owner, "draft.md", b"owner".to_vec()))
        .expect("write owner");

    let read_err = scratchpad
        .read(common::read_request(stranger, "draft.md", None))
        .expect_err("cross-scope read");
    assert!(matches!(read_err, ScratchpadError::NotFound));

    let delete_err = scratchpad
        .delete(common::delete_request(
            stranger,
            "draft.md",
            DeleteSelector::Latest,
        ))
        .expect_err("cross-scope delete");
    assert!(matches!(delete_err, ScratchpadError::NotFound));

    let publish_err = scratchpad
        .publish(common::publish_request(
            stranger,
            "draft.md",
            "canonical-run",
            "published.md",
        ))
        .expect_err("cross-scope publish");
    assert!(matches!(publish_err, ScratchpadError::NotFound));
}

// proposal § Assumption Register A9
// named risk: Scratchpad Domain Layer HIGH - caller names could collide with the reserved internal prefix
// selected level: unit
#[test]
fn scratchpad_name_rejects_empty_and_reserved_internal_prefix() {
    for invalid in ["", "scratchpad:", "scratchpad:abc"] {
        let error = common::name_result(invalid).expect_err("invalid name rejected");
        assert!(matches!(error, ScratchpadError::InvalidInput(_)));
    }
}
