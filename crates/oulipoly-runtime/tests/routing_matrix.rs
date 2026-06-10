use chrono::{DateTime, Duration, Utc};
use oulipoly_config::{ModelConfig, ProviderConfig, model::PromptMode};
use oulipoly_runtime::services::{
    ProductionRoutingService, RoutingServicePort, RoutingServiceRequest, ServiceError,
};
use oulipoly_state::{InvocationStart, QuotaWindowInput, SessionTurnIngest, StateDb};
use std::path::Path;
use uuid::Uuid;

#[derive(Clone, Copy)]
enum WindowKind {
    FiveHour,
    SevenDay,
}

#[derive(Clone, Copy)]
enum RefreshProximity {
    AboutToRefresh,
    JustRefreshed,
    MidWindow,
}

#[derive(Clone, Copy)]
struct WindowFixture {
    kind: WindowKind,
    remaining_pct: u8,
    proximity: RefreshProximity,
    delta_percent: f64,
    delta_calls: u64,
}

impl WindowFixture {
    fn standard(kind: WindowKind, remaining_pct: u8, proximity: RefreshProximity) -> Self {
        let (delta_percent, delta_calls) = match kind {
            WindowKind::FiveHour => (0.30, 22),
            WindowKind::SevenDay => (0.01, 22),
        };
        Self {
            kind,
            remaining_pct,
            proximity,
            delta_percent,
            delta_calls,
        }
    }

    fn with_delta(mut self, delta_percent: f64, delta_calls: u64) -> Self {
        self.delta_percent = delta_percent;
        self.delta_calls = delta_calls;
        self
    }
}

fn in_memory_state(case_label: &str) -> StateDb {
    StateDb::open(Path::new(":memory:"))
        .unwrap_or_else(|err| panic!("case={case_label}: failed to open in-memory state DB: {err}"))
}

fn account_pool(n: usize) -> ModelConfig {
    ModelConfig {
        name: "age59-routing-matrix".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: (1..=n)
            .map(|i| ProviderConfig::model_provider(format!("claude{i}"), vec![]))
            .collect(),
        inputs: vec![],
        provider: None,
    }
}

fn provider_name(index_one_based: usize) -> String {
    format!("claude{index_one_based}")
}

fn remaining_to_used(remaining_pct: u8) -> f64 {
    (100.0 - f64::from(remaining_pct)) / 100.0
}

fn reset_at(kind: WindowKind, proximity: RefreshProximity) -> DateTime<Utc> {
    let now = Utc::now();
    match (kind, proximity) {
        (WindowKind::FiveHour, RefreshProximity::AboutToRefresh) => now + Duration::minutes(15),
        (WindowKind::FiveHour, RefreshProximity::JustRefreshed) => now + Duration::hours(5),
        (WindowKind::FiveHour, RefreshProximity::MidWindow) => now + Duration::minutes(150),
        (WindowKind::SevenDay, RefreshProximity::AboutToRefresh) => now + Duration::hours(6),
        (WindowKind::SevenDay, RefreshProximity::JustRefreshed) => now + Duration::hours(24 * 7),
        (WindowKind::SevenDay, RefreshProximity::MidWindow) => now + Duration::hours(24 * 7 / 2),
    }
}

fn seed_windows(db: &StateDb, provider_name: &str, windows: &[WindowFixture], case_label: &str) {
    let inputs = quota_window_inputs(windows);
    db.upsert_quota_refresh(provider_name, &inputs)
        .unwrap_or_else(|err| panic!("case={case_label}: failed to seed {provider_name}: {err}"));
    seed_window_deltas(db, provider_name, windows, case_label);
}

fn quota_window_inputs(windows: &[WindowFixture]) -> Vec<QuotaWindowInput> {
    windows.iter().map(quota_window_input).collect()
}

fn quota_window_input(window: &WindowFixture) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent: remaining_to_used(window.remaining_pct),
        resets_at: reset_at(window.kind, window.proximity),
    }
}

fn seed_window_deltas(
    db: &StateDb,
    provider_name: &str,
    windows: &[WindowFixture],
    case_label: &str,
) {
    for (window_id, window) in windows.iter().enumerate() {
        db.set_window_delta_for_test(
            provider_name,
            window_id as u32,
            window.delta_percent,
            window.delta_calls,
        )
        .unwrap_or_else(|err| {
            panic!("case={case_label}: failed to seed learned delta for {provider_name}: {err}")
        });
    }
}

fn seed_two_window_provider(
    db: &StateDb,
    provider_name: &str,
    seven_day_remaining: u8,
    five_hour_remaining: u8,
    case_label: &str,
) {
    seed_windows(
        db,
        provider_name,
        &[
            WindowFixture::standard(
                WindowKind::SevenDay,
                seven_day_remaining,
                RefreshProximity::MidWindow,
            ),
            WindowFixture::standard(
                WindowKind::FiveHour,
                five_hour_remaining,
                RefreshProximity::MidWindow,
            ),
        ],
        case_label,
    );
}

fn seed_uniform_two_window_pool(
    db: &StateDb,
    model: &ModelConfig,
    seven_day_remaining: u8,
    five_hour_remaining: u8,
    case_label: &str,
) {
    for provider in &model.providers {
        seed_two_window_provider(
            db,
            &provider.name,
            seven_day_remaining,
            five_hour_remaining,
            case_label,
        );
    }
}

fn seed_quota_axis_case(
    bucket: u8,
    axis: WindowKind,
    case_label: &str,
) -> (ModelConfig, StateDb, &'static str) {
    let model = account_pool(4);
    let db = in_memory_state(case_label);
    seed_two_window_provider(&db, "claude1", 25, 25, case_label);
    match axis {
        WindowKind::FiveHour => {
            seed_two_window_provider(&db, "claude2", 50, bucket, case_label);
            seed_two_window_provider(&db, "claude3", 50, 50, case_label);
        }
        WindowKind::SevenDay => {
            seed_two_window_provider(&db, "claude2", bucket, 50, case_label);
            seed_two_window_provider(&db, "claude3", 50, 50, case_label);
        }
    }
    seed_two_window_provider(&db, "claude4", 10, 10, case_label);
    let expected = if bucket >= 50 { "claude2" } else { "claude3" };
    (model, db, expected)
}

fn seed_binding_slice_case(
    target: (u8, u8),
    competitor: (u8, u8),
    expected: &'static str,
    case_label: &str,
) -> (ModelConfig, StateDb, &'static str) {
    let model = account_pool(4);
    let db = in_memory_state(case_label);
    seed_two_window_provider(&db, "claude1", 10, 10, case_label);
    seed_two_window_provider(&db, "claude2", target.0, target.1, case_label);
    seed_two_window_provider(&db, "claude3", competitor.0, competitor.1, case_label);
    seed_two_window_provider(&db, "claude4", 5, 5, case_label);
    (model, db, expected)
}

fn seed_turn_case(
    turns: usize,
    expected: &'static str,
    case_label: &str,
) -> (ModelConfig, StateDb, &'static str) {
    let model = account_pool(4);
    let db = in_memory_state(case_label);
    seed_windows(
        &db,
        "claude1",
        &[WindowFixture::standard(
            WindowKind::SevenDay,
            10,
            RefreshProximity::MidWindow,
        )],
        case_label,
    );
    seed_windows(
        &db,
        "claude2",
        &[
            WindowFixture::standard(WindowKind::SevenDay, 100, RefreshProximity::MidWindow)
                .with_delta(0.60, 60),
        ],
        case_label,
    );
    seed_windows(
        &db,
        "claude3",
        &[
            WindowFixture::standard(WindowKind::SevenDay, 40, RefreshProximity::MidWindow)
                .with_delta(0.60, 60),
        ],
        case_label,
    );
    seed_windows(
        &db,
        "claude4",
        &[WindowFixture::standard(
            WindowKind::SevenDay,
            5,
            RefreshProximity::MidWindow,
        )],
        case_label,
    );
    seed_assistant_turns_since_refresh(&db, "claude2", turns, case_label);
    (model, db, expected)
}

fn seed_assistant_turns_since_refresh(
    db: &StateDb,
    provider_name: &str,
    count: usize,
    case_label: &str,
) {
    let refreshed_at = Utc::now() - Duration::hours(2);
    db.set_refreshed_at_for_test(provider_name, &refreshed_at)
        .unwrap_or_else(|err| {
            panic!("case={case_label}: failed to backdate refresh for {provider_name}: {err}")
        });
    if count == 0 {
        // The zero-turn bucket intentionally leaves calls_since_refresh at the
        // fresh-window value written by upsert_quota_refresh.
    }
    for _ in 0..count {
        db.increment_calls_since_refresh(provider_name)
            .unwrap_or_else(|err| {
                panic!(
                    "case={case_label}: failed to seed calls_since_refresh for {provider_name}: {err}"
                )
            });
    }
    let turns = assistant_turn_ingests(provider_name, count, refreshed_at);
    db.ingest_session_turns_batch(provider_name, &turns)
        .unwrap_or_else(|err| {
            panic!("case={case_label}: failed to seed assistant turns for {provider_name}: {err}")
        });
}

fn assistant_turn_ingests(
    provider_name: &str,
    count: usize,
    refreshed_at: DateTime<Utc>,
) -> Vec<SessionTurnIngest> {
    (0..count)
        .map(|i| assistant_turn_ingest(provider_name, i, refreshed_at))
        .collect()
}

fn assistant_turn_ingest(
    provider_name: &str,
    index: usize,
    refreshed_at: DateTime<Utc>,
) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: format!("{provider_name}-session"),
        turn_id: format!("{provider_name}-turn-{index}"),
        timestamp: refreshed_at + Duration::seconds((index + 1) as i64),
        role: "assistant".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: None,
    }
}

fn seed_refresh_proximity_case(
    window_kind: WindowKind,
    proximity: RefreshProximity,
    expected: &'static str,
    case_label: &str,
) -> (ModelConfig, StateDb, &'static str) {
    let model = account_pool(4);
    let db = in_memory_state(case_label);
    seed_windows(
        &db,
        "claude1",
        &[WindowFixture::standard(
            window_kind,
            10,
            RefreshProximity::MidWindow,
        )],
        case_label,
    );
    seed_windows(
        &db,
        "claude2",
        &[WindowFixture::standard(window_kind, 99, proximity)],
        case_label,
    );
    seed_windows(
        &db,
        "claude3",
        &[WindowFixture::standard(
            window_kind,
            50,
            RefreshProximity::MidWindow,
        )],
        case_label,
    );
    seed_windows(
        &db,
        "claude4",
        &[WindowFixture::standard(
            window_kind,
            5,
            RefreshProximity::MidWindow,
        )],
        case_label,
    );
    (model, db, expected)
}

fn seed_last_used_case(age: Duration, case_label: &str) -> (ModelConfig, StateDb, &'static str) {
    let model = account_pool(4);
    let db = in_memory_state(case_label);
    seed_uniform_two_window_pool(&db, &model, 75, 75, case_label);
    record_successful_invocation(&db, &model, "claude2", 1, case_label);
    db.set_last_invoked_at_for_test(&model.name, "claude2", &(Utc::now() - age))
        .unwrap_or_else(|err| {
            panic!("case={case_label}: failed to backdate last_invoked_at: {err}")
        });
    (model, db, "claude1")
}

fn record_successful_invocation(
    db: &StateDb,
    model: &ModelConfig,
    provider_name: &str,
    provider_index: usize,
    case_label: &str,
) {
    let id = db
        .start_invocation(&invocation_start(model, provider_name, provider_index))
        .unwrap_or_else(|err| {
            panic!("case={case_label}: failed to start invocation for {provider_name}: {err}")
        });
    db.finalize_invocation(id, true, 0, None, None)
        .unwrap_or_else(|err| {
            panic!("case={case_label}: failed to finalize invocation for {provider_name}: {err}")
        });
}

fn invocation_start(
    model: &ModelConfig,
    provider_name: &str,
    provider_index: usize,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: model.name.clone(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id: None,
    }
}

fn assert_route_winner(
    model: &ModelConfig,
    db: &StateDb,
    expected_provider_name: &str,
    case_label: &str,
) {
    let output = ProductionRoutingService::new()
        .select_route(RoutingServiceRequest {
            model,
            state: db,
            ctx: None,
            provider_pin: None,
        })
        .unwrap_or_else(|err| panic!("case={case_label}: select_route failed: {err}"));
    let winner_name = model
        .providers
        .get(output.provider_index)
        .map(|provider| provider.name.clone())
        .expect("select_route returned an out-of-range provider index");
    assert_eq!(winner_name, expected_provider_name, "case={case_label}");
}

macro_rules! route_test {
    ($name:ident, $case_label:expr, $fixture:expr) => {
        #[tokio::test]
        async fn $name() {
            let case_label = $case_label;
            let (model, db, expected_provider_name) = ($fixture)(case_label);
            assert_route_winner(&model, &db, expected_provider_name, case_label);
        }
    };
}

fn account_count_case(n: usize, case_label: &str) -> (ModelConfig, StateDb, &'static str) {
    let model = account_pool(n);
    let db = in_memory_state(case_label);
    seed_uniform_two_window_pool(&db, &model, 75, 75, case_label);
    (model, db, "claude1")
}

route_test!(
    account_count_scaling_routes_4_accounts,
    "account_count=4",
    |case_label| account_count_case(4, case_label)
);
route_test!(
    account_count_scaling_routes_5_accounts,
    "account_count=5",
    |case_label| account_count_case(5, case_label)
);
route_test!(
    account_count_scaling_routes_6_accounts,
    "account_count=6",
    |case_label| account_count_case(6, case_label)
);
route_test!(
    account_count_scaling_routes_8_accounts,
    "account_count=8",
    |case_label| account_count_case(8, case_label)
);
route_test!(
    account_count_scaling_routes_12_accounts,
    "account_count=12",
    |case_label| account_count_case(12, case_label)
);
route_test!(
    account_count_scaling_routes_16_accounts,
    "account_count=16",
    |case_label| account_count_case(16, case_label)
);

route_test!(
    five_hour_remaining_bucket_0_percent,
    "5h_remaining=0",
    |case_label| seed_quota_axis_case(0, WindowKind::FiveHour, case_label)
);
route_test!(
    five_hour_remaining_bucket_1_percent,
    "5h_remaining=1",
    |case_label| seed_quota_axis_case(1, WindowKind::FiveHour, case_label)
);
route_test!(
    five_hour_remaining_bucket_25_percent,
    "5h_remaining=25",
    |case_label| seed_quota_axis_case(25, WindowKind::FiveHour, case_label)
);
route_test!(
    five_hour_remaining_bucket_50_percent,
    "5h_remaining=50",
    |case_label| seed_quota_axis_case(50, WindowKind::FiveHour, case_label)
);
route_test!(
    five_hour_remaining_bucket_75_percent,
    "5h_remaining=75",
    |case_label| seed_quota_axis_case(75, WindowKind::FiveHour, case_label)
);
route_test!(
    five_hour_remaining_bucket_99_percent,
    "5h_remaining=99",
    |case_label| seed_quota_axis_case(99, WindowKind::FiveHour, case_label)
);
route_test!(
    five_hour_remaining_bucket_100_percent,
    "5h_remaining=100",
    |case_label| seed_quota_axis_case(100, WindowKind::FiveHour, case_label)
);

route_test!(
    seven_day_remaining_bucket_0_percent,
    "7d_remaining=0",
    |case_label| seed_quota_axis_case(0, WindowKind::SevenDay, case_label)
);
route_test!(
    seven_day_remaining_bucket_1_percent,
    "7d_remaining=1",
    |case_label| seed_quota_axis_case(1, WindowKind::SevenDay, case_label)
);
route_test!(
    seven_day_remaining_bucket_25_percent,
    "7d_remaining=25",
    |case_label| seed_quota_axis_case(25, WindowKind::SevenDay, case_label)
);
route_test!(
    seven_day_remaining_bucket_50_percent,
    "7d_remaining=50",
    |case_label| seed_quota_axis_case(50, WindowKind::SevenDay, case_label)
);
route_test!(
    seven_day_remaining_bucket_75_percent,
    "7d_remaining=75",
    |case_label| seed_quota_axis_case(75, WindowKind::SevenDay, case_label)
);
route_test!(
    seven_day_remaining_bucket_99_percent,
    "7d_remaining=99",
    |case_label| seed_quota_axis_case(99, WindowKind::SevenDay, case_label)
);
route_test!(
    seven_day_remaining_bucket_100_percent,
    "7d_remaining=100",
    |case_label| seed_quota_axis_case(100, WindowKind::SevenDay, case_label)
);

route_test!(
    binding_slice_weekly_headroom_loses_to_pressed_short_window,
    "binding_slice=weekly_headroom_pressed_short",
    |case_label| seed_binding_slice_case((99, 1), (50, 75), "claude3", case_label)
);
route_test!(
    binding_slice_weekly_headroom_wins_when_short_window_neutral,
    "binding_slice=weekly_headroom_short_neutral",
    |case_label| seed_binding_slice_case((99, 75), (50, 75), "claude2", case_label)
);
route_test!(
    binding_slice_short_headroom_beats_weekly_headroom,
    "binding_slice=short_headroom_binding",
    |case_label| seed_binding_slice_case((75, 25), (50, 99), "claude3", case_label)
);
route_test!(
    binding_slice_zero_weekly_loses_even_with_full_short_window,
    "binding_slice=zero_weekly",
    |case_label| seed_binding_slice_case((0, 100), (25, 75), "claude3", case_label)
);
route_test!(
    binding_slice_one_percent_weekly_loses_to_balanced_account,
    "binding_slice=one_percent_weekly",
    |case_label| seed_binding_slice_case((1, 99), (25, 50), "claude3", case_label)
);
route_test!(
    binding_slice_full_weekly_loses_when_short_window_empty,
    "binding_slice=full_weekly_short_empty",
    |case_label| seed_binding_slice_case((100, 0), (25, 25), "claude3", case_label)
);

route_test!(
    turn_accumulation_zero_turns_keeps_headroom_account,
    "turns=0",
    |case_label| seed_turn_case(0, "claude2", case_label)
);
route_test!(
    turn_accumulation_one_turn_keeps_headroom_account,
    "turns=1",
    |case_label| seed_turn_case(1, "claude2", case_label)
);
route_test!(
    turn_accumulation_halfway_keeps_headroom_account,
    "turns=halfway",
    |case_label| seed_turn_case(30, "claude2", case_label)
);
route_test!(
    turn_accumulation_just_under_threshold_keeps_headroom_account,
    "turns=just_under_threshold",
    |case_label| seed_turn_case(59, "claude2", case_label)
);
route_test!(
    turn_accumulation_exact_threshold_keeps_lower_index_tie,
    "turns=exact_threshold",
    |case_label| seed_turn_case(60, "claude2", case_label)
);
route_test!(
    turn_accumulation_over_threshold_routes_to_competitor,
    "turns=over_threshold",
    |case_label| seed_turn_case(61, "claude3", case_label)
);

route_test!(
    refresh_proximity_five_hour_about_to_refresh_routes_to_stable_window,
    "refresh=5h_about_to_refresh",
    |case_label| seed_refresh_proximity_case(
        WindowKind::FiveHour,
        RefreshProximity::AboutToRefresh,
        "claude3",
        case_label
    )
);
route_test!(
    refresh_proximity_five_hour_just_refreshed_routes_to_headroom,
    "refresh=5h_just_refreshed",
    |case_label| seed_refresh_proximity_case(
        WindowKind::FiveHour,
        RefreshProximity::JustRefreshed,
        "claude2",
        case_label
    )
);
route_test!(
    refresh_proximity_five_hour_mid_window_routes_to_headroom,
    "refresh=5h_mid_window",
    |case_label| seed_refresh_proximity_case(
        WindowKind::FiveHour,
        RefreshProximity::MidWindow,
        "claude2",
        case_label
    )
);
route_test!(
    refresh_proximity_seven_day_about_to_refresh_routes_to_stable_window,
    "refresh=7d_about_to_refresh",
    |case_label| seed_refresh_proximity_case(
        WindowKind::SevenDay,
        RefreshProximity::AboutToRefresh,
        "claude3",
        case_label
    )
);
route_test!(
    refresh_proximity_seven_day_just_refreshed_routes_to_headroom,
    "refresh=7d_just_refreshed",
    |case_label| seed_refresh_proximity_case(
        WindowKind::SevenDay,
        RefreshProximity::JustRefreshed,
        "claude2",
        case_label
    )
);
route_test!(
    refresh_proximity_seven_day_mid_window_routes_to_headroom,
    "refresh=7d_mid_window",
    |case_label| seed_refresh_proximity_case(
        WindowKind::SevenDay,
        RefreshProximity::MidWindow,
        "claude2",
        case_label
    )
);

route_test!(
    last_used_recency_just_used_does_not_change_density_winner,
    "last_used=just_used",
    |case_label| seed_last_used_case(Duration::seconds(5), case_label)
);
route_test!(
    last_used_recency_30_seconds_ago_does_not_change_density_winner,
    "last_used=30s_ago",
    |case_label| seed_last_used_case(Duration::seconds(30), case_label)
);
route_test!(
    last_used_recency_5_minutes_ago_does_not_change_density_winner,
    "last_used=5m_ago",
    |case_label| seed_last_used_case(Duration::minutes(5), case_label)
);
route_test!(
    last_used_recency_1_hour_ago_does_not_change_density_winner,
    "last_used=1h_ago",
    |case_label| seed_last_used_case(Duration::hours(1), case_label)
);
route_test!(
    last_used_recency_6_hours_ago_does_not_change_density_winner,
    "last_used=6h_ago",
    |case_label| seed_last_used_case(Duration::hours(6), case_label)
);

route_test!(
    bug_suspect_six_account_high_weekly_headroom_target_wins,
    "bug_suspect=six_account_high_weekly_headroom",
    |case_label| {
        let model = account_pool(6);
        let db = in_memory_state(case_label);
        for index in 1..=5 {
            seed_two_window_provider(&db, &provider_name(index), 38, 75, case_label);
        }
        seed_two_window_provider(&db, "claude6", 99, 75, case_label);
        (model, db, "claude6")
    }
);

route_test!(
    bug_suspect_high_weekly_headroom_plus_recent_local_activity_wins,
    "bug_suspect=high_weekly_recent_local_activity",
    |case_label| {
        let model = account_pool(8);
        let db = in_memory_state(case_label);
        seed_uniform_two_window_pool(&db, &model, 40, 75, case_label);
        seed_two_window_provider(&db, "claude6", 100, 75, case_label);
        for _ in 0..8 {
            record_successful_invocation(&db, &model, "claude6", 5, case_label);
        }
        (model, db, "claude6")
    }
);

route_test!(
    bug_suspect_high_weekly_headroom_missing_5h_topology_wins_when_idle,
    "bug_suspect=high_weekly_missing_5h_idle",
    |case_label| {
        let model = account_pool(6);
        let db = in_memory_state(case_label);
        seed_uniform_two_window_pool(&db, &model, 36, 20, case_label);
        seed_windows(
            &db,
            "claude6",
            &[WindowFixture::standard(
                WindowKind::SevenDay,
                99,
                RefreshProximity::MidWindow,
            )],
            case_label,
        );
        (model, db, "claude6")
    }
);

route_test!(
    bug_suspect_high_weekly_headroom_pressed_5h_loses_to_healthier_short_window,
    "bug_suspect=high_weekly_pressed_5h",
    |case_label| {
        let model = account_pool(6);
        let db = in_memory_state(case_label);
        seed_uniform_two_window_pool(&db, &model, 25, 25, case_label);
        seed_two_window_provider(&db, "claude2", 50, 99, case_label);
        seed_two_window_provider(&db, "claude6", 100, 1, case_label);
        (model, db, "claude2")
    }
);

route_test!(
    bug_suspect_large_pool_fanout_obvious_low_usage_winner,
    "bug_suspect=large_pool_fanout_obvious_winner",
    |case_label| {
        let model = account_pool(16);
        let db = in_memory_state(case_label);
        seed_uniform_two_window_pool(&db, &model, 50, 50, case_label);
        seed_two_window_provider(&db, "claude12", 100, 100, case_label);
        (model, db, "claude12")
    }
);

route_test!(
    production_service_spot_check_account_count_axis,
    "service_spot=account_count_axis",
    |case_label| account_count_case(8, case_label)
);
route_test!(
    production_service_spot_check_quota_binding_axis,
    "service_spot=quota_binding_axis",
    |case_label| seed_binding_slice_case((99, 1), (50, 75), "claude3", case_label)
);
route_test!(
    production_service_spot_check_turn_projection_axis,
    "service_spot=turn_projection_axis",
    |case_label| seed_turn_case(61, "claude3", case_label)
);
route_test!(
    production_service_spot_check_refresh_proximity_axis,
    "service_spot=refresh_proximity_axis",
    |case_label| seed_refresh_proximity_case(
        WindowKind::SevenDay,
        RefreshProximity::AboutToRefresh,
        "claude3",
        case_label
    )
);
route_test!(
    production_service_spot_check_last_used_axis,
    "service_spot=last_used_axis",
    |case_label| seed_last_used_case(Duration::minutes(5), case_label)
);

#[tokio::test]
async fn production_service_reports_all_quota_exhausted_pool() {
    let case_label = "service_error=all_quota_exhausted";
    let model = account_pool(2);
    let db = in_memory_state(case_label);
    seed_two_window_provider(&db, "claude1", 0, 50, case_label);
    seed_two_window_provider(&db, "claude2", 50, 0, case_label);

    let err = ProductionRoutingService::new()
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &db,
            ctx: None,
            provider_pin: None,
        })
        .unwrap_err();

    assert!(
        matches!(err, ServiceError::Unavailable { .. }),
        "expected unavailable route error, got {err:?}"
    );
    assert!(
        err.to_string()
            .contains("all providers in pool age59-routing-matrix are quota-exhausted"),
        "{err}"
    );
}
