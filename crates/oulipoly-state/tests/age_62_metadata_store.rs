mod fixtures;

use fixtures::age_62_deployment_fixtures::{
    coordinator_tables_sql, representative_rows, seed_coordinator,
};
use oulipoly_state::deployment::metadata::store::rows::{DeploymentId, DeploymentPhase};
use oulipoly_state::deployment::{
    DeploymentMetadataStore, SqliteDeploymentMetadataStore, ensure_coordinator_schema,
};
use rusqlite::Connection;
use uuid::Uuid;

// component_slug: deployment-metadata-schema
// observable signal: idempotent DDL sets the independent coordinator user_version.
#[test]
fn ensure_coordinator_schema_is_idempotent_and_sets_version_one() {
    let mut conn = Connection::open_in_memory().unwrap();

    ensure_coordinator_schema(&mut conn).unwrap();
    ensure_coordinator_schema(&mut conn).unwrap();

    let user_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 1);

    let tables = coordinator_table_names(&conn);
    assert_eq!(
        tables,
        vec![
            "deployments",
            "primary_pointer",
            "queue_state",
            "import_watermarks",
            "retention_state",
            "hook_outbox",
        ]
    );
    assert!(!tables.contains(&"write_queue".to_string()));
}

// component_slug: deployment-metadata-store
// observable signal: open creates WAL coordinator and all declared row families round-trip.
#[test]
fn sqlite_metadata_store_open_creates_wal_and_round_trips_declared_row_families() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteDeploymentMetadataStore::open(dir.path()).unwrap();
    let coordinator = dir.path().join("state-deploy.db");
    let journal_mode: String = Connection::open(&coordinator)
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

    let (deployment, pointer, queue, watermark, retention, hook) = representative_rows();
    store.create_deployment(deployment.clone()).unwrap();
    store.upsert_primary_pointer(pointer.clone()).unwrap();
    store.upsert_queue_state(queue.clone()).unwrap();
    store.upsert_import_watermark(watermark.clone()).unwrap();
    store.upsert_retention_state(retention.clone()).unwrap();
    store.enqueue_hook(hook.clone()).unwrap();

    assert_eq!(store.read_primary_pointer().unwrap(), pointer);
    assert_eq!(store.list_deployments().unwrap(), vec![deployment]);
    assert_eq!(store.list_import_watermarks().unwrap(), vec![watermark]);
    assert_eq!(store.list_hook_outbox().unwrap(), vec![hook.clone()]);

    let delivered_at = "2026-05-10T12:01:00Z".parse().unwrap();
    store
        .mark_hook_delivered(hook.hook_id, delivered_at)
        .unwrap();
    let mut delivered_hook = hook;
    delivered_hook.delivered_at = Some(delivered_at);
    assert_eq!(store.list_hook_outbox().unwrap(), vec![delivered_hook]);

    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.primary, pointer);
    assert_eq!(snapshot.queue_states, vec![queue]);
    assert_eq!(snapshot.retention, Some(retention));
}

#[test]
fn snapshot_retention_state_belongs_to_active_deployment() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteDeploymentMetadataStore::open(dir.path()).unwrap();
    let (active_deployment, pointer, _, _, active_retention, _) = representative_rows();
    let stale_id = DeploymentId(Uuid::from_u128(0x00000000000000000000000000000001));
    let mut stale_deployment = active_deployment.clone();
    stale_deployment.deployment_id = stale_id;
    stale_deployment.phase = DeploymentPhase::Completed;
    let mut stale_retention = active_retention.clone();
    stale_retention.deployment_id = stale_id;
    stale_retention.reverse_dual_write_active = false;

    store.create_deployment(stale_deployment).unwrap();
    store.upsert_retention_state(stale_retention).unwrap();
    store.create_deployment(active_deployment.clone()).unwrap();
    store.upsert_primary_pointer(pointer).unwrap();
    store
        .upsert_retention_state(active_retention.clone())
        .unwrap();

    let snapshot = store.snapshot().unwrap();

    assert_eq!(snapshot.active_deployment, Some(active_deployment));
    assert_eq!(snapshot.retention, Some(active_retention));
}

// Fixture contract guard: direct seeding keeps the six declared coordinator families available.
#[test]
fn deployment_fixture_schema_declares_all_metadata_families_without_queue_rows() {
    let dir = tempfile::tempdir().unwrap();
    seed_coordinator(dir.path(), None);
    let conn = Connection::open(dir.path().join("state-deploy.db")).unwrap();

    let tables = sorted_coordinator_table_names(&conn);

    assert_eq!(
        tables,
        vec![
            "deployments",
            "hook_outbox",
            "import_watermarks",
            "primary_pointer",
            "queue_state",
            "retention_state",
        ]
    );
    assert!(!coordinator_tables_sql().contains("write_queue"));
}

fn coordinator_table_names(conn: &Connection) -> Vec<String> {
    let mut tables = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY rowid",
        )
        .unwrap();
    tables
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn sorted_coordinator_table_names(conn: &Connection) -> Vec<String> {
    let mut tables = conn
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .unwrap();
    tables
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
