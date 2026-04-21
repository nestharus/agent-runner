use crate::config::{ModelConfig, ProvidersConfig, SessionsConfig};
use crate::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use crate::sessions::scan_provider;
use crate::state::{QuotaRecord, QuotaWindow, StateDb};
use chrono::Utc;
use serde::{Deserialize, Serialize};

const ERROR_WINDOW_MINUTES: i64 = 30;
const ERROR_THRESHOLD: u64 = 3;
const EPS_HOURS: f64 = 1.0 / 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    User,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub provider_index: usize,
    pub quota_tight_routing: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BalanceError {
    Exhausted(ExhaustedError),
}

impl std::fmt::Display for BalanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BalanceError::Exhausted(_) => {
                write!(f, "All providers projected above failure_threshold")
            }
        }
    }
}

impl std::error::Error for BalanceError {}

#[derive(Debug, Clone, PartialEq)]
pub struct ExhaustedError {
    pub model_name: String,
    pub risk_class: RiskClass,
    pub providers: Vec<ExhaustedProviderInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExhaustedProviderInfo {
    pub provider_name: String,
    pub projected_max_used_percent: f64,
    pub failure_threshold: f64,
    pub user_threshold: f64,
}

#[derive(Debug, Clone)]
struct ProviderEval {
    index: usize,
    binding_score: Option<f64>,
    hard_blocked: bool,
    user_blocked: bool,
    max_projected_used_percent: f64,
    unlearned: bool,
}

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
    risk_class: RiskClass,
) -> Result<Selection, BalanceError> {
    let n = model.providers.len();
    if n <= 1 {
        return Ok(Selection {
            provider_index: 0,
            quota_tight_routing: false,
        });
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
        return score_by_density(model, state, &quotas, &windows, risk_class);
    }

    // 4) Otherwise, fall back to lifetime invocation-count scoring.
    Ok(Selection {
        provider_index: score_by_invocation_count(model, state),
        quota_tight_routing: false,
    })
}

fn score_by_density(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    risk_class: RiskClass,
) -> Result<Selection, BalanceError> {
    let now = Utc::now();
    let mut evals = Vec::with_capacity(model.providers.len());

    for (i, ws) in windows.iter().enumerate() {
        let recent_errors = state
            .recent_error_count(&model.name, i, ERROR_WINDOW_MINUTES)
            .unwrap_or(0);
        if recent_errors >= ERROR_THRESHOLD {
            evals.push(ProviderEval {
                index: i,
                binding_score: None,
                hard_blocked: true,
                user_blocked: true,
                max_projected_used_percent: 1.0,
                unlearned: false,
            });
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

        let mut binding_score = f64::INFINITY;
        let mut max_projected_used_percent = 0.0_f64;
        let mut hard_blocked = false;
        let mut user_blocked = false;
        let mut unlearned = false;
        let mut scored_window = false;

        for window in ws {
            let Some(burn_rate) = bootstrap_burn_rate(i, window, quotas, windows) else {
                unlearned = true;
                continue;
            };

            let projected = project_used_percent(window.used_percent, turns, burn_rate);
            max_projected_used_percent = max_projected_used_percent.max(projected);
            if projected >= model.balancer.failure_threshold {
                hard_blocked = true;
            }
            if projected >= model.balancer.user_threshold {
                user_blocked = true;
            }

            let hours = ((window.resets_at - now).num_seconds() as f64 / 3600.0).max(EPS_HOURS);
            let remaining_headroom = (1.0 - projected).max(0.0);
            binding_score = binding_score.min(remaining_headroom * hours);
            scored_window = true;
        }

        evals.push(ProviderEval {
            index: i,
            binding_score: if unlearned || hard_blocked || !scored_window {
                None
            } else {
                Some(binding_score)
            },
            hard_blocked,
            user_blocked,
            max_projected_used_percent,
            unlearned,
        });
    }

    let hard_eligible: Vec<&ProviderEval> = evals
        .iter()
        .filter(|eval| !eval.hard_blocked && !eval.unlearned)
        .collect();

    if hard_eligible.is_empty() {
        if evals.iter().all(|eval| eval.unlearned) && evals.iter().all(|eval| !eval.hard_blocked) {
            return Ok(Selection {
                provider_index: round_robin_fallback(model, state),
                quota_tight_routing: false,
            });
        }
        return Err(BalanceError::Exhausted(exhausted_error(
            model, risk_class, &evals,
        )));
    }

    let winner = if risk_class == RiskClass::User {
        let user_eligible: Vec<&ProviderEval> = hard_eligible
            .iter()
            .copied()
            .filter(|eval| !eval.user_blocked)
            .collect();
        if user_eligible.is_empty() {
            let winner = best_binding_score(&hard_eligible);
            return Ok(Selection {
                provider_index: winner.index,
                quota_tight_routing: true,
            });
        }
        best_binding_score(&user_eligible)
    } else {
        best_binding_score(&hard_eligible)
    };

    Ok(Selection {
        provider_index: winner.index,
        quota_tight_routing: false,
    })
}

fn best_binding_score<'a>(evals: &[&'a ProviderEval]) -> &'a ProviderEval {
    debug_assert!(!evals.is_empty(), "best_binding_score: empty slice");
    debug_assert!(
        evals.iter().all(|e| e.binding_score.is_some()),
        "best_binding_score: caller must filter to providers with a learned binding_score"
    );
    evals
        .iter()
        .copied()
        .max_by(|a, b| {
            a.binding_score
                .unwrap()
                .partial_cmp(&b.binding_score.unwrap())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap()
}

fn exhausted_error(
    model: &ModelConfig,
    risk_class: RiskClass,
    evals: &[ProviderEval],
) -> ExhaustedError {
    ExhaustedError {
        model_name: model.name.clone(),
        risk_class,
        providers: evals
            .iter()
            .map(|eval| ExhaustedProviderInfo {
                provider_name: model.providers[eval.index].name.clone(),
                projected_max_used_percent: eval.max_projected_used_percent,
                failure_threshold: model.balancer.failure_threshold,
                user_threshold: model.balancer.user_threshold,
            })
            .collect(),
    }
}

fn project_used_percent(base_used_percent: f64, turns: u64, burn_rate: f64) -> f64 {
    (base_used_percent + (turns as f64) * burn_rate).max(0.0)
}

fn learned_rate(window: &QuotaWindow) -> Option<f64> {
    match (window.last_delta_percent, window.last_delta_calls) {
        (Some(delta_percent), Some(delta_calls)) if delta_percent > 0.0 && delta_calls > 0 => {
            Some(delta_percent / delta_calls as f64)
        }
        _ => None,
    }
}

fn bootstrap_burn_rate(
    provider_index: usize,
    window: &QuotaWindow,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
) -> Option<f64> {
    learned_rate(window)
        .or_else(|| pool_window_avg_percent_per_call(window.window_id, windows))
        .or_else(|| {
            duration_ratio_fallback_percent_per_call(provider_index, window, quotas, windows)
        })
}

fn pool_window_avg_percent_per_call(window_id: u32, windows: &[Vec<QuotaWindow>]) -> Option<f64> {
    let mut total_percent = 0.0;
    let mut total_calls: u64 = 0;
    for window in windows.iter().flatten() {
        if window.window_id == window_id
            && let (Some(delta_percent), Some(delta_calls)) =
                (window.last_delta_percent, window.last_delta_calls)
            && delta_percent > 0.0
            && delta_calls > 0
        {
            total_percent += delta_percent;
            total_calls += delta_calls;
        }
    }
    (total_calls > 0).then_some(total_percent / total_calls as f64)
}

fn duration_ratio_fallback_percent_per_call(
    provider_index: usize,
    target_window: &QuotaWindow,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
) -> Option<f64> {
    let target_refreshed_at = quotas
        .get(provider_index)
        .and_then(|quota| quota.as_ref())
        .and_then(|quota| quota.refreshed_at.as_ref())?;
    let target_hours = ((target_window.resets_at - *target_refreshed_at).num_seconds() as f64
        / 3600.0)
        .max(EPS_HOURS);

    let mut best: Option<(f64, f64)> = None;
    for (i, provider_windows) in windows.iter().enumerate() {
        let Some(refreshed_at) = quotas
            .get(i)
            .and_then(|quota| quota.as_ref())
            .and_then(|quota| quota.refreshed_at.as_ref())
        else {
            continue;
        };
        for window in provider_windows {
            let Some(rate) = learned_rate(window) else {
                continue;
            };
            let long_hours =
                ((window.resets_at - *refreshed_at).num_seconds() as f64 / 3600.0).max(EPS_HOURS);
            if long_hours <= target_hours {
                continue;
            }
            if best.is_none_or(|(_, best_hours)| long_hours > best_hours) {
                best = Some((rate, long_hours));
            }
        }
    }

    best.map(|(rate, long_hours)| duration_ratio_rate(rate, long_hours, target_hours))
}

fn duration_ratio_rate(long_rate: f64, long_hours: f64, target_hours: f64) -> f64 {
    long_rate * (long_hours / target_hours.max(EPS_HOURS))
}

#[cfg(test)]
pub(crate) fn project_used_percent_for_test(
    base_used_percent: f64,
    turns: u64,
    burn_rate: f64,
) -> f64 {
    project_used_percent(base_used_percent, turns, burn_rate)
}

#[cfg(test)]
pub(crate) fn bootstrap_burn_rate_for_test(
    model: &ModelConfig,
    state: &StateDb,
    provider_index: usize,
    window_id: u32,
) -> Option<f64> {
    let quotas: Vec<Option<QuotaRecord>> = model
        .providers
        .iter()
        .map(|provider| state.get_quota(&provider.name).ok().flatten())
        .collect();
    let windows: Vec<Vec<QuotaWindow>> = model
        .providers
        .iter()
        .map(|provider| state.get_windows(&provider.name).unwrap_or_default())
        .collect();
    let target = windows
        .get(provider_index)?
        .iter()
        .find(|window| window.window_id == window_id)?;
    bootstrap_burn_rate(provider_index, target, &quotas, &windows)
}

#[cfg(test)]
pub(crate) fn bootstrap_duration_ratio_for_test(
    long_rate: f64,
    long_hours: f64,
    target_hours: f64,
) -> f64 {
    duration_ratio_rate(long_rate, long_hours, target_hours)
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
                quota_tight_routing: false,
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
            balancer: Default::default(),
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
            balancer: Default::default(),
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
            balancer: Default::default(),
            inputs: vec![],
        };
        assert_eq!(
            select_provider(&model, &db, None, RiskClass::User)
                .unwrap()
                .provider_index,
            0
        );
    }

    #[test]
    fn round_robin_on_fresh_state() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        let first = select_provider(&model, &db, None, RiskClass::User)
            .unwrap()
            .provider_index;
        assert_eq!(first, 0);

        record_invocation_for_test(&db, "test", "a", 0, true);

        let second = select_provider(&model, &db, None, RiskClass::User)
            .unwrap()
            .provider_index;
        assert_eq!(second, 1);
    }

    #[test]
    fn avoids_errored_providers() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        for _ in 0..3 {
            record_invocation_for_test(&db, "test", "a", 0, false);
        }

        assert_eq!(
            select_provider(&model, &db, None, RiskClass::User)
                .unwrap()
                .provider_index,
            1
        );
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

    // Intentionally no test for the "A unlearned while B learned" case.
    //
    // The §Q3 bootstrap cascade makes that state unreachable when
    // siblings share a quota_script (the normal pool configuration):
    // step 2 matches by window_id and rescues A from any same-slot
    // sibling delta, and step 3 rescues short-window gaps from any
    // longer-duration sibling rate. The only state where A is unlearned
    // but some sibling is learned requires providers to emit mismatched
    // window_id layouts, which is off-pattern and already covered by
    // other tests (#11 sibling rescue, #12 duration-ratio rescue, #14
    // no learning anywhere, #16 fresh pool round-robin). Do not
    // resurrect this slot without first amending the cascade design.

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
