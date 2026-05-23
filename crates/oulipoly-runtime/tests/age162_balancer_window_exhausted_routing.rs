//! AGE-162 Symptom 1 — Balancer must not route to a provider whose cached
//! state shows a live window at or over 100% used, when a healthier sibling
//! is available.
//!
//! Live incident (2026-05-19): `agents -m claude-opus -f <prompt>` was routed
//! to provider account `claude5`. Snapshot at the same moment:
//!
//!   - claude5 window-0:  24% used (76% remaining) — healthy
//!   - claude5 window-1: 100% used  (0% remaining) — exhausted
//!
//! `claude4` was 0%/0% (fully available). The runtime still picked `claude5`
//! and the dispatch exited code 1 with `[diagnostics] rate_limit`.
//!
//! The documented contract (balancer/mod.rs::provider_is_quota_exhausted +
//! select_provider_hard_excludes_accounts_at_or_over_live_window_quota) is
//! that ANY live window at >= 100% used must hard-exclude the provider from
//! routing eligibility. These tests reproduce the live-snapshot inputs and
//! assert the healthier sibling is selected.

use chrono::{Duration, Utc};
use oulipoly_config::{
    ModelConfig, ProviderConfig, ProviderEntry, ProvidersConfig, SessionsConfig, model::PromptMode,
};
use oulipoly_runtime::balancer::{BalanceContext, select_provider};
use oulipoly_runtime::quota::InFlight;
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::path::Path;

const CLAUDE5_REPRO: &str = "claude5_repro";
const CLAUDE4_REPRO: &str = "claude4_repro";

fn two_provider_model() -> ModelConfig {
    ModelConfig {
        name: "age162-repro".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::model_provider(CLAUDE5_REPRO, vec![]),
            ProviderConfig::model_provider(CLAUDE4_REPRO, vec![]),
        ],
        inputs: vec![],
        provider: None,
    }
}

/// Seed the live-snapshot windows for `claude5_repro`: 7-day window at 24%
/// used (still live, ~3 days to reset) and 5-hour window at 100% used (still
/// live, ~2h to reset). Both `resets_at` must be in the future so the
/// `window.resets_at > now` live-window guard remains active.
fn seed_claude5_live_snapshot(db: &StateDb) {
    db.upsert_quota_refresh(
        CLAUDE5_REPRO,
        &[
            QuotaWindowInput {
                used_percent: 0.24,
                resets_at: Utc::now() + Duration::hours(3 * 24),
            },
            QuotaWindowInput {
                used_percent: 1.00,
                resets_at: Utc::now() + Duration::hours(2),
            },
        ],
    )
    .unwrap();
}

fn seed_claude4_healthy(db: &StateDb) {
    db.upsert_quota_refresh(
        CLAUDE4_REPRO,
        &[
            QuotaWindowInput {
                used_percent: 0.00,
                resets_at: Utc::now() + Duration::hours(6 * 24),
            },
            QuotaWindowInput {
                used_percent: 0.00,
                resets_at: Utc::now() + Duration::hours(4),
            },
        ],
    )
    .unwrap();
}

/// Repro via cache-direct seeding. Mirrors the live snapshot exactly:
/// claude5_repro has a live window at 100% used; claude4_repro is 0%/0%.
/// The documented contract is hard-exclusion of any provider with a live
/// >=100% window when a healthier sibling exists.
#[test]
fn age162_select_provider_does_not_route_to_window_exhausted_account_with_healthy_sibling() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_claude5_live_snapshot(&db);
    seed_claude4_healthy(&db);

    let selected_index = select_provider(&model, &db, None)
        .expect("balancer must return a healthy provider, not AllProvidersQuotaExhausted");
    let selected_name = &model.providers[selected_index].name;

    assert_eq!(
        selected_name, CLAUDE4_REPRO,
        "live snapshot reproduces AGE-162 Symptom 1: {CLAUDE5_REPRO} has a live window at 100% \
         used; {CLAUDE4_REPRO} is healthy. The balancer must hard-exclude {CLAUDE5_REPRO} and \
         route to {CLAUDE4_REPRO}. (actual selection={selected_name})"
    );
    assert_ne!(
        selected_name, CLAUDE5_REPRO,
        "the live-exhausted account must NEVER be selected while a healthier sibling exists"
    );
}

/// Repro via the refresh path. The script-emitted JSON mirrors what real
/// `anthropic-usage` produces — `used_percent` on a 0..100 scale, both
/// `resets_at` in the future. The balancer must hard-exclude
/// `claude5_repro` and route to `claude4_repro`.
///
/// Per the AGE-162 incident brief: if direct-seed exclusion already works,
/// the production failure must originate in the refresh path (script ->
/// parse_output -> upsert_quota_refresh -> select). This test runs that
/// chain end-to-end and asserts the same selection contract.
#[test]
fn age162_select_provider_with_refresh_context_picks_healthy_sibling_over_window_exhausted_account()
{
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    let claude5_resets_window_1 = (Utc::now() + Duration::hours(2)).to_rfc3339();
    let claude5_resets_window_0 = (Utc::now() + Duration::hours(3 * 24)).to_rfc3339();
    let claude4_resets_window_1 = (Utc::now() + Duration::hours(4)).to_rfc3339();
    let claude4_resets_window_0 = (Utc::now() + Duration::hours(6 * 24)).to_rfc3339();

    let claude5_script = format!(
        "printf '{{\"windows\":[\
            {{\"label\":\"weekly\",\"used_percent\":24,\"resets_at\":\"{claude5_resets_window_0}\"}},\
            {{\"label\":\"5h-burst\",\"used_percent\":100,\"resets_at\":\"{claude5_resets_window_1}\"}}\
         ]}}'"
    );
    let claude4_script = format!(
        "printf '{{\"windows\":[\
            {{\"label\":\"weekly\",\"used_percent\":0,\"resets_at\":\"{claude4_resets_window_0}\"}},\
            {{\"label\":\"5h-burst\",\"used_percent\":0,\"resets_at\":\"{claude4_resets_window_1}\"}}\
         ]}}'"
    );

    let mut providers_cfg = ProvidersConfig::default();
    providers_cfg.entries.insert(
        CLAUDE5_REPRO.to_string(),
        ProviderEntry {
            quota_script: Some(claude5_script),
            ..ProviderEntry::default()
        },
    );
    providers_cfg.entries.insert(
        CLAUDE4_REPRO.to_string(),
        ProviderEntry {
            quota_script: Some(claude4_script),
            ..ProviderEntry::default()
        },
    );
    let sessions_cfg = SessionsConfig::default();
    let in_flight = InFlight::new();

    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };

    let selected_index = select_provider(&model, &db, Some(&ctx))
        .expect("balancer must return a healthy provider, not AllProvidersQuotaExhausted");
    let selected_name = &model.providers[selected_index].name;

    assert_eq!(
        selected_name, CLAUDE4_REPRO,
        "after refresh path runs the claude5_repro script (returning window-1 at 100% used) \
         and the claude4_repro script (returning 0%/0%), the balancer MUST route to \
         {CLAUDE4_REPRO}. (actual selection={selected_name})"
    );

    let windows = db.get_windows(CLAUDE5_REPRO).unwrap();
    assert_eq!(
        windows.len(),
        2,
        "refresh path must have written both windows from the script output for {CLAUDE5_REPRO}"
    );
    let window_at_100 = windows
        .iter()
        .find(|w| w.used_percent >= 1.0)
        .expect("one window must be at 100% used per the live snapshot");
    assert!(
        window_at_100.resets_at > Utc::now(),
        "the 100% used window's resets_at must be in the future (live), not past-reset"
    );
}
