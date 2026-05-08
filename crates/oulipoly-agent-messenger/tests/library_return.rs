mod common;

use oulipoly_agent_messenger::{ReturnRequest, ReturnSource, return_artifact};
use oulipoly_agent_store::ArtifactKey;
use uuid::Uuid;

// proposal § Test-Intent Track row: inline bytes return
// contract § Expected observable signals row: inline bytes version 1
// named risk: Messenger Domain Layer HIGH - inline bytes could be stored under the wrong namespace or with corrupted content
// selected level: library_integration
#[test]
fn return_artifact_inline_bytes_stores_exact_content_under_return_namespace() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let bytes = common::binary_bytes();

    let receipt = return_artifact(common::inline_request(
        &db,
        invocation,
        "result.bin",
        bytes.clone(),
    ))
    .expect("return inline");

    assert_eq!(receipt.schema_version, 1);
    assert_eq!(
        receipt.store_address.workflow_run_id,
        format!("return:{invocation}")
    );
    assert_eq!(receipt.store_address.artifact_name, "result.bin");
    assert_eq!(receipt.store_address.version, 1);
    assert!(receipt.version_id.ends_with("/1"));
    assert_eq!(receipt.sha256, common::sha256_hex(&bytes));
    assert_eq!(receipt.content_len, bytes.len() as u64);
    assert_eq!(receipt.producer_invocation_uuid, invocation);

    let stored = store
        .get(
            &ArtifactKey {
                workflow_run_id: format!("return:{invocation}"),
                artifact_name: "result.bin".to_string(),
            },
            Some(1),
        )
        .expect("stored returned artifact");
    assert_eq!(stored.content, bytes);
}

// proposal § Test-Intent Track row: scratchpad source copy
// contract § Expected observable signals rows: scratchpad readability, private-prefix leak
// named risk: Messenger Domain Layer HIGH - scratchpad return could expose private backing addresses or move instead of copy
// selected level: library_integration
#[test]
fn return_artifact_scratchpad_source_copies_bytes_and_does_not_leak_private_address() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let bytes = common::text_bytes();
    common::write_scratchpad(
        &db,
        invocation,
        "draft.md",
        bytes.clone(),
        Some("text/markdown"),
        Some("SOURCE: usable"),
    );

    let receipt = return_artifact(common::scratchpad_request(
        &db,
        invocation,
        "proposal.md",
        "draft.md",
    ))
    .expect("return scratchpad");

    assert_eq!(common::read_scratchpad(&db, invocation, "draft.md"), bytes);
    assert_eq!(
        receipt.store_address.workflow_run_id,
        format!("return:{invocation}")
    );
    assert!(receipt.version_id.starts_with("store://return/"));
    common::assert_no_private_scratchpad_leak(&receipt);

    let stored = store
        .get(
            &ArtifactKey {
                workflow_run_id: format!("return:{invocation}"),
                artifact_name: "proposal.md".to_string(),
            },
            Some(1),
        )
        .expect("stored returned artifact");
    assert_eq!(stored.content, bytes);
}

// proposal § Test-Intent Track row: duplicate names append versions
// contract § Expected observable signals row: duplicate names append versions
// named risk: Messenger Domain Layer HIGH - repeated return names could overwrite the first caller-visible artifact
// selected level: library_integration
#[test]
fn duplicate_return_names_append_versions_in_call_order() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();

    let first = return_artifact(common::inline_request(
        &db,
        invocation,
        "proposal.md",
        common::text_bytes(),
    ))
    .expect("first return");
    let second = return_artifact(common::inline_request(
        &db,
        invocation,
        "proposal.md",
        common::replacement_text_bytes(),
    ))
    .expect("second return");

    assert_eq!(first.store_address.version, 1);
    assert_eq!(second.store_address.version, 2);
    assert!(first.version_id.ends_with("/1"));
    assert!(second.version_id.ends_with("/2"));
}

// proposal § Supported-Surface Track required type ReturnName
// contract § Library API contract
// named risk: Messenger Domain Layer HIGH - caller-visible names could collide with internal scratchpad/return namespaces
// selected level: unit
#[test]
fn return_name_rejects_empty_and_reserved_internal_prefixes() {
    for invalid in ["", "scratchpad:secret", "return:already-internal"] {
        assert!(
            matches!(
                oulipoly_agent_messenger::ReturnName::new(invalid),
                Err(oulipoly_agent_messenger::MessengerError::InvalidInput(_))
            ),
            "invalid return name must be rejected: {invalid}"
        );
    }
}

// proposal § Test-Intent Track row: channel append after store write failure
// contract § Operation semantics return_artifact step 6
// named risk: Messenger Receive Transport HIGH - channel append failure could roll back durable storage or report a caller-bound receipt
// selected level: library_integration
#[test]
fn channel_append_failure_after_store_write_leaves_store_version_listable() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let missing_parent = db.output_path("missing-parent").join("channel.jsonl");

    let err = return_artifact(ReturnRequest {
        return_channel: Some(missing_parent),
        source: ReturnSource::InlineBytes(common::text_bytes()),
        ..common::inline_request(&db, invocation, "orphan.md", Vec::new())
    })
    .expect_err("channel append should fail");

    assert!(matches!(
        err,
        oulipoly_agent_messenger::MessengerError::Io(_)
    ));
    store
        .get(
            &ArtifactKey {
                workflow_run_id: format!("return:{invocation}"),
                artifact_name: "orphan.md".to_string(),
            },
            Some(1),
        )
        .expect("orphaned store version remains visible");
}
