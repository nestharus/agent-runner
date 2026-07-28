mod common;

use oulipoly_agent_scratchpad::{DeleteSelector, ScratchpadError};
use uuid::Uuid;

// proposal § Test-Intent Track row 3
// named risk: Scratchpad Domain Layer HIGH - read could transform bytes or project store metadata incorrectly
// selected level: library_integration
#[test]
fn read_latest_and_explicit_versions_return_exact_bytes_and_metadata_projection() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::new_v4();
    let first_bytes = common::binary_bytes();
    let second_bytes = common::replacement_text_bytes();

    let first = scratchpad
        .write({
            let mut req = common::write_request(invocation, "artifact.bin", first_bytes.clone());
            req.format_hint = Some("application/octet-stream".to_string());
            req.verdict_line = Some("v1 verdict".to_string());
            req
        })
        .expect("write first");
    let second = scratchpad
        .write({
            let mut req = common::write_request(invocation, "artifact.bin", second_bytes.clone());
            req.predecessor_version = Some(first.version);
            req
        })
        .expect("write second");

    let latest = scratchpad
        .read(common::read_request(invocation, "artifact.bin", None))
        .expect("latest");
    let explicit = scratchpad
        .read(common::read_request(
            invocation,
            "artifact.bin",
            Some(first.version),
        ))
        .expect("explicit");

    assert_eq!(latest.meta.version, second.version);
    assert_eq!(latest.content, second_bytes);
    assert_eq!(explicit.meta.version, first.version);
    assert_eq!(explicit.content, first_bytes);
    assert_eq!(explicit.meta.invocation_uuid, invocation);
    assert_eq!(explicit.meta.name, common::name("artifact.bin"));
    assert_eq!(
        explicit.meta.format_hint.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(explicit.meta.verdict_line.as_deref(), Some("v1 verdict"));
    assert_eq!(latest.meta.predecessor_version, Some(first.version));
}

// proposal § Test-Intent Track rows 5, 6
// named risk: Scratchpad Domain Layer HIGH - delete/list could expose tombstones by default or hide audit metadata when requested
// selected level: library_integration
#[test]
fn list_is_scoped_ordered_and_include_tombstoned_controls_visibility() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::new_v4();
    let other_invocation = Uuid::new_v4();

    scratchpad
        .write(common::write_request(invocation, "b.md", b"b".to_vec()))
        .expect("b");
    let a1 = scratchpad
        .write(common::write_request(invocation, "a.md", b"a1".to_vec()))
        .expect("a1");
    let a2 = scratchpad
        .write(common::write_request(invocation, "a.md", b"a2".to_vec()))
        .expect("a2");
    scratchpad
        .write(common::write_request(
            other_invocation,
            "a.md",
            b"other".to_vec(),
        ))
        .expect("other");
    scratchpad
        .delete(common::delete_request(
            invocation,
            "a.md",
            DeleteSelector::Version(a2.version),
        ))
        .expect("delete a2");

    let active = scratchpad
        .list(common::list_request(invocation, None, false))
        .expect("active list");
    let active_projection: Vec<_> = active
        .iter()
        .map(|meta| {
            (
                meta.name.as_str().to_string(),
                meta.version,
                meta.tombstone.is_some(),
            )
        })
        .collect();
    assert_eq!(
        active_projection,
        vec![
            ("a.md".to_string(), a1.version, false),
            ("b.md".to_string(), 1, false)
        ]
    );

    let with_tombstones = scratchpad
        .list(common::list_request(invocation, Some("a.md"), true))
        .expect("list with tombstones");
    assert_eq!(with_tombstones.len(), 2);
    assert!(
        with_tombstones
            .iter()
            .any(|meta| meta.version == a2.version && meta.tombstone.is_some())
    );
    assert!(
        with_tombstones
            .iter()
            .all(|meta| meta.invocation_uuid == invocation)
    );
}

// proposal § Test-Intent Track row 5
// contract § Expected observable signals row delete-version-idempotent
// named risk: Scratchpad Domain Layer HIGH - explicit delete retries could be destructive or non-idempotent
// selected level: library_integration
#[test]
fn delete_explicit_version_is_idempotent_and_reports_status_vectors() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::new_v4();
    let written = scratchpad
        .write(common::write_request(
            invocation,
            "retry.md",
            b"retry".to_vec(),
        ))
        .expect("write");

    let first = scratchpad
        .delete(common::delete_request(
            invocation,
            "retry.md",
            DeleteSelector::Version(written.version),
        ))
        .expect("first delete");
    let second = scratchpad
        .delete(common::delete_request(
            invocation,
            "retry.md",
            DeleteSelector::Version(written.version),
        ))
        .expect("second delete");

    assert_eq!(first.tombstoned_versions, vec![written.version]);
    assert!(first.already_tombstoned_versions.is_empty());
    assert!(second.tombstoned_versions.is_empty());
    assert_eq!(second.already_tombstoned_versions, vec![written.version]);
    assert_eq!(first.actor, "agent-scratchpad");
    assert_eq!(first.reason, "scratchpad delete");
}

// proposal § Test-Intent Track row 5
// named risk: Scratchpad Domain Layer HIGH - delete selectors could tombstone wrong version sets
// selected level: library_integration
#[test]
fn delete_latest_and_all_versions_resolve_expected_private_versions() {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::new_v4();
    scratchpad
        .write(common::write_request(
            invocation,
            "history.md",
            b"v1".to_vec(),
        ))
        .expect("v1");
    let v2 = scratchpad
        .write(common::write_request(
            invocation,
            "history.md",
            b"v2".to_vec(),
        ))
        .expect("v2");
    let v3 = scratchpad
        .write(common::write_request(
            invocation,
            "history.md",
            b"v3".to_vec(),
        ))
        .expect("v3");

    let latest = scratchpad
        .delete(common::delete_request(
            invocation,
            "history.md",
            DeleteSelector::Latest,
        ))
        .expect("delete latest");
    assert_eq!(latest.tombstoned_versions, vec![v3.version]);

    let all = scratchpad
        .delete(common::delete_request(
            invocation,
            "history.md",
            DeleteSelector::AllVersions,
        ))
        .expect("delete all");
    assert_eq!(all.tombstoned_versions, vec![1, v2.version]);
    assert!(all.already_tombstoned_versions.is_empty());

    let not_found = scratchpad
        .read(common::read_request(invocation, "history.md", None))
        .expect_err("all versions hidden");
    assert!(matches!(not_found, ScratchpadError::NotFound));
}

#[test]
fn list_returns_first_metadata_decode_error_in_persistence_order() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::from_u128(1);
    let first = common::put_scratchpad_row(&store, invocation, "a-empty", b"first".to_vec());
    common::put_scratchpad_row(
        &store,
        invocation,
        "scratchpad:reserved",
        b"second".to_vec(),
    );
    let connection = rusqlite::Connection::open(db.path()).expect("open metadata fixture");
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .expect("allow malformed metadata fixture");
    connection
        .execute(
            "UPDATE artifact_versions SET artifact_name = '' \
             WHERE workflow_run_id = ?1 AND artifact_name = ?2 AND version = ?3",
            rusqlite::params![
                first.key.workflow_run_id,
                first.key.artifact_name,
                first.version as i64,
            ],
        )
        .expect("make first ordered artifact name malformed");
    drop(connection);

    let error = scratchpad
        .list(common::list_request(invocation, None, false))
        .expect_err("first malformed metadata must fail list");

    assert!(matches!(
        error,
        ScratchpadError::InvalidInput(reason)
            if reason == "scratchpad name must not be empty"
    ));
}
