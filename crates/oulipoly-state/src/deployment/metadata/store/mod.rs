mod api;
pub mod error;
mod filters;
mod formatters;
mod parsers;
mod queries;
pub mod rows;

pub use api::{DeploymentMetadataStore, SqliteDeploymentMetadataStore};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::metadata::store::rows::{
        DeploymentId, DeploymentPhase, DeploymentRow, HookOutboxRow, ImportWatermarkRow,
        PrimaryPointer, QueueStateRow, RetentionStateRow,
    };
    use crate::deployment::paths::DbRole;
    use chrono::Utc;
    use rusqlite::Connection;
    use uuid::Uuid;

    fn deployment_id() -> DeploymentId {
        DeploymentId(Uuid::from_u128(0x06200000000000000000000000000002))
    }

    // component_slug: deployment-metadata-store
    #[test]
    fn open_sets_wal_and_round_trips_all_declared_row_families() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteDeploymentMetadataStore::open(dir.path()).unwrap();
        let coordinator = dir.path().join("state-deploy.db");
        let journal_mode: String = Connection::open(&coordinator)
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

        let now = Utc::now();
        let deployment = DeploymentRow {
            deployment_id: deployment_id(),
            from_schema_version: 5,
            to_schema_version: 6,
            phase: DeploymentPhase::Importing,
            started_at: now,
            updated_at: now,
            notes: Some("fixture".to_string()),
        };
        let pointer = PrimaryPointer {
            schema_version: 6,
            deployment_id: Some(deployment_id()),
            role: DbRole::PostCutoverPrimary,
            updated_at: now,
        };
        let queue = QueueStateRow {
            deployment_id: deployment_id(),
            direction: "ForwardToB".to_string(),
            activation_state: "active".to_string(),
            last_sequence: 42,
            last_acked_sequence: 41,
            updated_at: now,
        };
        let watermark = ImportWatermarkRow {
            deployment_id: deployment_id(),
            table_name: "invocations".to_string(),
            last_pk_json: Some(r#"{"id":7}"#.to_string()),
            last_seen_row_version: 9,
            completed_pass: false,
            updated_at: now,
        };
        let retention = RetentionStateRow {
            deployment_id: deployment_id(),
            retention_started_at: Some(now),
            retention_completed_at: None,
            reverse_dual_write_active: true,
            updated_at: now,
        };
        let hook = HookOutboxRow {
            hook_id: Uuid::from_u128(0x06200000000000000000000000000003),
            deployment_id: Some(deployment_id()),
            hook_kind: "backup_retention".to_string(),
            payload_json: r#"{"kind":"fixture"}"#.to_string(),
            enqueued_at: now,
            delivered_at: None,
        };

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

        let delivered_at = Utc::now();
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
}
