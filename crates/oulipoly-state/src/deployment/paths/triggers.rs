use crate::deployment::metadata::store::rows::DeploymentSnapshot;

use super::trigger_cases::case_for;
use super::trigger_decisions::{DeploymentRoutingDecision, decision_for};

pub fn decide_create_or_resume(
    current_schema_version: u32,
    snapshot: &DeploymentSnapshot,
) -> DeploymentRoutingDecision {
    let case = case_for(snapshot, current_schema_version);
    decision_for(case, snapshot, current_schema_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::metadata::store::rows::{
        DeploymentId, DeploymentPhase, DeploymentRow, DeploymentSnapshot, PrimaryPointer,
    };
    use crate::deployment::paths::DbRole;
    use chrono::Utc;
    use uuid::Uuid;

    fn deployment_id() -> DeploymentId {
        DeploymentId(Uuid::from_u128(0x06200000000000000000000000000001))
    }

    fn snapshot(
        schema_version: u32,
        role: DbRole,
        active_phase: Option<DeploymentPhase>,
    ) -> DeploymentSnapshot {
        let active_deployment = active_phase.map(|phase| DeploymentRow {
            deployment_id: deployment_id(),
            from_schema_version: 5,
            to_schema_version: 6,
            phase,
            started_at: Utc::now(),
            updated_at: Utc::now(),
            notes: None,
        });
        DeploymentSnapshot {
            primary: PrimaryPointer {
                schema_version,
                deployment_id: active_deployment.as_ref().map(|row| row.deployment_id),
                role,
                updated_at: Utc::now(),
            },
            active_deployment,
            queue_states: Vec::new(),
            retention: None,
        }
    }

    // component_slug: deployment-paths-triggers
    #[test]
    fn routing_trigger_decisions_cover_all_declared_variants() {
        let cases = [
            (
                5,
                snapshot(5, DbRole::Steady, None),
                DeploymentRoutingDecision::Steady { schema_version: 5 },
            ),
            (
                5,
                snapshot(5, DbRole::PreCutoverPrimary, None),
                DeploymentRoutingDecision::UsePreCutoverPrimary {
                    deployment_id: deployment_id(),
                    schema_version: 5,
                },
            ),
            (
                6,
                snapshot(6, DbRole::PostCutoverPrimary, None),
                DeploymentRoutingDecision::UsePostCutoverPrimary {
                    deployment_id: deployment_id(),
                    schema_version: 6,
                },
            ),
            (
                6,
                snapshot(5, DbRole::Steady, None),
                DeploymentRoutingDecision::CreateDeployment {
                    from_version: 5,
                    to_version: 6,
                },
            ),
            (
                6,
                snapshot(
                    5,
                    DbRole::PreCutoverPrimary,
                    Some(DeploymentPhase::Importing),
                ),
                DeploymentRoutingDecision::ResumeDeployment {
                    deployment_id: deployment_id(),
                    phase: DeploymentPhase::Importing,
                },
            ),
        ];

        for (current_schema_version, snapshot, expected) in cases {
            assert_eq!(
                decide_create_or_resume(current_schema_version, &snapshot),
                expected
            );
        }
    }
}
