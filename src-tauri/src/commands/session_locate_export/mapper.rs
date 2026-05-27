//! Declared roles: mapper, formatter, validator

use oulipoly_config::{ModelConfig, ProvidersConfig};
use oulipoly_runtime::services::SessionExportServiceRequest;
use oulipoly_runtime::session_export::ExportError;
use oulipoly_runtime::session_metadata::MetadataError;
use oulipoly_state::StateDb;
use std::collections::HashMap;

pub(crate) fn metadata_error_exit_code(err: &MetadataError) -> i32 {
    match err {
        MetadataError::InvalidSessionId { .. } => 2,
        MetadataError::SessionNotFound { .. } => 10,
        MetadataError::AmbiguousSession { .. } => 11,
        MetadataError::UnsupportedStorage { .. } => 12,
        MetadataError::Operational { .. } => 1,
    }
}

pub(super) fn metadata_error_code(err: &MetadataError) -> &'static str {
    match err {
        MetadataError::InvalidSessionId { .. } => "invalid-session-id",
        MetadataError::SessionNotFound { .. } => "session-not-found",
        MetadataError::AmbiguousSession { .. } => "ambiguous-session",
        MetadataError::UnsupportedStorage { .. } => "unsupported-storage",
        MetadataError::Operational { .. } => "operational-error",
    }
}

pub(crate) fn metadata_error_message(err: &MetadataError) -> String {
    match err {
        MetadataError::InvalidSessionId { input } => {
            format!("invalid session id: {input}")
        }
        MetadataError::SessionNotFound { input } => {
            format!("session not found: {input}")
        }
        MetadataError::AmbiguousSession { input } => {
            format!("ambiguous session: {input}")
        }
        MetadataError::UnsupportedStorage {
            provider_name,
            reason,
        } => format!("unsupported storage for provider {provider_name}: {reason}"),
        MetadataError::Operational { message } => message.clone(),
    }
}

pub(super) fn operational_metadata_error(message: String) -> MetadataError {
    MetadataError::Operational { message }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LocateSessionIdRejection {
    InvalidSessionId { input: String },
}

pub(super) fn invalid_locate_session_id_rejection(input: &str) -> LocateSessionIdRejection {
    LocateSessionIdRejection::InvalidSessionId {
        input: input.to_string(),
    }
}

pub(super) fn locate_session_id_rejection_exit_code(rejection: &LocateSessionIdRejection) -> i32 {
    match rejection {
        LocateSessionIdRejection::InvalidSessionId { .. } => 2,
    }
}

pub(super) fn locate_session_id_rejection_metadata_error(
    rejection: &LocateSessionIdRejection,
) -> MetadataError {
    match rejection {
        LocateSessionIdRejection::InvalidSessionId { input } => MetadataError::InvalidSessionId {
            input: input.clone(),
        },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SessionExportArgsRejection {
    InvalidFormat { format: String },
    InvalidSessionId { input: String },
}

pub(super) fn invalid_session_export_format_rejection(format: &str) -> SessionExportArgsRejection {
    SessionExportArgsRejection::InvalidFormat {
        format: format.to_string(),
    }
}

pub(super) fn invalid_session_export_id_rejection(input: &str) -> SessionExportArgsRejection {
    SessionExportArgsRejection::InvalidSessionId {
        input: input.to_string(),
    }
}

pub(super) fn session_export_args_rejection_exit_code(
    rejection: &SessionExportArgsRejection,
) -> i32 {
    match rejection {
        SessionExportArgsRejection::InvalidFormat { .. } => 2,
        SessionExportArgsRejection::InvalidSessionId { input } => {
            export_error_exit_code(&ExportError::InvalidSessionId {
                input: input.clone(),
            })
        }
    }
}

pub(super) fn session_export_args_rejection_export_error(
    rejection: &SessionExportArgsRejection,
) -> Option<ExportError> {
    match rejection {
        SessionExportArgsRejection::InvalidFormat { .. } => None,
        SessionExportArgsRejection::InvalidSessionId { input } => {
            Some(ExportError::InvalidSessionId {
                input: input.clone(),
            })
        }
    }
}

pub(super) enum ExportOutputOutcome {
    Output(Vec<u8>),
    Error(ExportError),
}

pub(super) fn export_output_outcome(result: Result<Vec<u8>, ExportError>) -> ExportOutputOutcome {
    match result {
        Ok(output) => ExportOutputOutcome::Output(output),
        Err(err) => ExportOutputOutcome::Error(err),
    }
}

pub(super) fn export_error_exit_code(err: &ExportError) -> i32 {
    match err {
        ExportError::InvalidSessionId { .. } => 2,
        ExportError::SessionNotFound { .. } => 10,
        ExportError::AmbiguousSession { .. } => 11,
        ExportError::UnsupportedStorage { .. } => 12,
        ExportError::MalformedTranscript { .. } => 15,
        ExportError::Operational { .. } => 1,
    }
}

pub(super) fn export_error_code(err: &ExportError) -> &'static str {
    match err {
        ExportError::InvalidSessionId { .. } => "invalid-session-id",
        ExportError::SessionNotFound { .. } => "session-not-found",
        ExportError::AmbiguousSession { .. } => "ambiguous-session",
        ExportError::UnsupportedStorage { .. } => "unsupported-storage",
        ExportError::MalformedTranscript { .. } => "malformed-provider-transcript",
        ExportError::Operational { .. } => "operational-error",
    }
}

pub(super) fn export_error_message(err: &ExportError) -> String {
    match err {
        ExportError::InvalidSessionId { input } => format!("invalid session UUID: {input}"),
        ExportError::SessionNotFound { input } => format!("session not found: {input}"),
        ExportError::AmbiguousSession { input } => {
            format!("session id matches multiple recent chains: {input}")
        }
        ExportError::UnsupportedStorage {
            provider_name,
            reason,
        } => {
            format!("unsupported storage for provider {provider_name}: {reason}")
        }
        ExportError::MalformedTranscript { path, line, reason } => {
            if *line == 0 {
                format!("malformed transcript {}: {reason}", path.display())
            } else {
                format!(
                    "malformed transcript {} line {line}: {reason}",
                    path.display()
                )
            }
        }
        ExportError::Operational { message } => message.clone(),
    }
}

pub(super) fn session_export_service_request(session_id: &str) -> SessionExportServiceRequest {
    SessionExportServiceRequest {
        session_id: session_id.to_string(),
    }
}

pub(super) fn export_write_error_exit_code() -> i32 {
    1
}

pub(super) fn export_success_exit_code() -> i32 {
    0
}

pub(super) fn session_locate_environment(
    state: StateDb,
    providers_cfg: ProvidersConfig,
    models: HashMap<String, ModelConfig>,
    sessions_cfg: oulipoly_config::SessionsConfig,
) -> super::orchestration::SessionLocateEnvironment {
    super::orchestration::SessionLocateEnvironment {
        state,
        providers_cfg,
        models,
        sessions_cfg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_runtime::session_export::ExportError;
    use oulipoly_runtime::session_metadata::MetadataError;
    use std::path::PathBuf;

    #[test]
    fn metadata_error_exit_code_maps_all_variants() {
        let cases = [
            (
                MetadataError::InvalidSessionId {
                    input: "bad".into(),
                },
                2,
            ),
            (
                MetadataError::SessionNotFound {
                    input: "missing".into(),
                },
                10,
            ),
            (
                MetadataError::AmbiguousSession {
                    input: "ambiguous".into(),
                },
                11,
            ),
            (
                MetadataError::UnsupportedStorage {
                    provider_name: "provider".into(),
                    reason: "not configured".into(),
                },
                12,
            ),
            (
                MetadataError::Operational {
                    message: "boom".into(),
                },
                1,
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(metadata_error_exit_code(&err), expected, "{err:?}");
        }
    }

    #[test]
    fn metadata_error_code_maps_all_variants() {
        let cases = [
            (
                MetadataError::InvalidSessionId {
                    input: "bad".into(),
                },
                "invalid-session-id",
            ),
            (
                MetadataError::SessionNotFound {
                    input: "missing".into(),
                },
                "session-not-found",
            ),
            (
                MetadataError::AmbiguousSession {
                    input: "ambiguous".into(),
                },
                "ambiguous-session",
            ),
            (
                MetadataError::UnsupportedStorage {
                    provider_name: "provider".into(),
                    reason: "not configured".into(),
                },
                "unsupported-storage",
            ),
            (
                MetadataError::Operational {
                    message: "boom".into(),
                },
                "operational-error",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(metadata_error_code(&err), expected, "{err:?}");
        }
    }

    #[test]
    fn metadata_error_message_formats_all_variants_exactly() {
        let cases = [
            (
                MetadataError::InvalidSessionId { input: "x".into() },
                "invalid session id: x",
            ),
            (
                MetadataError::SessionNotFound { input: "x".into() },
                "session not found: x",
            ),
            (
                MetadataError::AmbiguousSession { input: "x".into() },
                "ambiguous session: x",
            ),
            (
                MetadataError::UnsupportedStorage {
                    provider_name: "provider".into(),
                    reason: "no locator".into(),
                },
                "unsupported storage for provider provider: no locator",
            ),
            (
                MetadataError::Operational {
                    message: "failed to open state".into(),
                },
                "failed to open state",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(metadata_error_message(&err), expected, "{err:?}");
        }
    }

    #[test]
    fn export_error_exit_code_maps_all_variants() {
        let cases = [
            (
                ExportError::InvalidSessionId {
                    input: "bad".into(),
                },
                2,
            ),
            (
                ExportError::SessionNotFound {
                    input: "missing".into(),
                },
                10,
            ),
            (
                ExportError::AmbiguousSession {
                    input: "ambiguous".into(),
                },
                11,
            ),
            (
                ExportError::UnsupportedStorage {
                    provider_name: "provider".into(),
                    reason: "not configured".into(),
                },
                12,
            ),
            (
                ExportError::MalformedTranscript {
                    path: PathBuf::from("transcript.jsonl"),
                    line: 1,
                    reason: "bad json".into(),
                },
                15,
            ),
            (
                ExportError::Operational {
                    message: "boom".into(),
                },
                1,
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(export_error_exit_code(&err), expected, "{err:?}");
        }
    }

    #[test]
    fn export_error_code_maps_all_variants() {
        let cases = [
            (
                ExportError::InvalidSessionId {
                    input: "bad".into(),
                },
                "invalid-session-id",
            ),
            (
                ExportError::SessionNotFound {
                    input: "missing".into(),
                },
                "session-not-found",
            ),
            (
                ExportError::AmbiguousSession {
                    input: "ambiguous".into(),
                },
                "ambiguous-session",
            ),
            (
                ExportError::UnsupportedStorage {
                    provider_name: "provider".into(),
                    reason: "not configured".into(),
                },
                "unsupported-storage",
            ),
            (
                ExportError::MalformedTranscript {
                    path: PathBuf::from("transcript.jsonl"),
                    line: 1,
                    reason: "bad json".into(),
                },
                "malformed-provider-transcript",
            ),
            (
                ExportError::Operational {
                    message: "boom".into(),
                },
                "operational-error",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(export_error_code(&err), expected, "{err:?}");
        }
    }

    #[test]
    fn export_error_message_formats_all_variants_exactly() {
        let cases = [
            (
                ExportError::InvalidSessionId { input: "x".into() },
                "invalid session UUID: x",
            ),
            (
                ExportError::SessionNotFound { input: "x".into() },
                "session not found: x",
            ),
            (
                ExportError::AmbiguousSession { input: "x".into() },
                "session id matches multiple recent chains: x",
            ),
            (
                ExportError::UnsupportedStorage {
                    provider_name: "provider".into(),
                    reason: "no locator".into(),
                },
                "unsupported storage for provider provider: no locator",
            ),
            (
                ExportError::MalformedTranscript {
                    path: PathBuf::from("transcript.jsonl"),
                    line: 0,
                    reason: "bad json".into(),
                },
                "malformed transcript transcript.jsonl: bad json",
            ),
            (
                ExportError::MalformedTranscript {
                    path: PathBuf::from("transcript.jsonl"),
                    line: 7,
                    reason: "bad json".into(),
                },
                "malformed transcript transcript.jsonl line 7: bad json",
            ),
            (
                ExportError::Operational {
                    message: "failed to open state".into(),
                },
                "failed to open state",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(export_error_message(&err), expected, "{err:?}");
        }
    }
}
