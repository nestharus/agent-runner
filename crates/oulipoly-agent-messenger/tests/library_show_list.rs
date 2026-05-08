mod common;

use oulipoly_agent_messenger::{
    ListReturnedRequest, ShowReturnedRequest, list_returned, return_artifact, show_returned,
};
use uuid::Uuid;

// proposal § Test-Intent Track row: list_returned invocation isolation
// contract § Expected observable signals row: list-returned invocation isolation
// named risk: Messenger Domain Layer HIGH - list queries could scan store rows outside the current invocation return namespace
// selected level: library_integration
#[test]
fn list_returned_filters_to_current_invocation_and_orders_by_name_then_version() {
    let (db, _store) = common::init_temp_store();
    let first_invocation = Uuid::new_v4();
    let second_invocation = Uuid::new_v4();

    return_artifact(common::inline_request(
        &db,
        first_invocation,
        "b.md",
        b"one".to_vec(),
    ))
    .expect("first b");
    return_artifact(common::inline_request(
        &db,
        first_invocation,
        "a.md",
        b"two".to_vec(),
    ))
    .expect("first a");
    return_artifact(common::inline_request(
        &db,
        second_invocation,
        "a.md",
        b"other".to_vec(),
    ))
    .expect("second a");

    let listed = list_returned(ListReturnedRequest {
        db_path: db.path().to_path_buf(),
        invocation_uuid: first_invocation,
        name: None,
    })
    .expect("list first invocation");

    let names: Vec<_> = listed
        .iter()
        .map(|item| (item.name.as_str(), item.store_address.version))
        .collect();
    assert_eq!(names, vec![("a.md", 1), ("b.md", 1)]);
    assert!(listed.iter().all(|item| {
        item.store_address.workflow_run_id == format!("return:{first_invocation}")
    }));
}

// proposal § Test-Intent Track row: show by version-id and by name/version
// contract § Expected observable signals row: show --version-id raw bytes
// named risk: Messenger CLI/Library HIGH - show could resolve latest instead of requested version or mutate bytes
// selected level: library_integration
#[test]
fn show_returned_reads_exact_bytes_by_version_id_and_by_name_version() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let bytes = common::binary_bytes();

    let receipt = return_artifact(common::inline_request(
        &db,
        invocation,
        "blob.bin",
        bytes.clone(),
    ))
    .expect("return blob");

    let by_version_id = show_returned(ShowReturnedRequest::VersionId {
        db_path: db.path().to_path_buf(),
        version_id: receipt.version_id.clone(),
    })
    .expect("show by version id");
    let by_name = show_returned(ShowReturnedRequest::Address {
        db_path: db.path().to_path_buf(),
        invocation_uuid: invocation,
        name: common::return_name("blob.bin"),
        version: Some(1),
    })
    .expect("show by name");

    assert_eq!(by_version_id.content, bytes);
    assert_eq!(by_name.content, bytes);
    assert_eq!(by_version_id.meta.version_id, receipt.version_id);
}

// proposal § Test-Intent Track row: show tombstoned/missing
// contract § Expected observable signals row: show missing exits NotFound
// named risk: Messenger Domain Layer HIGH - missing returned artifacts could silently resolve to another version
// selected level: library_integration
#[test]
fn show_missing_return_reports_not_found() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();

    let err = show_returned(ShowReturnedRequest::Address {
        db_path: db.path().to_path_buf(),
        invocation_uuid: invocation,
        name: common::return_name("missing.md"),
        version: Some(1),
    })
    .expect_err("missing return");

    assert!(matches!(
        err,
        oulipoly_agent_messenger::MessengerError::NotFound
    ));
}
