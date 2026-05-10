use super::api::SqliteDeploymentMetadataStore;
use super::error::MetadataError;
use super::formatters::{bool_to_i64, db_role_to_str, deployment_phase_to_str};
use super::rows::{
    DeploymentId, DeploymentPhase, DeploymentRow, HookOutboxRow, ImportWatermarkRow,
    PrimaryPointer, QueueStateRow, RetentionStateRow, deployment_from_row, hook_outbox_from_row,
    import_watermark_from_row, primary_pointer_from_row, queue_state_from_row,
    retention_state_from_row,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

impl SqliteDeploymentMetadataStore {
    pub(super) fn read_primary_pointer_query(&self) -> Result<PrimaryPointer, MetadataError> {
        let conn = self
            .conn
            .lock()
            .expect("deployment metadata mutex poisoned");
        conn.query_row(
            "SELECT schema_version, deployment_id, role, updated_at
             FROM primary_pointer WHERE pointer_id = 1",
            [],
            primary_pointer_from_row,
        )
        .optional()?
        .ok_or(MetadataError::NotFound)
    }

    pub(super) fn upsert_primary_pointer_query(
        &self,
        pointer: PrimaryPointer,
    ) -> Result<(), MetadataError> {
        let deployment_id = pointer
            .deployment_id
            .map(|id| id.0.as_bytes().as_slice().to_vec());
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "INSERT INTO primary_pointer
                    (pointer_id, schema_version, deployment_id, role, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4)
                 ON CONFLICT(pointer_id) DO UPDATE SET
                    schema_version = excluded.schema_version,
                    deployment_id = excluded.deployment_id,
                    role = excluded.role,
                    updated_at = excluded.updated_at",
                params![
                    pointer.schema_version,
                    deployment_id,
                    db_role_to_str(pointer.role),
                    pointer.updated_at.to_rfc3339(),
                ],
            )?;
        Ok(())
    }

    pub(super) fn create_deployment_query(&self, row: DeploymentRow) -> Result<(), MetadataError> {
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "INSERT INTO deployments
                    (deployment_id, from_schema_version, to_schema_version, phase, started_at, updated_at, notes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    row.deployment_id.0.as_bytes().as_slice(),
                    row.from_schema_version,
                    row.to_schema_version,
                    deployment_phase_to_str(row.phase),
                    row.started_at.to_rfc3339(),
                    row.updated_at.to_rfc3339(),
                    row.notes,
                ],
            )?;
        Ok(())
    }

    pub(super) fn update_deployment_phase_query(
        &self,
        id: DeploymentId,
        phase: DeploymentPhase,
    ) -> Result<(), MetadataError> {
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "UPDATE deployments
                 SET phase = ?1, updated_at = ?2
                 WHERE deployment_id = ?3",
                params![
                    deployment_phase_to_str(phase),
                    Utc::now().to_rfc3339(),
                    id.0.as_bytes().as_slice(),
                ],
            )?;
        Ok(())
    }

    pub(super) fn list_deployments_query(&self) -> Result<Vec<DeploymentRow>, MetadataError> {
        let conn = self
            .conn
            .lock()
            .expect("deployment metadata mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT deployment_id, from_schema_version, to_schema_version, phase,
                    started_at, updated_at, notes
             FROM deployments
             ORDER BY started_at, deployment_id",
        )?;
        Ok(stmt
            .query_map([], deployment_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub(super) fn upsert_queue_state_query(&self, row: QueueStateRow) -> Result<(), MetadataError> {
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "INSERT INTO queue_state
                    (deployment_id, direction, activation_state, last_sequence, last_acked_sequence, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(deployment_id) DO UPDATE SET
                    direction = excluded.direction,
                    activation_state = excluded.activation_state,
                    last_sequence = excluded.last_sequence,
                    last_acked_sequence = excluded.last_acked_sequence,
                    updated_at = excluded.updated_at",
                params![
                    row.deployment_id.0.as_bytes().as_slice(),
                    row.direction,
                    row.activation_state,
                    row.last_sequence as i64,
                    row.last_acked_sequence as i64,
                    row.updated_at.to_rfc3339(),
                ],
            )?;
        Ok(())
    }

    pub(super) fn upsert_import_watermark_query(
        &self,
        row: ImportWatermarkRow,
    ) -> Result<(), MetadataError> {
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "INSERT INTO import_watermarks
                    (deployment_id, table_name, last_pk_json, last_seen_row_version, completed_pass, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(deployment_id, table_name) DO UPDATE SET
                    last_pk_json = excluded.last_pk_json,
                    last_seen_row_version = excluded.last_seen_row_version,
                    completed_pass = excluded.completed_pass,
                    updated_at = excluded.updated_at",
                params![
                    row.deployment_id.0.as_bytes().as_slice(),
                    row.table_name,
                    row.last_pk_json,
                    row.last_seen_row_version as i64,
                    bool_to_i64(row.completed_pass),
                    row.updated_at.to_rfc3339(),
                ],
            )?;
        Ok(())
    }

    pub(super) fn list_import_watermarks_query(
        &self,
    ) -> Result<Vec<ImportWatermarkRow>, MetadataError> {
        let conn = self
            .conn
            .lock()
            .expect("deployment metadata mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT deployment_id, table_name, last_pk_json, last_seen_row_version,
                    completed_pass, updated_at
             FROM import_watermarks
             ORDER BY deployment_id, table_name",
        )?;
        Ok(stmt
            .query_map([], import_watermark_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub(super) fn upsert_retention_state_query(
        &self,
        row: RetentionStateRow,
    ) -> Result<(), MetadataError> {
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "INSERT INTO retention_state
                    (deployment_id, retention_started_at, retention_completed_at, reverse_dual_write_active, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(deployment_id) DO UPDATE SET
                    retention_started_at = excluded.retention_started_at,
                    retention_completed_at = excluded.retention_completed_at,
                    reverse_dual_write_active = excluded.reverse_dual_write_active,
                    updated_at = excluded.updated_at",
                params![
                    row.deployment_id.0.as_bytes().as_slice(),
                    row.retention_started_at.map(|value| value.to_rfc3339()),
                    row.retention_completed_at.map(|value| value.to_rfc3339()),
                    bool_to_i64(row.reverse_dual_write_active),
                    row.updated_at.to_rfc3339(),
                ],
            )?;
        Ok(())
    }

    pub(super) fn enqueue_hook_query(&self, row: HookOutboxRow) -> Result<(), MetadataError> {
        let deployment_id = row
            .deployment_id
            .map(|id| id.0.as_bytes().as_slice().to_vec());
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "INSERT INTO hook_outbox
                    (hook_id, deployment_id, hook_kind, payload_json, enqueued_at, delivered_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
                params![
                    row.hook_id.as_bytes().as_slice(),
                    deployment_id,
                    row.hook_kind,
                    row.payload_json,
                    row.enqueued_at.to_rfc3339(),
                ],
            )?;
        Ok(())
    }

    pub(super) fn list_hook_outbox_query(&self) -> Result<Vec<HookOutboxRow>, MetadataError> {
        let conn = self
            .conn
            .lock()
            .expect("deployment metadata mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT hook_id, deployment_id, hook_kind, payload_json, enqueued_at, delivered_at
             FROM hook_outbox
             ORDER BY hook_id",
        )?;
        Ok(stmt
            .query_map([], hook_outbox_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub(super) fn mark_hook_delivered_query(
        &self,
        hook_id: Uuid,
        delivered_at: DateTime<Utc>,
    ) -> Result<(), MetadataError> {
        self.conn
            .lock()
            .expect("deployment metadata mutex poisoned")
            .execute(
                "UPDATE hook_outbox SET delivered_at = ?1 WHERE hook_id = ?2",
                params![delivered_at.to_rfc3339(), hook_id.as_bytes().as_slice()],
            )?;
        Ok(())
    }

    pub(super) fn list_queue_states_query(&self) -> Result<Vec<QueueStateRow>, MetadataError> {
        let conn = self
            .conn
            .lock()
            .expect("deployment metadata mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT deployment_id, direction, activation_state, last_sequence,
                    last_acked_sequence, updated_at
             FROM queue_state
             ORDER BY deployment_id",
        )?;
        Ok(stmt
            .query_map([], queue_state_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub(super) fn read_retention_state_query(
        &self,
        deployment_id: DeploymentId,
    ) -> Result<Option<RetentionStateRow>, MetadataError> {
        let conn = self
            .conn
            .lock()
            .expect("deployment metadata mutex poisoned");
        Ok(conn
            .query_row(
                "SELECT deployment_id, retention_started_at, retention_completed_at,
                    reverse_dual_write_active, updated_at
             FROM retention_state
             WHERE deployment_id = ?1",
                params![deployment_id.0.as_bytes().as_slice()],
                retention_state_from_row,
            )
            .optional()?)
    }
}
