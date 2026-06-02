#![cfg(unix)]

use chrono::{DateTime, Duration, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, SessionSourceEntry,
    SessionsConfig,
};
use oulipoly_runtime::balancer::{BalanceContext, compute_projections, select_provider};
use oulipoly_runtime::quota::InFlight;
use oulipoly_runtime::services::{
    ProductionRoutingService, RoutingServicePort, RoutingServiceRequest,
};
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const AGE35_PROVIDER_A: &str = "age35-a";
const AGE35_PROVIDER_B: &str = "age35-b";
const AGE35_MODEL: &str = "age35-model";

struct ScriptFixture {
    _dir: tempfile::TempDir,
}

struct ProviderScriptPath {
    provider: String,
    script: String,
}

struct SessionScriptPath {
    provider: String,
    script: String,
    state_dir: PathBuf,
}

struct RouteHarness {
    _fixture: ScriptFixture,
    db: StateDb,
    model: ModelConfig,
    providers_cfg: ProvidersConfig,
    sessions_cfg: SessionsConfig,
    in_flight: InFlight,
}

struct CachedParityHarness {
    direct_db: StateDb,
    service_db: StateDb,
    model: ModelConfig,
}

struct LiveParityHarness {
    direct: RouteHarness,
    service: RouteHarness,
}

struct TopologyDriftHarness {
    service: RouteHarness,
    projection: RouteHarness,
}

impl ScriptFixture {
    fn new() -> Self {
        Self {
            _dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self._dir.path()
    }

    fn script_path(&self, name: &str) -> PathBuf {
        self.path().join(name)
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.script_path(name);
        let text = shell_script_text(body);
        write_executable_script(&path, &text);
        path
    }
}

fn shell_script_text(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn write_executable_script(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    set_executable(path);
}

fn set_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn open_memory_state() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

fn two_provider_model() -> ModelConfig {
    model_with(&[AGE35_PROVIDER_A, AGE35_PROVIDER_B])
}

fn model_with(names: &[&str]) -> ModelConfig {
    ModelConfig {
        name: AGE35_MODEL.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: names
            .iter()
            .map(|name| ProviderConfig::model_provider(*name, vec![]))
            .collect(),
        inputs: vec![],
        provider: None,
    }
}

fn age35_provider_quota_percent(provider: &str) -> u32 {
    match provider {
        AGE35_PROVIDER_A => 10,
        AGE35_PROVIDER_B => 20,
        _ => panic!("unexpected AGE-35 provider: {provider}"),
    }
}

fn provider_quota_script_name(provider: &str) -> String {
    format!("{provider}-quota.sh")
}

fn provider_session_script_name(provider: &str) -> String {
    format!("{provider}-sessions.sh")
}

fn provider_session_state_dir_name(provider: &str) -> String {
    format!("{provider}-session-state")
}

fn path_display_text(path: &Path) -> String {
    path.display().to_string()
}

fn future_timestamp(hours: i64) -> DateTime<Utc> {
    Utc::now() + Duration::hours(hours)
}

fn quota_output_body(used_percent: u32, resets_at: &DateTime<Utc>) -> String {
    let resets_at = resets_at.to_rfc3339();
    format!(
        r#"printf '%s\n' '{{"windows":[{{"used_percent":{used_percent},"resets_at":"{resets_at}"}}]}}'"#
    )
}

fn session_output_body(provider: &str) -> String {
    format!(
        r#"printf '%s\n' '{{"session_id":"{provider}-session","turn_id":"turn-1","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}'"#
    )
}

fn empty_quota_output_body() -> &'static str {
    r#"printf '%s' '{"windows":[]}'"#
}

fn two_window_quota_output_body(
    long_resets: &DateTime<Utc>,
    short_resets: &DateTime<Utc>,
) -> String {
    let long_resets = long_resets.to_rfc3339();
    let short_resets = short_resets.to_rfc3339();
    format!(
        r#"printf '%s' '{{"windows":[{{"used_percent":4,"resets_at":"{long_resets}"}},{{"used_percent":90,"resets_at":"{short_resets}"}}]}}'"#
    )
}

fn quota_script(fixture: &ScriptFixture, provider: &str, used_percent: u32) -> String {
    let body = quota_output_body(used_percent, &future_timestamp(24));
    let script = fixture.write_script(&provider_quota_script_name(provider), &body);
    path_display_text(&script)
}

fn session_script(fixture: &ScriptFixture, provider: &str) -> String {
    let body = session_output_body(provider);
    let script = fixture.write_script(&provider_session_script_name(provider), &body);
    path_display_text(&script)
}

fn age35_provider_entries(fixture: &ScriptFixture) -> HashMap<String, ProviderEntry> {
    provider_entries_from_scripts(
        [AGE35_PROVIDER_A, AGE35_PROVIDER_B]
            .iter()
            .map(|provider| ProviderScriptPath {
                provider: (*provider).to_string(),
                script: quota_script(fixture, provider, age35_provider_quota_percent(provider)),
            })
            .collect(),
    )
}

fn provider_entries_from_scripts(paths: Vec<ProviderScriptPath>) -> HashMap<String, ProviderEntry> {
    paths
        .into_iter()
        .map(|path| {
            (
                path.provider,
                ProviderEntry {
                    quota_script: Some(path.script),
                    ..ProviderEntry::default()
                },
            )
        })
        .collect()
}

fn providers_config_from_entries(entries: HashMap<String, ProviderEntry>) -> ProvidersConfig {
    ProvidersConfig { entries }
}

fn providers_config(fixture: &ScriptFixture) -> ProvidersConfig {
    providers_config_from_entries(age35_provider_entries(fixture))
}

fn materialize_provider_scripts(
    fixture: &ScriptFixture,
    scripts: &[(&str, &str)],
) -> Vec<ProviderScriptPath> {
    scripts
        .iter()
        .map(|(provider, body)| {
            let script = fixture.write_script(&provider_quota_script_name(provider), body);
            ProviderScriptPath {
                provider: (*provider).to_string(),
                script: path_display_text(&script),
            }
        })
        .collect()
}

fn providers_config_with_script_bodies(
    fixture: &ScriptFixture,
    scripts: &[(&str, &str)],
) -> ProvidersConfig {
    providers_config_from_entries(provider_entries_from_scripts(materialize_provider_scripts(
        fixture, scripts,
    )))
}

fn age35_session_scripts(fixture: &ScriptFixture) -> Vec<SessionScriptPath> {
    [AGE35_PROVIDER_A, AGE35_PROVIDER_B]
        .iter()
        .map(|provider| SessionScriptPath {
            provider: (*provider).to_string(),
            script: session_script(fixture, provider),
            state_dir: session_state_dir(fixture, provider),
        })
        .collect()
}

fn session_state_dir(fixture: &ScriptFixture, provider: &str) -> PathBuf {
    fixture
        .path()
        .join(provider_session_state_dir_name(provider))
}

fn session_entries_from_scripts(
    paths: Vec<SessionScriptPath>,
) -> HashMap<String, SessionSourceEntry> {
    paths
        .into_iter()
        .map(|path| {
            (
                path.provider,
                SessionSourceEntry {
                    turn_script: path.script,
                    transcript_locator: None,
                    state_dir: Some(path.state_dir),
                },
            )
        })
        .collect()
}

fn sessions_config_from_entries(entries: HashMap<String, SessionSourceEntry>) -> SessionsConfig {
    SessionsConfig { entries }
}

fn sessions_config(fixture: &ScriptFixture) -> SessionsConfig {
    sessions_config_from_entries(session_entries_from_scripts(age35_session_scripts(fixture)))
}

fn materialize_session_scripts(
    fixture: &ScriptFixture,
    scripts: &[(&str, &str)],
) -> Vec<SessionScriptPath> {
    scripts
        .iter()
        .map(|(provider, body)| {
            let script = fixture.write_script(&provider_session_script_name(provider), body);
            SessionScriptPath {
                provider: (*provider).to_string(),
                script: path_display_text(&script),
                state_dir: session_state_dir(fixture, provider),
            }
        })
        .collect()
}

fn sessions_config_with_script_bodies(
    fixture: &ScriptFixture,
    scripts: &[(&str, &str)],
) -> SessionsConfig {
    sessions_config_from_entries(session_entries_from_scripts(materialize_session_scripts(
        fixture, scripts,
    )))
}

fn quota_window_inputs(windows: &[(f64, i64)]) -> Vec<QuotaWindowInput> {
    windows
        .iter()
        .map(|(used_percent, reset_hours)| QuotaWindowInput {
            used_percent: *used_percent,
            resets_at: future_timestamp(*reset_hours),
        })
        .collect()
}

fn seed_windows(db: &StateDb, provider: &str, windows: &[(f64, i64)]) {
    let inputs = quota_window_inputs(windows);
    db.upsert_quota_refresh(provider, &inputs).unwrap();
}

fn seed_unavailable_marker(db: &StateDb, provider: &str) {
    db.record_provider_unavailable(provider, Some(future_timestamp(2)), "UpstreamApiDown")
        .unwrap();
}

fn seed_unreadable_cached_quota(db: &StateDb, provider: &str) {
    seed_unavailable_marker(db, provider);
    db.force_unreadable_cached_quota_for_test(provider).unwrap();
}

fn seed_unreadable_cached_windows(db: &StateDb, provider: &str) {
    seed_windows(db, provider, &[(1.0, 2)]);
    db.force_unreadable_cached_windows_for_test(provider)
        .unwrap();
}

fn seed_topology_drift_windows(db: &StateDb) {
    seed_windows(db, AGE35_PROVIDER_A, &[(0.02, 24 * 7)]);
    seed_windows(db, AGE35_PROVIDER_B, &[(0.66, 80), (0.16, 3)]);
}

fn assert_live_route_side_effects(db: &StateDb, label: &str) {
    for provider in [AGE35_PROVIDER_A, AGE35_PROVIDER_B] {
        let quota = db
            .get_quota(provider)
            .unwrap()
            .unwrap_or_else(|| panic!("missing refreshed quota row for {provider} in {label}"));
        assert_eq!(
            quota.calls_since_refresh, 0,
            "quota refresh should reset calls_since_refresh for {provider} in {label}"
        );
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            1,
            "live routing should persist one quota window for {provider} in {label}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            1,
            "live routing should scan session turns for {provider} in {label}"
        );
    }
}

fn assert_cached_route_has_no_live_side_effects(db: &StateDb, label: &str) {
    for provider in [AGE35_PROVIDER_A, AGE35_PROVIDER_B] {
        assert!(
            db.get_quota(provider).unwrap().is_none(),
            "cached-only routing should not refresh quota for {provider} in {label}"
        );
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            0,
            "cached-only routing should not write quota windows for {provider} in {label}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            0,
            "cached-only routing should not scan session turns for {provider} in {label}"
        );
    }
}

fn assert_selected_index(selected: usize, expected: usize, message: &str) {
    assert_eq!(selected, expected, "{message}");
}

fn assert_selected_index_is_valid(selected: usize, model: &ModelConfig) {
    assert!(selected < model.providers.len());
}

fn assert_cached_quota_read_fails(db: &StateDb, provider: &str) {
    assert!(
        db.get_quota(provider).is_err(),
        "fixture must force the public cached quota read to fail"
    );
}

fn assert_cached_window_read_fails(db: &StateDb, provider: &str) {
    assert!(
        db.get_windows(provider).is_err(),
        "fixture must force the public cached window read to fail"
    );
}

fn assert_log_lines(log_path: &Path, expected: &[&str], message: &str) {
    assert_eq!(read_log_lines(log_path), expected, "{message}");
}

fn assert_marker_cleared(db: &StateDb, provider: &str) {
    assert!(
        db.get_quota(provider)
            .unwrap()
            .expect("marker provider quota row should remain")
            .next_available_at
            .is_none(),
        "marker verification should clear a stale next_available_at before selection honors it"
    );
}

fn assert_route_parity(service_index: usize, direct_index: usize, message: &str) {
    assert_eq!(service_index, direct_index, "{message}");
}

fn assert_topology_probe_drift(service_db: &StateDb, projection_db: &StateDb) {
    assert_eq!(
        service_db.get_windows(AGE35_PROVIDER_A).unwrap().len(),
        2,
        "routing service must preserve select_provider(Some(ctx)) topology probe behavior"
    );
    assert_eq!(
        projection_db.get_windows(AGE35_PROVIDER_A).unwrap().len(),
        1,
        "compute_projections(Some(ctx)) must keep the accepted topology-probe drift"
    );
}

fn assert_shell_quote_safe(raw: &str) {
    assert!(
        !raw.contains('\''),
        "test temp path unexpectedly contains a single quote: {raw}"
    );
}

fn shell_single_quoted(raw: &str) -> String {
    format!("'{raw}'")
}

fn assert_shell_quote_safe_path(path: &Path) {
    let raw = path_display_text(path);
    assert_shell_quote_safe(&raw);
}

fn shell_single_quoted_path(path: &Path) -> String {
    let raw = path_display_text(path);
    shell_single_quoted(&raw)
}

fn append_log_line_body(quoted_log_path: &str, line: &str) -> String {
    format!("printf '%s\\n' '{line}' >> {quoted_log_path}")
}

fn failing_logged_body(quoted_log_path: &str, line: &str) -> String {
    format!("{}\nexit 1", append_log_line_body(quoted_log_path, line))
}

fn logged_quota_body(quoted_log_path: &str, line: &str, used_percent: u32) -> String {
    format!(
        "{}\nprintf '%s\\n' '{{\"windows\":[{{\"used_percent\":{used_percent},\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'",
        append_log_line_body(quoted_log_path, line)
    )
}

fn read_log_text(log_path: &Path) -> String {
    fs::read_to_string(log_path).unwrap()
}

fn parse_log_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn read_log_lines(log_path: &Path) -> Vec<String> {
    parse_log_lines(&read_log_text(log_path))
}

fn balance_context<'a>(
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

fn harness_context(harness: &RouteHarness) -> BalanceContext<'_> {
    balance_context(
        &harness.providers_cfg,
        &harness.sessions_cfg,
        &harness.in_flight,
    )
}

fn select_with_context(harness: &RouteHarness) -> usize {
    let ctx = harness_context(harness);
    select_provider(&harness.model, &harness.db, Some(&ctx)).unwrap()
}

fn select_cached(model: &ModelConfig, db: &StateDb) -> usize {
    select_provider(model, db, None).unwrap()
}

fn select_service_with_context(harness: &RouteHarness) -> usize {
    let ctx = harness_context(harness);
    ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model: &harness.model,
            state: &harness.db,
            ctx: Some(&ctx),
        })
        .unwrap()
        .provider_index
}

fn select_service_cached(model: &ModelConfig, db: &StateDb) -> usize {
    ProductionRoutingService
        .select_route(RoutingServiceRequest {
            model,
            state: db,
            ctx: None,
        })
        .unwrap()
        .provider_index
}

fn compute_projections_with_context(harness: &RouteHarness) {
    let ctx = harness_context(harness);
    let _ = compute_projections(&harness.model, &harness.db, Some(&ctx));
}

fn live_route_harness() -> RouteHarness {
    let fixture = ScriptFixture::new();
    let providers_cfg = providers_config(&fixture);
    let sessions_cfg = sessions_config(&fixture);
    RouteHarness {
        _fixture: fixture,
        db: open_memory_state(),
        model: two_provider_model(),
        providers_cfg,
        sessions_cfg,
        in_flight: InFlight::new(),
    }
}

fn cached_parity_harness() -> CachedParityHarness {
    CachedParityHarness {
        direct_db: open_memory_state(),
        service_db: open_memory_state(),
        model: two_provider_model(),
    }
}

fn live_parity_harness() -> LiveParityHarness {
    LiveParityHarness {
        direct: live_route_harness(),
        service: live_route_harness(),
    }
}

fn unreadable_quota_fixture(provider: &str) -> (StateDb, ModelConfig) {
    let db = open_memory_state();
    seed_unreadable_cached_quota(&db, provider);
    (db, model_with(&[provider]))
}

fn unreadable_window_fixture(provider: &str) -> (StateDb, ModelConfig) {
    let db = open_memory_state();
    seed_unreadable_cached_windows(&db, provider);
    (db, model_with(&[provider]))
}

fn refresh_failure_order_harness() -> (RouteHarness, PathBuf) {
    let fixture = ScriptFixture::new();
    let log_path = fixture.path().join("routing-order.log");
    assert_shell_quote_safe_path(&log_path);
    let quoted_log_path = shell_single_quoted_path(&log_path);
    let quota_a = failing_logged_body(&quoted_log_path, "quota:age222-order-a");
    let quota_b = failing_logged_body(&quoted_log_path, "quota:age222-order-b");
    let scan_a = failing_logged_body(&quoted_log_path, "scan:age222-order-a");
    let scan_b = failing_logged_body(&quoted_log_path, "scan:age222-order-b");
    let providers_cfg = providers_config_with_script_bodies(
        &fixture,
        &[
            ("age222-order-a", quota_a.as_str()),
            ("age222-order-b", quota_b.as_str()),
        ],
    );
    let sessions_cfg = sessions_config_with_script_bodies(
        &fixture,
        &[
            ("age222-order-a", scan_a.as_str()),
            ("age222-order-b", scan_b.as_str()),
        ],
    );
    (
        RouteHarness {
            _fixture: fixture,
            db: open_memory_state(),
            model: model_with(&["age222-order-a", "age222-order-b"]),
            providers_cfg,
            sessions_cfg,
            in_flight: InFlight::new(),
        },
        log_path,
    )
}

fn marker_order_harness() -> (RouteHarness, PathBuf) {
    let fixture = ScriptFixture::new();
    let db = open_memory_state();
    let log_path = fixture.path().join("marker-order.log");
    assert_shell_quote_safe_path(&log_path);
    let quoted_log_path = shell_single_quoted_path(&log_path);
    seed_unavailable_marker(&db, "age222-marker-a");
    let quota_a = logged_quota_body(&quoted_log_path, "marker-refresh:age222-marker-a", 3);
    let quota_b = logged_quota_body(&quoted_log_path, "quota:age222-marker-b", 4);
    let scan_a = append_log_line_body(&quoted_log_path, "scan:age222-marker-a");
    let scan_b = append_log_line_body(&quoted_log_path, "scan:age222-marker-b");
    let providers_cfg = providers_config_with_script_bodies(
        &fixture,
        &[
            ("age222-marker-a", quota_a.as_str()),
            ("age222-marker-b", quota_b.as_str()),
        ],
    );
    let sessions_cfg = sessions_config_with_script_bodies(
        &fixture,
        &[
            ("age222-marker-a", scan_a.as_str()),
            ("age222-marker-b", scan_b.as_str()),
        ],
    );
    (
        RouteHarness {
            _fixture: fixture,
            db,
            model: model_with(&["age222-marker-a", "age222-marker-b"]),
            providers_cfg,
            sessions_cfg,
            in_flight: InFlight::new(),
        },
        log_path,
    )
}

fn topology_drift_harness() -> TopologyDriftHarness {
    let service = topology_drift_route_harness();
    let projection = topology_drift_route_harness();
    seed_topology_drift_windows(&service.db);
    seed_topology_drift_windows(&projection.db);
    TopologyDriftHarness {
        service,
        projection,
    }
}

fn topology_drift_route_harness() -> RouteHarness {
    let fixture = ScriptFixture::new();
    let repair_a = two_window_quota_output_body(&future_timestamp(80), &future_timestamp(5));
    let providers_cfg = providers_config_with_script_bodies(
        &fixture,
        &[
            (AGE35_PROVIDER_A, repair_a.as_str()),
            (AGE35_PROVIDER_B, empty_quota_output_body()),
        ],
    );
    RouteHarness {
        _fixture: fixture,
        db: open_memory_state(),
        model: two_provider_model(),
        providers_cfg,
        sessions_cfg: SessionsConfig::default(),
        in_flight: InFlight::new(),
    }
}

#[test]
fn age_35_select_provider_with_balance_context_refreshes_stale_quotas_and_scans_sessions() {
    let harness = live_route_harness();

    let selected = select_with_context(&harness);

    assert_selected_index_is_valid(selected, &harness.model);
    assert_live_route_side_effects(&harness.db, "select_provider(Some(ctx))");
}

#[test]
fn age_222_select_provider_degrades_cached_quota_read_errors_to_missing_quota() {
    let (db, model) = unreadable_quota_fixture("age222-bad-quota");

    assert_cached_quota_read_fails(&db, "age222-bad-quota");
    let selected = select_cached(&model, &db);

    assert_selected_index(
        selected,
        0,
        "QuotaSnapshot currently treats cached quota read errors as None, so the unreadable marker is ignored",
    );
}

#[test]
fn age_222_select_provider_degrades_cached_window_read_errors_to_empty_windows() {
    let (db, model) = unreadable_window_fixture("age222-bad-window");

    assert_cached_window_read_fails(&db, "age222-bad-window");
    let selected = select_cached(&model, &db);

    assert_selected_index(
        selected,
        0,
        "QuotaSnapshot currently treats cached window read errors as an empty window list",
    );
}

#[test]
fn age_222_routing_refresh_inputs_calls_refresh_then_scan_per_provider_and_ignores_failures() {
    let (harness, log_path) = refresh_failure_order_harness();

    let selected = select_with_context(&harness);

    assert_selected_index(
        selected,
        0,
        "routing should continue with cached/missing state after refresh and scan failures",
    );
    assert_log_lines(
        &log_path,
        &[
            "quota:age222-order-a",
            "scan:age222-order-a",
            "quota:age222-order-b",
            "scan:age222-order-b",
        ],
        "refresh_routing_inputs currently performs stale routing refresh, then session scan, in provider order",
    );
}

#[test]
fn age_222_routing_verifies_marker_before_scan_and_next_provider() {
    let (harness, log_path) = marker_order_harness();

    let selected = select_with_context(&harness);

    assert_selected_index(selected, 0, "marker provider should remain selectable");
    assert_marker_cleared(&harness.db, "age222-marker-a");
    assert_log_lines(
        &log_path,
        &[
            "marker-refresh:age222-marker-a",
            "scan:age222-marker-a",
            "quota:age222-marker-b",
            "scan:age222-marker-b",
        ],
        "routing currently verifies provider A's marker before scanning A or advancing to provider B",
    );
}

#[test]
fn age_35_production_routing_service_matches_direct_select_provider_with_live_context() {
    let harness = live_parity_harness();

    let direct_index = select_with_context(&harness.direct);
    let service_index = select_service_with_context(&harness.service);

    assert_route_parity(
        service_index,
        direct_index,
        "production routing service must return the same provider index as direct select_provider",
    );
    assert_live_route_side_effects(&harness.direct.db, "direct select_provider");
    assert_live_route_side_effects(&harness.service.db, "ProductionRoutingService");
}

#[test]
fn age_35_production_routing_service_matches_direct_select_provider_cached_only() {
    let harness = cached_parity_harness();

    let direct_index = select_cached(&harness.model, &harness.direct_db);
    let service_index = select_service_cached(&harness.model, &harness.service_db);

    assert_route_parity(
        service_index,
        direct_index,
        "cached-only service routing must return the same provider index as direct select_provider",
    );
    assert_cached_route_has_no_live_side_effects(&harness.direct_db, "direct select_provider");
    assert_cached_route_has_no_live_side_effects(&harness.service_db, "ProductionRoutingService");
}

#[test]
fn age_35_routing_service_preserves_select_provider_topology_probe_drift_from_compute_projections()
{
    let harness = topology_drift_harness();

    let _ = select_service_with_context(&harness.service);
    compute_projections_with_context(&harness.projection);

    assert_topology_probe_drift(&harness.service.db, &harness.projection.db);
}
