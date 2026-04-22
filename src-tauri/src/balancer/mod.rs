use crate::config::{ModelConfig, ProvidersConfig, SessionsConfig};
use crate::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use crate::sessions::scan_provider;
use crate::state::{QuotaRecord, QuotaWindow, StateDb};
use chrono::Utc;

const ERROR_WINDOW_MINUTES: i64 = 30;
const ERROR_THRESHOLD: u64 = 3;
const EPS_HOURS: f64 = 1.0 / 60.0;

#[derive(Debug, Clone)]
struct ProviderEval {
    index: usize,
    binding_score: Option<f64>,
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
    let all_indices: Vec<usize> = (0..n).collect();
    let filtered_indices: Vec<usize> = all_indices
        .iter()
        .copied()
        .filter(|i| {
            quotas
                .get(*i)
                .and_then(|quota| quota.as_ref())
                .and_then(|quota| quota.exhausted_at.as_ref())
                .is_none()
        })
        .collect();
    // If every provider is flagged exhausted, DO NOT restore all_indices and
    // route back into the known-bad pool via normal scoring — that spams
    // already-exhausted accounts on every invocation and contradicts the
    // user-locked "wait until refresh" invariant. Short-circuit and return
    // the oldest-exhausted index directly (most likely to have recovered
    // on its next successful refresh). Callers see this choice as
    // best-guess routing; the subprocess will report quota_exhausted
    // stderr as ground truth either way.
    if filtered_indices.is_empty() {
        return all_indices
            .into_iter()
            .min_by(|&a, &b| {
                let ta = quotas[a].as_ref().and_then(|q| q.exhausted_at.as_ref());
                let tb = quotas[b].as_ref().and_then(|q| q.exhausted_at.as_ref());
                // Oldest exhausted_at first; ties break on provider index.
                ta.cmp(&tb).then_with(|| a.cmp(&b))
            })
            .unwrap_or(0);
    }
    let candidates: &[usize] = filtered_indices.as_slice();

    // 3) If every provider has at least one window, use density scoring.
    let all_have_windows = candidates.iter().all(|i| !windows[*i].is_empty());
    if all_have_windows {
        return score_by_density(model, state, &quotas, &windows, candidates);
    }

    // 4) Otherwise, fall back to lifetime invocation-count scoring.
    score_by_invocation_count(model, state, candidates)
}

fn score_by_density(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    candidates: &[usize],
) -> usize {
    let now = Utc::now();
    let mut evals = Vec::with_capacity(candidates.len());

    // Compute the max number of live (not past-reset) windows any candidate
    // reports. A provider whose upstream API returns fewer windows than
    // siblings (observed 2026-04-22: `anthropic-usage` returns only the 7d
    // window for heavily-used accounts because Anthropic's API hides the 5h
    // timer when the account is near weekly cap) would otherwise dodge the
    // constraining short-tier term in `min_w` and beat its siblings on the
    // 7d-tier-only score — the exact opposite of what we want. Defensive
    // pessimism: when this provider has fewer live windows than the pool
    // max, penalize the binding as if the missing slots were at 1.0 used
    // (capacity effectively consumed).
    let pool_max_live_windows = candidates
        .iter()
        .map(|&i| windows[i].iter().filter(|w| w.resets_at > now).count())
        .max()
        .unwrap_or(0);

    for &i in candidates {
        let ws = &windows[i];
        let recent_errors = state
            .recent_error_count(&model.name, i, ERROR_WINDOW_MINUTES)
            .unwrap_or(0);
        if recent_errors >= ERROR_THRESHOLD {
            evals.push(ProviderEval {
                index: i,
                binding_score: None,
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
        let mut unlearned = false;
        let mut scored_window = false;

        // If a sibling reports more live windows than we do, penalize the
        // binding for each missing slot by folding in a worst-case
        // (0 remaining headroom) contribution. The exact hours figure
        // doesn't matter — zero headroom multiplied by any positive hours
        // is still zero, pulling binding to zero for this provider. Only
        // apply the penalty when at least one sibling DOES report more
        // live windows; pools where every provider has the same live
        // window count skip this branch and rank on their own data.
        let live_window_count = ws.iter().filter(|w| w.resets_at > now).count();
        if live_window_count < pool_max_live_windows {
            binding_score = binding_score.min(0.0);
            scored_window = true;
        }

        for window in ws {
            // Skip windows whose reset already happened: the stored
            // used_percent is from the prior window instance, so treating it
            // as "how much headroom remains" is wrong. The refresh loop
            // should replace this row on its next successful fetch; if the
            // scraper keeps returning empty (observed 2026-04-22 on claude3
            // when anthropic-usage returned `{"windows":[]}`), preserving
            // the past-reset row poisons the binding score by clamping
            // hours-until-reset to EPS_HOURS and torpedoing the provider.
            // Drop it from ranking entirely.
            if window.resets_at <= now {
                continue;
            }
            let Some(burn_rate) = bootstrap_burn_rate(i, window, quotas, windows) else {
                unlearned = true;
                continue;
            };

            let projected = project_used_percent(window.used_percent, turns, burn_rate);
            let hours = ((window.resets_at - now).num_seconds() as f64 / 3600.0).max(EPS_HOURS);
            let remaining_headroom = (1.0 - projected).max(0.0);
            binding_score = binding_score.min(remaining_headroom * hours);
            scored_window = true;
        }

        evals.push(ProviderEval {
            index: i,
            binding_score: if unlearned || !scored_window {
                None
            } else {
                Some(binding_score)
            },
            unlearned,
        });
    }

    let eligible: Vec<&ProviderEval> = evals
        .iter()
        .filter(|eval| !eval.unlearned && eval.binding_score.is_some())
        .collect();

    if eligible.is_empty() {
        return round_robin_fallback(model, state, candidates);
    }

    best_binding_score(&eligible).index
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

fn score_by_invocation_count(model: &ModelConfig, state: &StateDb, candidates: &[usize]) -> usize {
    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(candidates.len());

    for &i in candidates {
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
        return round_robin_fallback(model, state, candidates);
    }
    scores[0].0
}

fn round_robin_fallback(model: &ModelConfig, state: &StateDb, candidates: &[usize]) -> usize {
    debug_assert!(
        !candidates.is_empty(),
        "round_robin_fallback: caller must pass a non-empty candidates slice"
    );
    let mut min_count = u64::MAX;
    let mut best = candidates.first().copied().unwrap_or(0);

    for &i in candidates {
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

    fn selected_provider_index(model: &ModelConfig, db: &StateDb) -> usize {
        select_provider(model, db, None)
    }

    fn assert_approx(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {actual} to be within {tolerance} of {expected}"
        );
    }

    #[test]
    fn select_provider_filters_exhausted_accounts() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.60, 24 * 7, 0.01, 22)]);
        db.mark_exhausted("a").unwrap();

        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    #[test]
    fn all_providers_exhausted_picks_oldest_exhausted() {
        // CodeRabbit pass 1 finding: when every provider is flagged
        // exhausted, the prior "fall back to round-robin on invocation
        // count" behavior routed right back into the known-bad pool,
        // effectively retrying exhausted accounts on every invocation —
        // contradicting the user-locked "wait until refresh" invariant.
        // The new behavior picks whichever exhausted provider has the
        // oldest `exhausted_at` (most likely to have recovered on its
        // next refresh). Ties break by first-seen index via stable sort.
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        db.upsert_quota_refresh("a", &[]).unwrap();
        db.upsert_quota_refresh("b", &[]).unwrap();

        // Mark `b` exhausted FIRST (older timestamp), `a` second.
        db.mark_exhausted("b").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.mark_exhausted("a").unwrap();

        // Even though invocation count would prefer `b`, the older
        // exhausted flag makes `b` the better bet on next refresh.
        // Expected: `b` (index 1) wins.
        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    #[test]
    fn score_by_density_skips_past_reset_windows() {
        // Live-caught 2026-04-22: claude3 had a 5h window whose resets_at
        // was hours in the past (anthropic-usage returning empty kept the
        // stale row alive via PR #6's preserve-on-empty path). The stored
        // used_percent is from the previous window instance, so it has no
        // bearing on current headroom. Previously the code clamped
        // hours_until_reset to EPS_HOURS = 1/60h, which torpedoed the
        // provider's binding score to near-zero and made a low-usage
        // account (64% weekly) lose to a heavily-used one (91% weekly).
        // Now past-reset windows are skipped during binding computation.
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        // Provider `a`: healthy 7d window (low usage) + stale past-reset
        // 5h window.
        use chrono::Duration;
        let a_windows = vec![
            crate::state::QuotaWindowInput {
                used_percent: 0.10,
                resets_at: Utc::now() + Duration::hours(24 * 7),
            },
            crate::state::QuotaWindowInput {
                used_percent: 0.90,
                resets_at: Utc::now() - Duration::hours(1), // RESET PASSED
            },
        ];
        db.upsert_quota_refresh("a", &a_windows).unwrap();
        db.set_window_delta_for_test("a", 0, 0.01, 22).unwrap();

        // Provider `b`: heavily-used 7d window, nothing past-reset.
        seed_windows_with_deltas(&db, "b", &[(0.85, 24 * 7, 0.01, 22)]);

        // With past-reset skipping, `a` is ranked only on its 7d window
        // (much more headroom than b's 7d). Without the skip, a's
        // near-zero 5h binding would lose to b.
        assert_eq!(selected_provider_index(&model, &db), 0);
    }

    #[test]
    fn score_by_density_penalizes_provider_missing_window_siblings_have() {
        // Live-caught 2026-04-22: claude in the claude-opus pool had
        // only a 7d window reported (anthropic-usage returned 1 window
        // because Anthropic's API hides the 5h timer when the account
        // is near weekly cap), while claude2 and claude3 both had 2
        // windows. Claude's 7d was at 91% used — the MOST pressed
        // account in the pool. But with only one window to min over
        // vs siblings' two, claude's binding ((1-0.91)*41h ≈ 3.65)
        // beat claude3's min((1-0.64)*41h, (1-0.04)*3.6h) ≈ 3.46
        // simply because claude3's 5h tier pulled its binding down.
        // 10/10 live invocations routed to the near-exhausted account.
        //
        // Defensive pessimism: when a sibling reports more live windows
        // than this provider, assume the missing slots are fully
        // consumed (0 remaining headroom) and pull the provider's
        // binding to zero. The "hidden 5h window" observation
        // strongly correlates with "account is out of budget."
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        // Provider `a`: ONE 7d window (mimics claude's "hidden 5h"
        // state), moderately used.
        seed_windows_with_deltas(&db, "a", &[(0.50, 24 * 7, 0.01, 22)]);

        // Provider `b`: TWO windows (7d + 5h), more used on 7d than `a`.
        seed_windows_with_deltas(&db, "b", &[(0.60, 24 * 7, 0.01, 22), (0.30, 5, 0.01, 22)]);

        // Without the penalty, `a` would win (0.50 7d < 0.60 7d, and
        // no 5h tier to constrain it). With the penalty, `a`'s binding
        // is forced to 0 because it's missing a window siblings have,
        // so `b` wins.
        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    #[test]
    fn exhausted_filter_does_not_prevent_refresh_loop_from_clearing() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.60, 24 * 7, 0.01, 22)]);
        db.mark_exhausted("a").unwrap();
        db.mark_exhausted("b").unwrap();

        // Simulate a successful non-empty refresh for b. The production
        // refresh loop must make this same state transition before filtering.
        db.upsert_quota_refresh("b", &[quota_window(0.60, 24 * 7)])
            .unwrap();

        assert_eq!(selected_provider_index(&model, &db), 1);
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

        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    #[test]
    fn density_picks_account_with_more_time_when_used_equal() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        // Both providers have learned equivalent burn rates and equal usage.
        // The account with more time to reset has more projected turns left.
        seed_windows_with_deltas(&db, "a", &[(0.50, 1, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.50, 24 * 7, 0.01, 22)]);

        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    #[test]
    fn binding_constraint_avoids_account_with_pressed_short_window() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22), (0.95, 5, 0.30, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.30, 24 * 7, 0.01, 22), (0.20, 5, 0.30, 22)]);

        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    #[test]
    fn falls_back_to_invocation_count_when_windows_missing() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.90, 24 * 7, 0.01, 22)]);
        record_invocation_for_test(&db, "test", "a", 0, true);

        assert_eq!(selected_provider_index(&model, &db), 1);
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

        assert_eq!(selected_provider_index(&model, &db), 1);
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
        assert_eq!(selected_provider_index(&model, &db), 0);
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

        assert_eq!(selected_provider_index(&model, &db), 1);
    }
}
