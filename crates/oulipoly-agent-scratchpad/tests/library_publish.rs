mod common;

use oulipoly_agent_scratchpad::{PublishRequest, ScratchpadError};
use oulipoly_agent_store::StoreError;
use uuid::Uuid;

// proposal § Test-Intent Track rows 7, 8, 19
// contract § Expected observable signals rows publish-source, publish-producer
// named risk: Agent-store Substrate Consumption HIGH - publish could mutate source or lose producer lineage
// selected level: library_integration
#[test]
fn publish_copies_bytes_to_canonical_row_preserves_source_and_sets_lineage() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let source_invocation = Uuid::new_v4();
    let source_bytes = common::binary_bytes();

    let source = scratchpad
        .write({
            let mut req =
                common::write_request(source_invocation, "draft.bin", source_bytes.clone());
            req.format_hint = Some("application/octet-stream".to_string());
            req.verdict_line = Some("PRIVATE: publishable".to_string());
            req
        })
        .expect("write source");

    let receipt = scratchpad
        .publish(common::publish_request(
            source_invocation,
            "draft.bin",
            "canonical-run",
            "artifact.bin",
        ))
        .expect("publish");

    assert_eq!(
        receipt.source,
        common::address(source_invocation, "draft.bin")
    );
    assert_eq!(receipt.source_version, source.version);
    assert_eq!(receipt.source_sha256, common::sha256_hex(&source_bytes));
    assert_eq!(
        receipt.destination,
        common::canonical("canonical-run", "artifact.bin")
    );
    assert_eq!(receipt.destination_version, 1);
    assert_eq!(
        receipt.destination_sha256,
        common::sha256_hex(&source_bytes)
    );
    assert_eq!(receipt.producer_invocation_uuid, source_invocation);
    assert_eq!(
        receipt.format_hint.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(
        receipt.verdict_line.as_deref(),
        Some("PRIVATE: publishable")
    );

    let source_after_publish = scratchpad
        .read(common::read_request(source_invocation, "draft.bin", None))
        .expect("source still readable");
    assert_eq!(source_after_publish.content, source_bytes);

    let canonical = store
        .get(&common::store_key("canonical-run", "artifact.bin"), None)
        .expect("canonical record");
    assert_eq!(canonical.content, source_after_publish.content);
    assert_eq!(
        canonical.meta.producer_invocation_uuid,
        Some(source_invocation)
    );
}

// proposal § Test-Intent Track row 7
// named risk: Agent-store Substrate Consumption HIGH - publish overrides could attach source version as canonical predecessor
// selected level: library_integration
#[test]
fn publish_uses_canonical_predecessor_and_metadata_overrides() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let source_invocation = Uuid::new_v4();
    common::put_canonical_row(&store, "canonical-run", "artifact.md");
    scratchpad
        .write({
            let mut req = common::write_request(source_invocation, "draft.md", b"draft".to_vec());
            req.format_hint = Some("text/markdown".to_string());
            req.verdict_line = Some("SOURCE verdict".to_string());
            req
        })
        .expect("source");

    let mut publish = common::publish_request(
        source_invocation,
        "draft.md",
        "canonical-run",
        "artifact.md",
    );
    publish.format_hint = Some("text/plain".to_string());
    publish.verdict_line = Some("CANONICAL verdict".to_string());
    publish.predecessor_version = Some(1);

    let receipt = scratchpad.publish(publish).expect("publish v2");

    assert_eq!(receipt.destination_version, 2);
    assert_eq!(receipt.predecessor_version, Some(1));
    assert_eq!(receipt.format_hint.as_deref(), Some("text/plain"));
    assert_eq!(receipt.verdict_line.as_deref(), Some("CANONICAL verdict"));

    let canonical = store
        .get_meta(
            &common::store_key("canonical-run", "artifact.md"),
            Some(receipt.destination_version),
        )
        .expect("canonical meta");
    assert_eq!(canonical.predecessor_version, Some(1));
    assert_eq!(canonical.format_hint.as_deref(), Some("text/plain"));
    assert_eq!(canonical.verdict_line.as_deref(), Some("CANONICAL verdict"));
}

// proposal § Test-Intent Track rows 7, 8
// named risk: Scratchpad Domain Layer HIGH - canonical destination could enter the reserved private namespace
// selected level: library_integration
#[test]
fn publish_rejects_reserved_scratchpad_destination_prefix() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let source_invocation = Uuid::new_v4();
    scratchpad
        .write(common::write_request(
            source_invocation,
            "draft.md",
            b"draft".to_vec(),
        ))
        .expect("source");

    let err = scratchpad
        .publish(common::publish_request(
            source_invocation,
            "draft.md",
            "scratchpad:canonical-looking",
            "artifact.md",
        ))
        .expect_err("reserved destination rejected");

    assert!(matches!(err, ScratchpadError::InvalidInput(_)));
}

// proposal § Anti-Scope and Test-Intent Track row 8
// named risk: Scratchpad CLI HIGH - publish could secretly write to the filesystem
// selected level: library_integration
#[test]
fn publish_request_has_no_filesystem_output_path_surface() {
    let request = PublishRequest {
        source: common::address(Uuid::new_v4(), "draft.md"),
        source_version: Some(1),
        destination: common::canonical("canonical-run", "artifact.md"),
        format_hint: None,
        verdict_line: None,
        predecessor_version: None,
    };

    assert_eq!(request.destination.workflow_run_id, "canonical-run");
    assert_eq!(request.destination.artifact_name, "artifact.md");
}

#[test]
fn publish_rejects_reserved_destination_before_missing_source_lookup() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::from_u128(1);

    let error = scratchpad
        .publish(common::publish_request(
            invocation,
            "missing.md",
            "scratchpad:reserved",
            "artifact.md",
        ))
        .expect_err("reserved destination must be validated first");

    assert!(matches!(
        error,
        ScratchpadError::InvalidInput(reason)
            if reason
                == "canonical workflow_run_id must not start with reserved prefix scratchpad:"
    ));
}

#[test]
fn publish_does_not_inherit_source_predecessor() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::from_u128(2);
    scratchpad
        .write({
            let mut request = common::write_request(invocation, "draft.md", b"draft".to_vec());
            request.predecessor_version = Some(41);
            request
        })
        .expect("write source with predecessor");

    let receipt = scratchpad
        .publish(common::publish_request(
            invocation,
            "draft.md",
            "canonical-run",
            "artifact.md",
        ))
        .expect("publish without predecessor override");
    let canonical = store
        .get_meta(
            &common::store_key("canonical-run", "artifact.md"),
            Some(receipt.destination_version),
        )
        .expect("canonical metadata");

    assert_eq!(receipt.predecessor_version, None);
    assert_eq!(canonical.predecessor_version, None);
}

#[test]
fn publish_rejects_explicit_tombstoned_source_version() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::from_u128(3);
    let source = scratchpad
        .write(common::write_request(
            invocation,
            "draft.md",
            b"draft".to_vec(),
        ))
        .expect("write source");
    scratchpad
        .delete(common::delete_request(
            invocation,
            "draft.md",
            oulipoly_agent_scratchpad::DeleteSelector::Version(source.version),
        ))
        .expect("tombstone source");
    let mut request =
        common::publish_request(invocation, "draft.md", "canonical-run", "artifact.md");
    request.source_version = Some(source.version);

    let error = scratchpad
        .publish(request)
        .expect_err("tombstoned source must be inactive");

    assert!(matches!(error, ScratchpadError::NotFound));
    assert!(matches!(
        store.get_meta(&common::store_key("canonical-run", "artifact.md"), None),
        Err(StoreError::NotFound)
    ));
}
