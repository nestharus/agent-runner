//! ## Declared roles
//! mapper, formatter
//!
//! Neutral S7c rotation-domain carriers shared by provider dispatch, host
//! apply, and journal recovery.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalRotationIdentity {
    pub model_name: String,
    pub source_provider: String,
    pub source_session_id: String,
    pub target_provider: String,
    pub provider_instance_id: Option<String>,
    pub settings_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalRotationError {
    #[error("malformed external identity: {reason}")]
    MalformedExternalIdentity { reason: String },
    #[error("external provider registry handle is missing")]
    MissingRegistryHandle,
    #[error("missing enabled provider artifact: {reason}")]
    MissingEnabledArtifact { reason: String },
    #[error("provider describe failed: {reason}")]
    DescribeFailure { reason: String },
    #[error("provider capability is missing: {capability}")]
    CapabilityMissing { capability: String },
    #[error("provider artifact is disabled: {reason}")]
    DisabledArtifact { reason: String },
    #[error("provider transport failed: {reason}")]
    ProviderTransportFailure { reason: String },
    #[error("provider protocol response was invalid: {reason}")]
    ProtocolInvalidResponse { reason: String },
    #[error("host state plan was rejected: {reason}")]
    SemanticHostPlanRejection { reason: String },
    #[error("rotation artifact verification failed: {reason}")]
    ArtifactVerificationFailure { reason: String },
    #[error("host apply conflict: {reason}")]
    HostApplyConflict { reason: String },
    #[error("rotation journal recovery failed: {reason}")]
    JournalRecoveryFailure { reason: String },
}

pub fn semantic_host_plan_rejection(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::SemanticHostPlanRejection {
        reason: reason.into(),
    }
}

pub fn host_apply_conflict(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::HostApplyConflict {
        reason: reason.into(),
    }
}

pub fn artifact_verification_failure(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::ArtifactVerificationFailure {
        reason: reason.into(),
    }
}

pub fn journal_recovery_failure(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::JournalRecoveryFailure {
        reason: reason.into(),
    }
}
