mod common;

use chrono::{DateTime, Utc};
use oulipoly_agent_scratchpad::Scratchpad;
use oulipoly_agent_scratchpad::{DeleteSelector, ScratchpadError};
use oulipoly_agent_store::{PutReceipt, Store};
use rusqlite::params;
use uuid::Uuid;

#[derive(Debug, Eq, PartialEq)]
struct CompatibilityObject {
    object_type: String,
    name: String,
    sql: String,
}

struct CompatibilityFixture {
    db: common::TempDb,
    store: Store,
    first: PutReceipt,
    second: PutReceipt,
    other: PutReceipt,
}

fn fixed_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixed timestamp")
        .with_timezone(&Utc)
}

fn setup_compatibility_fixture() -> CompatibilityFixture {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::parse_str("00000000-0000-4000-8000-000000000001")
        .expect("valid fixed invocation UUID");
    let other_invocation = Uuid::parse_str("00000000-0000-4000-8000-000000000002")
        .expect("valid fixed invocation UUID");
    let first = common::put_scratchpad_row(&store, invocation, "history.md", b"v1".to_vec());
    let second = common::put_scratchpad_row(&store, invocation, "history.md", b"v2".to_vec());
    let other =
        common::put_scratchpad_row(&store, other_invocation, "history.md", b"other".to_vec());
    drop(scratchpad);

    CompatibilityFixture {
        db,
        store,
        first,
        second,
        other,
    }
}

fn compatibility_objects(db: &common::TempDb) -> Vec<CompatibilityObject> {
    let connection = rusqlite::Connection::open(db.path()).expect("open compatibility database");
    let mut statement = connection
        .prepare(
            "SELECT type, name, sql FROM sqlite_master \
             WHERE name IN ('artifacts', 'artifacts_update_created_at') ORDER BY name",
        )
        .expect("prepare compatibility object query");
    statement
        .query_map([], |row| {
            Ok(CompatibilityObject {
                object_type: row.get(0)?,
                name: row.get(1)?,
                sql: row.get(2)?,
            })
        })
        .expect("query compatibility objects")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect compatibility objects")
}

fn update_created_at_through_artifacts(
    db: &common::TempDb,
    receipt: &PutReceipt,
    created_at: DateTime<Utc>,
) {
    let connection = rusqlite::Connection::open(db.path()).expect("open compatibility database");
    connection
        .execute(
            "UPDATE artifacts SET created_at = ?1 \
             WHERE workflow_run_id = ?2 AND artifact_name = ?3 AND version = ?4",
            params![
                created_at.to_rfc3339(),
                receipt.key.workflow_run_id,
                receipt.key.artifact_name,
                receipt.version as i64,
            ],
        )
        .expect("route created_at update through artifacts view");
}

// C-GAP-01: the compatibility object names, definitions, repeated-open behavior,
// and three-column update routing are observable parts of Scratchpad::open.
#[test]
fn repeated_open_preserves_named_compatibility_objects_and_update_routing() {
    let fixture = setup_compatibility_fixture();
    let installed = compatibility_objects(&fixture.db);

    drop(Scratchpad::open(fixture.db.path()).expect("second scratchpad open"));
    drop(Scratchpad::open(fixture.db.path()).expect("third scratchpad open"));
    let after_repeated_open = compatibility_objects(&fixture.db);

    let replacement_time = fixed_time("2024-02-03T04:05:06Z");
    update_created_at_through_artifacts(&fixture.db, &fixture.first, replacement_time);
    let first_after = fixture
        .store
        .get_meta(&fixture.first.key, Some(fixture.first.version))
        .expect("first version metadata");
    let second_after = fixture
        .store
        .get_meta(&fixture.second.key, Some(fixture.second.version))
        .expect("second version metadata");
    let other_after = fixture
        .store
        .get_meta(&fixture.other.key, Some(fixture.other.version))
        .expect("other key metadata");

    assert_eq!(installed.len(), 2);
    assert_eq!(installed[0].object_type, "view");
    assert_eq!(installed[0].name, "artifacts");
    assert!(installed[0].sql.contains("SELECT * FROM artifact_versions"));
    assert_eq!(installed[1].object_type, "trigger");
    assert_eq!(installed[1].name, "artifacts_update_created_at");
    assert!(
        installed[1]
            .sql
            .contains("INSTEAD OF UPDATE OF created_at ON artifacts")
    );
    for key_column in ["workflow_run_id", "artifact_name", "version"] {
        assert!(
            installed[1].sql.contains(key_column),
            "trigger must route by {key_column}: {}",
            installed[1].sql
        );
    }
    assert_eq!(after_repeated_open, installed);
    assert_eq!(first_after.created_at, replacement_time);
    assert_eq!(second_after.created_at, fixture.second.created_at);
    assert_eq!(other_after.created_at, fixture.other.created_at);
}

fn setup_later_tombstone_failure() -> (common::TempDb, Store, Scratchpad, Uuid) {
    let (db, store) = common::init_temp_store();
    let scratchpad = common::open_scratchpad(&db);
    let invocation = Uuid::from_u128(3);
    common::put_scratchpad_row(&store, invocation, "history.md", b"v1".to_vec());
    common::put_scratchpad_row(&store, invocation, "history.md", b"v2".to_vec());
    let workflow = common::scratchpad_workflow(invocation);
    let connection = rusqlite::Connection::open(db.path()).expect("open failure fixture");
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER fail_later_tombstone \
             BEFORE UPDATE OF tombstoned_at ON artifact_versions \
             FOR EACH ROW \
             WHEN OLD.workflow_run_id = '{workflow}' \
              AND OLD.artifact_name = 'history.md' \
              AND OLD.version = 2 \
              AND OLD.tombstoned_at IS NULL \
              AND NEW.tombstoned_at IS NOT NULL \
             BEGIN \
                 SELECT RAISE(ABORT, 'forced later tombstone failure'); \
             END;"
        ))
        .expect("install later tombstone failure");
    drop(connection);
    (db, store, scratchpad, invocation)
}

fn assert_forced_database_failure(error: ScratchpadError) {
    assert!(matches!(
        error,
        ScratchpadError::Database(ref source)
            if source.to_string().contains("forced later tombstone failure")
    ));
}

fn assert_only_first_version_tombstoned(store: &Store, invocation: Uuid) {
    let key = common::store_key(&common::scratchpad_workflow(invocation), "history.md");
    assert!(
        store
            .get_meta(&key, Some(1))
            .expect("first version metadata")
            .tombstone
            .is_some()
    );
    assert!(
        store
            .get_meta(&key, Some(2))
            .expect("second version metadata")
            .tombstone
            .is_none()
    );
}

#[test]
fn delete_preserves_earlier_tombstone_when_later_mutation_fails() {
    let (_db, store, scratchpad, invocation) = setup_later_tombstone_failure();

    let error = scratchpad
        .delete(common::delete_request(
            invocation,
            "history.md",
            DeleteSelector::AllVersions,
        ))
        .expect_err("second tombstone must fail");

    assert_forced_database_failure(error);
    assert_only_first_version_tombstoned(&store, invocation);
}

#[test]
fn gc_preserves_earlier_tombstone_when_later_mutation_fails() {
    let (_db, store, scratchpad, invocation) = setup_later_tombstone_failure();

    let error = scratchpad
        .gc(common::gc_invocation_request(invocation, false))
        .expect_err("second tombstone must fail");

    assert_forced_database_failure(error);
    assert_only_first_version_tombstoned(&store, invocation);
}

#[test]
fn open_verifies_schema_before_installing_compatibility_objects() {
    let (db, store) = common::init_temp_store();
    drop(store);
    let connection = rusqlite::Connection::open(db.path()).expect("open incompatible fixture");
    assert_eq!(
        connection
            .execute(
                "UPDATE schema_meta SET value = '2' WHERE key = 'schema_version'",
                [],
            )
            .expect("make schema marker incompatible"),
        1
    );
    drop(connection);
    assert!(compatibility_objects(&db).is_empty());

    let error = match Scratchpad::open(db.path()) {
        Ok(_) => panic!("incompatible schema unexpectedly opened"),
        Err(error) => error,
    };

    assert!(matches!(error, ScratchpadError::IncompatibleSchema));
    assert!(compatibility_objects(&db).is_empty());
}
