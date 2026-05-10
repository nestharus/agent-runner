use super::rows::{DeploymentPhase, DeploymentRow};

pub(super) fn active_deployment(deployments: &[DeploymentRow]) -> Option<DeploymentRow> {
    deployments.iter().find(is_active).cloned()
}

fn is_active(row: &&DeploymentRow) -> bool {
    !matches!(
        row.phase,
        DeploymentPhase::Completed | DeploymentPhase::Aborted
    )
}
