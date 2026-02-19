use crate::config::ModelConfig;
use crate::state::StateDb;

const ERROR_WINDOW_MINUTES: i64 = 30;
const ERROR_THRESHOLD: u64 = 3;

pub fn select_provider(model: &ModelConfig, state: &StateDb) -> usize {
    let n = model.providers.len();
    if n <= 1 {
        return 0;
    }

    // Collect stats for each provider
    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(n);

    for i in 0..n {
        let recent_errors = state
            .recent_error_count(&model.name, i, ERROR_WINDOW_MINUTES)
            .unwrap_or(0);

        // Providers with too many recent errors get a penalty
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

        // Score = invocation_count + error_penalty
        // Lower is better (round-robin effect: pick least-used)
        let error_penalty = recent_errors as f64 * 10.0;
        scores.push((i, invocation_count as f64 + error_penalty));
    }

    // Pick the provider with the lowest score
    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    // If all providers are penalized, fall back to the one with fewest total invocations
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
                ProviderConfig {
                    command: "a".to_string(),
                    args: vec![],
                },
                ProviderConfig {
                    command: "b".to_string(),
                    args: vec![],
                },
            ],
        }
    }

    #[test]
    fn single_provider_always_zero() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = ModelConfig {
            name: "single".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig {
                command: "x".to_string(),
                args: vec![],
            }],
        };
        assert_eq!(select_provider(&model, &db), 0);
    }

    #[test]
    fn round_robin_on_fresh_state() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        // Both at 0 invocations, should pick 0
        let first = select_provider(&model, &db);
        assert_eq!(first, 0);

        // Record invocation for provider 0
        db.record_invocation("test", 0, true, 0, None, None)
            .unwrap();

        // Now should pick provider 1 (fewer invocations)
        let second = select_provider(&model, &db);
        assert_eq!(second, 1);
    }

    #[test]
    fn avoids_errored_providers() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        // Provider 0 has 3 recent errors
        for _ in 0..3 {
            db.record_invocation("test", 0, false, 1, None, None)
                .unwrap();
        }

        // Should avoid provider 0
        assert_eq!(select_provider(&model, &db), 1);
    }
}
