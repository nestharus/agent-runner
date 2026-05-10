use crate::deployment::paths::DbRole;

use super::parsers::{db_role_from_str, deployment_phase_from_str, parse_ts};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct DeploymentId(pub Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DeploymentPhase {
    CreatingB,
    DualWriting,
    Importing,
    Draining,
    CutoverPending,
    CutoverCommitted,
    Retention,
    Completed,
    Aborted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentSnapshot {
    pub primary: PrimaryPointer,
    pub active_deployment: Option<DeploymentRow>,
    pub queue_states: Vec<QueueStateRow>,
    pub retention: Option<RetentionStateRow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimaryPointer {
    pub schema_version: u32,
    pub deployment_id: Option<DeploymentId>,
    pub role: DbRole,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentRow {
    pub deployment_id: DeploymentId,
    pub from_schema_version: u32,
    pub to_schema_version: u32,
    pub phase: DeploymentPhase,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueStateRow {
    pub deployment_id: DeploymentId,
    pub direction: String,
    pub activation_state: String,
    pub last_sequence: u64,
    pub last_acked_sequence: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportWatermarkRow {
    pub deployment_id: DeploymentId,
    pub table_name: String,
    pub last_pk_json: Option<String>,
    pub last_seen_row_version: u64,
    pub completed_pass: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionStateRow {
    pub deployment_id: DeploymentId,
    pub retention_started_at: Option<DateTime<Utc>>,
    pub retention_completed_at: Option<DateTime<Utc>>,
    pub reverse_dual_write_active: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookOutboxRow {
    pub hook_id: Uuid,
    pub deployment_id: Option<DeploymentId>,
    pub hook_kind: String,
    pub payload_json: String,
    pub enqueued_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

pub(super) fn build_snapshot(
    primary: PrimaryPointer,
    active_deployment: Option<DeploymentRow>,
    queue_states: Vec<QueueStateRow>,
    retention: Option<RetentionStateRow>,
) -> DeploymentSnapshot {
    DeploymentSnapshot {
        primary,
        active_deployment,
        queue_states,
        retention,
    }
}

pub(super) fn primary_pointer_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PrimaryPointer> {
    let deployment_id: Option<Vec<u8>> = row.get(1)?;
    Ok(PrimaryPointer {
        schema_version: row.get::<_, u32>(0)?,
        deployment_id: deployment_id
            .as_deref()
            .map(deployment_id_from_blob)
            .transpose()?,
        role: db_role_from_str(&row.get::<_, String>(2)?)?,
        updated_at: parse_ts(&row.get::<_, String>(3)?)?,
    })
}

pub(super) fn deployment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeploymentRow> {
    Ok(DeploymentRow {
        deployment_id: DeploymentId(uuid_from_blob(&row.get::<_, Vec<u8>>(0)?)?),
        from_schema_version: row.get(1)?,
        to_schema_version: row.get(2)?,
        phase: deployment_phase_from_str(&row.get::<_, String>(3)?)?,
        started_at: parse_ts(&row.get::<_, String>(4)?)?,
        updated_at: parse_ts(&row.get::<_, String>(5)?)?,
        notes: row.get(6)?,
    })
}

pub(super) fn queue_state_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueStateRow> {
    Ok(QueueStateRow {
        deployment_id: DeploymentId(uuid_from_blob(&row.get::<_, Vec<u8>>(0)?)?),
        direction: row.get(1)?,
        activation_state: row.get(2)?,
        last_sequence: row.get::<_, i64>(3)? as u64,
        last_acked_sequence: row.get::<_, i64>(4)? as u64,
        updated_at: parse_ts(&row.get::<_, String>(5)?)?,
    })
}

pub(super) fn import_watermark_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ImportWatermarkRow> {
    Ok(ImportWatermarkRow {
        deployment_id: DeploymentId(uuid_from_blob(&row.get::<_, Vec<u8>>(0)?)?),
        table_name: row.get(1)?,
        last_pk_json: row.get(2)?,
        last_seen_row_version: row.get::<_, i64>(3)? as u64,
        completed_pass: row.get::<_, i64>(4)? != 0,
        updated_at: parse_ts(&row.get::<_, String>(5)?)?,
    })
}

pub(super) fn retention_state_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RetentionStateRow> {
    let started_at: Option<String> = row.get(1)?;
    let completed_at: Option<String> = row.get(2)?;
    Ok(RetentionStateRow {
        deployment_id: DeploymentId(uuid_from_blob(&row.get::<_, Vec<u8>>(0)?)?),
        retention_started_at: started_at.as_deref().map(parse_ts).transpose()?,
        retention_completed_at: completed_at.as_deref().map(parse_ts).transpose()?,
        reverse_dual_write_active: row.get::<_, i64>(3)? != 0,
        updated_at: parse_ts(&row.get::<_, String>(4)?)?,
    })
}

pub(super) fn hook_outbox_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HookOutboxRow> {
    let deployment_id: Option<Vec<u8>> = row.get(1)?;
    let delivered_at: Option<String> = row.get(5)?;
    Ok(HookOutboxRow {
        hook_id: uuid_from_blob(&row.get::<_, Vec<u8>>(0)?)?,
        deployment_id: deployment_id
            .as_deref()
            .map(deployment_id_from_blob)
            .transpose()?,
        hook_kind: row.get(2)?,
        payload_json: row.get(3)?,
        enqueued_at: parse_ts(&row.get::<_, String>(4)?)?,
        delivered_at: delivered_at.as_deref().map(parse_ts).transpose()?,
    })
}

fn deployment_id_from_blob(bytes: &[u8]) -> rusqlite::Result<DeploymentId> {
    uuid_from_blob(bytes).map(DeploymentId)
}

fn uuid_from_blob(bytes: &[u8]) -> rusqlite::Result<Uuid> {
    Uuid::from_slice(bytes).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            bytes.len(),
            rusqlite::types::Type::Blob,
            Box::new(err),
        )
    })
}
