use crate::error::{
    HostErrorKind, ProviderClientError, ProviderDiagnostics, check_contract_and_request,
};
use crate::generated::{LaunchEvent, ProcessStatus, TerminalSignal};
use crate::schemas::{SchemaRegistry, SchemaValidationError};
use base64::Engine;
use serde_json::Value;
use std::io::Read;

#[derive(Debug, Clone, PartialEq)]
pub enum DecodedLaunchEvent {
    Stdout {
        seq: u64,
        data: Vec<u8>,
    },
    Stderr {
        seq: u64,
        data: Vec<u8>,
    },
    Marker {
        seq: u64,
        name: String,
        value: Value,
    },
    Heartbeat {
        seq: u64,
        detail: Option<String>,
    },
    Exit(LaunchExit),
}

impl DecodedLaunchEvent {
    pub fn seq(&self) -> u64 {
        match self {
            Self::Stdout { seq, .. }
            | Self::Stderr { seq, .. }
            | Self::Marker { seq, .. }
            | Self::Heartbeat { seq, .. } => *seq,
            Self::Exit(exit) => exit.seq,
        }
    }

    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Stdout { data, .. } | Self::Stderr { data, .. } => Some(data),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchExit {
    pub seq: u64,
    pub status: ProcessStatus,
    pub terminal_signal: TerminalSignal,
    pub session: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchResult {
    pub events: Vec<DecodedLaunchEvent>,
    pub exit: LaunchExit,
    pub diagnostics: ProviderDiagnostics,
}

impl LaunchResult {
    pub fn stdout_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for event in &self.events {
            if let DecodedLaunchEvent::Stdout { data, .. } = event {
                bytes.extend_from_slice(data);
            }
        }
        bytes
    }

    pub fn stderr_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for event in &self.events {
            if let DecodedLaunchEvent::Stderr { data, .. } = event {
                bytes.extend_from_slice(data);
            }
        }
        bytes
    }
}

#[derive(Debug, Clone)]
pub struct LaunchJsonlReader {
    request_id: String,
    registry: SchemaRegistry,
}

impl LaunchJsonlReader {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            registry: SchemaRegistry::new(),
        }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn read(&self, mut reader: impl Read) -> Result<LaunchResult, ProviderClientError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).map_err(|error| {
            ProviderClientError::protocol(
                HostErrorKind::MalformedLine,
                "launch",
                Some(self.request_id.clone()),
                ProviderDiagnostics::with_description(error.to_string()),
            )
        })?;
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            ProviderClientError::protocol(
                HostErrorKind::InvalidUtf8,
                "launch",
                Some(self.request_id.clone()),
                ProviderDiagnostics::with_description(error.to_string()),
            )
        })?;

        let mut events = Vec::new();
        let mut exit = None;
        let mut expected_seq = 1_u64;
        let mut last_seq = None;
        let mut deferred_initial_skip = false;
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                return Err(protocol_error_with_description(
                    HostErrorKind::MalformedLine,
                    &self.request_id,
                    format!(
                        "launch JSONL line {} was blank or whitespace-only",
                        index + 1
                    ),
                ));
            }
            if exit.is_some() {
                let value = parse_line(line, &self.request_id)?;
                let kind = value.get("kind").and_then(Value::as_str);
                let error_kind = if kind == Some("exit") {
                    HostErrorKind::DuplicateExit
                } else {
                    HostErrorKind::EventAfterExit
                };
                return Err(protocol_error(error_kind, &self.request_id));
            }
            let value = parse_line(line, &self.request_id)?;
            let kind = event_kind(&value, &self.request_id)?;
            check_contract_and_request(
                &value,
                &self.request_id,
                "launch",
                ProviderDiagnostics::default(),
            )?;
            validate_event_schema(&self.registry, kind, &value, &self.request_id)?;
            let seq = value.get("seq").and_then(Value::as_u64).unwrap_or(0);
            if let Some(last) = last_seq {
                if seq == last {
                    return Err(protocol_error(
                        HostErrorKind::DuplicateSeq,
                        &self.request_id,
                    ));
                }
                if seq < last {
                    return Err(protocol_error(
                        HostErrorKind::DecreasingSeq,
                        &self.request_id,
                    ));
                }
                if seq > expected_seq {
                    return Err(protocol_error(HostErrorKind::SkippedSeq, &self.request_id));
                }
            } else if seq > expected_seq {
                deferred_initial_skip = true;
            }
            if deferred_initial_skip && last_seq.is_some() {
                return Err(protocol_error(HostErrorKind::SkippedSeq, &self.request_id));
            }
            last_seq = Some(seq);
            expected_seq = seq + 1;

            let decoded = decode_event(value, &self.request_id)?;
            if let DecodedLaunchEvent::Exit(decoded_exit) = &decoded {
                exit = Some(decoded_exit.clone());
            }
            events.push(decoded);
        }
        if deferred_initial_skip {
            return Err(protocol_error(HostErrorKind::SkippedSeq, &self.request_id));
        }
        let Some(exit) = exit else {
            return Err(protocol_error(
                HostErrorKind::MissingFinalExit,
                &self.request_id,
            ));
        };
        Ok(LaunchResult {
            events,
            exit,
            diagnostics: ProviderDiagnostics::default(),
        })
    }
}

fn parse_line(line: &str, request_id: &str) -> Result<Value, ProviderClientError> {
    serde_json::from_str(line).map_err(|_| protocol_error(HostErrorKind::MalformedLine, request_id))
}

fn event_kind<'a>(value: &'a Value, request_id: &str) -> Result<&'a str, ProviderClientError> {
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return Err(protocol_error(HostErrorKind::UnknownEventKind, request_id));
    };
    match kind {
        "stdout" | "stderr" | "marker" | "heartbeat" | "exit" => Ok(kind),
        _ => Err(protocol_error(HostErrorKind::UnknownEventKind, request_id)),
    }
}

fn validate_event_schema(
    registry: &SchemaRegistry,
    kind: &str,
    value: &Value,
    request_id: &str,
) -> Result<(), ProviderClientError> {
    registry
        .validate_launch_event(kind, value)
        .map_err(|error| match error {
            SchemaValidationError::UnknownLaunchEventKind(_) => {
                protocol_error(HostErrorKind::UnknownEventKind, request_id)
            }
            _ => protocol_error(HostErrorKind::SchemaInvalidEvent, request_id),
        })
}

fn decode_event(value: Value, request_id: &str) -> Result<DecodedLaunchEvent, ProviderClientError> {
    let event: LaunchEvent = serde_json::from_value(value)
        .map_err(|_| protocol_error(HostErrorKind::SchemaInvalidEvent, request_id))?;
    match event {
        LaunchEvent::Stdout {
            seq, data_base64, ..
        } => Ok(DecodedLaunchEvent::Stdout {
            seq,
            data: decode_base64(&data_base64, request_id)?,
        }),
        LaunchEvent::Stderr {
            seq, data_base64, ..
        } => Ok(DecodedLaunchEvent::Stderr {
            seq,
            data: decode_base64(&data_base64, request_id)?,
        }),
        LaunchEvent::Marker {
            seq, name, value, ..
        } => Ok(DecodedLaunchEvent::Marker { seq, name, value }),
        LaunchEvent::Heartbeat { seq, detail, .. } => {
            Ok(DecodedLaunchEvent::Heartbeat { seq, detail })
        }
        LaunchEvent::Exit {
            seq,
            status,
            terminal_signal,
            session,
            ..
        } => Ok(DecodedLaunchEvent::Exit(LaunchExit {
            seq,
            status,
            terminal_signal,
            session,
        })),
    }
}

fn decode_base64(text: &str, request_id: &str) -> Result<Vec<u8>, ProviderClientError> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| protocol_error(HostErrorKind::InvalidBase64, request_id))
}

fn protocol_error(kind: HostErrorKind, request_id: &str) -> ProviderClientError {
    protocol_error_with_diagnostics(kind, request_id, ProviderDiagnostics::default())
}

fn protocol_error_with_description(
    kind: HostErrorKind,
    request_id: &str,
    description: String,
) -> ProviderClientError {
    protocol_error_with_diagnostics(
        kind,
        request_id,
        ProviderDiagnostics::with_description(description),
    )
}

fn protocol_error_with_diagnostics(
    kind: HostErrorKind,
    request_id: &str,
    diagnostics: ProviderDiagnostics,
) -> ProviderClientError {
    ProviderClientError::protocol(kind, "launch", Some(request_id.to_owned()), diagnostics)
}
