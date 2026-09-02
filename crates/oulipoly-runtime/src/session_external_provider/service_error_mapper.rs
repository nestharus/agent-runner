//! Role: mapper.

use super::client_error_formatter::client_error_message;
use super::provider_error::ExternalSessionProviderError;
use super::provider_error_formatter::provider_error_message;
use super::registry_error_formatter::registry_error_message;
use crate::provider_registry::ProviderRegistryError;
use crate::session_export::ExportError;
use crate::session_replace::ReplaceError;
use oulipoly_provider::error::ProviderClientError;
use std::path::PathBuf;

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
    if let ProviderClientError::ProviderCapability(capability) = &error {
        let provider_error = capability.error();
        if provider_error.code == "malformed_provider_transcript" {
            let details = provider_error.details.as_ref();
            let path = details
                .and_then(|value| value.get("path"))
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default();
            let line = details
                .and_then(|value| value.get("line"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            let reason = details
                .and_then(|value| value.get("reason"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&provider_error.message)
                .to_string();
            return ExportError::MalformedTranscript { path, line, reason };
        }
        if provider_error.code == "unsupported_storage" {
            let provider_name = provider_error
                .details
                .as_ref()
                .and_then(|value| value.get("provider_name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("external-provider")
                .to_string();
            return ExportError::UnsupportedStorage {
                provider_name,
                reason: provider_error.message.clone(),
            };
        }
    }
    ExportError::Operational {
        message: client_error_message(error),
    }
}

pub(crate) fn replace_client_error(error: ProviderClientError) -> ReplaceError {
    if let ProviderClientError::ProviderCapability(capability) = &error {
        let provider_error = capability.error();
        let details = provider_error.details.as_ref();
        match provider_error.code.as_str() {
            "unsupported_storage" => {
                return ReplaceError::UnsupportedStorage {
                    provider_name: details
                        .and_then(|value| value.get("provider_name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("external-provider")
                        .to_string(),
                    reason: provider_error.message.clone(),
                };
            }
            "preimage_sha256_mismatch" => {
                return ReplaceError::PreimageMismatch {
                    expected: details
                        .and_then(|value| value.get("expected"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    actual: details
                        .and_then(|value| value.get("actual"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                };
            }
            "invalid_canonical_input" | "unsupported_canonical_record" => {
                return ReplaceError::InvalidInputTranscript {
                    reason: provider_error.message.clone(),
                    line: details
                        .and_then(|value| value.get("line"))
                        .and_then(serde_json::Value::as_u64),
                };
            }
            _ => {}
        }
    }
    ReplaceError::OperationalError {
        message: client_error_message(error),
    }
}
