#![cfg(unix)]

mod fixtures;

use agent_runner_lib::process::{OutputSpec, StdinSpec};
use agent_runner_lib::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use agent_runner_lib::state::QuotaRepository;
use fixtures::b1_state_repos::{PROVIDER, StateRepoFixture};
use fixtures::b2_process_runner::*;
use std::time::Duration;

/// Risk: T4 (quota refresh persists parsed windows through repository)
/// Source: proposal §8 T4; contract §3 quota::refresh_provider
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn quota_refresh_provider_uses_runner_shell_spec_and_persists_windows() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;
    let runner = FakeProcessRunner::new();
    runner.push_stdout(br#"{"windows":[{"used_percent":25,"resets_at":"2099-01-01T00:00:00Z"}]}"#);
    let providers = quota_providers_config(PROVIDER, Some("quota command"), None);
    let in_flight = InFlight::new();

    let outcome = refresh_provider(PROVIDER, &providers, &in_flight, repo, &runner);

    match outcome {
        RefreshOutcome::Updated { windows } => {
            assert_eq!(windows.len(), 1);
            assert!((windows[0].used_percent - 0.25).abs() < 1e-6);
        }
        other => panic!("expected updated refresh outcome, got {other:?}"),
    }
    assert_eq!(
        fixture.one_i64(
            "SELECT COUNT(*) FROM provider_quota_windows WHERE provider_name = 'fixture-provider'"
        ),
        1
    );
    let call = runner.only_call();
    assert_eq!(call.program, "sh");
    assert_eq!(call.args, vec!["-c", "quota command"]);
    assert_eq!(call.stdin, StdinSpec::Null);
    assert_eq!(call.stdout, OutputSpec::Capture);
    assert_eq!(call.stderr, OutputSpec::Capture);
    assert_eq!(call.timeout, Some(Duration::from_secs(30)));
}

/// Risk: T4 (quota non-zero process output remains domain-classified)
/// Source: proposal §8 T4; contract §5 quota refresh failure
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn quota_refresh_provider_nonzero_script_returns_failed_and_does_not_persist_windows() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;
    let runner = FakeProcessRunner::new();
    runner.push_stderr_exit("quota denied", 7);
    let providers = quota_providers_config(PROVIDER, Some("quota command"), None);
    let in_flight = InFlight::new();

    let outcome = refresh_provider(PROVIDER, &providers, &in_flight, repo, &runner);

    match outcome {
        RefreshOutcome::Failed(msg) => {
            assert!(msg.contains("quota denied") || msg.contains("7"), "{msg}");
        }
        other => panic!("expected failed refresh outcome, got {other:?}"),
    }
    assert_eq!(
        fixture.one_i64(
            "SELECT COUNT(*) FROM provider_quota_windows WHERE provider_name = 'fixture-provider'"
        ),
        0
    );
}

/// Risk: T4 (auth refresh retry ordering preservation)
/// Source: proposal §8 T4; contract §5 quota auth retry
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn quota_refresh_provider_retries_script_after_auth_refresh_command() {
    let fixture = StateRepoFixture::new();
    fixture.seed_quota_window(PROVIDER, 0, 0.50);
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;
    let runner = FakeProcessRunner::new();
    runner.push_stderr_exit("auth expired", 1);
    runner.push_response(Ok(output(b"ignored stdout", b"", 0)));
    runner.push_stdout(br#"{"windows":[{"used_percent":3,"resets_at":"2099-01-01T00:00:00Z"}]}"#);
    let providers = quota_providers_config(
        PROVIDER,
        Some("quota command"),
        Some("auth refresh command"),
    );
    let in_flight = InFlight::new();

    let outcome = refresh_provider(PROVIDER, &providers, &in_flight, repo, &runner);

    match outcome {
        RefreshOutcome::Updated { windows } => {
            assert_eq!(windows.len(), 1);
            assert!((windows[0].used_percent - 0.03).abs() < 1e-6);
        }
        other => panic!("expected updated refresh outcome, got {other:?}"),
    }
    let calls = runner.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].args, vec!["-c", "quota command"]);
    assert_eq!(calls[0].timeout, Some(Duration::from_secs(30)));
    assert_eq!(calls[1].args, vec!["-c", "auth refresh command"]);
    assert_eq!(calls[1].stdout, OutputSpec::Null);
    assert_eq!(calls[1].stderr, OutputSpec::Capture);
    assert_eq!(calls[1].timeout, Some(Duration::from_secs(15)));
    assert_eq!(calls[2].args, vec!["-c", "quota command"]);
}

/// Risk: T4 (first-time empty windows do not trigger auth refresh)
/// Source: proposal §8 T4; contract §5 quota empty windows
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn quota_refresh_provider_first_time_empty_windows_do_not_run_auth_refresh() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;
    let runner = FakeProcessRunner::new();
    runner.push_stdout(br#"{"windows":[]}"#);
    let providers = quota_providers_config(
        PROVIDER,
        Some("quota command"),
        Some("auth refresh command"),
    );
    let in_flight = InFlight::new();

    let outcome = refresh_provider(PROVIDER, &providers, &in_flight, repo, &runner);

    match outcome {
        RefreshOutcome::Updated { windows } => assert!(windows.is_empty()),
        other => panic!("expected empty updated refresh outcome, got {other:?}"),
    }
    assert_eq!(runner.calls().len(), 1);
}

/// Risk: T4 (quota in-flight suppression stays service-owned)
/// Source: proposal §8 T4; contract §3 quota::refresh_provider
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn quota_refresh_provider_already_in_flight_does_not_call_runner() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;
    let runner = FakeProcessRunner::new();
    let providers = quota_providers_config(PROVIDER, Some("quota command"), None);
    let in_flight = InFlight::new();
    let _guard = in_flight.try_claim(PROVIDER).unwrap();

    let outcome = refresh_provider(PROVIDER, &providers, &in_flight, repo, &runner);

    assert!(matches!(outcome, RefreshOutcome::AlreadyInFlight));
    assert!(runner.calls().is_empty());
}

/// Risk: T4 (quota runner errors preserve timeout/spawn categories)
/// Source: proposal §8 T4; contract §6 process timeout
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs; src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn quota_refresh_provider_runner_error_returns_failed_without_persistence() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;
    let runner = FakeProcessRunner::new();
    runner.push_error("Quota script timed out after 30s");
    let providers = quota_providers_config(PROVIDER, Some("quota command"), None);
    let in_flight = InFlight::new();

    let outcome = refresh_provider(PROVIDER, &providers, &in_flight, repo, &runner);

    match outcome {
        RefreshOutcome::Failed(msg) => assert!(msg.contains("timed out"), "{msg}"),
        other => panic!("expected failed refresh outcome, got {other:?}"),
    }
    assert_eq!(fixture.one_i64("SELECT COUNT(*) FROM provider_quotas"), 0);
}

/// Risk: T4/T14 (quota stale decision reads through QuotaRepository)
/// Source: proposal §8 T4/T14; contract §3 quota::is_stale; assumption A6
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn quota_is_stale_accepts_quota_repository_trait_object() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;

    assert!(is_stale(repo, PROVIDER));
}
