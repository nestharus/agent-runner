#![cfg(unix)]

use agent_runner_lib::balancer::{self, BalanceEffects};
use agent_runner_lib::config::{ModelConfig, PromptMode, ProviderConfig};
use agent_runner_lib::state::{ProviderRecord, QuotaRecord, QuotaWindow, RoutingRepository};
use chrono::{DateTime, Utc};
use std::sync::Mutex;

fn model_with_providers(providers: &[&str]) -> ModelConfig {
    ModelConfig {
        name: "fixture-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: providers
            .iter()
            .map(|name| ProviderConfig::model_provider((*name).to_string(), Vec::new()))
            .collect(),
        inputs: Vec::new(),
    }
}

#[derive(Default)]
struct LoggingRoutingRepo {
    calls: Mutex<Vec<String>>,
}

impl LoggingRoutingRepo {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: impl Into<String>) {
        self.calls.lock().unwrap().push(call.into());
    }
}

impl RoutingRepository for LoggingRoutingRepo {
    fn get_provider(
        &self,
        _model_name: &str,
        provider_name: &str,
    ) -> Result<Option<ProviderRecord>, String> {
        self.record(format!("read_provider:{provider_name}"));
        Ok(None)
    }

    fn recent_error_count(
        &self,
        _model_name: &str,
        provider_name: &str,
        _window_minutes: i64,
    ) -> Result<u64, String> {
        self.record(format!("read_errors:{provider_name}"));
        Ok(0)
    }

    fn get_quota(&self, provider_name: &str) -> Result<Option<QuotaRecord>, String> {
        self.record(format!("read_quota:{provider_name}"));
        Ok(None)
    }

    fn get_windows(&self, provider_name: &str) -> Result<Vec<QuotaWindow>, String> {
        self.record(format!("read_windows:{provider_name}"));
        Ok(Vec::new())
    }

    fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        _since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        self.record(format!("read_turns:{provider_name}"));
        Ok(0)
    }
}

struct LoggingEffects<'a> {
    repo: &'a LoggingRoutingRepo,
}

impl BalanceEffects for LoggingEffects<'_> {
    fn refresh_quota_if_stale(&self, provider_name: &str) {
        self.repo.record(format!("refresh:{provider_name}"));
    }

    fn scan_provider_sessions(&self, provider_name: &str) {
        self.repo.record(format!("scan:{provider_name}"));
    }
}

fn assert_effects_before_quota_reads(calls: &[String], provider: &str) {
    let refresh = calls
        .iter()
        .position(|call| call == &format!("refresh:{provider}"))
        .expect("missing refresh effect");
    let scan = calls
        .iter()
        .position(|call| call == &format!("scan:{provider}"))
        .expect("missing scan effect");
    let first_quota_read = calls
        .iter()
        .position(|call| {
            call == &format!("read_quota:{provider}") || call == &format!("read_windows:{provider}")
        })
        .expect("missing quota/window read");

    assert!(refresh < first_quota_read, "{calls:?}");
    assert!(scan < first_quota_read, "{calls:?}");
}

/// Risk: T3 - balancer side effects can drift later than quota/window reads
/// when routing moves from concrete `StateDb` to repository traits.
/// Level: unit.
/// Source: proposal §8 T3; B3 contract §3 `balancer`.
/// Observable: every provider refresh/scan happens before that provider's
/// quota/window reads through `&dyn RoutingRepository`.
#[test]
fn select_provider_runs_balance_effects_before_quota_reads() {
    let model = model_with_providers(&["claude", "codex"]);
    let repo = LoggingRoutingRepo::default();
    let effects = LoggingEffects { repo: &repo };

    let _selected = balancer::select_provider(&model, &repo, Some(&effects));

    let calls = repo.calls();
    assert_effects_before_quota_reads(&calls, "claude");
    assert_effects_before_quota_reads(&calls, "codex");
}

/// Risk: T3 - projection computation repeats routing side effects and can keep
/// an old concrete path even after `select_provider` is migrated.
/// Level: unit.
/// Source: proposal §8 T3; B3 contract §4 call-site migration map.
/// Observable: `compute_projections` uses the same effect-before-read ordering.
#[test]
fn compute_projections_runs_balance_effects_before_quota_reads() {
    let model = model_with_providers(&["claude", "codex"]);
    let repo = LoggingRoutingRepo::default();
    let effects = LoggingEffects { repo: &repo };

    let projections = balancer::compute_projections(&model, &repo, Some(&effects));

    assert_eq!(projections.len(), 2);
    let calls = repo.calls();
    assert_effects_before_quota_reads(&calls, "claude");
    assert_effects_before_quota_reads(&calls, "codex");
}

/// Risk: T3 - the single-provider fast path can accidentally start refreshing
/// quota or scanning sessions during a DI refactor.
/// Level: unit.
/// Source: B3 contract §6 balancer edge cases.
/// Observable: one-provider selection returns index 0 without repository reads
/// or side-effect calls.
#[test]
fn single_provider_fast_path_does_not_call_effects_or_repository() {
    let model = model_with_providers(&["claude"]);
    let repo = LoggingRoutingRepo::default();
    let effects = LoggingEffects { repo: &repo };

    let selected = balancer::select_provider(&model, &repo, Some(&effects));

    assert_eq!(selected, 0);
    assert!(repo.calls().is_empty());
}
