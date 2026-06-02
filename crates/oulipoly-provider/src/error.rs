use crate::generated::{
    CONTRACT_VERSION, ErrorCategory, ErrorObject, ErrorResponseEnvelope, ProcessStatus,
};
use serde_json::Value;

pub use crate::process::CapturedBytes;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostErrorKind {
    SpawnFailed,
    WaitFailed,
    Timeout,
    Cancelled,
    UnknownSubcommand,
    SchemaInvalidRequest,
    SchemaInvalidResponse,
    SchemaInvalidErrorResponse,
    EmptyStdout,
    InvalidUtf8,
    InvalidJson,
    NonObjectJson,
    MultipleJsonObjects,
    LeadingStdoutText,
    TrailingNonWhitespace,
    MismatchedContract,
    MismatchedRequestId,
    ProviderProcessNonzero,
    ProviderProcessNonzeroWithSuccess,
    ProviderClosedStdinEarly,
    StdoutLimitExceeded,
    MalformedLine,
    UnknownEventKind,
    SchemaInvalidEvent,
    InvalidBase64,
    DuplicateSeq,
    SkippedSeq,
    DecreasingSeq,
    MissingFinalExit,
    DuplicateExit,
    EventAfterExit,
    RuntimeDisabled,
    MissingArtifact,
    NotExecutable,
    InvalidBinaryName,
    UnsafeBinary,
    Other(String),
}

impl HostErrorKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::SpawnFailed => "spawn_failed",
            Self::WaitFailed => "wait_failed",
            Self::Timeout => "host_timeout",
            Self::Cancelled => "host_cancelled",
            Self::UnknownSubcommand => "unknown_subcommand",
            Self::SchemaInvalidRequest => "schema_invalid_request",
            Self::SchemaInvalidResponse => "schema_invalid_response",
            Self::SchemaInvalidErrorResponse => "schema_invalid_error_response",
            Self::EmptyStdout => "empty_stdout",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::InvalidJson => "invalid_json",
            Self::NonObjectJson => "non_object_json",
            Self::MultipleJsonObjects => "multiple_json_objects",
            Self::LeadingStdoutText => "leading_stdout_text",
            Self::TrailingNonWhitespace => "trailing_non_whitespace",
            Self::MismatchedContract => "mismatched_contract",
            Self::MismatchedRequestId => "mismatched_request_id",
            Self::ProviderProcessNonzero => "provider_process_nonzero",
            Self::ProviderProcessNonzeroWithSuccess => "provider_process_nonzero_with_success",
            Self::ProviderClosedStdinEarly => "provider_closed_stdin_early",
            Self::StdoutLimitExceeded => "stdout_limit_exceeded",
            Self::MalformedLine => "malformed_line",
            Self::UnknownEventKind => "unknown_event_kind",
            Self::SchemaInvalidEvent => "schema_invalid_event",
            Self::InvalidBase64 => "invalid_base64",
            Self::DuplicateSeq => "duplicate_seq",
            Self::SkippedSeq => "skipped_seq",
            Self::DecreasingSeq => "decreasing_seq",
            Self::MissingFinalExit => "missing_final_exit",
            Self::DuplicateExit => "duplicate_exit",
            Self::EventAfterExit => "event_after_exit",
            Self::RuntimeDisabled => "runtime_disabled",
            Self::MissingArtifact => "missing_artifact",
            Self::NotExecutable => "not_executable",
            Self::InvalidBinaryName => "invalid_binary_name",
            Self::UnsafeBinary => "unsafe_binary",
            Self::Other(kind) => kind,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderDiagnostics {
    pub stdout: CapturedBytes,
    pub stderr: CapturedBytes,
    pub stdin_closed_early: bool,
    pub process_was_force_killed: bool,
    pub process_was_reaped: bool,
    pub provider_process_nonzero: bool,
    pub provider_exit_code: Option<i32>,
    pub host_cancellation_requested: bool,
    pub description: Option<String>,
}

impl ProviderDiagnostics {
    pub fn with_description(description: String) -> Self {
        Self {
            description: Some(description),
            ..Self::default()
        }
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr.bytes).into_owned()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderCapabilityError {
    subcommand: String,
    envelope: ErrorResponseEnvelope,
    diagnostics: Box<ProviderDiagnostics>,
    process_status: Option<Box<ProcessStatus>>,
}

impl ProviderCapabilityError {
    pub fn from_valid_envelope(
        subcommand: impl Into<String>,
        envelope: Value,
        diagnostics: ProviderDiagnostics,
        process_status: Option<ProcessStatus>,
    ) -> Result<Self, ProviderClientError> {
        let subcommand = subcommand.into();
        let request_id = request_id_from(&envelope);
        let envelope: ErrorResponseEnvelope =
            serde_json::from_value(envelope).map_err(|error| ProviderClientError::Protocol {
                kind: HostErrorKind::SchemaInvalidErrorResponse,
                subcommand: subcommand.clone(),
                request_id,
                description: error.to_string(),
                diagnostics: Box::new(diagnostics.clone()),
                process_status: process_status.clone().map(Box::new),
            })?;
        Ok(Self {
            subcommand,
            envelope,
            diagnostics: Box::new(diagnostics),
            process_status: process_status.map(Box::new),
        })
    }

    pub fn request_id(&self) -> &str {
        &self.envelope.request_id
    }

    pub fn subcommand(&self) -> &str {
        &self.subcommand
    }

    pub fn error(&self) -> &ErrorObject {
        &self.envelope.error
    }

    pub fn diagnostics(&self) -> &ProviderDiagnostics {
        self.diagnostics.as_ref()
    }

    pub fn process_status(&self) -> Option<&ProcessStatus> {
        self.process_status.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderClientError {
    Transport {
        kind: HostErrorKind,
        subcommand: String,
        request_id: Option<String>,
        description: String,
        diagnostics: Box<ProviderDiagnostics>,
        process_status: Option<Box<ProcessStatus>>,
    },
    Protocol {
        kind: HostErrorKind,
        subcommand: String,
        request_id: Option<String>,
        description: String,
        diagnostics: Box<ProviderDiagnostics>,
        process_status: Option<Box<ProcessStatus>>,
    },
    ProviderCapability(Box<ProviderCapabilityError>),
}

impl ProviderClientError {
    pub fn host_transport(
        kind: HostErrorKind,
        subcommand: impl Into<String>,
        request_id: Option<String>,
        diagnostics: ProviderDiagnostics,
    ) -> Self {
        Self::Transport {
            description: kind.as_str().to_owned(),
            kind,
            subcommand: subcommand.into(),
            request_id,
            diagnostics: Box::new(diagnostics),
            process_status: None,
        }
    }

    pub fn protocol(
        kind: HostErrorKind,
        subcommand: impl Into<String>,
        request_id: Option<String>,
        diagnostics: ProviderDiagnostics,
    ) -> Self {
        Self::Protocol {
            description: kind.as_str().to_owned(),
            kind,
            subcommand: subcommand.into(),
            request_id,
            diagnostics: Box::new(diagnostics),
            process_status: None,
        }
    }

    pub fn from_capability(error: ProviderCapabilityError) -> Self {
        Self::ProviderCapability(Box::new(error))
    }

    pub fn with_process_context(
        self,
        diagnostics: ProviderDiagnostics,
        process_status: ProcessStatus,
    ) -> Self {
        match self {
            Self::Transport {
                kind,
                subcommand,
                request_id,
                description,
                ..
            } => Self::Transport {
                kind,
                subcommand,
                request_id,
                description,
                diagnostics: Box::new(diagnostics),
                process_status: Some(Box::new(process_status)),
            },
            Self::Protocol {
                kind,
                subcommand,
                request_id,
                description,
                ..
            } => Self::Protocol {
                kind,
                subcommand,
                request_id,
                description,
                diagnostics: Box::new(diagnostics),
                process_status: Some(Box::new(process_status)),
            },
            Self::ProviderCapability(error) => Self::ProviderCapability(error),
        }
    }

    pub fn with_request_id_if_missing(self, fallback_request_id: Option<String>) -> Self {
        match self {
            Self::Transport {
                kind,
                subcommand,
                request_id,
                description,
                diagnostics,
                process_status,
            } => Self::Transport {
                kind,
                subcommand,
                request_id: request_id.or(fallback_request_id),
                description,
                diagnostics,
                process_status,
            },
            Self::Protocol {
                kind,
                subcommand,
                request_id,
                description,
                diagnostics,
                process_status,
            } => Self::Protocol {
                kind,
                subcommand,
                request_id: request_id.or(fallback_request_id),
                description,
                diagnostics,
                process_status,
            },
            Self::ProviderCapability(error) => Self::ProviderCapability(error),
        }
    }

    pub fn classify_non_launch(
        subcommand: &str,
        envelope: Option<Value>,
        process_status: ProcessStatus,
        diagnostics: ProviderDiagnostics,
        _stdin_error: Option<HostErrorKind>,
    ) -> Result<Value, Self> {
        let Some(envelope) = envelope else {
            if process_status.exited_successfully() {
                return Err(Self::protocol(
                    HostErrorKind::EmptyStdout,
                    subcommand,
                    None,
                    diagnostics,
                ));
            }
            return Err(Self::host_transport(
                HostErrorKind::ProviderProcessNonzero,
                subcommand,
                None,
                diagnostics,
            ));
        };
        match envelope.get("ok").and_then(Value::as_bool) {
            Some(false) => Err(Self::from_capability(
                ProviderCapabilityError::from_valid_envelope(
                    subcommand,
                    envelope,
                    diagnostics,
                    Some(process_status),
                )?,
            )),
            Some(true) if process_status.exited_successfully() => Ok(envelope),
            Some(true) => Err(Self::host_transport(
                HostErrorKind::ProviderProcessNonzeroWithSuccess,
                subcommand,
                request_id_from(&envelope),
                diagnostics,
            )),
            None => Err(Self::protocol(
                HostErrorKind::SchemaInvalidResponse,
                subcommand,
                request_id_from(&envelope),
                diagnostics,
            )),
        }
    }

    pub fn is_provider_capability(&self) -> bool {
        matches!(self, Self::ProviderCapability(_))
    }

    pub fn is_host_transport_or_protocol(&self) -> bool {
        matches!(self, Self::Transport { .. } | Self::Protocol { .. })
    }

    pub fn transport_kind(&self) -> &str {
        match self {
            Self::Transport { kind, .. } | Self::Protocol { kind, .. } => kind.as_str(),
            Self::ProviderCapability(_) => "provider_capability",
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Transport { request_id, .. } | Self::Protocol { request_id, .. } => {
                request_id.as_deref()
            }
            Self::ProviderCapability(error) => Some(error.request_id()),
        }
    }

    pub fn provider_category(&self) -> Option<ErrorCategory> {
        match self {
            Self::ProviderCapability(error) => Some(error.error().category.clone()),
            _ => None,
        }
    }

    pub fn provider_error_code(&self) -> Option<&str> {
        match self {
            Self::ProviderCapability(error) => Some(&error.error().code),
            _ => None,
        }
    }

    pub fn process_status(&self) -> Option<&ProcessStatus> {
        match self {
            Self::Transport { process_status, .. } | Self::Protocol { process_status, .. } => {
                process_status.as_deref()
            }
            Self::ProviderCapability(error) => error.process_status(),
        }
    }

    pub fn diagnostics(&self) -> &ProviderDiagnostics {
        match self {
            Self::Transport { diagnostics, .. } | Self::Protocol { diagnostics, .. } => {
                diagnostics.as_ref()
            }
            Self::ProviderCapability(error) => error.diagnostics(),
        }
    }
}

impl std::fmt::Display for ProviderClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "provider client error: {}",
            self.transport_kind()
        )
    }
}

impl std::error::Error for ProviderClientError {}

pub(crate) fn request_id_from(value: &Value) -> Option<String> {
    value
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

pub(crate) fn check_contract_and_request(
    value: &Value,
    expected_request_id: &str,
    subcommand: &str,
    diagnostics: ProviderDiagnostics,
) -> Result<(), ProviderClientError> {
    if value.get("contract").and_then(Value::as_str) != Some(CONTRACT_VERSION) {
        return Err(ProviderClientError::protocol(
            HostErrorKind::MismatchedContract,
            subcommand,
            request_id_from(value),
            diagnostics,
        ));
    }
    if value.get("request_id").and_then(Value::as_str) != Some(expected_request_id) {
        return Err(ProviderClientError::protocol(
            HostErrorKind::MismatchedRequestId,
            subcommand,
            request_id_from(value),
            diagnostics,
        ));
    }
    Ok(())
}
