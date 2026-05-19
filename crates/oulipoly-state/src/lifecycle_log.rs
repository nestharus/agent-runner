use serde_json::{Value, json};
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TARGET: &str = "oulipoly.invocation_lifecycle";
const RESULT_OK: &str = "ok";
const RESULT_SQLITE_ERROR: &str = "sqlite_error";

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
    if is_ok_record(&record) {
        tracing::info!(target: TARGET, lifecycle_record = %record);
    } else {
        tracing::warn!(target: TARGET, lifecycle_record = %record);
    }
    sink.forward(&record);
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
        Ok(invocation_row_id) => build_start_record(
            context,
            &StartOutcome {
                invocation_row_id: *invocation_row_id,
            },
        ),
        Err(err) => build_start_error_record(context, format_error_chain(err)),
    }
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
        Ok(()) => build_finalize_record(context, &FinalizeOutcome { terminal_status }),
        Err(message) => build_finalize_error_record(context, error_chain_from_message(message)),
    }
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
    json!({
        "event_name": input.event_name,
        "invocation_uuid": input.invocation_uuid,
        "provider_source": input.provider_source,
        "chain_id": input.chain_id,
        "session_id": input.session_id,
        "latency_us": input.latency_us,
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
