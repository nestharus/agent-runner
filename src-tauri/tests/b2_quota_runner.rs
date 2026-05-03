#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;
mod fixtures;

use agent_runner_lib::config::{ProviderEntry, ProvidersConfig};
use agent_runner_lib::process::{OutputSpec, StdinSpec};
use agent_runner_lib::quota::{self, InFlight};
use b2_process_runner::{FakeProcessRunner, ok_output};
use fixtures::b1_state_repos::{CLAUDE_PROVIDER, StateRepoFixture};
use std::time::Duration;

fn providers_with(script: &str, auth_refresh: Option<&str>) -> ProvidersConfig {
    let mut providers = ProvidersConfig::default();
    providers.entries.insert(
        CLAUDE_PROVIDER.to_string(),
        ProviderEntry {
            quota_script: Some(script.to_string()),
            auth_refresh_command: auth_refresh.map(str::to_string),
            ..ProviderEntry::default()
        },
    );
    providers
}

fn quota_json() -> &'static str {
    r#"[{"window_id":0,"used_percent":0.25,"resets_at":"2027-01-01T00:00:00Z"}]"#
}

fn quota_windows_json() -> &'static str {
    r#"{"windows":[{"used_percent":0.25,"resets_at":"2027-01-01T00:00:00Z"}]}"#
}

/// Risk: T4 - shell command composition may drift while moving quota scripts
/// behind `ProcessRunner`; non-zero exits must remain caller-classified and
/// must not persist quota windows.
/// Level: component.
/// Source: B2 contract sections 3 and 5 quota refresh.
/// Observable: quota script uses `sh -c`, captures both streams, times out
/// after 30s, and a failed script leaves state unchanged.
#[test]
fn refresh_provider_uses_runner_for_quota_script_and_does_not_persist_on_failure() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let runner = FakeProcessRunner::with_responses(vec![ok_output("", "quota failed", 9)]);
    let providers = providers_with("quota-script --json", None);
    let in_flight = InFlight::new();

    let _outcome = quota::refresh_provider(CLAUDE_PROVIDER, &providers, &in_flight, &db, &runner);

    assert_eq!(fixture.quota_window_count(CLAUDE_PROVIDER), 0);
    let call = runner.single_call();
    assert_eq!(call.program, "sh");
    assert_eq!(call.args, ["-c", "quota-script --json"]);
    assert_eq!(call.stdin, StdinSpec::Null);
    assert_eq!(call.stdout, OutputSpec::Capture);
    assert_eq!(call.stderr, OutputSpec::Capture);
    assert_eq!(call.timeout, Some(Duration::from_secs(30)));
}

/// Risk: T4 - auth-refresh retry wiring can be lost when script spawning is
/// extracted; pre-B behavior retries the quota script once after a configured
/// auth refresh command succeeds.
/// Level: component.
/// Source: B2 contract sections 3 and 5 quota auth retry.
/// Observable: first script failure, auth refresh, and retry are three ordered
/// runner calls with the documented timeouts and output modes.
#[test]
fn refresh_provider_retries_script_after_auth_refresh_command() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("", "expired token", 1),
        ok_output("", "", 0),
        ok_output(quota_windows_json(), "", 0),
    ]);
    let providers = providers_with("quota-script --json", Some("auth-refresh"));
    let in_flight = InFlight::new();

    let _outcome = quota::refresh_provider(CLAUDE_PROVIDER, &providers, &in_flight, &db, &runner);

    assert_eq!(fixture.quota_window_count(CLAUDE_PROVIDER), 1);
    let calls = runner.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].args, ["-c", "quota-script --json"]);
    assert_eq!(calls[0].stdout, OutputSpec::Capture);
    assert_eq!(calls[0].timeout, Some(Duration::from_secs(30)));
    assert_eq!(calls[1].args, ["-c", "auth-refresh"]);
    assert_eq!(calls[1].stdout, OutputSpec::Null);
    assert_eq!(calls[1].stderr, OutputSpec::Capture);
    assert_eq!(calls[1].timeout, Some(Duration::from_secs(15)));
    assert_eq!(calls[2].args, ["-c", "quota-script --json"]);
}

/// Drift pin F-03: pre-B quota parsing accepted object envelopes only
/// (`{"windows": [...]}` or legacy single-window), not a top-level array.
#[test]
fn refresh_provider_rejects_bare_array_quota_script_output() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let runner = FakeProcessRunner::with_responses(vec![ok_output(quota_json(), "", 0)]);
    let providers = providers_with("quota-script --json", None);
    let in_flight = InFlight::new();

    let outcome = quota::refresh_provider(CLAUDE_PROVIDER, &providers, &in_flight, &db, &runner);

    match outcome {
        quota::RefreshOutcome::Failed(message) => {
            assert!(message.contains("Invalid JSON"), "unexpected error: {message}");
        }
        other => panic!("expected bare array output to fail, got {other:?}"),
    }
    assert_eq!(fixture.quota_window_count(CLAUDE_PROVIDER), 0);
}

/// Risk: T4/T14 - moving stale checks from concrete `StateDb` to
/// `QuotaRepository` may change the current fresh-window behavior.
/// Level: particular-integration.
/// Source: B2 contract section 3 `is_stale`.
/// Observable: a freshly seeded quota row is read through the repository trait
/// and is not stale.
#[test]
fn is_stale_reads_through_quota_repository() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    fixture.seed_quota_window(CLAUDE_PROVIDER);

    assert!(!quota::is_stale(&db, CLAUDE_PROVIDER));
}
