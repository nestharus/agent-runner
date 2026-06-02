//! Role: mapper.

use super::client_error_formatter::client_error_message;
use super::provider_error::ExternalSessionProviderError;
use super::provider_error_formatter::provider_error_message;
use super::registry_error_formatter::registry_error_message;
use crate::provider_registry::ProviderRegistryError;
use crate::session_export::ExportError;
use crate::session_replace::ReplaceError;
use oulipoly_provider::error::ProviderClientError;

pub(crate) fn export_adapter_error(error: ExternalSessionProviderError) -> ExportError {
    ExportError::Operational {
        message: provider_error_message(&error),
    }
}

pub(crate) fn replace_adapter_error(error: ExternalSessionProviderError) -> ReplaceError {
    ReplaceError::OperationalError {
        message: provider_error_message(&error),
    }
}

pub(crate) fn export_registry_error(error: ProviderRegistryError) -> ExportError {
    ExportError::Operational {
        message: registry_error_message(&error),
    }
}

pub(crate) fn replace_registry_error(error: ProviderRegistryError) -> ReplaceError {
    ReplaceError::OperationalError {
        message: registry_error_message(&error),
    }
}

pub(crate) fn export_client_error(error: ProviderClientError) -> ExportError {
    ExportError::Operational {
        message: client_error_message(error),
    }
}

pub(crate) fn replace_client_error(error: ProviderClientError) -> ReplaceError {
    ReplaceError::OperationalError {
        message: client_error_message(error),
    }
}
