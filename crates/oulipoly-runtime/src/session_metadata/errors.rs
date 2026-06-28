//! ## Declared roles
//! formatter, mapper
//!
//! Metadata error construction and external-error mapping.

use super::MetadataError;
use super::locator::{LocatorError, locator_error_to_stem};
use oulipoly_state::ResumeError;

pub(super) fn invalid_session_id_error(input: &str) -> MetadataError {
    MetadataError::InvalidSessionId {
        input: input.to_string(),
    }
}

pub(super) fn session_not_found_error(input: &str) -> MetadataError {
    MetadataError::SessionNotFound {
        input: input.to_string(),
    }
}

pub(super) fn metadata_db_result<T>(result: Result<T, String>) -> Result<T, MetadataError> {
    result.map_err(operational_error)
}

pub(super) fn map_resume_error(err: ResumeError) -> MetadataError {
    match err {
        ResumeError::InvalidUuid { input } => MetadataError::InvalidSessionId { input },
        ResumeError::InvalidResumeInput { input, .. } => MetadataError::InvalidSessionId { input },
        ResumeError::NoChainFound { input } => MetadataError::SessionNotFound { input },
        ResumeError::WrongIdKind { input, .. } => MetadataError::SessionNotFound { input },
        ResumeError::Ambiguous { input, .. } => MetadataError::AmbiguousSession { input },
        ResumeError::UnknownModel { model_name } => {
            operational_error(unknown_model_message(&model_name))
        }
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            ..
        } => operational_error(provider_model_mismatch_message(
            &model_name,
            &active_provider,
        )),
        ResumeError::ProviderNotConfigured { provider } => MetadataError::UnsupportedStorage {
            provider_name: provider.clone(),
            reason: provider_not_configured_reason(&provider),
        },
        ResumeError::ActiveSegmentMissing { chain_id } => {
            MetadataError::SessionNotFound { input: chain_id }
        }
        ResumeError::ProviderMissingResume { provider_name } => MetadataError::UnsupportedStorage {
            provider_name: provider_name.clone(),
            reason: provider_missing_resume_reason(&provider_name),
        },
        ResumeError::Db { message } => MetadataError::Operational { message },
    }
}

pub(super) fn operational_error(message: String) -> MetadataError {
    MetadataError::Operational { message }
}

pub(super) fn provider_model_mismatch_message(model_name: &str, active_provider: &str) -> String {
    format!("model {model_name} does not include active provider {active_provider}")
}

pub(super) fn provider_resolution_error(provider_name: &str, reason: String) -> MetadataError {
    MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason,
    }
}

pub(super) fn retired_provider_message(provider_name: &str, session_id: &str) -> String {
    format!(
        "session {session_id} was last active on provider {provider_name}, which is no longer \
         configured, and no earlier segment of its chain is resumable; re-add the provider or \
         migrate the chain to a configured provider"
    )
}

pub(super) fn locator_error_to_metadata_error(
    provider_name: &str,
    error: LocatorError,
) -> MetadataError {
    MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason: locator_error_to_stem(error),
    }
}

pub(super) fn unsupported_storage_error(provider_name: &str, reason: String) -> MetadataError {
    MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason,
    }
}

fn unknown_model_message(model_name: &str) -> String {
    format!("unknown model {model_name}")
}

fn provider_not_configured_reason(provider: &str) -> String {
    format!("provider {provider} is not configured")
}

fn provider_missing_resume_reason(provider_name: &str) -> String {
    format!("provider {provider_name} has no resume strategy")
}
