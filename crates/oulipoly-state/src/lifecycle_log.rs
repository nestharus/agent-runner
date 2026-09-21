use crate::event_store::{
    CorrelationId, EventCorrelations, EventKind, PayloadNormalizationPolicy, normalize_payload_v1,
    session_correlation_digest,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TARGET: &str = "oulipoly.invocation_lifecycle";
const RESULT_OK: &str = "ok";
const RESULT_SQLITE_ERROR: &str = "sqlite_error";
const MAX_LIFECYCLE_FIELDS: usize = 32;
const LIFECYCLE_PAYLOAD_FIELDS: &[&str] = &[
    "artifact_path_fingerprints",
    "artifact_roles",
    "capture_method",
    "chain_id",
    "error_category",
    "error_chain",
    "event_name",
    "exit_code",
    "invocation_row_id",
    "invocation_uuid",
    "latency_us",
    "marker_emitted",
    "model",
    "operation_result",
    "parent_invocation_uuid",
    "provider",
    "provider_source",
    "resume_input_present",
    "session_correlation_sha256",
    "terminal_reason",
    "terminal_status",
    "terminal_status_attempt",
];

#[derive(Debug, Clone)]
pub(crate) struct NormalizedLifecycleObservation {
    pub(crate) kind: EventKind,
    pub(crate) recorded_at_unix_micros: i64,
    pub(crate) correlations: EventCorrelations,
    pub(crate) payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LifecycleNormalizationError {
    NotAnObject,
    UnknownEventName,
    MissingInvocationUuid,
    InvalidInvocationUuid,
    InvalidSessionId,
    MissingRecordedAt,
    InvalidRecordedAt,
    InvalidRetentionProjection,
    UnknownField(String),
    NonScalarField(String),
    InvalidArtifactPaths,
    InvalidEventKind,
    FieldLimit,
}

pub(crate) type LifecycleTimer = Instant;

pub trait LifecycleEventSink: Send {
    fn forward(&mut self, record: &serde_json::Value);
}

pub struct NoopLifecycleEventSink;

impl LifecycleEventSink for NoopLifecycleEventSink {
    fn forward(&mut self, _record: &serde_json::Value) {}
}

#[derive(Debug, Clone)]
pub(crate) struct RawArtifactPaths {
    pub(crate) stdout_path: PathBuf,
    pub(crate) stderr_path: PathBuf,
    pub(crate) result_path: PathBuf,
    pub(crate) events_jsonl_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct StartContext {
    pub(crate) invocation_uuid: String,
    pub(crate) provider_source: Option<String>,
    pub(crate) chain_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) latency_us: u64,
    pub(crate) model: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) parent_invocation_uuid: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StartOutcome {
    pub(crate) invocation_row_id: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionContext {
    pub(crate) invocation_uuid: String,
    pub(crate) provider_source: Option<String>,
    pub(crate) chain_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) latency_us: u64,
    pub(crate) invocation_row_id: i64,
    pub(crate) capture_method: String,
    pub(crate) marker_emitted: bool,
    pub(crate) resume_input_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionOutcome;

#[derive(Debug, Clone)]
pub(crate) struct FinalizeContext {
    pub(crate) invocation_uuid: String,
    pub(crate) provider_source: Option<String>,
    pub(crate) chain_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) latency_us: u64,
    pub(crate) invocation_row_id: Option<i64>,
    pub(crate) terminal_status_attempt: String,
    pub(crate) exit_code: i32,
    pub(crate) error_category: Option<String>,
    pub(crate) terminal_reason: Option<String>,
    pub(crate) raw_artifact_paths: Option<RawArtifactPaths>,
    pub(crate) operation_result: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct FinalizeOutcome {
    pub(crate) terminal_status: String,
}

struct CommonRecordInput<'a> {
    event_name: &'a str,
    invocation_uuid: &'a str,
    provider_source: Option<&'a str>,
    chain_id: Option<&'a str>,
    session_id: Option<&'a str>,
    latency_us: u64,
    operation_result: &'a str,
    error_chain: Option<String>,
}

pub(crate) fn build_start_record(ctx: &StartContext, outcome: &StartOutcome) -> Value {
    let mut record = common_record(CommonRecordInput {
        event_name: "invocation.started",
        invocation_uuid: ctx.invocation_uuid.as_str(),
        provider_source: ctx.provider_source.as_deref(),
        chain_id: ctx.chain_id.as_deref(),
        session_id: ctx.session_id.as_deref(),
        latency_us: ctx.latency_us,
        operation_result: RESULT_OK,
        error_chain: None,
    });
    insert(
        &mut record,
        "invocation_row_id",
        json!(outcome.invocation_row_id),
    );
    insert(&mut record, "model", json!(ctx.model));
    insert(&mut record, "provider", json!(ctx.provider));
    insert(
        &mut record,
        "parent_invocation_uuid",
        json!(ctx.parent_invocation_uuid),
    );
    record
}

pub(crate) fn build_start_error_record(ctx: &StartContext, error_chain: String) -> Value {
    let mut record = common_record(CommonRecordInput {
        event_name: "invocation.start_failed",
        invocation_uuid: ctx.invocation_uuid.as_str(),
        provider_source: ctx.provider_source.as_deref(),
        chain_id: ctx.chain_id.as_deref(),
        session_id: ctx.session_id.as_deref(),
        latency_us: ctx.latency_us,
        operation_result: RESULT_SQLITE_ERROR,
        error_chain: Some(error_chain),
    });
    insert(&mut record, "model", json!(ctx.model));
    insert(&mut record, "provider", json!(ctx.provider));
    insert(
        &mut record,
        "parent_invocation_uuid",
        json!(ctx.parent_invocation_uuid),
    );
    record
}

pub(crate) fn build_session_record(ctx: &SessionContext, _outcome: &SessionOutcome) -> Value {
    let mut record = common_record(CommonRecordInput {
        event_name: "invocation.session_captured",
        invocation_uuid: ctx.invocation_uuid.as_str(),
        provider_source: ctx.provider_source.as_deref(),
        chain_id: ctx.chain_id.as_deref(),
        session_id: ctx.session_id.as_deref(),
        latency_us: ctx.latency_us,
        operation_result: RESULT_OK,
        error_chain: None,
    });
    insert(
        &mut record,
        "invocation_row_id",
        json!(ctx.invocation_row_id),
    );
    insert(&mut record, "capture_method", json!(ctx.capture_method));
    insert(&mut record, "marker_emitted", json!(ctx.marker_emitted));
    insert(&mut record, "resume_input_id", json!(ctx.resume_input_id));
    record
}

pub(crate) fn build_session_error_record(ctx: &SessionContext, error_chain: String) -> Value {
    let mut record = common_record(CommonRecordInput {
        event_name: "invocation.session_capture_failed",
        invocation_uuid: ctx.invocation_uuid.as_str(),
        provider_source: ctx.provider_source.as_deref(),
        chain_id: ctx.chain_id.as_deref(),
        session_id: ctx.session_id.as_deref(),
        latency_us: ctx.latency_us,
        operation_result: RESULT_SQLITE_ERROR,
        error_chain: Some(error_chain),
    });
    insert(
        &mut record,
        "invocation_row_id",
        json!(ctx.invocation_row_id),
    );
    insert(&mut record, "capture_method", json!(ctx.capture_method));
    insert(&mut record, "marker_emitted", json!(ctx.marker_emitted));
    insert(&mut record, "resume_input_id", json!(ctx.resume_input_id));
    record
}

pub(crate) fn build_finalize_record(ctx: &FinalizeContext, outcome: &FinalizeOutcome) -> Value {
    let mut record = common_record(CommonRecordInput {
        event_name: "invocation.finalized",
        invocation_uuid: ctx.invocation_uuid.as_str(),
        provider_source: ctx.provider_source.as_deref(),
        chain_id: ctx.chain_id.as_deref(),
        session_id: ctx.session_id.as_deref(),
        latency_us: ctx.latency_us,
        operation_result: RESULT_OK,
        error_chain: None,
    });
    insert(
        &mut record,
        "invocation_row_id",
        json!(ctx.invocation_row_id),
    );
    insert(
        &mut record,
        "terminal_status",
        json!(outcome.terminal_status),
    );
    insert(&mut record, "exit_code", json!(ctx.exit_code));
    insert(&mut record, "error_category", json!(ctx.error_category));
    insert(&mut record, "terminal_reason", json!(ctx.terminal_reason));
    insert(
        &mut record,
        "raw_artifact_paths",
        raw_artifact_paths_json(ctx.raw_artifact_paths.as_ref()),
    );
    record
}

pub(crate) fn build_finalize_error_record(ctx: &FinalizeContext, error_chain: String) -> Value {
    let mut record = common_record(CommonRecordInput {
        event_name: "invocation.finalize_failed",
        invocation_uuid: ctx.invocation_uuid.as_str(),
        provider_source: ctx.provider_source.as_deref(),
        chain_id: ctx.chain_id.as_deref(),
        session_id: ctx.session_id.as_deref(),
        latency_us: ctx.latency_us,
        operation_result: ctx.operation_result,
        error_chain: Some(error_chain),
    });
    insert(
        &mut record,
        "invocation_row_id",
        json!(ctx.invocation_row_id),
    );
    insert(
        &mut record,
        "terminal_status_attempt",
        json!(ctx.terminal_status_attempt),
    );
    insert(&mut record, "exit_code", json!(ctx.exit_code));
    insert(&mut record, "error_category", json!(ctx.error_category));
    insert(&mut record, "terminal_reason", json!(ctx.terminal_reason));
    insert(
        &mut record,
        "raw_artifact_paths",
        raw_artifact_paths_json(ctx.raw_artifact_paths.as_ref()),
    );
    record
}

pub(crate) fn emit_and_forward<S: LifecycleEventSink + ?Sized>(sink: &mut S, record: Value) {
    // Build the typed, redacted event-store input before any telemetry sink is
    // selected. Failure is diagnostic-only and never changes the authoritative
    // State lifecycle result forwarded below.
    match normalize_lifecycle_record(&record) {
        Ok(normalized) => {
            let _ = crate::diagnostic_recorder::process_recorder()
                .record_lifecycle_observation(&normalized);
            if is_ok_record(&record) {
                tracing::info!(target: TARGET, lifecycle_record = %normalized.payload);
            } else {
                tracing::warn!(target: TARGET, lifecycle_record = %normalized.payload);
            }
        }
        Err(_) => {
            crate::diagnostic_recorder::report_diagnostic_gap("lifecycle_normalize");
            tracing::warn!(target: TARGET, lifecycle_record = "normalization_failed");
        }
    }
    sink.forward(&record);
}

pub(crate) fn lifecycle_payload_fields() -> &'static [&'static str] {
    LIFECYCLE_PAYLOAD_FIELDS
}

pub(crate) fn normalize_lifecycle_record(
    record: &Value,
) -> Result<NormalizedLifecycleObservation, LifecycleNormalizationError> {
    let object = record
        .as_object()
        .ok_or(LifecycleNormalizationError::NotAnObject)?;
    if object.len() > MAX_LIFECYCLE_FIELDS {
        return Err(LifecycleNormalizationError::FieldLimit);
    }
    let known = [
        "event_name",
        "invocation_uuid",
        "provider_source",
        "chain_id",
        "session_id",
        "latency_us",
        "operation_result",
        "error_chain",
        "invocation_row_id",
        "model",
        "provider",
        "parent_invocation_uuid",
        "capture_method",
        "marker_emitted",
        "resume_input_id",
        "terminal_status",
        "exit_code",
        "error_category",
        "terminal_reason",
        "raw_artifact_paths",
        "terminal_status_attempt",
        "recorded_at",
        "retention_eligible_at",
        "retention_status",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if let Some(field) = object.keys().find(|field| !known.contains(field.as_str())) {
        return Err(LifecycleNormalizationError::UnknownField(field.clone()));
    }

    let (event_name, registered_kind) = match object.get("event_name").and_then(Value::as_str) {
        Some("invocation.started") => ("invocation.started", "invocation.started"),
        Some("invocation.start_failed") => ("invocation.start_failed", "invocation.start_failed"),
        Some("invocation.session_captured") => {
            ("invocation.session_captured", "invocation.session_captured")
        }
        Some("invocation.session_capture_failed") => (
            "invocation.session_capture_failed",
            "invocation.session_capture_failed",
        ),
        Some("invocation.finalized") => ("invocation.finalized", "invocation.finalized"),
        Some("invocation.finalize_failed") => {
            ("invocation.finalize_failed", "invocation.finalize_failed")
        }
        _ => return Err(LifecycleNormalizationError::UnknownEventName),
    };
    let invocation = object
        .get("invocation_uuid")
        .and_then(Value::as_str)
        .ok_or(LifecycleNormalizationError::MissingInvocationUuid)?;
    let invocation = uuid::Uuid::parse_str(invocation)
        .map_err(|_| LifecycleNormalizationError::InvalidInvocationUuid)?;
    let session = match object.get("session_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(session_correlation_digest(value)),
        Some(_) => return Err(LifecycleNormalizationError::InvalidSessionId),
    };
    let recorded_at_text = object
        .get("recorded_at")
        .and_then(Value::as_str)
        .ok_or(LifecycleNormalizationError::MissingRecordedAt)?;
    let recorded_at_unix_micros = DateTime::parse_from_rfc3339(recorded_at_text)
        .map_err(|_| LifecycleNormalizationError::InvalidRecordedAt)?
        .timestamp_micros();
    if recorded_at_unix_micros < 0 {
        return Err(LifecycleNormalizationError::InvalidRecordedAt);
    }
    match (
        object.get("retention_eligible_at"),
        object.get("retention_status"),
    ) {
        (None, None) => {}
        (Some(Value::String(eligible_at)), Some(Value::String(status)))
            if eligible_at == recorded_at_text && status == "eligible" => {}
        _ => return Err(LifecycleNormalizationError::InvalidRetentionProjection),
    }

    let mut payload = Map::new();
    payload.insert(
        "event_name".to_string(),
        Value::String(event_name.to_string()),
    );
    payload.insert(
        "invocation_uuid".to_string(),
        Value::String(invocation.to_string()),
    );
    if let Some(session) = session {
        payload.insert(
            "session_correlation_sha256".to_string(),
            Value::String(format!("sha256:{}", encode_hex(session.as_bytes()))),
        );
    }
    for (key, value) in object {
        if matches!(
            key.as_str(),
            "event_name"
                | "invocation_uuid"
                | "session_id"
                | "raw_artifact_paths"
                | "resume_input_id"
                | "recorded_at"
                | "retention_eligible_at"
                | "retention_status"
        ) {
            continue;
        }
        if !value.is_null() && !value.is_boolean() && !value.is_number() && !value.is_string() {
            return Err(LifecycleNormalizationError::NonScalarField(key.clone()));
        }
        payload.insert(key.clone(), value.clone());
    }
    if let Some(resume_input) = object.get("resume_input_id") {
        payload.insert(
            "resume_input_present".to_string(),
            Value::Bool(!resume_input.is_null()),
        );
    }
    let (roles, fingerprints) = normalize_artifact_paths(object.get("raw_artifact_paths"))?;
    if !roles.is_empty() {
        payload.insert(
            "artifact_roles".to_string(),
            Value::Array(roles.into_iter().map(Value::String).collect()),
        );
        payload.insert(
            "artifact_path_fingerprints".to_string(),
            Value::Object(fingerprints),
        );
    }

    let kind = EventKind::registered(registered_kind)
        .map_err(|_| LifecycleNormalizationError::InvalidEventKind)?;
    let correlations = EventCorrelations {
        invocation_uuid: Some(CorrelationId::from(invocation)),
        session_correlation_sha256: session,
        ..EventCorrelations::default()
    };
    let policy = PayloadNormalizationPolicy::registered(LIFECYCLE_PAYLOAD_FIELDS)
        .map_err(|_| LifecycleNormalizationError::InvalidEventKind)?;
    let payload = normalize_payload_v1(Value::Object(payload), &policy)
        .map_err(|_| LifecycleNormalizationError::NonScalarField("payload".to_string()))?;
    Ok(NormalizedLifecycleObservation {
        kind,
        recorded_at_unix_micros,
        correlations,
        payload,
    })
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn normalize_artifact_paths(
    value: Option<&Value>,
) -> Result<(Vec<String>, Map<String, Value>), LifecycleNormalizationError> {
    let Some(value) = value else {
        return Ok((Vec::new(), Map::new()));
    };
    if value.is_null() {
        return Ok((Vec::new(), Map::new()));
    }
    let object = value
        .as_object()
        .ok_or(LifecycleNormalizationError::InvalidArtifactPaths)?;
    let allowed = [
        "stdout_path",
        "stderr_path",
        "result_path",
        "events_jsonl_path",
    ];
    if object.len() > allowed.len() || object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(LifecycleNormalizationError::InvalidArtifactPaths);
    }
    let mut roles = Vec::new();
    let mut fingerprints = Map::new();
    for role in allowed {
        let Some(path) = object.get(role) else {
            continue;
        };
        let path = path
            .as_str()
            .ok_or(LifecycleNormalizationError::InvalidArtifactPaths)?;
        let role = role.trim_end_matches("_path");
        let mut digest = Sha256::new();
        digest.update(b"oulipoly.lifecycle-artifact-path.v1\0");
        digest.update(role.as_bytes());
        digest.update(b"\0");
        digest.update(path.as_bytes());
        roles.push(role.to_string());
        fingerprints.insert(
            role.to_string(),
            Value::String(format!("sha256:{:x}", digest.finalize())),
        );
    }
    Ok((roles, fingerprints))
}

pub(crate) fn start_lifecycle_timer() -> LifecycleTimer {
    Instant::now()
}

pub(crate) fn elapsed_microseconds_saturating(timer: &LifecycleTimer) -> u64 {
    elapsed_duration_microseconds_saturating(timer.elapsed())
}

pub(crate) fn sqlite_io_error_from(err: rusqlite::Error) -> io::Error {
    io::Error::other(err)
}

pub(crate) fn message_io_error_from(message: String) -> io::Error {
    io::Error::other(message)
}

pub(crate) fn start_context_with_latency(
    mut context: StartContext,
    latency_us: u64,
) -> StartContext {
    context.latency_us = latency_us;
    context
}

pub(crate) fn session_context_with_latency(
    mut context: SessionContext,
    latency_us: u64,
) -> SessionContext {
    context.latency_us = latency_us;
    context
}

pub(crate) fn finalize_context_with_latency(
    mut context: FinalizeContext,
    latency_us: u64,
) -> FinalizeContext {
    context.latency_us = latency_us;
    context
}

pub(crate) fn build_start_record_for_result(
    context: &StartContext,
    result: &Result<i64, io::Error>,
) -> Value {
    match result {
        Ok(invocation_row_id) => build_start_record(context, &start_outcome_ok(invocation_row_id)),
        Err(err) => build_start_error_record(context, start_outcome_err(err)),
    }
}

pub(crate) fn start_outcome_ok(invocation_row_id: &i64) -> StartOutcome {
    StartOutcome {
        invocation_row_id: *invocation_row_id,
    }
}

pub(crate) fn start_outcome_err(err: &io::Error) -> String {
    format_error_chain(err)
}

pub(crate) fn build_optional_session_record_for_result(
    context: Option<&SessionContext>,
    result: &Result<(), String>,
) -> Option<Value> {
    context.map(|context| build_session_record_for_result(context, result))
}

pub(crate) fn build_session_record_for_result(
    context: &SessionContext,
    result: &Result<(), String>,
) -> Value {
    match result {
        Ok(()) => build_session_record(context, &SessionOutcome),
        Err(message) => build_session_error_record(context, error_chain_from_message(message)),
    }
}

pub(crate) fn build_finalize_record_for_result(
    context: &FinalizeContext,
    result: &Result<(), String>,
    terminal_status: String,
) -> Value {
    match result {
        Ok(()) => build_finalize_record(context, &finalize_outcome_ok(terminal_status)),
        Err(message) => build_finalize_error_record(context, finalize_outcome_err(message)),
    }
}

pub(crate) fn finalize_outcome_ok(terminal_status: String) -> FinalizeOutcome {
    FinalizeOutcome { terminal_status }
}

pub(crate) fn finalize_outcome_err(message: &str) -> String {
    error_chain_from_message(message)
}

fn elapsed_duration_microseconds_saturating(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

pub(crate) fn context_resolution_error_result() -> &'static str {
    "context_resolution_error"
}

pub(crate) fn ok_result() -> &'static str {
    RESULT_OK
}

pub(crate) fn sqlite_error_result() -> &'static str {
    RESULT_SQLITE_ERROR
}

pub(crate) fn format_error_chain(err: &dyn Error) -> String {
    let mut chain = vec![err.to_string()];
    let mut source = err.source();
    while let Some(err) = source {
        chain.push(err.to_string());
        source = err.source();
    }
    chain.join("\n")
}

fn error_chain_from_message(message: &str) -> String {
    format_error_chain(&io_error_from_message(message))
}

fn io_error_from_message(message: &str) -> io::Error {
    io::Error::other(message.to_string())
}

fn common_record(input: CommonRecordInput<'_>) -> Value {
    let recorded_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    json!({
        "event_name": input.event_name,
        "invocation_uuid": input.invocation_uuid,
        "provider_source": input.provider_source,
        "chain_id": input.chain_id,
        "session_id": input.session_id,
        "latency_us": input.latency_us,
        "recorded_at": recorded_at.clone(),
        "retention_eligible_at": recorded_at,
        "retention_status": "eligible",
        "operation_result": input.operation_result,
        "error_chain": input.error_chain,
    })
}

fn insert(record: &mut Value, key: &str, value: Value) {
    if let Some(object) = record.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

fn raw_artifact_paths_json(paths: Option<&RawArtifactPaths>) -> Value {
    let Some(paths) = paths else {
        return Value::Null;
    };
    json!({
        "stdout_path": paths.stdout_path.display().to_string(),
        "stderr_path": paths.stderr_path.display().to_string(),
        "result_path": paths.result_path.display().to_string(),
        "events_jsonl_path": paths.events_jsonl_path.display().to_string(),
    })
}

fn is_ok_record(record: &Value) -> bool {
    record
        .get("operation_result")
        .and_then(Value::as_str)
        .is_some_and(|result| result == RESULT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_normalizer_hashes_session_and_paths_and_redacts_strings() {
        let invocation = uuid::Uuid::new_v4();
        let record = json!({
            "event_name": "invocation.finalized",
            "invocation_uuid": invocation.to_string(),
            "provider_source": "token=provider-secret",
            "chain_id": "chain",
            "session_id": "raw-provider-session",
            "latency_us": 7,
            "recorded_at": "2026-09-20T12:34:56.123456Z",
            "retention_eligible_at": "2026-09-20T12:34:56.123456Z",
            "retention_status": "eligible",
            "operation_result": "ok",
            "error_chain": "password: secret at /private/error.log",
            "invocation_row_id": 4,
            "terminal_status": "completed",
            "exit_code": 0,
            "error_category": null,
            "terminal_reason": "result at /private/result.txt",
            "raw_artifact_paths": {
                "stdout_path": "/private/stdout",
                "stderr_path": "/private/stderr",
                "result_path": "/private/result",
                "events_jsonl_path": "/private/events"
            }
        });
        let normalized = normalize_lifecycle_record(&record).unwrap();
        let encoded = serde_json::to_string(&normalized.payload).unwrap();
        assert!(!encoded.contains("raw-provider-session"));
        assert!(!encoded.contains("provider-secret"));
        assert!(!encoded.contains("/private"));
        assert!(encoded.contains("[REDACTED]"));
        assert!(encoded.contains("[PATH_REDACTED]"));
        assert!(encoded.contains("artifact_path_fingerprints"));
        assert!(
            !normalized
                .payload
                .as_object()
                .unwrap()
                .contains_key("recorded_at")
        );
        assert!(
            !normalized
                .payload
                .as_object()
                .unwrap()
                .contains_key("retention_eligible_at")
        );
        assert_eq!(
            normalized.recorded_at_unix_micros,
            DateTime::parse_from_rfc3339("2026-09-20T12:34:56.123456Z")
                .unwrap()
                .timestamp_micros()
        );
        assert_eq!(
            normalized.correlations.invocation_uuid,
            Some(CorrelationId::from(invocation))
        );
        assert_eq!(
            normalized.correlations.session_correlation_sha256,
            Some(session_correlation_digest("raw-provider-session"))
        );
    }

    #[test]
    fn lifecycle_normalizer_rejects_unknown_names_fields_and_invalid_invocations() {
        let base = json!({
            "event_name": "invocation.started",
            "invocation_uuid": uuid::Uuid::new_v4().to_string(),
            "latency_us": 1,
            "recorded_at": "2026-09-20T12:34:56.123456Z",
            "operation_result": "ok"
        });
        let mut unknown = base.clone();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("raw_extra".to_string(), json!("copy me"));
        assert!(matches!(
            normalize_lifecycle_record(&unknown),
            Err(LifecycleNormalizationError::UnknownField(_))
        ));
        let mut invalid = base.clone();
        invalid["invocation_uuid"] = json!("not-a-uuid");
        assert_eq!(
            normalize_lifecycle_record(&invalid).unwrap_err(),
            LifecycleNormalizationError::InvalidInvocationUuid
        );
        let mut unknown_name = base;
        unknown_name["event_name"] = json!("invocation.user-controlled");
        assert_eq!(
            normalize_lifecycle_record(&unknown_name).unwrap_err(),
            LifecycleNormalizationError::UnknownEventName
        );

        let mut invalid_retention = json!({
            "event_name": "invocation.started",
            "invocation_uuid": uuid::Uuid::new_v4().to_string(),
            "latency_us": 1,
            "recorded_at": "2026-09-20T12:34:56.123456Z",
            "retention_eligible_at": "2026-09-20T12:34:57.123456Z",
            "retention_status": "eligible",
            "operation_result": "ok"
        });
        assert_eq!(
            normalize_lifecycle_record(&invalid_retention).unwrap_err(),
            LifecycleNormalizationError::InvalidRetentionProjection
        );
        invalid_retention["retention_eligible_at"] = invalid_retention["recorded_at"].clone();
        invalid_retention["retention_status"] = json!("pending");
        assert_eq!(
            normalize_lifecycle_record(&invalid_retention).unwrap_err(),
            LifecycleNormalizationError::InvalidRetentionProjection
        );
    }
}
