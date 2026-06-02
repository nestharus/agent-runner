//! ## Declared roles
//! mapper

use crate::rotation_domain::ExternalRotationIdentity;
use oulipoly_provider::generated::RotationMaterializeResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationJournalState {
    Absent,
    PendingAfterArtifact,
    PendingDuringApply,
    Quarantine { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationRecoveryPlan {
    Noop,
    RollForward,
    RollBack,
    Quarantine { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationJournalPhase {
    CrashAfterArtifact,
    CrashDuringApply,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationJournalRecord {
    pub schema_version: u32,
    pub phase: RotationJournalPhase,
    pub identity: ExternalRotationIdentity,
    pub preimage: RotationJournalPreimage,
    pub result: RotationMaterializeResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotationJournalPreimage {
    pub chain_id: String,
    pub active_provider: String,
    pub active_session_id: String,
    pub active_started_at: String,
    pub active_ended_at: Option<String>,
    pub active_last_turn_id: Option<String>,
    pub latest_turn_at: Option<String>,
}
