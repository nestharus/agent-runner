use chrono::{Duration, Utc};
use oulipoly_config::{ModelConfig, ProviderConfig, model::PromptMode};
use oulipoly_runtime::balancer::select_provider;
use oulipoly_state::{
    InvocationStart, QuotaWindowInput, SessionTurnIngestStreamKey, SessionTurnPageApply,
    SessionTurnStreamProjection, StateDb,
};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{
    Dispatch, Event, Id, Level, Metadata, Subscriber,
    field::{Field, Visit},
    metadata::LevelFilter,
    span::{Attributes, Record},
    subscriber::Interest,
};
use uuid::Uuid;

// risk: Dispatcher branch could accidentally consider non-candidate missing windows; level: particular-integration; source: AGE-224 proposal test-intent track / assumption A1.
#[test]
fn cached_window_gate_ignores_ineligible_missing_window_provider() {
    let db = in_memory_state();
    let model = model_with("age224-window-gate", &["a", "b", "c"]);

    db.mark_exhausted("a").unwrap();
    seed_windows_with_deltas(&db, "b", &[(0.10, 24 * 7, 0.01, 22)]);
    seed_windows_with_deltas(&db, "c", &[(0.50, 24 * 7, 0.01, 22)]);
    for _ in 0..5 {
        record_invocation(&db, &model.name, "b", 1, true);
    }
    mark_model_turn_counts_caught_up(&db, &model);

    let selected = select_provider(&model, &db, None).unwrap();

    assert_eq!(
        model.providers[selected].name, "b",
        "provider a has no cached windows but is not an eligible candidate; density must choose b. \
         Invocation fallback over b/c would choose c because b has more persisted invocations."
    );
}

// risk: Invocation fallback could drop below-threshold recent-error penalty or suppress too early; level: particular-integration; source: AGE-224 proposal test-intent track / assumption A1.
#[test]
fn invocation_fallback_penalizes_recent_errors_below_suppression_threshold() {
    let eligible_after_two_errors = no_window_model_with_failed_a_and_successful_b(2, 25);
    let selected_eligible = select_provider(
        &eligible_after_two_errors.model,
        &eligible_after_two_errors.db,
        None,
    )
    .unwrap();
    assert_eq!(
        eligible_after_two_errors.model.providers[selected_eligible].name, "a",
        "two recent failures must not suppress a before the three-error threshold"
    );

    let penalized_but_not_suppressed = no_window_model_with_failed_a_and_successful_b(2, 15);
    let selected_penalized = select_provider(
        &penalized_but_not_suppressed.model,
        &penalized_but_not_suppressed.db,
        None,
    )
    .unwrap();
    assert_eq!(
        penalized_but_not_suppressed.model.providers[selected_penalized].name, "b",
        "two recent failures add the current 20-point soft penalty before suppression"
    );
}

// risk: Density extraction could preserve provider choice but lose fanout trace payload/order; level: particular-integration; source: AGE-224 proposal test-intent track / assumption A1.
#[test]
fn fanout_trace_emits_selected_provider_band_members_and_score() {
    let db = in_memory_state();
    let model = model_with("age224-fanout-trace", &["a", "b"]);
    seed_windows_with_deltas(&db, "a", &[(0.40, 100, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.10, 40, 0.01, 22)]);
    mark_model_turn_counts_caught_up(&db, &model);

    let (selected, events) = capture_trace_events(|| select_provider(&model, &db, None).unwrap());
    let event = trace_event_with_message(&events, "fanout selected");

    assert_eq!(model.providers[selected].name, "b");
    assert_eq!(event.level, Level::INFO);
    assert_trace_field(event, "selected_provider_name", "b");
    assert_trace_field(event, "band_member_names", "a,b");
    assert_selected_binding_score(event, 35.0..=36.0);
}

struct RouteFixture {
    db: StateDb,
    model: ModelConfig,
}

fn no_window_model_with_failed_a_and_successful_b(
    failed_a: usize,
    successful_b: usize,
) -> RouteFixture {
    let db = in_memory_state();
    let model = model_with("age224-invocation-fallback", &["a", "b"]);
    for _ in 0..failed_a {
        record_invocation(&db, &model.name, "a", 0, false);
    }
    for _ in 0..successful_b {
        record_invocation(&db, &model.name, "b", 1, true);
    }
    RouteFixture { db, model }
}

fn in_memory_state() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

fn model_with(name: &str, provider_names: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: provider_names
            .iter()
            .map(|provider| ProviderConfig::model_provider(*provider, vec![]))
            .collect(),
        inputs: vec![],
        provider: None,
    }
}

fn seed_windows_with_deltas(db: &StateDb, provider_name: &str, windows: &[LearnedWindow]) {
    let inputs: Vec<_> = windows
        .iter()
        .map(|(used_percent, hours_until_reset, _, _)| QuotaWindowInput {
            used_percent: *used_percent,
            resets_at: Utc::now() + Duration::hours(*hours_until_reset),
        })
        .collect();
    db.upsert_quota_refresh(provider_name, &inputs).unwrap();
    for (window_id, (_, _, delta_percent, delta_calls)) in windows.iter().enumerate() {
        db.set_window_delta_for_test(
            provider_name,
            window_id as u32,
            *delta_percent,
            *delta_calls,
        )
        .unwrap();
    }
}

fn mark_model_turn_counts_caught_up(db: &StateDb, model: &ModelConfig) {
    for provider in &model.providers {
        let key = SessionTurnIngestStreamKey {
            provider_name: provider.name.clone(),
            provider_instance_id: provider.name.clone(),
            settings_id: "age224-settings".to_string(),
            session_id: format!("{}-age224", provider.name),
            projection: SessionTurnStreamProjection::CanonicalIngest,
        };
        db.enqueue_session_turn_ingest_stream(&key).unwrap();
        let now = Utc::now();
        let stream = db
            .lease_ready_session_turn_ingest_stream(
                SessionTurnStreamProjection::CanonicalIngest,
                "age224-worker",
                now,
                now + Duration::minutes(1),
            )
            .unwrap()
            .unwrap();
        db.apply_session_turn_page(&SessionTurnPageApply {
            key,
            lease_owner: "age224-worker".to_string(),
            expected_generation: stream.checkpoint_generation,
            request_token_sha256: "1".repeat(64),
            snapshot_id: format!("{}-age224-snapshot", provider.name),
            page_index: stream.expected_page_index,
            page_start_sequence: stream.expected_turn_sequence,
            page_turn_count: 0,
            scan_progress: false,
            snapshot_complete: true,
            next_page_token: None,
            resume_token: Some(format!("{}-age224-resume", provider.name)),
            page_digest: "2".repeat(64),
            turns: Vec::new(),
        })
        .unwrap();
    }
}

type LearnedWindow = (f64, i64, f64, u64);

fn record_invocation(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
    success: bool,
) {
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();
    db.finalize_invocation(id, success, if success { 0 } else { 1 }, None, None)
        .unwrap();
}

#[derive(Clone, Debug)]
struct CapturedTraceEvent {
    level: Level,
    fields: Vec<(String, String)>,
}

#[derive(Clone, Default)]
struct CapturedTraceEvents {
    records: Arc<Mutex<Vec<CapturedTraceEvent>>>,
}

impl CapturedTraceEvents {
    fn snapshot(&self) -> Vec<CapturedTraceEvent> {
        self.records.lock().unwrap().clone()
    }
}

#[derive(Clone, Default)]
struct TraceCaptureSubscriber {
    events: CapturedTraceEvents,
}

impl Subscriber for TraceCaptureSubscriber {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }

    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::TRACE)
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        self.events
            .records
            .lock()
            .unwrap()
            .push(captured_trace_event(event));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

fn captured_trace_event(event: &Event<'_>) -> CapturedTraceEvent {
    CapturedTraceEvent {
        level: *event.metadata().level(),
        fields: trace_event_fields(event),
    }
}

fn trace_event_fields(event: &Event<'_>) -> Vec<(String, String)> {
    let mut visitor = TraceFieldVisitor::default();
    event.record(&mut visitor);
    visitor.fields
}

#[derive(Default)]
struct TraceFieldVisitor {
    fields: Vec<(String, String)>,
}

impl Visit for TraceFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record_field(field, format!("{value:?}"));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_field(field, value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_field(field, value.to_string());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.record_field(field, value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_field(field, value.to_string());
    }
}

impl TraceFieldVisitor {
    fn record_field(&mut self, field: &Field, value: String) {
        self.fields.push((field.name().to_string(), value));
    }
}

fn capture_trace_events<T>(action: impl FnOnce() -> T) -> (T, Vec<CapturedTraceEvent>) {
    let _guard = trace_capture_lock().lock().unwrap();
    let subscriber = TraceCaptureSubscriber::default();
    let events = subscriber.events.clone();
    let dispatch = Dispatch::new(subscriber);
    let result = tracing::dispatcher::with_default(&dispatch, || {
        tracing::callsite::rebuild_interest_cache();
        let result = action();
        tracing::callsite::rebuild_interest_cache();
        result
    });
    (result, events.snapshot())
}

fn trace_capture_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn trace_event_with_message<'a>(
    events: &'a [CapturedTraceEvent],
    message: &str,
) -> &'a CapturedTraceEvent {
    events
        .iter()
        .find(|event| trace_field(event, "message") == Some(message))
        .expect("expected trace event must be emitted")
}

fn trace_field<'a>(event: &'a CapturedTraceEvent, name: &str) -> Option<&'a str> {
    event
        .fields
        .iter()
        .find(|(field_name, _)| field_name == name)
        .map(|(_, value)| value.as_str())
}

fn assert_trace_field(event: &CapturedTraceEvent, name: &str, expected: &str) {
    assert_eq!(trace_field(event, name), Some(expected));
}

fn assert_selected_binding_score(
    event: &CapturedTraceEvent,
    expected_range: std::ops::RangeInclusive<f64>,
) {
    let raw = trace_field(event, "selected_binding_score")
        .expect("selected_binding_score must be present");
    let score: f64 = raw
        .parse()
        .unwrap_or_else(|err| panic!("selected_binding_score must be numeric, got {raw}: {err}"));
    assert!(
        expected_range.contains(&score),
        "selected_binding_score {score} must be in {expected_range:?}"
    );
}
