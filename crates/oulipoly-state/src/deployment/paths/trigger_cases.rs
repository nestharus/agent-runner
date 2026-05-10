use crate::deployment::metadata::store::rows::DeploymentSnapshot;

use super::types::DbRole;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TriggerCase {
    Steady,
    UsePreCutoverPrimary,
    UsePostCutoverPrimary,
    Create,
    Resume,
}

pub(super) fn case_for(snapshot: &DeploymentSnapshot, current_schema_version: u32) -> TriggerCase {
    if is_active_deployment(snapshot) {
        return TriggerCase::Resume;
    }

    match snapshot.primary.role {
        DbRole::PreCutoverPrimary => TriggerCase::UsePreCutoverPrimary,
        DbRole::PostCutoverPrimary => TriggerCase::UsePostCutoverPrimary,
        DbRole::Steady | DbRole::RetentionSecondary => {
            if snapshot.primary.schema_version == current_schema_version {
                TriggerCase::Steady
            } else {
                TriggerCase::Create
            }
        }
    }
}

fn is_active_deployment(snapshot: &DeploymentSnapshot) -> bool {
    snapshot.active_deployment.is_some()
}
