use chrono::{Duration, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, SessionsConfig,
};
use oulipoly_runtime::balancer::{BalanceContext, select_provider};
use oulipoly_runtime::quota::InFlight;
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{
    Dispatch, Event, Id, Level, Metadata, Subscriber,
    field::{Field, Visit},
    metadata::LevelFilter,
    span::{Attributes, Record},
    subscriber::Interest,
};

#[test]
fn all_providers_exhausted_non_empty_display_is_byte_pinned() {
    let db = in_memory_state();
    let model = two_provider_model();

    seed_empty_quota_row(&db, "a");
    seed_empty_quota_row(&db, "b");
    mark_exhausted(&db, "b");
    mark_exhausted(&db, "a");

    let err = select_provider(&model, &db, None).unwrap_err();

    assert_eq!(
        err.to_string(),
        "all providers in pool test are quota-exhausted: a, b"
    );
}

#[test]
fn topology_probe_expected_pool_count_uses_recorded_peak_when_current_counts_shrink() {
    let db = in_memory_state();
    let model = two_provider_model();
    seed_two_then_one_window_topology(&db, "a");
    seed_one_window_topology(&db, "b", 0.30);
    assert_recorded_topology_peak(&db, "a", 2);
    let providers_cfg = providers_config_with_scripts(&[("a", repaired_two_window_script())]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = routing_context(&providers_cfg, &sessions_cfg, &in_flight);

    with_trace_capture_lock(|| {
        select_provider(&model, &db, Some(&ctx)).unwrap();
    });

    assert_window_count(&db, "a", 2);
    assert_topology_probe_recorded(&db, "a");
}

#[test]
fn topology_probe_records_timestamp_before_failed_refresh_and_preserves_it() {
    let db = in_memory_state();
    let model = two_provider_model();
    seed_topology_probe_candidate(&db);
    let providers_cfg =
        providers_config_with_scripts(&[("a", "printf 'probe failed' >&2; exit 17")]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = routing_context(&providers_cfg, &sessions_cfg, &in_flight);

    with_trace_capture_lock(|| {
        select_provider(&model, &db, Some(&ctx)).unwrap();
    });

    assert_topology_probe_recorded(&db, "a");
    assert_window_count(&db, "a", 1);
}

#[test]
fn topology_probe_trace_event_name_and_fields_are_preserved() {
    let db = in_memory_state();
    let model = two_provider_model();
    seed_topology_probe_candidate(&db);
    let providers_cfg = providers_config_with_scripts(&[("a", repaired_two_window_script())]);
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();
    let ctx = routing_context(&providers_cfg, &sessions_cfg, &in_flight);

    let (_selected, events) =
        capture_trace_events(|| select_provider(&model, &db, Some(&ctx)).unwrap());
    let event = trace_event_with_message(&events, "topology probe fired");

    assert_eq!(event.level, Level::INFO);
    assert_trace_field(event, "provider_name", "a");
    assert_trace_field(event, "live_window_count", "1");
    assert_trace_field(event, "pool_expected_live_window_count", "2");
    assert_trace_field(event, "topology_peak_live_window_count", "1");
}

#[test]
fn clear_reset_implied_flags_does_not_abort_when_clear_fails() {
    let (_dir, path, db) = file_backed_state("clear-reset-fails");
    let model = single_provider_model();
    seed_windows_with_deltas(&db, "a", &[(1.0, -1, 0.01, 22)]);
    mark_exhausted(&db, "a");
    drop(db);
    let readonly_db = StateDb::open_read_only(&path).unwrap();

    let (selected, events) = capture_trace_events(|| select_provider(&model, &readonly_db, None));

    assert_eq!(selected.unwrap(), 0);
    assert_exhausted_flag_is_still_set(&readonly_db, "a");
    let event = trace_event_with_message(
        &events,
        "failed to clear reset-implied quota exhaustion flag",
    );
    assert_eq!(event.level, Level::WARN);
    assert_trace_field(event, "provider_name", "a");
    assert_trace_field_is_present(event, "error");
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
        append_captured_trace_event(&self.events, captured_trace_event(event));
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

fn append_captured_trace_event(events: &CapturedTraceEvents, event: CapturedTraceEvent) {
    events.records.lock().unwrap().push(event);
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
    with_trace_capture_lock(|| {
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
    })
}

fn with_trace_capture_lock<T>(action: impl FnOnce() -> T) -> T {
    let _guard = trace_capture_lock().lock().unwrap();
    action()
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

fn assert_trace_field_is_present(event: &CapturedTraceEvent, name: &str) {
    assert!(
        trace_field(event, name).is_some(),
        "trace field {name} must be present"
    );
}

fn in_memory_state() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

fn file_backed_state(label: &str) -> (tempfile::TempDir, PathBuf, StateDb) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{label}.db"));
    let db = StateDb::open(&path).unwrap();
    (dir, path, db)
}

fn two_provider_model() -> ModelConfig {
    ModelConfig {
        name: "test".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::new("a", vec![]),
            ProviderConfig::new("b", vec![]),
        ],
        inputs: vec![],
        provider: None,
    }
}

fn single_provider_model() -> ModelConfig {
    ModelConfig {
        name: "single".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new("a", vec![])],
        inputs: vec![],
        provider: None,
    }
}

fn routing_context<'a>(
    providers_cfg: &'a ProvidersConfig,
    sessions_cfg: &'a SessionsConfig,
    in_flight: &'a InFlight,
) -> BalanceContext<'a> {
    BalanceContext {
        providers_cfg,
        sessions_cfg,
        in_flight,
    }
}

fn providers_config_with_scripts(scripts: &[(&str, &str)]) -> ProvidersConfig {
    ProvidersConfig {
        entries: scripts.iter().map(provider_entry_with_script).collect(),
    }
}

fn provider_entry_with_script(input: &(&str, &str)) -> (String, ProviderEntry) {
    (
        input.0.to_string(),
        ProviderEntry {
            quota_script: Some(input.1.to_string()),
            ..ProviderEntry::default()
        },
    )
}

fn repaired_two_window_script() -> &'static str {
    r#"printf '%s' '{"windows":[{"used_percent":4,"resets_at":"2099-01-01T00:00:00Z"},{"used_percent":5,"resets_at":"2099-01-01T05:00:00Z"}]}'"#
}

fn seed_empty_quota_row(db: &StateDb, provider_name: &str) {
    db.upsert_quota_refresh(provider_name, &[]).unwrap();
}

fn mark_exhausted(db: &StateDb, provider_name: &str) {
    db.mark_exhausted(provider_name).unwrap();
}

fn seed_two_then_one_window_topology(db: &StateDb, provider_name: &str) {
    db.upsert_quota_refresh(
        provider_name,
        &[quota_window(0.20, 24 * 7), quota_window(0.20, 5)],
    )
    .unwrap();
    db.upsert_quota_refresh(provider_name, &one_window(0.20, 24 * 7))
        .unwrap();
}

fn seed_one_window_topology(db: &StateDb, provider_name: &str, used_percent: f64) {
    db.upsert_quota_refresh(provider_name, &one_window(used_percent, 24 * 7))
        .unwrap();
}

fn seed_topology_probe_candidate(db: &StateDb) {
    seed_windows_with_deltas(db, "a", &[(0.02, 24 * 7, 0.01, 40)]);
    seed_windows_with_deltas(db, "b", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);
}

fn seed_windows_with_deltas(db: &StateDb, provider_name: &str, windows: &[(f64, i64, f64, u64)]) {
    let inputs = quota_window_inputs(windows);
    db.upsert_quota_refresh(provider_name, &inputs).unwrap();
    seed_window_deltas(db, provider_name, windows);
}

fn quota_window_inputs(windows: &[(f64, i64, f64, u64)]) -> Vec<QuotaWindowInput> {
    windows
        .iter()
        .map(|(used, hours, _, _)| quota_window(*used, *hours))
        .collect()
}

fn seed_window_deltas(db: &StateDb, provider_name: &str, windows: &[(f64, i64, f64, u64)]) {
    for (window_id, window) in windows.iter().enumerate() {
        seed_window_delta(db, provider_name, window_id as u32, window);
    }
}

fn seed_window_delta(
    db: &StateDb,
    provider_name: &str,
    window_id: u32,
    window: &(f64, i64, f64, u64),
) {
    db.set_window_delta_for_test(provider_name, window_id, window.2, window.3)
        .unwrap();
}

fn one_window(used_percent: f64, hours_until_reset: i64) -> Vec<QuotaWindowInput> {
    vec![quota_window(used_percent, hours_until_reset)]
}

fn quota_window(used_percent: f64, hours_until_reset: i64) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent,
        resets_at: Utc::now() + Duration::hours(hours_until_reset),
    }
}

fn assert_recorded_topology_peak(db: &StateDb, provider_name: &str, expected: usize) {
    assert_eq!(
        db.get_quota(provider_name)
            .unwrap()
            .unwrap()
            .topology_peak_live_window_count,
        expected,
        "fixture requires a recorded topology peak larger than the current live count"
    );
}

fn assert_window_count(db: &StateDb, provider_name: &str, expected: usize) {
    assert_eq!(
        db.get_windows(provider_name).unwrap().len(),
        expected,
        "cached topology window count must match expected behavior"
    );
}

fn assert_topology_probe_recorded(db: &StateDb, provider_name: &str) {
    assert!(
        db.get_quota(provider_name)
            .unwrap()
            .unwrap()
            .last_topology_probe_at
            .is_some(),
        "topology repair must stamp the probe timestamp"
    );
}

fn assert_exhausted_flag_is_still_set(db: &StateDb, provider_name: &str) {
    assert!(
        db.get_quota(provider_name)
            .unwrap()
            .unwrap()
            .exhausted_at
            .is_some(),
        "failed reset-implied clear is swallowed after warning and leaves the flag intact"
    );
}
