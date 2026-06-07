//! ## Declared roles
//!
//! Roles: parser, validator, mapper, accessor, filter, orchestration, formatter.
//!
//! - parser: `LaunchJsonlReader::read`, `LaunchStreamParser::{push, finish}`,
//!   `parse_line`, `decode_event`, and `decode_base64` parse launch JSONL into
//!   decoded event/result structures.
//! - validator: `event_kind`, `validate_event_schema`,
//!   `LaunchStreamParser::validate_sequence`, and line-size/finality checks
//!   enforce launch stream contract, request correlation, event schema, and
//!   sequence invariants.
//! - mapper: `decode_event`, protocol/transport error helpers, and retained
//!   cost helpers translate generated DTOs, base64 payloads, and parse failures
//!   into host-facing launch events or provider-client errors.
//! - accessor: `DecodedLaunchEvent::{seq, bytes}`, `LaunchResult` accessors,
//!   and `LaunchJsonlReader::request_id` expose decoded event/result state.
//! - filter: `LaunchEventRetention`, `LaunchMarkerRetention`,
//!   `ByteAccumulator`, and `ByteTailAccumulator` keep bounded event, marker,
//!   stdout, stderr, and raw-stdout evidence.
//! - orchestration: `LaunchStdoutProcessor` connects live process stdout
//!   draining to incremental launch stream parsing while preserving raw
//!   diagnostics.
//! - formatter: line-limit and malformed-line helpers format stable diagnostic
//!   descriptions for oversized or invalid launch JSONL input.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-provider/src/stream.rs
//!     role: adapter
//!     Translates:
//!       - launch-jsonl-stream-contract
//!       - oulipoly-provider-generated-dto-contract
//!       - byte-limit-capture-contract
//!       - provider-client-error-contract
//!       - launch-event-retention-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-provider/src/stream.rs
//!     role: intrinsic-surface
//!     Domain: launch JSONL decoding and bounded retention
//!     Owns:
//!       - DecodedLaunchEvent, LaunchExit, and LaunchResult shapes
//!       - LaunchStreamLimits defaults and bounded-by projection
//!       - event order, finality, contract, request-id, and schema checks
//!       - retained event, marker, stdout, stderr, and raw stdout budgets
//!       - launch stdout drain behavior for live subprocess integration
//! ```

use crate::error::{
    HostErrorKind, ProviderClientError, ProviderDiagnostics, check_contract_and_request,
};
use crate::generated::{LaunchEvent, ProcessStatus, TerminalSignal};
use crate::process::{
    ByteAccumulator, ByteLimit, ByteTailAccumulator, CapturedBytes, StdoutDrainOutput,
    StdoutProcessor,
};
use crate::schemas::{SchemaRegistry, SchemaValidationError};
use base64::Engine;
use serde_json::Value;
use std::io::Read;

const DEFAULT_RETAINED_LAUNCH_EVENTS: usize = 1024;
const DEFAULT_LAUNCH_RETAINED_BYTES: usize = 1024 * 1024;

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
    stdout: CapturedBytes,
    stderr: CapturedBytes,
    retained_markers: Vec<RetainedLaunchMarker>,
    retained_events_omitted: usize,
}

impl LaunchResult {
    pub fn stdout_bytes(&self) -> Vec<u8> {
        self.stdout.bytes.clone()
    }

    pub fn stderr_bytes(&self) -> Vec<u8> {
        self.stderr.bytes.clone()
    }

    pub fn retained_events_omitted(&self) -> usize {
        self.retained_events_omitted
    }

    pub fn retained_marker_value(&self, name: &str) -> Option<&Value> {
        self.retained_markers
            .iter()
            .find(|marker| marker.name == name)
            .map(|marker| &marker.value)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RetainedLaunchMarker {
    name: String,
    value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchStreamLimits {
    pub retained_events: usize,
    pub retained_event_bytes: usize,
    pub retained_output_bytes: usize,
    pub max_line_bytes: usize,
}

impl LaunchStreamLimits {
    pub fn bounded_by(bytes: usize) -> Self {
        let bytes = bytes.max(1);
        Self {
            retained_events: DEFAULT_RETAINED_LAUNCH_EVENTS,
            retained_event_bytes: bytes,
            retained_output_bytes: bytes,
            max_line_bytes: bytes,
        }
    }
}

impl Default for LaunchStreamLimits {
    fn default() -> Self {
        Self {
            retained_events: DEFAULT_RETAINED_LAUNCH_EVENTS,
            retained_event_bytes: DEFAULT_LAUNCH_RETAINED_BYTES,
            retained_output_bytes: DEFAULT_LAUNCH_RETAINED_BYTES,
            max_line_bytes: DEFAULT_LAUNCH_RETAINED_BYTES,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LaunchJsonlReader {
    request_id: String,
    registry: SchemaRegistry,
    limits: LaunchStreamLimits,
}

impl LaunchJsonlReader {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            registry: SchemaRegistry::new(),
            limits: LaunchStreamLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: LaunchStreamLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn read(&self, mut reader: impl Read) -> Result<LaunchResult, ProviderClientError> {
        let mut parser =
            LaunchStreamParser::new(self.request_id.clone(), self.registry.clone(), self.limits);
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => parser.push(&buffer[..read])?,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    return Err(ProviderClientError::protocol(
                        HostErrorKind::MalformedLine,
                        "launch",
                        Some(self.request_id.clone()),
                        ProviderDiagnostics::with_description(error.to_string()),
                    ));
                }
            }
        }
        parser.finish()
    }
}

#[derive(Debug)]
pub(crate) struct LaunchStdoutProcessor {
    parser: LaunchStreamParser,
    raw_stdout: ByteTailAccumulator,
    error: Option<ProviderClientError>,
}

impl LaunchStdoutProcessor {
    pub(crate) fn new(request_id: impl Into<String>, stdout_limit: ByteLimit) -> Self {
        let limit_bytes = stdout_limit.bytes();
        Self {
            parser: LaunchStreamParser::new(
                request_id.into(),
                SchemaRegistry::new(),
                LaunchStreamLimits::bounded_by(limit_bytes),
            ),
            raw_stdout: ByteTailAccumulator::new(stdout_limit),
            error: None,
        }
    }
}

impl StdoutProcessor for LaunchStdoutProcessor {
    type Output = LaunchStdoutDrain;

    fn push(&mut self, chunk: &[u8]) {
        self.raw_stdout.push(chunk);
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.parser.push(chunk) {
            self.error = Some(error);
        }
    }

    fn finish(self) -> Self::Output {
        let captured = self.raw_stdout.finish();
        let result = match self.error {
            Some(error) => Err(error),
            None => self.parser.finish(),
        };
        LaunchStdoutDrain { result, captured }
    }
}

#[derive(Debug)]
pub(crate) struct LaunchStdoutDrain {
    pub(crate) result: Result<LaunchResult, ProviderClientError>,
    captured: CapturedBytes,
}

impl Default for LaunchStdoutDrain {
    fn default() -> Self {
        Self {
            result: Err(protocol_error(
                HostErrorKind::MissingFinalExit,
                "launch-stdout-drain",
            )),
            captured: CapturedBytes::default(),
        }
    }
}

impl StdoutDrainOutput for LaunchStdoutDrain {
    fn captured_bytes(&self) -> CapturedBytes {
        self.captured.clone()
    }
}

#[derive(Debug, Clone)]
struct LaunchStreamParser {
    request_id: String,
    registry: SchemaRegistry,
    limits: LaunchStreamLimits,
    pending_line: Vec<u8>,
    line_number: usize,
    retention: LaunchEventRetention,
    markers: LaunchMarkerRetention,
    stdout: ByteAccumulator,
    stderr: ByteAccumulator,
    exit: Option<LaunchExit>,
    expected_seq: u64,
    last_seq: Option<u64>,
    deferred_initial_skip: bool,
}

impl LaunchStreamParser {
    fn new(request_id: String, registry: SchemaRegistry, limits: LaunchStreamLimits) -> Self {
        Self {
            request_id,
            registry,
            limits,
            pending_line: Vec::new(),
            line_number: 0,
            retention: LaunchEventRetention::new(
                limits.retained_events,
                limits.retained_event_bytes,
            ),
            markers: LaunchMarkerRetention::new(
                limits.retained_events,
                limits.retained_event_bytes,
            ),
            stdout: ByteAccumulator::new(ByteLimit::new(limits.retained_output_bytes)),
            stderr: ByteAccumulator::new(ByteLimit::new(limits.retained_output_bytes)),
            exit: None,
            expected_seq: 1,
            last_seq: None,
            deferred_initial_skip: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), ProviderClientError> {
        self.pending_line.extend_from_slice(chunk);
        while let Some(newline) = self.pending_line.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending_line.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            self.process_line_bytes(&line)?;
        }
        self.ensure_pending_line_within_limit()
    }

    fn finish(mut self) -> Result<LaunchResult, ProviderClientError> {
        if !self.pending_line.is_empty() {
            let line = std::mem::take(&mut self.pending_line);
            self.process_line_bytes(&line)?;
        }
        if self.deferred_initial_skip {
            return Err(protocol_error(HostErrorKind::SkippedSeq, &self.request_id));
        }
        let Some(exit) = self.exit else {
            return Err(protocol_error(
                HostErrorKind::MissingFinalExit,
                &self.request_id,
            ));
        };
        Ok(LaunchResult {
            events: self.retention.events,
            exit,
            diagnostics: ProviderDiagnostics::default(),
            stdout: self.stdout.finish(),
            stderr: self.stderr.finish(),
            retained_markers: self.markers.markers,
            retained_events_omitted: self.retention.omitted,
        })
    }

    fn ensure_pending_line_within_limit(&self) -> Result<(), ProviderClientError> {
        if self.pending_line.len() > self.limits.max_line_bytes {
            return Err(transport_error_with_description(
                HostErrorKind::StdoutLimitExceeded,
                &self.request_id,
                format!(
                    "launch JSONL line exceeded {} bytes",
                    self.limits.max_line_bytes
                ),
            ));
        }
        Ok(())
    }

    fn process_line_bytes(&mut self, line: &[u8]) -> Result<(), ProviderClientError> {
        self.line_number += 1;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.len() > self.limits.max_line_bytes {
            return Err(transport_error_with_description(
                HostErrorKind::StdoutLimitExceeded,
                &self.request_id,
                format!(
                    "launch JSONL line {} exceeded {} bytes",
                    self.line_number, self.limits.max_line_bytes
                ),
            ));
        }
        let line = std::str::from_utf8(line).map_err(|error| {
            ProviderClientError::protocol(
                HostErrorKind::InvalidUtf8,
                "launch",
                Some(self.request_id.clone()),
                ProviderDiagnostics::with_description(error.to_string()),
            )
        })?;
        self.process_line(line)
    }

    fn process_line(&mut self, line: &str) -> Result<(), ProviderClientError> {
        if line.trim().is_empty() {
            return Err(protocol_error_with_description(
                HostErrorKind::MalformedLine,
                &self.request_id,
                format!(
                    "launch JSONL line {} was blank or whitespace-only",
                    self.line_number
                ),
            ));
        }
        if self.exit.is_some() {
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
        self.validate_sequence(&value)?;

        let decoded = decode_event(value, &self.request_id)?;
        self.record_decoded_bytes(&decoded);
        self.record_marker(&decoded);
        if let DecodedLaunchEvent::Exit(decoded_exit) = &decoded {
            self.exit = Some(decoded_exit.clone());
        }
        self.retention.push(decoded);
        Ok(())
    }

    fn validate_sequence(&mut self, value: &Value) -> Result<(), ProviderClientError> {
        let seq = value.get("seq").and_then(Value::as_u64).unwrap_or(0);
        if let Some(last) = self.last_seq {
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
            if seq > self.expected_seq {
                return Err(protocol_error(HostErrorKind::SkippedSeq, &self.request_id));
            }
        } else if seq > self.expected_seq {
            self.deferred_initial_skip = true;
        }
        if self.deferred_initial_skip && self.last_seq.is_some() {
            return Err(protocol_error(HostErrorKind::SkippedSeq, &self.request_id));
        }
        self.last_seq = Some(seq);
        self.expected_seq = seq + 1;
        Ok(())
    }

    fn record_decoded_bytes(&mut self, event: &DecodedLaunchEvent) {
        match event {
            DecodedLaunchEvent::Stdout { data, .. } => self.stdout.push(data),
            DecodedLaunchEvent::Stderr { data, .. } => self.stderr.push(data),
            _ => {}
        }
    }

    fn record_marker(&mut self, event: &DecodedLaunchEvent) {
        let DecodedLaunchEvent::Marker { name, value, .. } = event else {
            return;
        };
        self.markers.push(name, value);
    }
}

#[derive(Debug, Clone)]
struct LaunchEventRetention {
    events: Vec<DecodedLaunchEvent>,
    omitted: usize,
    max_events: usize,
    max_bytes: usize,
    retained_bytes: usize,
}

#[derive(Debug, Clone)]
struct LaunchMarkerRetention {
    markers: Vec<RetainedLaunchMarker>,
    max_markers: usize,
    max_bytes: usize,
    retained_bytes: usize,
}

impl LaunchMarkerRetention {
    fn new(max_markers: usize, max_bytes: usize) -> Self {
        Self {
            markers: Vec::new(),
            max_markers,
            max_bytes,
            retained_bytes: 0,
        }
    }

    fn push(&mut self, name: &str, value: &Value) {
        if self.markers.iter().any(|marker| marker.name == name) {
            return;
        }
        let cost = retained_marker_cost(name, value);
        if self.markers.len() >= self.max_markers
            || self.retained_bytes.saturating_add(cost) > self.max_bytes
        {
            return;
        }
        self.retained_bytes += cost;
        self.markers.push(RetainedLaunchMarker {
            name: name.to_owned(),
            value: value.clone(),
        });
    }
}

impl LaunchEventRetention {
    fn new(max_events: usize, max_bytes: usize) -> Self {
        Self {
            events: Vec::new(),
            omitted: 0,
            max_events,
            max_bytes,
            retained_bytes: 0,
        }
    }

    fn push(&mut self, event: DecodedLaunchEvent) {
        let cost = retained_event_cost(&event);
        if self.events.len() < self.max_events
            && self.retained_bytes.saturating_add(cost) <= self.max_bytes
        {
            self.retained_bytes += cost;
            self.events.push(event);
        } else {
            self.omitted += 1;
        }
    }
}

fn retained_event_cost(event: &DecodedLaunchEvent) -> usize {
    match event {
        DecodedLaunchEvent::Stdout { data, .. } | DecodedLaunchEvent::Stderr { data, .. } => {
            data.len()
        }
        DecodedLaunchEvent::Marker { name, value, .. } => name.len() + value.to_string().len(),
        DecodedLaunchEvent::Heartbeat { detail, .. } => detail.as_ref().map_or(0, String::len),
        DecodedLaunchEvent::Exit(exit) => exit
            .session
            .as_ref()
            .map_or(0, |session| session.to_string().len()),
    }
}

fn retained_marker_cost(name: &str, value: &Value) -> usize {
    name.len() + value.to_string().len()
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

fn transport_error_with_description(
    kind: HostErrorKind,
    request_id: &str,
    description: String,
) -> ProviderClientError {
    ProviderClientError::host_transport(
        kind,
        "launch",
        Some(request_id.to_owned()),
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
