#![cfg(unix)]

mod fixtures;

use agent_runner_lib::state::{
    CliProviderRepository, DefaultStateDbOpener, DiscoveryRepository, InvocationRepository,
    QuotaRepository, RoutingRepository, SessionChainRepository, StateDbOpener,
};
use fixtures::b1_state_repos::{
    CHAIN_ID, CLAUDE_ALT_PROVIDER, CLAUDE_PROVIDER, MODEL_NAME, SESSION_ID, StateRepoFixture,
    ensure_parent_missing,
};

/// Risk: T1/T2 - moving invocation lifecycle behind a trait may drop the current
/// last-call-wins correlation-column behavior.
/// Level: particular-integration.
/// Source: proposal section 8 T1/T2; contract section 5 `InvocationRepository`.
/// Observable: session capture updates through `&dyn InvocationRepository`
/// overwrite previous capture data and remain queryable by UUID.
#[test]
fn invocation_repository_session_capture_is_last_call_wins() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn InvocationRepository = &db;
    let uuid = "11111111-1111-4111-8111-111111111111";

    let id = fixture.start_invocation(repo, uuid, CLAUDE_PROVIDER, 0);
    repo.update_session_capture(id, Some("first-session"), "captured")
        .unwrap();
    repo.update_session_capture(id, Some("second-session"), "resumed")
        .unwrap();

    let row = repo.get_invocation_by_uuid(uuid).unwrap().unwrap();
    assert_eq!(row.session_id.as_deref(), Some("second-session"));
    assert_eq!(row.session_capture_method.as_deref(), Some("resumed"));
}

/// Risk: T1 - routing repository extraction may regress the post-PR-25
/// provider-name keyed aggregate semantics back to stale provider-index reads.
/// Level: particular-integration.
/// Source: proposal section 8 T1; contract section 1.5 and section 5 `RoutingRepository`.
/// Observable: recent errors are counted by provider name even when the legacy
/// provider index would point at a different provider.
#[test]
fn routing_repository_recent_errors_are_provider_name_keyed() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let invocations: &dyn InvocationRepository = &db;
    fixture.record_failed_invocation(
        invocations,
        "22222222-2222-4222-8222-222222222222",
        CLAUDE_ALT_PROVIDER,
        0,
    );

    let routing: &dyn RoutingRepository = &db;
    assert_eq!(
        routing
            .recent_error_count(MODEL_NAME, CLAUDE_PROVIDER, 60)
            .unwrap(),
        0
    );
    assert_eq!(
        routing
            .recent_error_count(MODEL_NAME, CLAUDE_ALT_PROVIDER, 60)
            .unwrap(),
        1
    );
}

/// Risk: T1/T8 - quota repository extraction may treat empty refresh input as a
/// destructive refresh, changing existing quota-window and call-count state.
/// Level: particular-integration.
/// Source: proposal section 8 T1/T8; contract section 5 `QuotaRepository`.
/// Observable: empty refresh is accepted, preserves existing windows, and does
/// not reset `calls_since_refresh`.
#[test]
fn quota_repository_empty_refresh_preserves_windows_and_call_count() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    fixture.seed_quota_window(CLAUDE_PROVIDER);

    let repo: &dyn QuotaRepository = &db;
    repo.increment_calls_since_refresh(CLAUDE_PROVIDER).unwrap();
    repo.upsert_quota_refresh(CLAUDE_PROVIDER, &[]).unwrap();

    assert_eq!(fixture.quota_window_count(CLAUDE_PROVIDER), 1);
    assert_eq!(fixture.calls_since_refresh(CLAUDE_PROVIDER), 1);
    assert!(fixture.exhausted_at_is_set(CLAUDE_PROVIDER));
}

/// Risk: T1/T2 - moving persisted session-chain operations out of balancer may
/// lose the active segment identity used by later session services.
/// Level: particular-integration.
/// Source: proposal section 8 T1/T2; contract section 5 `SessionChainRepository`.
/// Observable: active provider/session lookup returns the seeded active segment
/// id through the trait, without model-catalog validation.
#[test]
fn session_chain_repository_active_segment_lookup_returns_existing_row_id() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let expected_id = fixture.seed_active_chain(CHAIN_ID, CLAUDE_PROVIDER, SESSION_ID);

    let repo: &dyn SessionChainRepository = &db;
    let actual = repo
        .active_segment_id_for_chain_provider_session(CHAIN_ID, CLAUDE_PROVIDER, SESSION_ID)
        .unwrap();

    assert_eq!(actual, Some(expected_id));
}

/// Risk: T1/T8 - account/provider command wiring may convert missing-account
/// deletion into an error while extracting the CLI provider repository.
/// Level: particular-integration.
/// Source: proposal section 8 T1/T8; contract section 5 `CliProviderRepository`.
/// Observable: deleting a missing account is a successful no-op returning false.
#[test]
fn cli_provider_repository_delete_missing_account_returns_false() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn CliProviderRepository = &db;

    assert!(
        !repo
            .delete_account("missing-account", CLAUDE_PROVIDER)
            .unwrap()
    );
}

/// Risk: T1/T8 - discovery repository extraction may turn missing discovery
/// rows or parameter rows into errors.
/// Level: particular-integration.
/// Source: proposal section 8 T1/T8; contract section 5 `DiscoveryRepository`.
/// Observable: missing discovered models and model parameters return empty
/// vectors through the trait.
#[test]
fn discovery_repository_missing_reads_return_empty_vectors() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn DiscoveryRepository = &db;

    assert!(
        repo.list_discovered_models(Some(CLAUDE_PROVIDER))
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.list_model_parameters(MODEL_NAME, CLAUDE_PROVIDER)
            .unwrap()
            .is_empty()
    );
}

/// Risk: T1/T2 - replacing direct `StateDb::open_read_only` calls may start
/// creating directories, migrating, or backfilling during read-only inspection.
/// Level: particular-integration.
/// Source: proposal section 8 T1/T2; contract section 5 `StateDbOpener`.
/// Observable: read-only open on a missing path errors and leaves the path and
/// parent directory absent.
#[test]
fn state_db_opener_read_only_missing_path_does_not_create_files() {
    let fixture = StateRepoFixture::new();
    let path = fixture.missing_read_only_path();
    ensure_parent_missing(&path);

    let opener = DefaultStateDbOpener;
    let repo: &dyn StateDbOpener = &opener;
    assert!(repo.open_read_only(&path).is_err());

    ensure_parent_missing(&path);
}
