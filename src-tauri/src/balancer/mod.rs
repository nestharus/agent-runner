use crate::config::{ModelConfig, ProvidersConfig};
use crate::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use crate::state::{QuotaRecord, StateDb};

const ERROR_WINDOW_MINUTES: i64 = 30;
const ERROR_THRESHOLD: u64 = 3;

/// Contextual dependencies for quota-aware balancing. When present,
/// `select_provider` will trigger a synchronous refresh for any provider
/// whose cached quota is stale (older than `REFRESH_TTL_HOURS`). Pass
/// `None` to use cached-only scoring (e.g. from inside an async handler
/// where blocking on a network call isn't desirable).
pub struct BalanceContext<'a> {
    pub providers_cfg: &'a ProvidersConfig,
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
    if let Some(ctx) = ctx {
        for p in &model.providers {
            if is_stale(state, &p.name) {
                // Swallow the result — a failed refresh just leaves stale
                // (or missing) data, which the fallback logic below handles.
                let _: RefreshOutcome =
                    refresh_provider(&p.name, ctx.providers_cfg, ctx.in_flight, state);
            }
        }
    }

    // 2) Gather quota snapshots for each provider (cached reads only).
    let quotas: Vec<Option<QuotaRecord>> = model
        .providers
        .iter()
        .map(|p| state.get_quota(&p.name).ok().flatten())
        .collect();

    // 3) If every provider has a quota reading, use quota-aware scoring.
    let all_have_quota = quotas.iter().all(|q| q.is_some());
    if all_have_quota {
        return score_by_quota(model, state, &quotas);
    }

    // 4) Otherwise, fall back to lifetime invocation-count scoring.
    score_by_invocation_count(model, state)
}

fn score_by_quota(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
) -> usize {
    let avg = global_avg_percent_per_call(quotas);

    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(model.providers.len());
    for (i, q) in quotas.iter().enumerate() {
        let q = q.as_ref().expect("all_have_quota was checked");

        // Skip providers with too many recent errors — quota headroom doesn't
        // help if calls keep failing.
        let recent_errors = state
            .recent_error_count(&model.name, i, ERROR_WINDOW_MINUTES)
            .unwrap_or(0);
        if recent_errors >= ERROR_THRESHOLD {
            scores.push((i, f64::MAX));
            continue;
        }

        // Project used_percent forward using calls since last refresh.
        // avg is Δpercent per Δcall; if we don't have a learned avg yet,
        // fall back to pure used_percent (calls term is zero-weighted).
        let projected = q.used_percent + (q.calls_since_refresh as f64) * avg;
        scores.push((i, projected));
    }

    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    if scores.iter().all(|(_, s)| *s == f64::MAX) {
        return round_robin_fallback(model, state);
    }
    scores[0].0
}

/// Compute a global avg percent-per-call across all providers that have a
/// valid delta. Returns 0.0 if no provider has learned a delta yet (first
/// refresh only — see bootstrap handling in the design doc).
fn global_avg_percent_per_call(quotas: &[Option<QuotaRecord>]) -> f64 {
    let mut total_percent = 0.0;
    let mut total_calls: u64 = 0;
    for q in quotas.iter().flatten() {
        if let (Some(dp), Some(dc)) = (q.last_delta_percent, q.last_delta_calls) {
            if dc > 0 && dp > 0.0 {
                total_percent += dp;
                total_calls += dc;
            }
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

        db.record_invocation("test", 0, true, 0, None, None)
            .unwrap();

        let second = select_provider(&model, &db, None);
        assert_eq!(second, 1);
    }

    #[test]
    fn avoids_errored_providers() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        for _ in 0..3 {
            db.record_invocation("test", 0, false, 1, None, None)
                .unwrap();
        }

        assert_eq!(select_provider(&model, &db, None), 1);
    }

    #[test]
    fn quota_scoring_picks_lowest_used_percent() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = three_provider_model();

        // All three providers have quota data: a=0.50, b=0.10, c=0.30.
        db.upsert_quota_refresh("a", 0.50, None).unwrap();
        db.upsert_quota_refresh("b", 0.10, None).unwrap();
        db.upsert_quota_refresh("c", 0.30, None).unwrap();

        assert_eq!(select_provider(&model, &db, None), 1);
    }

    #[test]
    fn quota_scoring_projects_using_calls_since_refresh() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = three_provider_model();

        // Seed quota readings with a learned delta: prior refresh saw +0.10
        // across 10 calls globally. avg = 0.01 percent/call.
        // Simulate that by doing two refreshes per provider so last_delta is set.
        db.upsert_quota_refresh("a", 0.0, None).unwrap();
        for _ in 0..10 {
            db.increment_calls_since_refresh("a").unwrap();
        }
        db.upsert_quota_refresh("a", 0.10, None).unwrap();

        // Now b and c also have fresh readings (no delta history).
        db.upsert_quota_refresh("b", 0.05, None).unwrap();
        db.upsert_quota_refresh("c", 0.08, None).unwrap();

        // a=0.10, b=0.05, c=0.08 → pick b.
        assert_eq!(select_provider(&model, &db, None), 1);

        // If b receives many calls since refresh, projection should push its
        // score above c's. avg from a's delta = 0.10/10 = 0.01.
        // b projected = 0.05 + 10 * 0.01 = 0.15 > c=0.08 → now pick c.
        for _ in 0..10 {
            db.increment_calls_since_refresh("b").unwrap();
        }
        assert_eq!(select_provider(&model, &db, None), 2);
    }

    #[test]
    fn falls_back_to_invocation_count_when_quota_missing() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        // Only one provider has a quota reading → fallback to invocation-count
        // where both are at 0.
        db.upsert_quota_refresh("a", 0.90, None).unwrap();

        // Provider b has no quota; fallback picks by invocation_count (both 0),
        // sorted stably → picks index 0.
        assert_eq!(select_provider(&model, &db, None), 0);

        // Record an invocation for 0 → fallback should now pick 1.
        db.record_invocation("test", 0, true, 0, None, None)
            .unwrap();
        assert_eq!(select_provider(&model, &db, None), 1);
    }
}
