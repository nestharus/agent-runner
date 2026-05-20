//! AGE-162 Symptom 2 — A transient probe-endpoint 429 must NOT be
//! interpreted as an account-unavailability signal.
//!
//! Live incident (2026-05-19): `agents --usage` snapshot at T+0 showed
//! `claude4 window-0 (error: HTTP 429: rate-limited)`. Direct
//! `anthropic-usage ~/.claude4/.credentials.json` at T+30s returned
//! `{"used_percent":0.0}` for both windows — claude4 was fully available.
//! The 429 was on the quota-probe endpoint itself (parallel probes
//! hammered it), not on the account. The balancer apparently treated that
//! probe failure as an account-down signal and removed `claude4` from
//! rotation.
//!
//! Intended contract: a transient probe-script failure (429 / nonzero
//! exit / parse error) must NOT:
//!   1. Cause `select_provider` to err with AllProvidersQuotaExhausted
//!      when a healthier sibling exists.
//!   2. Cause the failing provider's cached `exhausted_at` to be set.
//!   3. Cause the failing provider to be excluded from the eligible set
//!      based on the probe failure alone.
//!   4. Cause its cached windows to be replaced with synthetic
//!      100%-used / exhausted rows.
//!
//! The intent is captured in `quota::refresh_provider_from_script`:
//! `RefreshOutcome::Failed` must leave routing state untouched. These
//! tests pin that contract from the balancer's point of view.

use chrono::{Duration, Utc};
use oulipoly_config::{
    ModelConfig, ProviderConfig, ProviderEntry, ProvidersConfig, SessionsConfig,
    model::PromptMode,
};
use oulipoly_runtime::balancer::{BalanceContext, select_provider};
use oulipoly_runtime::quota::InFlight;
use oulipoly_state::{QuotaWindowInput, StateDb};
use std::path::Path;

const CLAUDE4_PROBED: &str = "claude4_probed";
const SIBLING: &str = "sibling_healthy";

fn two_provider_model() -> ModelConfig {
    ModelConfig {
        name: "age162-probe-repro".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::model_provider(CLAUDE4_PROBED, vec![]),
            ProviderConfig::model_provider(SIBLING, vec![]),
        ],
        inputs: vec![],
    }
}

/// Quota script that emits the exact stderr signature observed live
/// (`HTTP 429: rate-limited`) and exits non-zero. Mirrors
/// `anthropic-usage`'s HTTP-error code path (`exit 4`).
fn probe_returns_429_script() -> String {
    "printf 'HTTP 429: rate-limited\\n' >&2; exit 4".to_string()
}

/// Quota script that emits a healthy `{"used_percent":0.0}` reading for
/// both windows.
fn healthy_probe_script() -> String {
    let weekly_resets = (Utc::now() + Duration::hours(6 * 24)).to_rfc3339();
    let burst_resets = (Utc::now() + Duration::hours(4)).to_rfc3339();
    format!(
        "printf '{{\"windows\":[\
            {{\"label\":\"weekly\",\"used_percent\":0,\"resets_at\":\"{weekly_resets}\"}},\
            {{\"label\":\"5h-burst\",\"used_percent\":0,\"resets_at\":\"{burst_resets}\"}}\
         ]}}'"
    )
}

/// Symptom 2 primary repro: probe failure alone must NOT exclude the
/// failing provider from rotation. This pins the contract that a
/// `RefreshOutcome::Failed` outcome leaves routing eligibility intact —
/// `claude4_probed` had healthy cached windows prior to the probe failure
/// (matching the live invariant: the account was actually at 0%/0%) and
/// must remain eligible after the failed refresh.
#[test]
fn age162_transient_probe_429_does_not_exclude_provider_with_healthy_prior_cache() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    // Prior healthy cache for both providers — matches the operator's
    // T+30s direct probe showing claude4 at 0%/0%.
    db.upsert_quota_refresh(
        CLAUDE4_PROBED,
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
    db.upsert_quota_refresh(
        SIBLING,
        &[QuotaWindowInput {
            used_percent: 0.30,
            resets_at: Utc::now() + Duration::hours(5 * 24),
        }],
    )
    .unwrap();
    // Force the routing path to consider both rows stale so the probe
    // refresh fires for the failing provider.
    db.set_refreshed_at_for_test(
        CLAUDE4_PROBED,
        &(Utc::now() - chrono::Duration::seconds(120)),
    )
    .unwrap();
    db.set_refreshed_at_for_test(SIBLING, &(Utc::now() - chrono::Duration::seconds(120)))
        .unwrap();

    let mut providers_cfg = ProvidersConfig::default();
    providers_cfg.entries.insert(
        CLAUDE4_PROBED.to_string(),
        ProviderEntry {
            quota_script: Some(probe_returns_429_script()),
            ..ProviderEntry::default()
        },
    );
    providers_cfg.entries.insert(
        SIBLING.to_string(),
        ProviderEntry {
            quota_script: Some(healthy_probe_script()),
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

    let _ = select_provider(&model, &db, Some(&ctx))
        .expect("with one provider's probe failing and a healthy sibling, select_provider must \
                 still return a provider, not AllProvidersQuotaExhausted");

    let claude4_quota = db
        .get_quota(CLAUDE4_PROBED)
        .unwrap()
        .expect("claude4_probed quota row must remain after a failed probe");
    assert!(
        claude4_quota.exhausted_at.is_none(),
        "AGE-162 Symptom 2: a transient probe 429 must NOT set exhausted_at on the \
         failing provider; the account was actually healthy (0%/0%). got exhausted_at={:?}",
        claude4_quota.exhausted_at
    );

    let claude4_windows = db.get_windows(CLAUDE4_PROBED).unwrap();
    assert_eq!(
        claude4_windows.len(),
        2,
        "prior healthy windows for claude4_probed must be preserved across a failed probe; \
         got {} windows",
        claude4_windows.len()
    );
    for window in &claude4_windows {
        assert!(
            window.used_percent < 1.0,
            "no preserved window may be at >=100% used after a failed probe; got \
             window_id={} used_percent={}",
            window.window_id,
            window.used_percent
        );
    }
}

/// Symptom 2 first-contact repro: probe failure on a provider with NO
/// prior cache must NOT remove the provider from rotation either.
///
/// This is the "fresh provisioning" path — operator added `claude4` to
/// the pool, the first quota probe got a 429 (parallel-probe storm), and
/// the balancer must NOT permanently exclude an account whose only
/// signal so far is one failed probe call.
#[test]
fn age162_transient_probe_429_does_not_exclude_first_contact_provider_with_healthy_sibling() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    // Sibling has a healthy seeded cache so the pool is not empty.
    db.upsert_quota_refresh(
        SIBLING,
        &[QuotaWindowInput {
            used_percent: 0.20,
            resets_at: Utc::now() + Duration::hours(5 * 24),
        }],
    )
    .unwrap();

    let mut providers_cfg = ProvidersConfig::default();
    providers_cfg.entries.insert(
        CLAUDE4_PROBED.to_string(),
        ProviderEntry {
            quota_script: Some(probe_returns_429_script()),
            ..ProviderEntry::default()
        },
    );
    providers_cfg.entries.insert(
        SIBLING.to_string(),
        ProviderEntry {
            quota_script: Some(healthy_probe_script()),
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

    select_provider(&model, &db, Some(&ctx)).expect(
        "first-contact probe failure on one provider with a healthy sibling must not \
         drop the entire pool to AllProvidersQuotaExhausted",
    );

    // After the failed first-contact probe, claude4_probed must not be
    // recorded as exhausted. The probe outcome carries no signal about
    // the account's true window state.
    if let Some(quota) = db.get_quota(CLAUDE4_PROBED).unwrap() {
        assert!(
            quota.exhausted_at.is_none(),
            "AGE-162 Symptom 2 first-contact: a failed first probe must not set \
             exhausted_at; got {:?}",
            quota.exhausted_at
        );
    }
    let claude4_windows = db.get_windows(CLAUDE4_PROBED).unwrap();
    for window in &claude4_windows {
        assert!(
            window.used_percent < 1.0,
            "no synthesized 100%-used window may be written from a failed first \
             probe; got window_id={} used_percent={}",
            window.window_id,
            window.used_percent
        );
    }
}
