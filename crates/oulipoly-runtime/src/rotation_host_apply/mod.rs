//! ## Declared roles
//! orchestration
//!
//! ## Adapter declarations
//! adapter_declarations:
//!   - component: s7c-host-state-plan-adapter
//!     role: adapter
//!
//! Host apply intentionally preserves the built-in rotation quirk that
//! `session_chains.last_used_at` is not advanced during materialization.

mod artifact_access;
mod artifact_digest_mapper;
mod artifact_orchestration;
mod artifact_verification;
mod error_formatter;
mod mutation_mapper;
mod plan_mapper;
mod plan_validation;
mod predicates;
mod state_access;
mod transaction_orchestration;
mod types;

use crate::migration::MigratedSegment;
use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
use crate::services::MigrationServiceRequest;
use oulipoly_provider::generated::{Artifact, RotationMaterializeResult};
use serde_json::Value;

pub use types::{ChainSegmentMutations, ChainSegmentSnapshot};

pub fn validate_host_state_plan(
    host_state_plan: &Value,
    result_artifacts: &[Artifact],
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
) -> Result<(), ExternalRotationError> {
    let plan = plan_validation::validate_host_plan_header(host_state_plan, request, identity)?;
    let snapshot = transaction_orchestration::load_chain_segment_snapshot(request)?;
    plan_validation::validate_host_plan_body(plan, &snapshot, result_artifacts, identity)?;
    artifact_orchestration::validate_plan_artifact_files(
        &plan_validation::validate_plan_artifact_list(plan)?,
    )?;
    transaction_orchestration::ensure_no_conflicting_active_segment(
        request,
        identity,
        plan_validation::required_plan_string(plan, "target_session_id")?,
    )
}

pub fn validate_no_change_host_state_plan(
    host_state_plan: &Value,
    result_artifacts: &[Artifact],
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
) -> Result<(), ExternalRotationError> {
    let plan = plan_validation::validate_host_plan_header(host_state_plan, request, identity)?;
    let snapshot = transaction_orchestration::load_chain_segment_snapshot(request)?;
    plan_validation::validate_host_plan_body(plan, &snapshot, result_artifacts, identity)
}

pub fn verify_rotation_artifacts(artifacts: &[Artifact]) -> Result<(), String> {
    artifact_orchestration::verify_rotation_artifacts(artifacts)
}

pub fn load_chain_segment_snapshot(
    request: &MigrationServiceRequest<'_>,
) -> Result<ChainSegmentSnapshot, ExternalRotationError> {
    transaction_orchestration::load_chain_segment_snapshot(request)
}

pub fn compute_chain_segment_mutations(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<ChainSegmentMutations, ExternalRotationError> {
    transaction_orchestration::compute_validated_chain_segment_mutations(request, identity, result)
}

pub fn apply_chain_segment_transaction(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<MigratedSegment, ExternalRotationError> {
    transaction_orchestration::apply_chain_segment_transaction(request, identity, result)
}

pub fn rotation_already_applied(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<Option<MigratedSegment>, ExternalRotationError> {
    transaction_orchestration::rotation_already_applied(request, identity, result)
}

pub fn close_active_segment_returning(
    request: &MigrationServiceRequest<'_>,
    ended_at: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ExternalRotationError> {
    state_access::close_active_segment_returning(request, ended_at)
}

pub fn open_chain_segment(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    mutations: &ChainSegmentMutations,
    started_at: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ExternalRotationError> {
    state_access::open_chain_segment(request, identity, mutations, started_at)
}

pub(super) fn semantic_host_plan_rejection(reason: impl Into<String>) -> ExternalRotationError {
    crate::rotation_domain::semantic_host_plan_rejection(reason)
}

pub(super) fn host_apply_conflict(reason: impl Into<String>) -> ExternalRotationError {
    crate::rotation_domain::host_apply_conflict(reason)
}
