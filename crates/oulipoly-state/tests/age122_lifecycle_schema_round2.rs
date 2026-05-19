use oulipoly_state::{InvocationStart, LifecycleEventSink, NoopLifecycleEventSink, StateDb};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{Layer, registry};

const TARGET: &str = "oulipoly.invocation_lifecycle";
const MODEL: &str = "codex~high";
const PROVIDER: &str = "codex2";

#[derive(Clone, Default)]
struct CapturingLifecycleEventSink {
    records: Arc<Mutex<Vec<Value>>>,
}

impl CapturingLifecycleEventSink {
    fn new(records: Arc<Mutex<Vec<Value>>>) -> Self {
        Self { records }
    }
}

impl LifecycleEventSink for CapturingLifecycleEventSink {
    fn forward(&mut self, record: &Value) {
        self.records.lock().unwrap().push(record.clone());
    }
}

#[derive(Clone, Debug)]
struct CapturedTrace {
    target: String,
    level: String,
    lifecycle_record: Value,
}

#[derive(Clone, Default)]
struct TraceCapture {
    records: Arc<Mutex<Vec<CapturedTrace>>>,
}

impl TraceCapture {
    fn records(&self) -> Vec<CapturedTrace> {
        self.records.lock().unwrap().clone()
    }
}

struct LifecycleTraceLayer {
    capture: TraceCapture,
}

impl LifecycleTraceLayer {
    fn new(capture: TraceCapture) -> Self {
        Self { capture }
    }
}

impl<S> Layer<S> for LifecycleTraceLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if !is_lifecycle_target(event) {
            return;
        }

        let Some(record) = parse_lifecycle_record_from_event(event) else {
            return;
        };

        self.capture
            .records
            .lock()
            .unwrap()
            .push(build_trace_event(event, record));
    }
}

fn is_lifecycle_target(event: &Event<'_>) -> bool {
    event.metadata().target() == TARGET
}

fn parse_lifecycle_record_from_event(event: &Event<'_>) -> Option<Value> {
    let mut visitor = LifecycleRecordVisitor::default();
    event.record(&mut visitor);
    visitor
        .lifecycle_record
        .map(|raw_record| parse_lifecycle_record(&raw_record))
}

fn build_trace_event(event: &Event<'_>, lifecycle_record: Value) -> CapturedTrace {
    CapturedTrace {
        target: event.metadata().target().to_string(),
        level: event.metadata().level().to_string().to_ascii_lowercase(),
        lifecycle_record,
    }
}

#[derive(Default)]
struct LifecycleRecordVisitor {
    lifecycle_record: Option<String>,
}

impl Visit for LifecycleRecordVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "lifecycle_record" {
            self.lifecycle_record = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "lifecycle_record" {
            self.lifecycle_record = Some(format!("{value:?}"));
        }
    }
}

fn parse_lifecycle_record(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::String(inner)) => serde_json::from_str(&inner).unwrap(),
        Ok(value) => value,
        Err(_) => {
            let unescaped: String = serde_json::from_str(raw).unwrap();
            serde_json::from_str(&unescaped).unwrap()
        }
    }
}

fn with_trace_capture<T>(body: impl FnOnce(TraceCapture) -> T) -> (T, Vec<CapturedTrace>) {
    let capture = TraceCapture::default();
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_test_writer()
        .with_span_events(FmtSpan::NONE);
    let subscriber = registry()
        .with(fmt_layer)
        .with(LifecycleTraceLayer::new(capture.clone()));
    let result = tracing::subscriber::with_default(subscriber, || body(capture.clone()));
    (result, capture.records())
}

fn fixture_db_with_capture() -> (tempfile::TempDir, PathBuf, StateDb, Arc<Mutex<Vec<Value>>>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = CapturingLifecycleEventSink::new(records.clone());
    let db = StateDb::open_with_sink(&db_path, Box::new(sink)).unwrap();
    (dir, db_path, db, records)
}

fn fixture_memory_db_with_capture() -> (StateDb, Arc<Mutex<Vec<Value>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = CapturingLifecycleEventSink::new(records.clone());
    let db = StateDb::open_with_sink(Path::new(":memory:"), Box::new(sink)).unwrap();
    (db, records)
}

fn start(uuid: &str) -> InvocationStart {
    InvocationStart {
        invocation_uuid: uuid.to_string(),
        model_name: MODEL.to_string(),
        provider_name: PROVIDER.to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

fn child_start(uuid: &str, parent_row_id: i64) -> InvocationStart {
    InvocationStart {
        parent_invocation_id: Some(parent_row_id),
        ..start(uuid)
    }
}

fn assert_exact_keys(record: &Value, expected: &[&str]) {
    let object = record.as_object().expect("lifecycle record is an object");
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected,
        "lifecycle record keys mismatch: {record:#?}"
    );
}

fn assert_string_or_null(record: &Value, key: &str) {
    let value = &record[key];
    assert!(
        value.is_string() || value.is_null(),
        "{key} must be string|null: {record:#?}"
    );
}

fn assert_common_record(record: &Value, event_name: &str, invocation_uuid: &str, result: &str) {
    assert_eq!(record["event_name"], event_name);
    assert_eq!(record["invocation_uuid"], invocation_uuid);
    assert_string_or_null(record, "provider_source");
    assert_string_or_null(record, "chain_id");
    assert_string_or_null(record, "session_id");
    assert!(
        record["latency_us"].as_u64().is_some(),
        "latency_us must be a u64: {record:#?}"
    );
    assert_eq!(record["operation_result"], result);

    if result == "ok" {
        assert!(record["error_chain"].is_null(), "{record:#?}");
    } else {
        assert!(
            record["error_chain"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "error_chain must be populated on errors: {record:#?}"
        );
    }
}

fn assert_start_keys(record: &Value) {
    assert_exact_keys(
        record,
        &[
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
        ],
    );
}

fn assert_start_failed_keys(record: &Value) {
    assert_exact_keys(
        record,
        &[
            "event_name",
            "invocation_uuid",
            "provider_source",
            "chain_id",
            "session_id",
            "latency_us",
            "operation_result",
            "error_chain",
            "model",
            "provider",
            "parent_invocation_uuid",
        ],
    );
    assert_string_or_null(record, "model");
    assert_string_or_null(record, "provider");
    assert_string_or_null(record, "parent_invocation_uuid");
}

fn assert_session_keys(record: &Value) {
    assert_exact_keys(
        record,
        &[
            "event_name",
            "invocation_uuid",
            "provider_source",
            "chain_id",
            "session_id",
            "latency_us",
            "operation_result",
            "error_chain",
            "invocation_row_id",
            "capture_method",
            "marker_emitted",
            "resume_input_id",
        ],
    );
}

fn assert_session_capture_failed_field_types(record: &Value) {
    assert!(
        record["capture_method"].is_string(),
        "capture_method must be a string: {record:#?}"
    );
    assert!(
        record["marker_emitted"].is_boolean(),
        "marker_emitted must be a bool: {record:#?}"
    );
    assert_string_or_null(record, "resume_input_id");
}

fn assert_finalize_keys(record: &Value) {
    assert_exact_keys(
        record,
        &[
            "event_name",
            "invocation_uuid",
            "provider_source",
            "chain_id",
            "session_id",
            "latency_us",
            "operation_result",
            "error_chain",
            "invocation_row_id",
            "terminal_status",
            "exit_code",
            "error_category",
            "terminal_reason",
            "raw_artifact_paths",
        ],
    );
}

fn assert_finalize_failed_keys(record: &Value) {
    assert_exact_keys(
        record,
        &[
            "event_name",
            "invocation_uuid",
            "provider_source",
            "chain_id",
            "session_id",
            "latency_us",
            "operation_result",
            "error_chain",
            "invocation_row_id",
            "terminal_status_attempt",
            "exit_code",
            "error_category",
            "terminal_reason",
            "raw_artifact_paths",
        ],
    );
}

fn assert_raw_artifact_paths_shape(record: &Value) {
    let paths = &record["raw_artifact_paths"];
    if paths.is_null() {
        return;
    }
    let object = paths
        .as_object()
        .expect("raw_artifact_paths must be object|null");
    assert!(
        object
            .values()
            .all(|value| value.is_string() || value.is_null()),
        "raw_artifact_paths values must be string|null: {paths:#?}"
    );
}

fn sink_records(records: &Arc<Mutex<Vec<Value>>>) -> Vec<Value> {
    records.lock().unwrap().clone()
}

fn sink_event(records: &[Value], event_name: &str, uuid: &str) -> Value {
    records
        .iter()
        .find(|record| {
            record["event_name"].as_str() == Some(event_name)
                && record["invocation_uuid"].as_str() == Some(uuid)
        })
        .unwrap_or_else(|| panic!("missing sink event {event_name} for {uuid}: {records:#?}"))
        .clone()
}

fn trace_event(records: &[CapturedTrace], event_name: &str, uuid: &str) -> CapturedTrace {
    records
        .iter()
        .find(|record| {
            record.lifecycle_record["event_name"].as_str() == Some(event_name)
                && record.lifecycle_record["invocation_uuid"].as_str() == Some(uuid)
        })
        .unwrap_or_else(|| panic!("missing trace event {event_name} for {uuid}: {records:#?}"))
        .clone()
}

fn sink_events_by_name(records: &[Value], event_name: &str) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record["event_name"].as_str() == Some(event_name))
        .cloned()
        .collect()
}

fn trace_events_by_name(records: &[CapturedTrace], event_name: &str) -> Vec<CapturedTrace> {
    records
        .iter()
        .filter(|record| record.lifecycle_record["event_name"].as_str() == Some(event_name))
        .cloned()
        .collect()
}

#[allow(clippy::type_complexity)]
fn query_session_columns(
    db: &StateDb,
    row_id: i64,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    db.connection()
        .query_row(
            "SELECT session_id, session_capture_method, provider_session_id,
                    resume_input_id, provider_session_capture_method
             FROM invocations WHERE id = ?1",
            rusqlite::params![row_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap()
}

fn invocation_status(db: &StateDb, row_id: i64) -> String {
    db.connection()
        .query_row(
            "SELECT status FROM invocations WHERE id = ?1",
            rusqlite::params![row_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn install_session_failure_trigger(db: &StateDb) {
    // Contract § 6a permits test-time triggers on tempdir-backed StateDb files.
    db.connection()
        .execute_batch(
            "CREATE TRIGGER age129_fail_session_capture
             BEFORE UPDATE OF session_capture_method ON invocations
             BEGIN
                 SELECT RAISE(FAIL, 'age129 session capture failure');
             END;",
        )
        .unwrap();
}

fn install_finalize_failure_trigger(db: &StateDb) {
    // Contract § 6a permits test-time triggers on tempdir-backed StateDb files.
    db.connection()
        .execute_batch(
            "CREATE TRIGGER age129_fail_finalize
             BEFORE UPDATE OF status ON invocations
             BEGIN
                 SELECT RAISE(FAIL, 'age129 finalize failure');
             END;",
        )
        .unwrap();
}

fn contains_events_jsonl(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    std::fs::read_dir(dir).unwrap().any(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() {
            return contains_events_jsonl(&path);
        }
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".events.jsonl"))
    })
}

#[test]
fn start_invocation_emits_structured_record_with_invocation_row_id_model_provider_parent_and_raw_paths()
 {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let parent_uuid = "12900000-0000-4000-8000-000000000101";
        let child_uuid = "12900000-0000-4000-8000-000000000102";
        let parent_row_id = db.start_invocation(&start(parent_uuid)).unwrap();

        let child_row_id = db
            .start_invocation(&child_start(child_uuid, parent_row_id))
            .unwrap();

        let records = sink_records(&sink);
        let record = sink_event(&records, "invocation.started", child_uuid);
        assert_start_keys(&record);
        assert_common_record(&record, "invocation.started", child_uuid, "ok");
        assert_eq!(record["invocation_row_id"], json!(child_row_id));
        assert_eq!(record["model"], MODEL);
        assert_eq!(record["provider"], PROVIDER);
        assert_eq!(record["provider_source"], PROVIDER);
        assert_eq!(record["parent_invocation_uuid"], parent_uuid);
    });

    let trace = trace_event(
        &traces,
        "invocation.started",
        "12900000-0000-4000-8000-000000000102",
    );
    assert_eq!(trace.lifecycle_record["event_name"], "invocation.started");
    assert_eq!(trace.target, TARGET);
}

#[test]
fn session_capture_emits_structured_record_with_invocation_row_id_capture_method_marker_emitted_and_resume_fields()
 {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let invocation_uuid = "12900000-0000-4000-8000-000000000201";
        let row_id = db.start_invocation(&start(invocation_uuid)).unwrap();

        db.update_session_capture(row_id, Some("resume-input-129"), "resumed")
            .unwrap();

        let records = sink_records(&sink);
        let record = sink_event(&records, "invocation.session_captured", invocation_uuid);
        assert_session_keys(&record);
        assert_common_record(
            &record,
            "invocation.session_captured",
            invocation_uuid,
            "ok",
        );
        assert_eq!(record["invocation_row_id"], json!(row_id));
        assert_eq!(record["capture_method"], "resumed");
        assert_eq!(record["marker_emitted"], true);
        assert_eq!(record["resume_input_id"], "resume-input-129");
    });

    let trace = trace_event(
        &traces,
        "invocation.session_captured",
        "12900000-0000-4000-8000-000000000201",
    );
    assert_eq!(
        trace.lifecycle_record["event_name"],
        "invocation.session_captured"
    );
}

#[test]
fn finalize_invocation_emits_structured_record_with_terminal_status_success_exit_code_error_category_and_paths()
 {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let invocation_uuid = "12900000-0000-4000-8000-000000000301";
        let row_id = db.start_invocation(&start(invocation_uuid)).unwrap();

        db.finalize_invocation(row_id, true, 0, None, Some("done"))
            .unwrap();

        let records = sink_records(&sink);
        let record = sink_event(&records, "invocation.finalized", invocation_uuid);
        assert_finalize_keys(&record);
        assert_common_record(&record, "invocation.finalized", invocation_uuid, "ok");
        assert_eq!(record["invocation_row_id"], json!(row_id));
        assert_eq!(record["terminal_status"], "success");
        assert_eq!(record["exit_code"], 0);
        assert!(record["error_category"].is_null());
        assert_eq!(record["terminal_reason"], "done");
        assert_raw_artifact_paths_shape(&record);
    });

    let trace = trace_event(
        &traces,
        "invocation.finalized",
        "12900000-0000-4000-8000-000000000301",
    );
    assert_eq!(trace.lifecycle_record["event_name"], "invocation.finalized");
}

#[test]
fn error_variants_emit_full_schema_for_start_session_finalize() {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let duplicate_uuid = "12900000-0000-4000-8000-000000000401";
        db.start_invocation(&start(duplicate_uuid)).unwrap();
        db.start_invocation(&start(duplicate_uuid)).unwrap_err();

        let session_uuid = "12900000-0000-4000-8000-000000000402";
        let session_row_id = db.start_invocation(&start(session_uuid)).unwrap();
        install_session_failure_trigger(&db);
        db.update_session_capture(session_row_id, Some("session-err"), "fresh")
            .unwrap_err();
        db.connection()
            .execute_batch("DROP TRIGGER age129_fail_session_capture")
            .unwrap();

        let finalize_uuid = "12900000-0000-4000-8000-000000000403";
        let finalize_row_id = db.start_invocation(&start(finalize_uuid)).unwrap();
        install_finalize_failure_trigger(&db);
        db.finalize_invocation(
            finalize_row_id,
            false,
            9,
            Some("sqlite"),
            Some("triggered finalize failure"),
        )
        .unwrap_err();

        let records = sink_records(&sink);

        let start_error = sink_event(&records, "invocation.start_failed", duplicate_uuid);
        assert_start_failed_keys(&start_error);
        assert_common_record(
            &start_error,
            "invocation.start_failed",
            duplicate_uuid,
            "sqlite_error",
        );
        assert!(
            !start_error
                .as_object()
                .unwrap()
                .contains_key("invocation_row_id")
        );

        let session_error = sink_event(&records, "invocation.session_capture_failed", session_uuid);
        assert_session_keys(&session_error);
        assert_session_capture_failed_field_types(&session_error);
        assert_common_record(
            &session_error,
            "invocation.session_capture_failed",
            session_uuid,
            "sqlite_error",
        );
        assert_eq!(session_error["invocation_row_id"], json!(session_row_id));

        let finalize_error = sink_event(&records, "invocation.finalize_failed", finalize_uuid);
        assert_finalize_failed_keys(&finalize_error);
        assert_common_record(
            &finalize_error,
            "invocation.finalize_failed",
            finalize_uuid,
            "sqlite_error",
        );
        assert_eq!(finalize_error["invocation_row_id"], json!(finalize_row_id));
        assert_eq!(finalize_error["terminal_status_attempt"], "failed");
        assert_eq!(finalize_error["exit_code"], 9);
        assert_eq!(finalize_error["error_category"], "sqlite");
        assert_eq!(
            finalize_error["terminal_reason"],
            "triggered finalize failure"
        );
        assert_raw_artifact_paths_shape(&finalize_error);
    });

    assert!(
        trace_event(
            &traces,
            "invocation.start_failed",
            "12900000-0000-4000-8000-000000000401"
        )
        .lifecycle_record["error_chain"]
            .as_str()
            .unwrap()
            .contains("Failed to insert invocation")
    );
    assert!(
        trace_event(
            &traces,
            "invocation.session_capture_failed",
            "12900000-0000-4000-8000-000000000402"
        )
        .lifecycle_record["error_chain"]
            .as_str()
            .unwrap()
            .contains("age129 session capture failure")
    );
    assert!(
        trace_event(
            &traces,
            "invocation.finalize_failed",
            "12900000-0000-4000-8000-000000000403"
        )
        .lifecycle_record["error_chain"]
            .as_str()
            .unwrap()
            .contains("age129 finalize failure")
    );
}

#[test]
fn lifecycle_records_reach_tracing_subscriber_with_expected_target_level_and_field() {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, _sink) = fixture_db_with_capture();
        let ok_uuid = "12900000-0000-4000-8000-000000000501";
        let err_uuid = "12900000-0000-4000-8000-000000000502";

        db.start_invocation(&start(ok_uuid)).unwrap();
        db.start_invocation(&start(err_uuid)).unwrap();
        db.start_invocation(&start(err_uuid)).unwrap_err();
    });

    let ok = trace_event(
        &traces,
        "invocation.started",
        "12900000-0000-4000-8000-000000000501",
    );
    assert_eq!(ok.target, TARGET);
    assert_eq!(ok.level, "info");
    assert_eq!(ok.lifecycle_record["operation_result"], "ok");

    let err = trace_event(
        &traces,
        "invocation.start_failed",
        "12900000-0000-4000-8000-000000000502",
    );
    assert_eq!(err.target, TARGET);
    assert_eq!(err.level, "warn");
    assert_eq!(err.lifecycle_record["operation_result"], "sqlite_error");
}

#[test]
fn default_lifecycle_sink_noops_without_creating_events_jsonl_or_raw_io_directory() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let db = StateDb::open_with_sink(&db_path, Box::new(NoopLifecycleEventSink)).unwrap();
    let invocation_uuid = "12900000-0000-4000-8000-000000000601";
    let row_id = db.start_invocation(&start(invocation_uuid)).unwrap();
    db.update_session_capture(row_id, Some("session-noop"), "fresh")
        .unwrap();
    db.finalize_invocation(row_id, true, 0, None, Some("done"))
        .unwrap();

    assert!(
        !dir.path().join("invocations").join("raw-io").exists(),
        "AGE-129 NoopLifecycleEventSink must not create raw-io"
    );
    assert!(
        !contains_events_jsonl(dir.path()),
        "AGE-129 NoopLifecycleEventSink must not create .events.jsonl files"
    );
}

#[test]
fn memory_lifecycle_records_do_not_construct_raw_io_paths() {
    let (memory_db, memory_sink) = fixture_memory_db_with_capture();
    let memory_uuid = "12900000-0000-4000-8000-000000000602";
    let memory_row_id = memory_db.start_invocation(&start(memory_uuid)).unwrap();
    memory_db
        .finalize_invocation(memory_row_id, true, 0, None, Some("memory done"))
        .unwrap();
    let memory_records = sink_records(&memory_sink);
    let memory_finalize = sink_event(&memory_records, "invocation.finalized", memory_uuid);
    assert!(
        memory_finalize["raw_artifact_paths"].is_null(),
        ":memory: StateDb must not construct raw-io/events paths"
    );
}

#[test]
fn lifecycle_method_sink_forward_invoked_with_same_record() {
    let (sink_records, traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let ok_uuid = "12900000-0000-4000-8000-000000000701";
        let session_uuid = "12900000-0000-4000-8000-000000000702";
        let finalize_uuid = "12900000-0000-4000-8000-000000000703";
        let duplicate_uuid = "12900000-0000-4000-8000-000000000704";
        let session_err_uuid = "12900000-0000-4000-8000-000000000705";
        let finalize_err_uuid = "12900000-0000-4000-8000-000000000706";

        db.start_invocation(&start(ok_uuid)).unwrap();

        let session_row_id = db.start_invocation(&start(session_uuid)).unwrap();
        db.update_session_capture(session_row_id, Some("session-ok"), "fresh")
            .unwrap();

        let finalize_row_id = db.start_invocation(&start(finalize_uuid)).unwrap();
        db.finalize_invocation(finalize_row_id, true, 0, None, Some("done"))
            .unwrap();

        db.start_invocation(&start(duplicate_uuid)).unwrap();
        db.start_invocation(&start(duplicate_uuid)).unwrap_err();

        let session_err_row_id = db.start_invocation(&start(session_err_uuid)).unwrap();
        install_session_failure_trigger(&db);
        db.update_session_capture(session_err_row_id, Some("session-err"), "fresh")
            .unwrap_err();
        db.connection()
            .execute_batch("DROP TRIGGER age129_fail_session_capture")
            .unwrap();

        let finalize_err_row_id = db.start_invocation(&start(finalize_err_uuid)).unwrap();
        install_finalize_failure_trigger(&db);
        db.finalize_invocation(finalize_err_row_id, false, 7, Some("sqlite"), Some("boom"))
            .unwrap_err();

        sink_records(&sink)
    });

    assert!(
        !sink_records.is_empty(),
        "test must capture lifecycle sink records"
    );

    let trace_records = traces
        .iter()
        .map(|trace| trace.lifecycle_record.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        sink_records, trace_records,
        "tracing and sink must receive JSON-equal lifecycle records in emission order"
    );
}

#[test]
fn start_invocation_callsite_emits_success_event_after_row_id_known() {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let invocation_uuid = "12900000-0000-4000-8000-000000000801";

        let row_id = db.start_invocation(&start(invocation_uuid)).unwrap();
        let db_uuid: String = db
            .connection()
            .query_row(
                "SELECT invocation_uuid FROM invocations WHERE id = ?1",
                rusqlite::params![row_id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(db_uuid, invocation_uuid);
        let records = sink_records(&sink);
        let record = sink_event(&records, "invocation.started", invocation_uuid);
        assert_eq!(record["invocation_row_id"], json!(row_id));
    });

    let trace = trace_event(
        &traces,
        "invocation.started",
        "12900000-0000-4000-8000-000000000801",
    );
    assert!(
        trace.lifecycle_record["invocation_row_id"]
            .as_i64()
            .unwrap()
            > 0
    );
}

#[test]
fn update_session_capture_callsite_emits_success_event_preserving_session_columns() {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let invocation_uuid = "12900000-0000-4000-8000-000000000901";
        let row_id = db.start_invocation(&start(invocation_uuid)).unwrap();

        db.update_session_capture(row_id, Some("session-preserved"), "fresh")
            .unwrap();

        let columns = query_session_columns(&db, row_id);
        assert_eq!(
            columns,
            (
                Some("session-preserved".to_string()),
                Some("fresh".to_string()),
                Some("session-preserved".to_string()),
                None,
                Some("fresh".to_string())
            )
        );

        let records = sink_records(&sink);
        let record = sink_event(&records, "invocation.session_captured", invocation_uuid);
        assert_eq!(record["invocation_row_id"], json!(row_id));
        assert_eq!(record["capture_method"], "fresh");
    });

    let trace = trace_event(
        &traces,
        "invocation.session_captured",
        "12900000-0000-4000-8000-000000000901",
    );
    assert_eq!(trace.lifecycle_record["capture_method"], "fresh");
}

#[test]
fn finalize_invocation_callsite_emits_single_terminal_event_after_commit() {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let invocation_uuid = "12900000-0000-4000-8000-000000001001";
        let row_id = db.start_invocation(&start(invocation_uuid)).unwrap();

        db.finalize_invocation(row_id, true, 0, None, Some("done"))
            .unwrap();
        assert_eq!(invocation_status(&db, row_id), "succeeded");
        let second_err = db
            .finalize_invocation(row_id, true, 0, None, Some("done again"))
            .unwrap_err();
        assert_eq!(
            second_err,
            format!("Invocation {row_id} is already finalized")
        );

        let records = sink_records(&sink);
        let terminal_records = records
            .iter()
            .filter(|record| {
                matches!(
                    record["event_name"].as_str(),
                    Some("invocation.finalized") | Some("invocation.finalize_failed")
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            terminal_records.len(),
            2,
            "exactly one terminal lifecycle event per finalize_invocation call"
        );
        assert_eq!(terminal_records[0]["event_name"], "invocation.finalized");
        assert_eq!(
            terminal_records[1]["event_name"],
            "invocation.finalize_failed"
        );
    });

    let terminal_traces = traces
        .iter()
        .filter(|trace| {
            matches!(
                trace.lifecycle_record["event_name"].as_str(),
                Some("invocation.finalized") | Some("invocation.finalize_failed")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_traces.len(), 2);
}

#[test]
fn lifecycle_method_errors_emit_sqlite_error_records_without_changing_return_errors() {
    let ((), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let duplicate_uuid = "12900000-0000-4000-8000-000000001101";
        db.start_invocation(&start(duplicate_uuid)).unwrap();
        let start_err = db.start_invocation(&start(duplicate_uuid)).unwrap_err();
        assert_eq!(
            start_err,
            "Failed to insert invocation: UNIQUE constraint failed: invocations.invocation_uuid"
        );

        let session_uuid = "12900000-0000-4000-8000-000000001102";
        let session_row_id = db.start_invocation(&start(session_uuid)).unwrap();
        install_session_failure_trigger(&db);
        let session_err = db
            .update_session_capture(session_row_id, Some("session-err"), "fresh")
            .unwrap_err();
        assert_eq!(
            session_err,
            format!(
                "Failed to update session capture for invocation {session_row_id}: age129 session capture failure"
            )
        );
        db.connection()
            .execute_batch("DROP TRIGGER age129_fail_session_capture")
            .unwrap();

        let finalize_uuid = "12900000-0000-4000-8000-000000001103";
        let finalize_row_id = db.start_invocation(&start(finalize_uuid)).unwrap();
        install_finalize_failure_trigger(&db);
        let finalize_err = db
            .finalize_invocation(
                finalize_row_id,
                false,
                42,
                Some("sqlite"),
                Some("finalize trigger"),
            )
            .unwrap_err();
        assert_eq!(
            finalize_err,
            format!("Failed to finalize invocation {finalize_row_id}: age129 finalize failure")
        );

        let records = sink_records(&sink);
        assert_eq!(
            sink_event(&records, "invocation.start_failed", duplicate_uuid)["operation_result"],
            "sqlite_error"
        );
        assert_eq!(
            sink_event(&records, "invocation.session_capture_failed", session_uuid)["operation_result"],
            "sqlite_error"
        );
        assert_eq!(
            sink_event(&records, "invocation.finalize_failed", finalize_uuid)["operation_result"],
            "sqlite_error"
        );
    });

    assert_eq!(
        trace_event(
            &traces,
            "invocation.start_failed",
            "12900000-0000-4000-8000-000000001101"
        )
        .lifecycle_record["operation_result"],
        "sqlite_error"
    );
}

#[test]
fn finalize_failed_when_context_lookup_fails_emits_record_with_null_row_id() {
    let missing_row_id = 129_129_129_i64;
    let ((result, sink_records), traces) = with_trace_capture(|_| {
        let (_dir, _db_path, db, sink) = fixture_db_with_capture();
        let result = db.finalize_invocation(
            missing_row_id,
            false,
            77,
            Some("context_lookup"),
            Some("missing invocation row"),
        );

        (result, sink_records(&sink))
    });

    assert_eq!(
        result.unwrap_err(),
        format!("Invocation {missing_row_id} not found")
    );

    let finalize_failed = sink_events_by_name(&sink_records, "invocation.finalize_failed");
    assert_eq!(
        finalize_failed.len(),
        1,
        "forward() must receive one finalize_failed record for missing-row context lookup failure: {sink_records:#?}"
    );
    let record = &finalize_failed[0];
    assert_finalize_failed_keys(record);
    assert_eq!(record["event_name"], "invocation.finalize_failed");
    assert!(
        record["invocation_uuid"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "unresolved-context finalize_failed record still carries a string invocation_uuid: {record:#?}"
    );
    assert_string_or_null(record, "provider_source");
    assert_string_or_null(record, "chain_id");
    assert_string_or_null(record, "session_id");
    assert!(
        record["latency_us"].as_u64().is_some(),
        "latency_us must be a u64: {record:#?}"
    );
    assert!(
        record["operation_result"].as_str() == Some("context_resolution_error"),
        "missing-row finalize failure must be classified as context_resolution_error: {record:#?}"
    );
    assert!(
        record["error_chain"]
            .as_str()
            .is_some_and(|value| value.contains(&format!("Invocation {missing_row_id} not found"))),
        "error_chain must include the legacy missing-row error: {record:#?}"
    );
    assert_eq!(
        record.get("invocation_row_id"),
        Some(&Value::Null),
        "invocation_row_id must be JSON null per contract § 1 stability rule"
    );
    assert_eq!(record["terminal_status_attempt"], "failed");
    assert_eq!(record["exit_code"], 77);
    assert_eq!(record["error_category"], "context_lookup");
    assert_eq!(record["terminal_reason"], "missing invocation row");
    assert_raw_artifact_paths_shape(record);

    let finalize_failed_traces = trace_events_by_name(&traces, "invocation.finalize_failed");
    assert_eq!(
        finalize_failed_traces.len(),
        1,
        "tracing must receive the same missing-row finalize_failed record"
    );
    assert_eq!(finalize_failed_traces[0].target, TARGET);
    assert_eq!(finalize_failed_traces[0].level, "warn");
    assert_eq!(finalize_failed_traces[0].lifecycle_record, *record);
}
