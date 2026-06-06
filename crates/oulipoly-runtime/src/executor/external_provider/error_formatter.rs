//! Role: formatter.

use super::errors::ExternalProviderDispatchError;
use oulipoly_provider::generated::Diagnostic;

pub(crate) fn format_external_dispatch_error(error: ExternalProviderDispatchError) -> String {
    match error {
        ExternalProviderDispatchError::MissingCapability { capability } => {
            format!("external provider missing required capability: {capability}")
        }
        ExternalProviderDispatchError::RuntimeDisabledCrate => {
            "external provider artifact is runtime-disabled: runtime_disabled".to_string()
        }
        ExternalProviderDispatchError::ProviderTransport { category } => {
            format!("external provider transport failed: {category}")
        }
        ExternalProviderDispatchError::ProviderProtocol { category } => {
            format!("external provider protocol failed: {category}")
        }
        ExternalProviderDispatchError::CancellationFallback { reason } => {
            let _ = reason;
            "external provider launch cancelled before final event".to_string()
        }
        ExternalProviderDispatchError::PolicyRejected { diagnostics } => {
            format_policy_rejection(&diagnostics)
        }
    }
}

pub(crate) fn format_external_input_validation_error(message: &str) -> String {
    format!("external provider input validation failed: {message}")
}

fn format_policy_rejection(diagnostics: &[Diagnostic]) -> String {
    let base = "external provider policy rejected launch";
    if diagnostics.is_empty() {
        base.to_string()
    } else {
        format!("{base}: diagnostics: {}", format_diagnostics(diagnostics))
    }
}

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(format_diagnostic)
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_diagnostic(diagnostic: &Diagnostic) -> String {
    let mut parts = vec![diagnostic.severity.as_str()];
    if let Some(code) = diagnostic.code.as_deref() {
        parts.push(code);
    }
    if let Some(path) = diagnostic.path.as_deref() {
        parts.push(path);
    }
    format!("{}: {}", parts.join(" "), diagnostic.message)
}
