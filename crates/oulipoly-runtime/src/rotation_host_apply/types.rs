//! ## Declared roles
//! mapper

use std::path::PathBuf;

pub struct ChainSegmentMutations {
    pub(super) target_provider_index: usize,
    pub(super) target_session_id: String,
    pub(super) target_jsonl_path: PathBuf,
    pub(super) reason: crate::balancer::TransitionReason,
    pub(super) changed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidatedMutationInputs {
    pub(super) target_provider_index: usize,
    pub(super) target_session_id: String,
    pub(super) target_jsonl_path: PathBuf,
    pub(super) reason: crate::balancer::TransitionReason,
    pub(super) changed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSegmentSnapshot {
    pub chain_id: String,
    pub active_provider: String,
    pub active_session_id: String,
    pub active_started_at: String,
    pub active_ended_at: Option<String>,
    pub active_last_turn_id: Option<String>,
    pub latest_turn_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PlanArtifact {
    pub(super) path: String,
    pub(super) sha256: String,
}
