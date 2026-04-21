use crate::config::{ModelConfig, ProvidersConfig, SessionsConfig};
use crate::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use crate::sessions::scan_provider;
use crate::state::{QuotaRecord, QuotaWindow, StateDb};
use chrono::Utc;

const ERROR_WINDOW_MINUTES: i64 = 30;
const ERROR_THRESHOLD: u64 = 3;

/// Contextual dependencies for quota-aware balancing. When present,
/// `select_provider` will trigger a synchronous refresh for any provider
/// whose cached quota is stale (older than `REFRESH_TTL_HOURS`) AND scan
/// each provider's CLI session logs for new turns. Pass `None` to use
/// cached-only scoring (e.g. from inside an async handler where blocking
/// on a network call isn't desirable).
pub struct BalanceContext<'a> {
    pub providers_cfg: &'a ProvidersConfig,
    pub sessions_cfg: &'a SessionsConfig,
    pub in_flight: &'a InFlight,
}

pub fn select_provider(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) -> usize {
    let n = model.providers.len();
    if n <= 1 {
        return 0;
    }

    // 1) Opportunistic refresh of any stale provider whose quota we can fetch.
    //    Only worthwhile when we're actually load-balancing (n > 1).
    //    Also scan CLI session logs so calls_since_refresh reflects ALL
    //    activity (agent-runner invocations + direct user UI prompts).
    if let Some(ctx) = ctx {
        for p in &model.providers {
            if is_stale(state, &p.name) {
                // Swallow the result — a failed refresh just leaves stale
                // (or missing) data, which the fallback logic below handles.
                let _: RefreshOutcome =
                    refresh_provider(&p.name, ctx.providers_cfg, ctx.in_flight, state);
            }
            // Session scan errors don't abort the pick — we just project with
            // a stale turn count instead of an up-to-date one.
            let _ = scan_provider(&p.name, ctx.sessions_cfg, state);
        }
    }

    // 2) Gather quota records + windows for each provider (cached reads only).
    let quotas: Vec<Option<QuotaRecord>> = model
        .providers
        .iter()
        .map(|p| state.get_quota(&p.name).ok().flatten())
        .collect();
    let windows: Vec<Vec<QuotaWindow>> = model
        .providers
        .iter()
        .map(|p| state.get_windows(&p.name).unwrap_or_default())
        .collect();

    // 3) If every provider has at least one window, use density scoring.
    let all_have_windows = windows.iter().all(|w| !w.is_empty());
    if all_have_windows {
        return score_by_density(model, state, &quotas, &windows);
    }

    // 4) Otherwise, fall back to lifetime invocation-count scoring.
    score_by_invocation_count(model, state)
}

/// Density-based scoring across multi-window quotas.
///
/// For each provider:
///   1. Project `used_percent` forward per window: `used + turns * avg`
///      (`avg` is the global learned percent-per-turn from refreshes).
///   2. Compute density per window: `(1 - projected_used) / hours_until_reset`
///      — remaining headroom per unit time. Higher = more slack.
///   3. Take the **binding constraint** = `min(density across windows)`.
///      The tightest window dictates how much room a provider really has.
///
/// Pick the provider with the **highest** binding density.
///
/// This handles two scenarios cleanly:
/// - Different reset days/times: density normalizes by how much time is left.
/// - Multi-tier rate limits (5h + weekly): a near-exhausted 5h window forces
///   that provider's density toward zero even if its weekly is fine.
fn score_by_density(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
) -> usize {
    let avg = global_avg_percent_per_call(quotas);
    let now = Utc::now();

    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(model.providers.len());
    for (i, ws) in windows.iter().enumerate() {
        // Drop providers that are erroring out — headroom doesn't help when
        // calls keep failing.
        let recent_errors = state
            .recent_error_count(&model.name, i, ERROR_WINDOW_MINUTES)
            .unwrap_or(0);
        if recent_errors >= ERROR_THRESHOLD {
            scores.push((i, f64::NEG_INFINITY));
            continue;
        }

        let q = quotas[i].as_ref();
        let turns = q
            .and_then(|q| {
                state
                    .count_assistant_turns_since(&model.providers[i].name, q.refreshed_at.as_ref())
                    .ok()
            })
            .unwrap_or(0);

        // Per-window density. Empty `min` is impossible — `all_have_windows`
        // gate is checked at the call site.
        let binding = ws
            .iter()
            .map(|w| {
                let projected = (w.used_percent + (turns as f64) * avg).clamp(0.0, 1.0);
                let remaining = (1.0 - projected).max(0.0);
                let hours = ((w.resets_at - now).num_seconds() as f64) / 3600.0;
                // Floor at a small epsilon so a window 1 second from reset
                // doesn't produce infinite density.
                let hours = hours.max(1.0 / 60.0);
                remaining / hours
            })
            .fold(f64::INFINITY, f64::min);
        scores.push((i, binding));
    }

    // Sort descending: highest binding density wins.
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    if scores.iter().all(|(_, s)| *s == f64::NEG_INFINITY) {
        return round_robin_fallback(model, state);
    }
    scores[0].0
}

/// Global avg percent-per-turn learned from refreshes. The delta is captured
/// in `provider_quotas.last_delta_percent / last_delta_calls`, recorded
/// against the longest window per provider (most stable signal). Sums across
/// all providers in the pool to avoid first-refresh blind spots.
fn global_avg_percent_per_call(quotas: &[Option<QuotaRecord>]) -> f64 {
    let mut total_percent = 0.0;
    let mut total_calls: u64 = 0;
    for q in quotas.iter().flatten() {
        if let (Some(dp), Some(dc)) = (q.last_delta_percent, q.last_delta_calls)
            && dc > 0
            && dp > 0.0
        {
            total_percent += dp;
            total_calls += dc;
        }
    }
    if total_calls == 0 {
        0.0
    } else {
        total_percent / total_calls as f64
    }
}

fn score_by_invocation_count(model: &ModelConfig, state: &StateDb) -> usize {
    let n = model.providers.len();
    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(n);

    for i in 0..n {
        let recent_errors = state
            .recent_error_count(&model.name, i, ERROR_WINDOW_MINUTES)
            .unwrap_or(0);

        if recent_errors >= ERROR_THRESHOLD {
            scores.push((i, f64::MAX));
            continue;
        }

        let invocation_count = state
            .get_provider(&model.name, i)
            .ok()
            .flatten()
            .map(|p| p.invocation_count)
            .unwrap_or(0);

        let error_penalty = recent_errors as f64 * 10.0;
        scores.push((i, invocation_count as f64 + error_penalty));
    }

    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    if scores.iter().all(|(_, s)| *s == f64::MAX) {
        return round_robin_fallback(model, state);
    }
    scores[0].0
}

fn round_robin_fallback(model: &ModelConfig, state: &StateDb) -> usize {
    let n = model.providers.len();
    let mut min_count = u64::MAX;
    let mut best = 0;

    for i in 0..n {
        let count = state
            .get_provider(&model.name, i)
            .ok()
            .flatten()
            .map(|p| p.invocation_count)
            .unwrap_or(0);
        if count < min_count {
            min_count = count;
            best = i;
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderConfig, model::PromptMode};
    use std::path::Path;
    use uuid::Uuid;

    fn record_invocation_for_test(
        db: &StateDb,
        model_name: &str,
        provider_name: &str,
        provider_index: usize,
        success: bool,
    ) {
        let id = db
            .start_invocation(&crate::state::InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: model_name.to_string(),
                provider_name: provider_name.to_string(),
                provider_index,
                parent_invocation_id: None,
            })
            .unwrap();
        db.finalize_invocation(id, success, if success { 0 } else { 1 }, None, None)
            .unwrap();
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
        }
    }

    fn three_provider_model() -> ModelConfig {
        ModelConfig {
            name: "test3".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                ProviderConfig::new("a", vec![]),
                ProviderConfig::new("b", vec![]),
                ProviderConfig::new("c", vec![]),
            ],
            inputs: vec![],
        }
    }

    #[test]
    fn single_provider_always_zero() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = ModelConfig {
            name: "single".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new("x", vec![])],
            inputs: vec![],
        };
        assert_eq!(select_provider(&model, &db, None), 0);
    }

    #[test]
    fn round_robin_on_fresh_state() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        let first = select_provider(&model, &db, None);
        assert_eq!(first, 0);

        record_invocation_for_test(&db, "test", "a", 0, true);

        let second = select_provider(&model, &db, None);
        assert_eq!(second, 1);
    }

    #[test]
    fn avoids_errored_providers() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        for _ in 0..3 {
            record_invocation_for_test(&db, "test", "a", 0, false);
        }

        assert_eq!(select_provider(&model, &db, None), 1);
    }

    fn quota_window(used: f64, hours_until_reset: i64) -> crate::state::QuotaWindowInput {
        use chrono::Duration;
        crate::state::QuotaWindowInput {
            used_percent: used,
            resets_at: Utc::now() + Duration::hours(hours_until_reset),
        }
    }

    fn one_window(used: f64, hours_until_reset: i64) -> Vec<crate::state::QuotaWindowInput> {
        vec![quota_window(used, hours_until_reset)]
    }

    fn seed_windows_with_deltas(
        db: &StateDb,
        provider_name: &str,
        windows: &[(f64, i64, f64, u64)],
    ) {
        let inputs: Vec<_> = windows
            .iter()
            .map(|(used, hours, _, _)| quota_window(*used, *hours))
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

    fn seed_assistant_turns_since_refresh(db: &StateDb, provider_name: &str, count: usize) {
        use chrono::Duration;

        let refreshed_at = Utc::now() - Duration::hours(1);
        db.set_refreshed_at_for_test(provider_name, &refreshed_at)
            .unwrap();
        let turns: Vec<_> = (0..count)
            .map(|i| crate::state::SessionTurnIngest {
                session_id: format!("{provider_name}-session"),
                turn_id: format!("{provider_name}-turn-{i}"),
                timestamp: refreshed_at + Duration::seconds((i + 1) as i64),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
            })
            .collect();
        db.ingest_session_turns_batch(provider_name, &turns)
            .unwrap();
    }

    fn selected_provider_index(model: &ModelConfig, db: &StateDb, risk_class: RiskClass) -> usize {
        select_provider(model, db, None, risk_class)
            .expect("provider should be selectable")
            .provider_index
    }

    fn selected_provider(model: &ModelConfig, db: &StateDb, risk_class: RiskClass) -> Selection {
        select_provider(model, db, None, risk_class).expect("provider should be selectable")
    }

    fn assert_approx(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {actual} to be within {tolerance} of {expected}"
        );
    }

    #[test]
    fn density_scoring_picks_lowest_used_when_windows_match() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = three_provider_model();

        // All three providers reset in the same window length (7d) so density
        // collapses to remaining-headroom comparison: a=0.50, b=0.10, c=0.30.
        // Highest remaining = b (0.90) → pick b.
        seed_windows_with_deltas(&db, "a", &[(0.50, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.10, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "c", &[(0.30, 24 * 7, 0.01, 22)]);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
    }

    #[test]
    fn density_picks_account_with_more_time_when_used_equal() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        // Both providers have learned equivalent burn rates and equal usage.
        // The account with more time to reset has more projected turns left.
        seed_windows_with_deltas(&db, "a", &[(0.50, 1, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.50, 24 * 7, 0.01, 22)]);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
    }

    #[test]
    fn binding_constraint_avoids_account_with_pressed_short_window() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22), (0.95, 5, 0.30, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.30, 24 * 7, 0.01, 22), (0.20, 5, 0.30, 22)]);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
    }

    #[test]
    fn falls_back_to_invocation_count_when_windows_missing() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.90, 24 * 7, 0.01, 22)]);
        record_invocation_for_test(&db, "test", "a", 0, true);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
    }

    #[test]
    fn high_weekly_account_stops_winning_after_cumulative_turns() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        let long_delta = 0.01;
        let long_calls = 22;

        seed_windows_with_deltas(
            &db,
            "a",
            &[
                (0.80, 24 * 7, long_delta, long_calls),
                (0.04, 5, 0.30, long_calls),
            ],
        );
        seed_windows_with_deltas(
            &db,
            "b",
            &[
                (0.10, 24 * 7, long_delta, long_calls),
                (0.85, 5, 0.30, long_calls),
            ],
        );
        seed_assistant_turns_since_refresh(&db, "a", 500);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
    }

    #[test]
    fn user_threshold_hides_provider_from_user_class_only() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.75, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.10, 1, 0.01, 22)]);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
        assert_eq!(
            selected_provider_index(&model, &db, RiskClass::Background),
            0
        );
    }

    #[test]
    fn user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.80, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.75, 24 * 7, 0.01, 22)]);

        let selection = selected_provider(&model, &db, RiskClass::User);
        assert_eq!(selection.provider_index, 1);
        assert!(selection.quota_tight_routing);
    }

    #[test]
    fn failure_threshold_hard_blocks_all_classes() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.96, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.20, 24 * 7, 0.01, 22)]);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
        assert_eq!(
            selected_provider_index(&model, &db, RiskClass::Background),
            1
        );
    }

    #[test]
    fn failure_threshold_returns_exhausted_not_roundrobin_when_all_fail() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.96, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.99, 24 * 7, 0.01, 22)]);

        for risk_class in [RiskClass::User, RiskClass::Background] {
            match select_provider(&model, &db, None, risk_class) {
                Err(BalanceError::Exhausted(err)) => {
                    assert_eq!(err.model_name, model.name);
                    assert_eq!(err.risk_class, risk_class);
                    assert_eq!(err.providers.len(), 2);
                    assert!(err.providers.iter().all(|provider| {
                        provider.projected_max_used_percent >= provider.failure_threshold
                            && provider.projected_max_used_percent >= 0.95
                    }));
                }
                other => panic!("expected exhausted error for {risk_class:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn per_window_burn_rate_projects_short_window_faster_than_long() {
        let long_rate = 0.01 / 22.0;
        let short_rate = long_rate * 30.0;

        let long_projected = project_used_percent_for_test(0.10, 100, long_rate);
        let short_projected = project_used_percent_for_test(0.10, 100, short_rate);

        assert_approx(short_projected - 0.10, (long_projected - 0.10) * 30.0, 1e-9);
        assert!(short_projected >= 0.95);
        assert!(long_projected < 0.95);
    }

    #[test]
    fn bootstrap_uses_sibling_pool_when_own_delta_absent() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        let sibling_rate = 0.012 / 24.0;

        db.upsert_quota_refresh("a", &one_window(0.20, 24 * 7))
            .unwrap();
        seed_windows_with_deltas(&db, "b", &[(0.30, 24 * 7, 0.012, 24)]);

        let bootstrapped = bootstrap_burn_rate_for_test(&model, &db, 0, 0).unwrap();
        assert_approx(bootstrapped, sibling_rate, 1e-12);
        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 0);
    }

    #[test]
    fn bootstrap_uses_duration_ratio_when_pool_has_only_long_delta() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        let long_rate = 0.01 / 22.0;
        let expected_short_rate = long_rate * (168.0 / 5.0);

        db.upsert_quota_refresh("a", &[quota_window(0.20, 24 * 7), quota_window(0.20, 5)])
            .unwrap();
        seed_windows_with_deltas(&db, "b", &[(0.30, 24 * 7, 0.01, 22)]);

        let bootstrapped = bootstrap_burn_rate_for_test(&model, &db, 0, 1).unwrap();
        assert_approx(
            bootstrapped,
            expected_short_rate,
            expected_short_rate * 0.10,
        );
    }

    #[test]
    fn bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio() {
        let long_rate = 0.01 / 22.0;
        let derived = bootstrap_duration_ratio_for_test(long_rate, 168.0, 5.0);

        assert_approx(derived, long_rate * (168.0 / 5.0), long_rate * 0.05);
        assert!(derived > long_rate);
    }

    #[test]
    fn bootstrap_returns_none_when_no_sibling_has_learned_rate() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        db.upsert_quota_refresh("a", &one_window(0.20, 24 * 7))
            .unwrap();
        db.upsert_quota_refresh("b", &one_window(0.30, 24 * 7))
            .unwrap();

        assert!(bootstrap_burn_rate_for_test(&model, &db, 0, 0).is_none());
    }

    #[test]
    fn unlearned_provider_is_ineligible_when_siblings_are_learned() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        db.upsert_quota_refresh("a", &one_window(0.10, 24 * 7))
            .unwrap();
        seed_windows_with_deltas(&db, "b", &[(0.90, 24 * 7, 0.01, 22)]);

        assert_eq!(selected_provider_index(&model, &db, RiskClass::User), 1);
    }

    #[test]
    fn fresh_pool_falls_through_to_invocation_count_round_robin() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        record_invocation_for_test(&db, "test", "a", 0, true);

        let selection = selected_provider(&model, &db, RiskClass::User);
        assert_eq!(selection.provider_index, 1);
        assert!(!selection.quota_tight_routing);
    }
}
