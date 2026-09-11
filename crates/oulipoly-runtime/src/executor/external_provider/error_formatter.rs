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
        ExternalProviderDispatchError::ProviderTransport {
            category,
            diagnostic,
        } => {
            let base = format!("external provider transport failed: {category}");
            diagnostic.map_or_else(|| base.clone(), |detail| format!("{base}; {detail}"))
        }
        ExternalProviderDispatchError::ProviderProtocol { category } => {
            format!("external provider protocol failed: {category}")
        }
        ExternalProviderDispatchError::ProviderFailure {
            operation,
            code,
            message,
            diagnostics,
        } => format_provider_failure(&operation, &code, &message, &diagnostics),
        ExternalProviderDispatchError::CancellationFallback { reason } => {
            let _ = reason;
            "external provider launch cancelled before final event".to_string()
        }
        ExternalProviderDispatchError::PolicyRejected { diagnostics } => {
            format_policy_rejection(&diagnostics)
        }
    }
}

fn format_provider_failure(
    operation: &str,
    code: &str,
    message: &str,
    diagnostics: &[Diagnostic],
) -> String {
    let base = format!("external provider {operation} failed: {code}: {message}");
    if diagnostics.is_empty() {
        base
    } else {
        format!("{base}: diagnostics: {}", format_diagnostics(diagnostics))
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

// Only fixed labels and typed scalar observations cross this display boundary.
// Free-form descriptions, argv and captured stream bytes remain in typed evidence.
// Hash the *whole* observed wire ID, never parse/strip it or invent an invocation ID.
pub(super) fn format_transport_diagnostic(
    error: &oulipoly_provider::error::ProviderClientError,
) -> String {
    use sha2::{Digest, Sha256};
    let operation = match match error {
        oulipoly_provider::error::ProviderClientError::Transport { subcommand, .. }
        | oulipoly_provider::error::ProviderClientError::Protocol { subcommand, .. } => {
            subcommand.as_str()
        }
        oulipoly_provider::error::ProviderClientError::ProviderCapability(error) => {
            error.subcommand()
        }
    } {
        value @ ("describe"
        | "launch"
        | "policy.evaluate"
        | "terminal.classify"
        | "quota.probe"
        | "auth.refresh"
        | "session.read"
        | "session.capture"
        | "rotation.assess"
        | "rotation.materialize"
        | "discover") => value,
        _ => "redacted",
    };
    let request = error.request_id().map_or_else(
        || "absent".to_string(),
        |id| format!("sha256:{:x}", Sha256::digest(id.as_bytes())),
    );
    let d = error.diagnostics();
    let status = format_transport_status(error.process_status());
    let first = format_first_failure(d.description.as_deref());
    let code = d
        .provider_exit_code
        .map_or_else(|| "absent".to_string(), |code| code.to_string());
    format!(
        "operation={operation}; observed_request_id={request}; first_failure: {first}; collection: status={status} reaped={} force_killed={} nonzero={} exit_code={code} stdin_closed_early={} cancellation_requested={} stdout_bytes={} stdout_discarded={} stderr_bytes={} stderr_discarded={}",
        d.process_was_reaped,
        d.process_was_force_killed,
        d.provider_process_nonzero,
        d.stdin_closed_early,
        d.host_cancellation_requested,
        d.stdout.bytes.len(),
        d.stdout.discarded_len,
        d.stderr.bytes.len(),
        d.stderr.discarded_len
    )
}

fn format_first_failure(description: Option<&str>) -> String {
    let Some(description) = description else {
        return "absent".into();
    };
    // Reject oversized/unrecognized carriers rather than exposing a truncated secret.
    if description.len() > 1024 {
        return "redacted".into();
    }
    let Some(first) = description.strip_prefix("first_failure: ") else {
        return "unavailable".into();
    };
    let first = first.split("; collection: ").next().unwrap_or(first);
    if first == "worker_failure_observed" {
        return first.into();
    }
    let Some((operation, detail)) = first.split_once(": ") else {
        return "redacted".into();
    };
    if !matches!(
        operation,
        "waitid_wnowait"
            | "owned_wait"
            | "owned_try_wait"
            | "cleanup_admission_or_signal"
            | "cleanup_waitid_wnowait"
            | "cleanup_identity"
            | "cleanup_remote_signal"
            | "cleanup_group_kill"
    ) {
        return "redacted".into();
    }
    let errno = match detail.rsplit_once("; errno=") {
        Some((_, "unavailable")) => "unavailable".into(),
        Some((_, value)) => value
            .parse::<i32>()
            .map_or_else(|_| "redacted".into(), |value| value.to_string()),
        None => "absent".into(),
    };
    let custody = match detail.split("; ").next() {
        Some("actor_custody=present") => "; actor_custody=present",
        Some("actor_custody=absent") => "; actor_custody=absent",
        _ => "",
    };
    format!("{operation}; errno={errno}{custody}; detail=redacted")
}

fn format_transport_status(status: Option<&oulipoly_provider::generated::ProcessStatus>) -> String {
    use oulipoly_provider::generated::ProcessStatus;
    match status {
        None => "absent".into(),
        Some(ProcessStatus::Exited { code }) => format!("exited:{code}"),
        Some(ProcessStatus::SignalTerminated { signal }) => format!("signal:{signal}"),
        Some(ProcessStatus::SpawnError { .. }) => "spawn_error:reason_redacted".into(),
        Some(ProcessStatus::ProlongedSilence { .. }) => "prolonged_silence:reason_redacted".into(),
        Some(ProcessStatus::Cancelled) => "cancelled".into(),
        Some(ProcessStatus::Unknown) => "unknown".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_failure_preserves_boundary_errno_and_only_typed_custody() {
        for operation in [
            "cleanup_waitid_wnowait",
            "cleanup_identity",
            "cleanup_remote_signal",
            "cleanup_group_kill",
        ] {
            for custody in ["present", "absent"] {
                assert_eq!(
                    format_first_failure(Some(&format!(
                        "first_failure: {operation}: actor_custody={custody}; errno=3; collection: secret"
                    ))),
                    format!("{operation}; errno=3; actor_custody={custody}; detail=redacted")
                );
            }
        }
        assert_eq!(
            format_first_failure(Some(
                "first_failure: cleanup_group_kill: actor_custody=secret; errno=secret"
            )),
            "cleanup_group_kill; errno=redacted; detail=redacted"
        );
    }

    #[test]
    fn first_failure_projects_only_allowlisted_boundary_and_errno() {
        assert_eq!(format_first_failure(None), "absent");
        assert_eq!(format_first_failure(Some("secret")), "unavailable");
        assert_eq!(
            format_first_failure(Some(
                "first_failure: worker_failure_observed; collection: secret"
            )),
            "worker_failure_observed"
        );
        assert_eq!(
            format_first_failure(Some(
                "first_failure: owned_wait: secret; errno=10; collection: secret"
            )),
            "owned_wait; errno=10; detail=redacted"
        );
        assert_eq!(
            format_first_failure(Some("first_failure: owned_try_wait: secret")),
            "owned_try_wait; errno=absent; detail=redacted"
        );
        assert_eq!(
            format_first_failure(Some(
                "first_failure: cleanup_admission_or_signal: secret; errno=unavailable"
            )),
            "cleanup_admission_or_signal; errno=unavailable; detail=redacted"
        );
        assert_eq!(
            format_first_failure(Some("first_failure: owned_wait: secret; errno=token")),
            "owned_wait; errno=redacted; detail=redacted"
        );
        assert_eq!(
            format_first_failure(Some("first_failure: secret: secret; errno=10")),
            "redacted"
        );
        assert_eq!(format_first_failure(Some(&"x".repeat(1025))), "redacted");
    }
}
