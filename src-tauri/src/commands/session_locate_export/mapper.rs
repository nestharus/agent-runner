//! Declared roles: mapper

use oulipoly_runtime::services::SessionExportServiceRequest;
use oulipoly_runtime::session_export::ExportError;
use oulipoly_runtime::session_metadata::MetadataError;

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
