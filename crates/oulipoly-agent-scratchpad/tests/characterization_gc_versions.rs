mod common;

use chrono::{DateTime, TimeDelta, Utc};
use oulipoly_agent_scratchpad::{DeleteReceipt, DeleteSelector, GcRequest, GcSelector, Scratchpad};
use oulipoly_agent_store::PutRequest;
use oulipoly_agent_store::Store;
use uuid::Uuid;

fn fixed_uuid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn fixed_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixed timestamp")
        .with_timezone(&Utc)
}

struct ExpiredGcFixture {
    _db: common::TempDb,
    scratchpad: Scratchpad,
    cutoff: DateTime<Utc>,
    first_invocation: Uuid,
    second_invocation: Uuid,
}

fn setup_expired_gc_fixture() -> ExpiredGcFixture {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let cutoff = fixed_time("2026-02-08T00:00:00Z");
    let first_invocation = fixed_uuid(31);
    let second_invocation = fixed_uuid(32);
    common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        first_invocation,
        "z-boundary.md",
        b"boundary".to_vec(),
        cutoff - TimeDelta::days(7),
    );
    common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        first_invocation,
        "a-old.md",
        b"old".to_vec(),
        cutoff - TimeDelta::days(8),
    );
    common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        first_invocation,
        "m-fresh.md",
        b"fresh".to_vec(),
        cutoff - TimeDelta::days(7) + TimeDelta::nanoseconds(1),
    );
    common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        second_invocation,
        "b-boundary.md",
        b"boundary".to_vec(),
        cutoff - TimeDelta::days(7),
    );
    ExpiredGcFixture {
        _db: db,
        scratchpad,
        cutoff,
        first_invocation,
        second_invocation,
    }
}

fn setup_invocation_gc_fixture() -> (common::TempDb, Store, Scratchpad, Uuid) {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = fixed_uuid(33);
    common::put_scratchpad_row(&store, invocation, "b.md", b"b".to_vec());
    common::put_scratchpad_row(&store, invocation, "a.md", b"a".to_vec());
    (db, store, scratchpad, invocation)
}

struct ActiveVersionFixture {
    _db: common::TempDb,
    store: Store,
    scratchpad: Scratchpad,
    invocation: Uuid,
    first_content: Vec<u8>,
}

fn setup_newest_tombstoned_fixture() -> ActiveVersionFixture {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = fixed_uuid(34);
    let first_content = b"active-v1".to_vec();
    scratchpad
        .write(common::write_request(
            invocation,
            "history.md",
            first_content.clone(),
        ))
        .expect("write v1");
    let second = scratchpad
        .write(common::write_request(
            invocation,
            "history.md",
            b"newest-v2".to_vec(),
        ))
        .expect("write v2");
    scratchpad
        .delete(common::delete_request(
            invocation,
            "history.md",
            DeleteSelector::Version(second.version),
        ))
        .expect("tombstone newest version");
    ActiveVersionFixture {
        _db: db,
        store,
        scratchpad,
        invocation,
        first_content,
    }
}

fn setup_no_active_versions_fixture() -> (common::TempDb, Scratchpad, Uuid) {
    let (db, _store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = fixed_uuid(35);
    scratchpad
        .write(common::write_request(
            invocation,
            "empty.md",
            b"v1".to_vec(),
        ))
        .expect("write v1");
    scratchpad
        .write(common::write_request(
            invocation,
            "empty.md",
            b"v2".to_vec(),
        ))
        .expect("write v2");
    scratchpad
        .delete(common::delete_request(
            invocation,
            "empty.md",
            DeleteSelector::AllVersions,
        ))
        .expect("delete every active version");
    (db, scratchpad, invocation)
}

// C-GAP-08: exact seven-day equality is expired, results retain store order,
// defaults are selector-specific, and evaluated_at honestly brackets the call.
#[test]
fn expired_gc_equality_order_defaults_and_evaluation_time_are_stable() {
    let fixture = setup_expired_gc_fixture();
    let before = Utc::now();

    let report = fixture
        .scratchpad
        .gc(common::gc_expired_request(fixture.cutoff, true))
        .expect("expired GC dry run");
    let after = Utc::now();

    assert_eq!(report.selector, GcSelector::ExpiredBefore(fixture.cutoff));
    assert!(report.dry_run);
    assert_eq!(
        report.tombstoned_rows,
        vec![
            common::address(fixture.first_invocation, "a-old.md"),
            common::address(fixture.first_invocation, "z-boundary.md"),
            common::address(fixture.second_invocation, "b-boundary.md"),
        ]
    );
    assert!(report.already_tombstoned_rows.is_empty());
    assert_eq!(report.actor, "agent-scratchpad-gc");
    assert_eq!(report.reason, "scratchpad gc expired");
    assert!(before <= report.evaluated_at && report.evaluated_at <= after);
}

// C-GAP-08: invocation GC has deterministic address order and preserves both
// its default metadata and caller-supplied custom metadata.
#[test]
fn invocation_gc_order_and_default_custom_metadata_are_stable() {
    let (_db, _store, scratchpad, invocation) = setup_invocation_gc_fixture();

    let default_report = scratchpad
        .gc(common::gc_invocation_request(invocation, true))
        .expect("default invocation GC");
    let custom_report = scratchpad
        .gc(GcRequest {
            selector: GcSelector::Invocation(invocation),
            dry_run: true,
            actor: Some("custom-actor".to_string()),
            reason: Some("custom reason".to_string()),
        })
        .expect("custom invocation GC");

    let expected_rows = vec![
        common::address(invocation, "a.md"),
        common::address(invocation, "b.md"),
    ];
    assert_eq!(default_report.selector, GcSelector::Invocation(invocation));
    assert_eq!(default_report.tombstoned_rows, expected_rows);
    assert!(default_report.already_tombstoned_rows.is_empty());
    assert_eq!(default_report.actor, "agent-scratchpad-gc");
    assert_eq!(default_report.reason, "scratchpad gc invocation");
    assert_eq!(custom_report.tombstoned_rows, expected_rows);
    assert!(custom_report.already_tombstoned_rows.is_empty());
    assert_eq!(custom_report.actor, "custom-actor");
    assert_eq!(custom_report.reason, "custom reason");
}

#[test]
fn nullable_private_lineage_round_trips_through_read_list_and_gc_dry_run() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = fixed_uuid(37);
    let content = b"private row without producer lineage\n".to_vec();
    let receipt = store
        .put(PutRequest {
            key: common::store_key(
                &common::scratchpad_workflow(invocation),
                "nullable-lineage.md",
            ),
            producer_invocation_uuid: None,
            format_hint: None,
            verdict_line: None,
            predecessor_version: None,
            content: content.clone(),
        })
        .expect("put valid private row with nullable lineage");

    let record = scratchpad
        .read(common::read_request(
            invocation,
            "nullable-lineage.md",
            Some(receipt.version),
        ))
        .expect("read nullable-lineage row");
    assert_eq!(record.content, content);
    assert_eq!(record.meta.producer_invocation_uuid, None);

    let listed = scratchpad
        .list(common::list_request(invocation, None, false))
        .expect("list nullable-lineage row");
    assert_eq!(listed, vec![record.meta.clone()]);
    assert_eq!(listed[0].producer_invocation_uuid, None);

    let report = scratchpad
        .gc(common::gc_invocation_request(invocation, true))
        .expect("dry-run GC nullable-lineage row");
    assert!(report.dry_run);
    assert_eq!(
        report.tombstoned_rows,
        vec![common::address(invocation, "nullable-lineage.md")]
    );
    assert!(report.already_tombstoned_rows.is_empty());

    let after = scratchpad
        .read(common::read_request(
            invocation,
            "nullable-lineage.md",
            Some(receipt.version),
        ))
        .expect("read row after dry-run GC");
    assert_eq!(after, record);
    let stored = store
        .get_meta(&receipt.key, Some(receipt.version))
        .expect("store metadata after dry-run GC");
    assert_eq!(stored.producer_invocation_uuid, None);
    assert!(stored.tombstone.is_none());
}

// C-GAP-11: latest read, publish, and delete all fall back to the newest
// active version when the numerically newest version is tombstoned.
#[test]
fn latest_operations_fall_back_after_newest_version_is_tombstoned() {
    let fixture = setup_newest_tombstoned_fixture();

    let read = fixture
        .scratchpad
        .read(common::read_request(fixture.invocation, "history.md", None))
        .expect("read latest active version");
    let publish = fixture
        .scratchpad
        .publish(common::publish_request(
            fixture.invocation,
            "history.md",
            "canonical-run",
            "published.md",
        ))
        .expect("publish latest active version");
    let delete = fixture
        .scratchpad
        .delete(common::delete_request(
            fixture.invocation,
            "history.md",
            DeleteSelector::Latest,
        ))
        .expect("delete latest active version");

    assert_eq!(read.meta.version, 1);
    assert_eq!(read.content, fixture.first_content);
    assert_eq!(publish.source_version, 1);
    assert_eq!(
        publish.source_sha256,
        common::sha256_hex(&fixture.first_content)
    );
    let canonical = fixture
        .store
        .get(
            &common::store_key("canonical-run", "published.md"),
            Some(publish.destination_version),
        )
        .expect("published canonical row");
    assert_eq!(canonical.content, fixture.first_content);
    assert_eq!(delete.selector, DeleteSelector::Latest);
    assert_eq!(delete.tombstoned_versions, vec![1]);
    assert!(delete.already_tombstoned_versions.is_empty());
}

// C-GAP-11: delete-all remains a successful empty receipt when no active
// versions remain, including the current null tombstone timestamp.
#[test]
fn delete_all_with_no_active_versions_returns_empty_success_receipt() {
    let (_db, scratchpad, invocation) = setup_no_active_versions_fixture();

    let receipt = scratchpad
        .delete(common::delete_request(
            invocation,
            "empty.md",
            DeleteSelector::AllVersions,
        ))
        .expect("empty delete-all succeeds");

    assert_eq!(
        receipt,
        DeleteReceipt {
            address: common::address(invocation, "empty.md"),
            selector: DeleteSelector::AllVersions,
            tombstoned_versions: Vec::new(),
            already_tombstoned_versions: Vec::new(),
            actor: "agent-scratchpad".to_string(),
            reason: "scratchpad delete".to_string(),
            tombstoned_at: None,
        }
    );
}

#[test]
fn gc_selectors_skip_preexisting_tombstones() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = fixed_uuid(36);
    let cutoff = fixed_time("2026-02-08T00:00:00Z");
    common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        invocation,
        "a-active.md",
        b"active".to_vec(),
        cutoff - TimeDelta::days(8),
    );
    let tombstoned = common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        invocation,
        "b-tombstoned.md",
        b"tombstoned".to_vec(),
        cutoff - TimeDelta::days(8),
    );
    store
        .tombstone(&tombstoned.key, tombstoned.version, "tester", "seed")
        .expect("seed preexisting tombstone");

    let invocation_report = scratchpad
        .gc(common::gc_invocation_request(invocation, true))
        .expect("invocation GC dry run");
    let expired_report = scratchpad
        .gc(common::gc_expired_request(cutoff, true))
        .expect("expired GC dry run");
    let expected = vec![common::address(invocation, "a-active.md")];

    assert_eq!(invocation_report.tombstoned_rows, expected);
    assert!(invocation_report.already_tombstoned_rows.is_empty());
    assert_eq!(expired_report.tombstoned_rows, expected);
    assert!(expired_report.already_tombstoned_rows.is_empty());
}

#[test]
fn expired_gc_filters_before_decode_and_decodes_all_candidates_before_mutation() {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = fixed_uuid(1);
    let cutoff = fixed_time("2026-02-08T00:00:00Z");
    let expired_at = cutoff - TimeDelta::days(8);
    let fresh_at = cutoff - TimeDelta::days(6);
    let valid = common::put_scratchpad_row_with_created_at(
        &db,
        &store,
        invocation,
        "a-valid.md",
        b"valid".to_vec(),
        expired_at,
    );
    let malformed = store
        .put(PutRequest {
            key: common::store_key("scratchpad:not-a-uuid", "z-malformed.md"),
            producer_invocation_uuid: None,
            format_hint: None,
            verdict_line: None,
            predecessor_version: None,
            content: b"malformed".to_vec(),
        })
        .expect("put store-valid malformed scratchpad row");
    let connection = rusqlite::Connection::open(db.path()).expect("open malformed-row fixture");
    connection
        .execute(
            "UPDATE artifact_versions SET created_at = ?1 \
             WHERE workflow_run_id = ?2 AND artifact_name = ?3 AND version = ?4",
            rusqlite::params![
                fresh_at.to_rfc3339(),
                malformed.key.workflow_run_id,
                malformed.key.artifact_name,
                malformed.version as i64,
            ],
        )
        .expect("make malformed row fresh");

    let fresh_report = scratchpad
        .gc(common::gc_expired_request(cutoff, true))
        .expect("fresh malformed row is filtered before decode");
    assert_eq!(
        fresh_report.tombstoned_rows,
        vec![common::address(invocation, "a-valid.md")]
    );

    connection
        .execute(
            "UPDATE artifact_versions SET created_at = ?1 \
             WHERE workflow_run_id = ?2 AND artifact_name = ?3 AND version = ?4",
            rusqlite::params![
                expired_at.to_rfc3339(),
                malformed.key.workflow_run_id,
                malformed.key.artifact_name,
                malformed.version as i64,
            ],
        )
        .expect("make malformed row expired");
    drop(connection);

    let error = scratchpad
        .gc(common::gc_expired_request(cutoff, false))
        .expect_err("expired malformed row must fail decode");
    assert!(matches!(
        error,
        oulipoly_agent_scratchpad::ScratchpadError::MetadataDecode(reason)
            if reason.starts_with(
                "workflow_run_id \"scratchpad:not-a-uuid\" has invalid scratchpad UUID:"
            )
    ));
    assert!(
        store
            .get_meta(&valid.key, Some(valid.version))
            .expect("valid candidate metadata")
            .tombstone
            .is_none(),
        "complete conversion must finish before the first mutation"
    );
    assert!(
        store
            .get_meta(&malformed.key, Some(malformed.version))
            .expect("malformed candidate metadata")
            .tombstone
            .is_none()
    );
}
