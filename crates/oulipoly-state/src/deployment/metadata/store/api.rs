use crate::deployment::metadata::schema::ensure_coordinator_schema;

use super::error::{MetadataError, metadata_open_error};
use super::filters::active_deployment;
use super::rows::{
    DeploymentId, DeploymentPhase, DeploymentRow, DeploymentSnapshot, HookOutboxRow,
    ImportWatermarkRow, PrimaryPointer, QueueStateRow, RetentionStateRow, build_snapshot,
};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

pub trait DeploymentMetadataStore: Send + Sync {
    fn open(data_root: &Path) -> Result<Box<dyn DeploymentMetadataStore>, MetadataError>
    where
        Self: Sized;

    fn snapshot(&self) -> Result<DeploymentSnapshot, MetadataError>;
    fn read_primary_pointer(&self) -> Result<PrimaryPointer, MetadataError>;
    fn upsert_primary_pointer(&self, pointer: PrimaryPointer) -> Result<(), MetadataError>;
    fn create_deployment(&self, row: DeploymentRow) -> Result<(), MetadataError>;
    fn update_deployment_phase(
        &self,
        id: DeploymentId,
        phase: DeploymentPhase,
    ) -> Result<(), MetadataError>;
    fn list_deployments(&self) -> Result<Vec<DeploymentRow>, MetadataError>;
    fn upsert_queue_state(&self, row: QueueStateRow) -> Result<(), MetadataError>;
    fn upsert_import_watermark(&self, row: ImportWatermarkRow) -> Result<(), MetadataError>;
    fn list_import_watermarks(&self) -> Result<Vec<ImportWatermarkRow>, MetadataError>;
    fn upsert_retention_state(&self, row: RetentionStateRow) -> Result<(), MetadataError>;
    fn enqueue_hook(&self, row: HookOutboxRow) -> Result<(), MetadataError>;
    fn list_hook_outbox(&self) -> Result<Vec<HookOutboxRow>, MetadataError>;
    fn mark_hook_delivered(
        &self,
        hook_id: Uuid,
        delivered_at: DateTime<Utc>,
    ) -> Result<(), MetadataError>;
}

pub struct SqliteDeploymentMetadataStore {
    pub(super) conn: Mutex<Connection>,
    #[allow(dead_code)]
    pub(super) path: PathBuf,
}

impl DeploymentMetadataStore for SqliteDeploymentMetadataStore {
    fn open(data_root: &Path) -> Result<Box<dyn DeploymentMetadataStore>, MetadataError> {
        std::fs::create_dir_all(data_root)?;
        let path = data_root.join("state-deploy.db");
        let mut conn = Connection::open(&path).map_err(metadata_open_error)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        ensure_coordinator_schema(&mut conn)?;
        Ok(Box::new(Self {
            conn: Mutex::new(conn),
            path,
        }))
    }

    fn snapshot(&self) -> Result<DeploymentSnapshot, MetadataError> {
        let primary = self.primary_pointer_or_default()?;
        let deployments = self.list_deployments_query()?;
        let queue_states = self.list_queue_states_query()?;
        let active = active_deployment(&deployments);
        let retention = active
            .as_ref()
            .map(|row| self.read_retention_state_query(row.deployment_id))
            .transpose()?
            .flatten();

        Ok(build_snapshot(primary, active, queue_states, retention))
    }

    fn read_primary_pointer(&self) -> Result<PrimaryPointer, MetadataError> {
        self.read_primary_pointer_query()
    }

    fn upsert_primary_pointer(&self, pointer: PrimaryPointer) -> Result<(), MetadataError> {
        self.upsert_primary_pointer_query(pointer)
    }

    fn create_deployment(&self, row: DeploymentRow) -> Result<(), MetadataError> {
        self.create_deployment_query(row)
    }

    fn update_deployment_phase(
        &self,
        id: DeploymentId,
        phase: DeploymentPhase,
    ) -> Result<(), MetadataError> {
        self.update_deployment_phase_query(id, phase)
    }

    fn list_deployments(&self) -> Result<Vec<DeploymentRow>, MetadataError> {
        self.list_deployments_query()
    }

    fn upsert_queue_state(&self, row: QueueStateRow) -> Result<(), MetadataError> {
        self.upsert_queue_state_query(row)
    }

    fn upsert_import_watermark(&self, row: ImportWatermarkRow) -> Result<(), MetadataError> {
        self.upsert_import_watermark_query(row)
    }

    fn list_import_watermarks(&self) -> Result<Vec<ImportWatermarkRow>, MetadataError> {
        self.list_import_watermarks_query()
    }

    fn upsert_retention_state(&self, row: RetentionStateRow) -> Result<(), MetadataError> {
        self.upsert_retention_state_query(row)
    }

    fn enqueue_hook(&self, row: HookOutboxRow) -> Result<(), MetadataError> {
        self.enqueue_hook_query(row)
    }

    fn list_hook_outbox(&self) -> Result<Vec<HookOutboxRow>, MetadataError> {
        self.list_hook_outbox_query()
    }

    fn mark_hook_delivered(
        &self,
        hook_id: Uuid,
        delivered_at: DateTime<Utc>,
    ) -> Result<(), MetadataError> {
        self.mark_hook_delivered_query(hook_id, delivered_at)
    }
}

impl SqliteDeploymentMetadataStore {
    fn primary_pointer_or_default(&self) -> Result<PrimaryPointer, MetadataError> {
        match self.read_primary_pointer() {
            Ok(pointer) => Ok(pointer),
            Err(MetadataError::NotFound) => Ok(default_primary_pointer()),
            Err(err) => Err(err),
        }
    }
}

fn default_primary_pointer() -> PrimaryPointer {
    PrimaryPointer {
        schema_version: 0,
        deployment_id: None,
        role: crate::deployment::paths::DbRole::Steady,
        updated_at: chrono::DateTime::from_timestamp(0, 0).expect("unix epoch timestamp is valid"),
    }
}
