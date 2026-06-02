//! Role: mapper.

use super::error_format::format_terminal_classify_error;
use super::errors::TerminalClassifyError;
use crate::provider_registry::ProviderRegistryError;
use crate::services::ServiceError;
use oulipoly_provider::error::ProviderClientError;

pub(crate) fn registry_error(_error: ProviderRegistryError) -> ServiceError {
    classify_error(TerminalClassifyError::registry())
}

pub(crate) fn client_error(_error: ProviderClientError) -> ServiceError {
    classify_error(TerminalClassifyError::provider_client())
}

pub(crate) fn projection_error(_error: serde_json::Error) -> ServiceError {
    classify_error(TerminalClassifyError::projection())
}

pub(crate) fn classify_error(error: TerminalClassifyError) -> ServiceError {
    ServiceError::Dependency {
        message: format_terminal_classify_error(&error),
    }
}
