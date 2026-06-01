//! ## Declared roles
//! formatter

use super::ExternalRotationError;

pub fn malformed_external_identity(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::MalformedExternalIdentity {
        reason: reason.into(),
    }
}

pub fn missing_registry_handle() -> ExternalRotationError {
    ExternalRotationError::MissingRegistryHandle
}

pub fn missing_enabled_artifact(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::MissingEnabledArtifact {
        reason: reason.into(),
    }
}

pub fn describe_failure(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::DescribeFailure {
        reason: reason.into(),
    }
}

pub fn capability_missing(capability: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::CapabilityMissing {
        capability: capability.into(),
    }
}

pub fn disabled_artifact(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::DisabledArtifact {
        reason: reason.into(),
    }
}

pub fn provider_transport_failure(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::ProviderTransportFailure {
        reason: reason.into(),
    }
}

pub fn protocol_invalid_response(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::ProtocolInvalidResponse {
        reason: reason.into(),
    }
}

pub fn semantic_host_plan_rejection(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::SemanticHostPlanRejection {
        reason: reason.into(),
    }
}

pub fn artifact_verification_failure(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::ArtifactVerificationFailure {
        reason: reason.into(),
    }
}

pub fn host_apply_conflict(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::HostApplyConflict {
        reason: reason.into(),
    }
}

pub fn journal_recovery_failure(reason: impl Into<String>) -> ExternalRotationError {
    ExternalRotationError::JournalRecoveryFailure {
        reason: reason.into(),
    }
}
