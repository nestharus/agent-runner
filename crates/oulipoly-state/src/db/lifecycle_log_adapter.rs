//! ## Declared roles
//!
//! Role set: { accessor, orchestration, mapper, formatter }
//!
//! Per ACR-249/ACR-250 lifecycle_log_adapter.rs is a declared multi-role adapter over
//! crate::lifecycle_log helper contract and Oulipoly StateDb's lifecycle-emission
//! contract. Adapter `Translates:` contracts are listed in the existing
//! `## Adapter declarations` block below.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/db/lifecycle_log_adapter.rs
//!     role: adapter
//!     Translates:
//!       - crate::lifecycle_log helper contract (timer start, latency saturation, context-with-latency, operation-result labels, record building, emit_and_forward)
//!       - Oulipoly StateDb lifecycle-emission contract via `emit_start`, `emit_finalize`, `emit_session_capture` operations and context DTO constructors consumed from `db.rs`
//!       - SQLite/message I/O error to lifecycle-event error conversion contract
//! ```

use crate::lifecycle_log;
use crate::lifecycle_log::{LifecycleEventSink, LifecycleTimer};
use std::sync::Mutex;

use super::sqlite_adapter as sqlite;
pub(super) use crate::lifecycle_log::{
    FinalizeContext, RawArtifactPaths, SessionContext, StartContext,
};

pub(super) fn start_timer() -> LifecycleTimer {
    lifecycle_log::start_lifecycle_timer()
}

pub(super) fn emit_start(
    sink: &Mutex<Box<dyn LifecycleEventSink + Send>>,
    timer: LifecycleTimer,
    context: StartContext,
    result: &Result<i64, std::io::Error>,
) {
    let latency_us = lifecycle_log::elapsed_microseconds_saturating(&timer);
    let context = lifecycle_log::start_context_with_latency(context, latency_us);
    let record = lifecycle_log::build_start_record_for_result(&context, result);
    emit_record(sink, record);
}

pub(super) fn emit_finalize(
    sink: &Mutex<Box<dyn LifecycleEventSink + Send>>,
    timer: LifecycleTimer,
    context: FinalizeContext,
    result: &Result<(), String>,
    terminal_status: String,
) {
    let latency_us = lifecycle_log::elapsed_microseconds_saturating(&timer);
    let context = lifecycle_log::finalize_context_with_latency(context, latency_us);
    let record = lifecycle_log::build_finalize_record_for_result(&context, result, terminal_status);
    emit_record(sink, record);
}

pub(super) fn emit_session_capture(
    sink: &Mutex<Box<dyn LifecycleEventSink + Send>>,
    timer: LifecycleTimer,
    context: Option<SessionContext>,
    result: &Result<(), String>,
) {
    let latency_us = lifecycle_log::elapsed_microseconds_saturating(&timer);
    let context =
        context.map(|context| lifecycle_log::session_context_with_latency(context, latency_us));
    if let Some(record) =
        lifecycle_log::build_optional_session_record_for_result(context.as_ref(), result)
    {
        emit_record(sink, record);
    }
}

pub(super) fn start_invocation_io_error(err: sqlite::Error) -> std::io::Error {
    let sqlite_error = lifecycle_log::sqlite_io_error_from(err);
    let message = format!("Failed to insert invocation: {sqlite_error}");
    lifecycle_log::message_io_error_from(message)
}

pub(super) fn finalize_operation_result(success: bool, sqlite_error: bool) -> &'static str {
    if success {
        lifecycle_log::ok_result()
    } else if sqlite_error {
        lifecycle_log::sqlite_error_result()
    } else {
        lifecycle_log::context_resolution_error_result()
    }
}

fn emit_record(sink: &Mutex<Box<dyn LifecycleEventSink + Send>>, record: serde_json::Value) {
    let mut sink = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    lifecycle_log::emit_and_forward(sink.as_mut(), record);
}
