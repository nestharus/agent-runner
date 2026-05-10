use crate::deployment::paths::DbRole;

use super::rows::DeploymentPhase;

pub(super) fn db_role_to_str(role: DbRole) -> &'static str {
    match role {
        DbRole::Steady => "Steady",
        DbRole::PreCutoverPrimary => "PreCutoverPrimary",
        DbRole::PostCutoverPrimary => "PostCutoverPrimary",
        DbRole::RetentionSecondary => "RetentionSecondary",
    }
}

pub(super) fn deployment_phase_to_str(phase: DeploymentPhase) -> &'static str {
    match phase {
        DeploymentPhase::CreatingB => "creating_b",
        DeploymentPhase::DualWriting => "dual_writing",
        DeploymentPhase::Importing => "importing",
        DeploymentPhase::Draining => "draining",
        DeploymentPhase::CutoverPending => "cutover_pending",
        DeploymentPhase::CutoverCommitted => "cutover_committed",
        DeploymentPhase::Retention => "retention",
        DeploymentPhase::Completed => "completed",
        DeploymentPhase::Aborted => "aborted",
    }
}

pub(super) fn bool_to_i64(value: bool) -> i64 {
    i64::from(value)
}
