use crate::deployment::metadata::store::rows::{DeploymentId, DeploymentPhase, DeploymentSnapshot};
use uuid::Uuid;

use super::trigger_cases::TriggerCase;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeploymentRoutingDecision {
    Steady {
        schema_version: u32,
    },
    UsePreCutoverPrimary {
        deployment_id: DeploymentId,
        schema_version: u32,
    },
    UsePostCutoverPrimary {
        deployment_id: DeploymentId,
        schema_version: u32,
    },
    CreateDeployment {
        from_version: u32,
        to_version: u32,
    },
    ResumeDeployment {
        deployment_id: DeploymentId,
        phase: DeploymentPhase,
    },
}

pub(super) fn decision_for(
    case: TriggerCase,
    snapshot: &DeploymentSnapshot,
    current_schema_version: u32,
) -> DeploymentRoutingDecision {
    match case {
        TriggerCase::Steady => DeploymentRoutingDecision::Steady {
            schema_version: snapshot.primary.schema_version,
        },
        TriggerCase::UsePreCutoverPrimary => DeploymentRoutingDecision::UsePreCutoverPrimary {
            deployment_id: deployment_id_for_decision(snapshot),
            schema_version: snapshot.primary.schema_version,
        },
        TriggerCase::UsePostCutoverPrimary => DeploymentRoutingDecision::UsePostCutoverPrimary {
            deployment_id: deployment_id_for_decision(snapshot),
            schema_version: snapshot.primary.schema_version,
        },
        TriggerCase::Create => DeploymentRoutingDecision::CreateDeployment {
            from_version: snapshot.primary.schema_version,
            to_version: current_schema_version,
        },
        TriggerCase::Resume => {
            let active = snapshot
                .active_deployment
                .as_ref()
                .expect("resume trigger case guarantees a row");
            DeploymentRoutingDecision::ResumeDeployment {
                deployment_id: active.deployment_id,
                phase: active.phase,
            }
        }
    }
}

fn deployment_id_for_decision(snapshot: &DeploymentSnapshot) -> DeploymentId {
    snapshot
        .primary
        .deployment_id
        .or_else(|| {
            snapshot
                .active_deployment
                .as_ref()
                .map(|row| row.deployment_id)
        })
        .unwrap_or_else(|| DeploymentId(Uuid::from_u128(0x06200000000000000000000000000001)))
}
