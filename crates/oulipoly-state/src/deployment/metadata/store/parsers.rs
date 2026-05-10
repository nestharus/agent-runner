use crate::deployment::paths::DbRole;

use super::rows::DeploymentPhase;
use chrono::{DateTime, Utc};
use std::io::{Error, ErrorKind};

pub(super) fn parse_ts(value: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })
}

pub(super) fn db_role_from_str(value: &str) -> rusqlite::Result<DbRole> {
    match value {
        "Steady" | "steady" => Ok(DbRole::Steady),
        "PreCutoverPrimary" | "pre_cutover_primary" => Ok(DbRole::PreCutoverPrimary),
        "PostCutoverPrimary" | "post_cutover_primary" => Ok(DbRole::PostCutoverPrimary),
        "RetentionSecondary" | "retention_secondary" => Ok(DbRole::RetentionSecondary),
        _ => Err(unknown_text_value("db role", value)),
    }
}

pub(super) fn deployment_phase_from_str(value: &str) -> rusqlite::Result<DeploymentPhase> {
    match value {
        "CreatingB" | "creating_b" => Ok(DeploymentPhase::CreatingB),
        "DualWriting" | "dual_writing" => Ok(DeploymentPhase::DualWriting),
        "Importing" | "importing" => Ok(DeploymentPhase::Importing),
        "Draining" | "draining" => Ok(DeploymentPhase::Draining),
        "CutoverPending" | "cutover_pending" => Ok(DeploymentPhase::CutoverPending),
        "CutoverCommitted" | "cutover_committed" => Ok(DeploymentPhase::CutoverCommitted),
        "Retention" | "retention" => Ok(DeploymentPhase::Retention),
        "Completed" | "completed" => Ok(DeploymentPhase::Completed),
        "Aborted" | "aborted" => Ok(DeploymentPhase::Aborted),
        _ => Err(unknown_text_value("deployment phase", value)),
    }
}

fn unknown_text_value(field: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(Error::new(
            ErrorKind::InvalidData,
            format!("unknown {field}: {value}"),
        )),
    )
}
