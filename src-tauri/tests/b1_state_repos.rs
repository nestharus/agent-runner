#![cfg(unix)]

mod fixtures;

use agent_runner_lib::state::{
    CliProviderRepository, DiscoveryRepository, InvocationRepository, InvocationStatus,
    QuotaRepository, ReadOnlyOpenError, RoutingRepository, SessionChainRepository, StateDbOpener,
    TransitionReason,
};
use chrono::TimeZone;
use fixtures::b1_state_repos::*;

/// Risk: T1 (state DB invocation lifecycle preservation)
/// Source: proposal §8 T1; contract §5 InvocationRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn invocation_repository_start_invocation_persists_running_row() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn InvocationRepository = &db;

    let id = repo.start_invocation(&root_invocation()).unwrap();

    assert!(id > 0);
    let row = repo.get_invocation_by_uuid(ROOT_UUID).unwrap().unwrap();
    assert_eq!(row.id, id);
    assert_eq!(row.model_name, MODEL);
    assert_eq!(row.provider_name.as_deref(), Some(PROVIDER));
    assert_eq!(row.status, InvocationStatus::Running);
}

/// Risk: T1 (state DB invocation lifecycle preservation)
/// Source: proposal §8 T1; contract §5 InvocationRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn invocation_repository_finalize_invocation_updates_status_and_provider_aggregate() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn InvocationRepository = &db;
    let id = repo.start_invocation(&root_invocation()).unwrap();

    repo.finalize_invocation(id, false, 42, Some("quota"), Some("limit hit"))
        .unwrap();

    let row = repo.get_invocation_by_uuid(ROOT_UUID).unwrap().unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(42));
    let routing: &dyn RoutingRepository = &db;
    assert!(routing.get_provider(MODEL, PROVIDER).unwrap().is_some());
}

/// Risk: T1 (state DB invocation lifecycle preservation)
/// Source: proposal §8 T1; contract §5 InvocationRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn invocation_repository_finalize_missing_invocation_returns_error() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn InvocationRepository = &db;

    let err = repo
        .finalize_invocation(404, true, 0, None, None)
        .unwrap_err();

    assert!(err.contains("invocation") || err.contains("404"), "{err}");
}

/// Risk: T1 (state DB invocation lifecycle preservation)
/// Source: proposal §8 T1; contract §5 InvocationRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn invocation_repository_finalize_already_finalized_invocation_returns_error() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn InvocationRepository = &db;
    let id = repo.start_invocation(&root_invocation()).unwrap();
    repo.finalize_invocation(id, true, 0, None, None).unwrap();

    let err = repo
        .finalize_invocation(id, true, 0, None, None)
        .unwrap_err();

    assert!(err.contains("final") || err.contains("running"), "{err}");
}

/// Risk: T2 (session correlation is preserved while state/config dependency is split)
/// Source: proposal §8 T2; contract §5 InvocationRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn invocation_repository_session_capture_and_resume_acceptance_are_last_call_wins() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn InvocationRepository = &db;
    let id = repo.start_invocation(&root_invocation()).unwrap();

    repo.update_session_capture(id, Some("first-session"), "stdout")
        .unwrap();
    repo.update_session_capture(id, Some("second-session"), "transcript")
        .unwrap();
    repo.update_resume_acceptance(id, "rejected", Some("first evidence"))
        .unwrap();
    repo.update_resume_acceptance(id, "accepted", Some("second evidence"))
        .unwrap();

    let row = repo.get_invocation_by_uuid(ROOT_UUID).unwrap().unwrap();
    assert_eq!(row.session_id.as_deref(), Some("second-session"));
    assert_eq!(row.session_capture_method.as_deref(), Some("transcript"));
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("accepted"));
    assert_eq!(
        row.resume_acceptance_evidence.as_deref(),
        Some("second evidence")
    );
}

/// Risk: T1 (state DB invocation tree preservation)
/// Source: proposal §8 T1; contract §5 InvocationRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn invocation_repository_missing_queries_return_none_and_empty_children() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn InvocationRepository = &db;

    assert!(repo.get_invocation_by_uuid(ROOT_UUID).unwrap().is_none());
    assert!(repo.list_invocation_children(12345).unwrap().is_empty());
}

/// Risk: T1 (provider routing state remains name-keyed)
/// Source: proposal §8 T1/T3; contract §5 RoutingRepository; assumption A1
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn routing_repository_get_provider_is_model_and_provider_name_keyed() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let invocations: &dyn InvocationRepository = &db;
    let routing: &dyn RoutingRepository = &db;
    let id = invocations.start_invocation(&root_invocation()).unwrap();
    invocations
        .finalize_invocation(id, true, 0, None, None)
        .unwrap();

    assert!(routing.get_provider(MODEL, PROVIDER).unwrap().is_some());
    assert!(
        routing
            .get_provider(OTHER_MODEL, PROVIDER)
            .unwrap()
            .is_none()
    );
    assert!(
        routing
            .get_provider(MODEL, OTHER_PROVIDER)
            .unwrap()
            .is_none()
    );
}

/// Risk: T1 (routing error accounting preservation)
/// Source: proposal §8 T1/T3; contract §5 RoutingRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn routing_repository_recent_error_count_counts_recent_failed_invocations_only() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let invocations: &dyn InvocationRepository = &db;
    let routing: &dyn RoutingRepository = &db;
    let failed = invocations.start_invocation(&root_invocation()).unwrap();
    invocations
        .finalize_invocation(failed, false, 7, Some("error"), None)
        .unwrap();

    assert_eq!(routing.recent_error_count(MODEL, PROVIDER, 60).unwrap(), 1);
    assert_eq!(
        routing
            .recent_error_count(MODEL, OTHER_PROVIDER, 60)
            .unwrap(),
        0
    );
}

/// Risk: T1 (quota read shape preserved for routing)
/// Source: proposal §8 T1/T3; contract §5 RoutingRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn routing_repository_missing_quota_and_windows_are_not_errors() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let routing: &dyn RoutingRepository = &db;

    assert!(routing.get_quota(PROVIDER).unwrap().is_none());
    assert!(routing.get_windows(PROVIDER).unwrap().is_empty());
}

/// Risk: T1 (assistant turn burn-rate reads preserved)
/// Source: proposal §8 T1/T3; contract §5 RoutingRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn routing_repository_count_assistant_turns_since_is_exclusive() {
    let fixture = StateRepoFixture::new();
    fixture.seed_turn(PROVIDER, SESSION_A, "before", "2026-05-02T08:59:59Z");
    fixture.seed_turn(PROVIDER, SESSION_A, "boundary", "2026-05-02T09:00:00Z");
    fixture.seed_turn(PROVIDER, SESSION_A, "after", "2026-05-02T09:00:01Z");
    let db = fixture.open_db();
    let routing: &dyn RoutingRepository = &db;
    let since = chrono::Utc.with_ymd_and_hms(2026, 5, 2, 9, 0, 0).unwrap();

    assert_eq!(
        routing
            .count_assistant_turns_since(PROVIDER, Some(&since))
            .unwrap(),
        1
    );
    assert_eq!(
        routing.count_assistant_turns_since(PROVIDER, None).unwrap(),
        3
    );
}

/// Risk: T4/T14/T15 (quota persistence semantics preserved)
/// Source: proposal §8 T4/T14/T15; contract §5 QuotaRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn quota_repository_mark_exhausted_creates_or_updates_exhausted_row() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;

    repo.mark_exhausted(PROVIDER).unwrap();

    assert_eq!(
        fixture.one_i64(
            "SELECT COUNT(*) FROM provider_quotas WHERE provider_name = 'fixture-provider' AND exhausted_at IS NOT NULL"
        ),
        1
    );
}

/// Risk: T4/T14/T15 (empty quota refresh does not erase usable quota state)
/// Source: proposal §8 T4/T14/T15; contract §5 QuotaRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn quota_repository_empty_refresh_preserves_windows_and_calls_since_refresh() {
    let fixture = StateRepoFixture::new();
    fixture.seed_quota_row(PROVIDER, 0.9, 5, true);
    fixture.seed_quota_window(PROVIDER, 0, 0.9);
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;

    repo.upsert_quota_refresh(PROVIDER, &[]).unwrap();

    assert_eq!(
        fixture.one_i64(
            "SELECT calls_since_refresh FROM provider_quotas WHERE provider_name = 'fixture-provider'"
        ),
        5
    );
    assert_eq!(
        fixture.one_i64(
            "SELECT COUNT(*) FROM provider_quota_windows WHERE provider_name = 'fixture-provider'"
        ),
        1
    );
}

/// Risk: T4/T14/T15 (non-empty quota refresh replaces stale windows)
/// Source: proposal §8 T4/T14/T15; contract §5 QuotaRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn quota_repository_non_empty_refresh_replaces_windows_resets_calls_and_clears_exhaustion() {
    let fixture = StateRepoFixture::new();
    fixture.seed_quota_row(PROVIDER, 0.9, 5, true);
    fixture.seed_quota_window(PROVIDER, 0, 0.9);
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;
    let windows = [quota_window_input(0.25), quota_window_input(0.35)];

    repo.upsert_quota_refresh(PROVIDER, &windows).unwrap();

    assert_eq!(
        fixture.one_i64(
            "SELECT calls_since_refresh FROM provider_quotas WHERE provider_name = 'fixture-provider'"
        ),
        0
    );
    assert_eq!(
        fixture.one_i64(
            "SELECT COUNT(*) FROM provider_quotas WHERE provider_name = 'fixture-provider' AND exhausted_at IS NULL"
        ),
        1
    );
    assert_eq!(
        fixture.one_i64(
            "SELECT COUNT(*) FROM provider_quota_windows WHERE provider_name = 'fixture-provider'"
        ),
        2
    );
}

/// Risk: T4/T14/T15 (quota call counter creation semantics preserved)
/// Source: proposal §8 T4/T14/T15; contract §5 QuotaRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn quota_repository_increment_calls_since_refresh_creates_row_when_absent() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn QuotaRepository = &db;

    repo.increment_calls_since_refresh(PROVIDER).unwrap();

    assert_eq!(
        fixture.one_i64(
            "SELECT calls_since_refresh FROM provider_quotas WHERE provider_name = 'fixture-provider'"
        ),
        1
    );
}

/// Risk: T2 (transition reason persistence values survive state ownership move)
/// Source: proposal §8 T2; contract §5 SessionChainRepository; assumption A1
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn session_chain_repository_open_chain_segment_persists_state_owned_transition_reason() {
    let fixture = StateRepoFixture::new();
    fixture.seed_active_chain(CHAIN_A, PROVIDER, SESSION_A, MODEL, "2026-05-02T09:00:00Z");
    let db = fixture.open_db();
    let repo: &dyn SessionChainRepository = &db;
    let started_at = chrono::Utc.with_ymd_and_hms(2026, 5, 2, 10, 0, 0).unwrap();

    let segment_id = repo
        .open_chain_segment(
            CHAIN_A,
            OTHER_PROVIDER,
            SESSION_B,
            &started_at,
            TransitionReason::QuotaThreshold,
        )
        .unwrap();

    assert!(segment_id > 0);
    assert_eq!(
        fixture.one_string(
            "SELECT transition_reason FROM session_chain_segments WHERE provider_name = 'other-provider'"
        ),
        "quota_threshold"
    );
}

/// Risk: T2/T9/T10/T13 (resume DB facts split from model validation)
/// Source: proposal §8 T2/T9/T10/T13; contract §5 SessionChainRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn session_chain_repository_resolve_resume_facts_returns_db_facts_without_model_catalog_validation()
{
    let fixture = StateRepoFixture::new();
    fixture.seed_active_chain(
        CHAIN_A,
        PROVIDER,
        SESSION_A,
        "model-not-in-test-catalog",
        "2026-05-02T09:00:00Z",
    );
    let db = fixture.open_db();
    let repo: &dyn SessionChainRepository = &db;

    let facts = repo.resolve_resume_facts(SESSION_A, None).unwrap();

    assert_eq!(facts.chain_id, CHAIN_A);
    assert_eq!(
        facts.inferred_model_name.as_deref(),
        Some("model-not-in-test-catalog")
    );
    assert_eq!(facts.active_provider, PROVIDER);
    assert_eq!(facts.active_session_id, SESSION_A);
}

/// Risk: T2/T9 (repository-level resume errors are DB/UUID facts only)
/// Source: proposal §8 T2/T9; contract §5 SessionChainRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn session_chain_repository_resolve_resume_facts_rejects_invalid_uuid_shape() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn SessionChainRepository = &db;

    let err = repo.resolve_resume_facts("not-a-uuid", None).unwrap_err();

    assert!(!format!("{err:?}").contains("UnknownModel"));
}

/// Risk: T1/T2/T9 (resume previews keep newest-first ordering)
/// Source: proposal §8 T1/T2/T9; contract §5 SessionChainRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn session_chain_repository_resume_previews_sort_newest_first() {
    let fixture = StateRepoFixture::new();
    fixture.seed_active_chain(CHAIN_A, PROVIDER, SESSION_A, MODEL, "2026-05-02T09:00:00Z");
    fixture.seed_active_chain(
        CHAIN_B,
        OTHER_PROVIDER,
        SESSION_B,
        MODEL,
        "2026-05-02T10:00:00Z",
    );
    let db = fixture.open_db();
    let repo: &dyn SessionChainRepository = &db;

    let previews = repo.resume_previews("5169694d").unwrap();

    assert!(previews.len() >= 1);
    assert!(
        previews
            .windows(2)
            .all(|pair| pair[0].last_used_at >= pair[1].last_used_at)
    );
}

/// Risk: T1/T2 (session chain missing lookup shape is preserved)
/// Source: proposal §8 T1/T2; contract §5 SessionChainRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn session_chain_repository_missing_segment_lookups_return_none() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn SessionChainRepository = &db;

    assert!(
        repo.chain_id_for_segment(PROVIDER, SESSION_A)
            .unwrap()
            .is_none()
    );
    assert!(
        repo.active_segment_id_for_chain_provider_session(CHAIN_A, PROVIDER, SESSION_A)
            .unwrap()
            .is_none()
    );
}

/// Risk: T1/T2/T13 (session detection window remains repository-visible)
/// Source: proposal §8 T1/T2/T13; contract §5 SessionChainRepository
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn session_chain_repository_find_session_for_invocation_window_returns_best_ranked_session() {
    let fixture = StateRepoFixture::new();
    fixture.seed_turn(PROVIDER, SESSION_A, "inside", "2026-05-02T09:00:03Z");
    let db = fixture.open_db();
    let repo: &dyn SessionChainRepository = &db;
    let started = chrono::Utc.with_ymd_and_hms(2026, 5, 2, 9, 0, 0).unwrap();
    let finished = chrono::Utc.with_ymd_and_hms(2026, 5, 2, 9, 0, 5).unwrap();

    assert_eq!(
        repo.find_session_for_invocation_window(PROVIDER, &started, &finished)
            .unwrap()
            .as_deref(),
        Some(SESSION_A)
    );
}

/// Risk: T8/T16 (CLI provider and account repository semantics preserved)
/// Source: proposal §8 T8/T16; contract §5 CliProviderRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn cli_provider_repository_upsert_overwrites_and_get_missing_returns_none() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn CliProviderRepository = &db;

    repo.upsert_cli_provider(&cli_provider("claude", "1.0.0"))
        .unwrap();
    repo.upsert_cli_provider(&cli_provider("claude", "2.0.0"))
        .unwrap();

    let provider = repo.get_cli_provider("claude").unwrap().unwrap();
    assert_eq!(provider.version.as_deref(), Some("2.0.0"));
    assert!(repo.get_cli_provider("missing").unwrap().is_none());
}

/// Risk: T8/T16 (account delete return shape preserved)
/// Source: proposal §8 T8/T16; contract §5 CliProviderRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn cli_provider_repository_account_insert_list_filter_and_missing_delete() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn CliProviderRepository = &db;
    repo.upsert_cli_provider(&cli_provider("claude", "1.0.0"))
        .unwrap();
    repo.upsert_cli_provider(&cli_provider("codex", "1.0.0"))
        .unwrap();
    repo.insert_account(&account("work", "claude")).unwrap();
    repo.insert_account(&account("personal", "codex")).unwrap();

    assert_eq!(repo.list_accounts(None).unwrap().len(), 2);
    assert_eq!(repo.list_accounts(Some("claude")).unwrap().len(), 1);
    assert!(!repo.delete_account("missing", "claude").unwrap());
    assert!(repo.delete_account("work", "claude").unwrap());
}

/// Risk: T8/T16 (repository surfaces DB constraint failures)
/// Source: proposal §8 T8/T16; contract §5 CliProviderRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn cli_provider_repository_insert_account_constraint_failure_returns_error() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn CliProviderRepository = &db;

    let err = repo
        .insert_account(&account("work", "missing-provider"))
        .unwrap_err();

    assert!(
        err.contains("constraint") || err.contains("foreign"),
        "{err}"
    );
}

/// Risk: T8/T17 (discovery persistence remains keyed by canonical model and provider)
/// Source: proposal §8 T8/T17; contract §5 DiscoveryRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn discovery_repository_upsert_model_is_keyed_by_canonical_name_and_provider() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn DiscoveryRepository = &db;

    repo.upsert_discovered_model(&discovered_model("claude-sonnet", "claude", "1.0"))
        .unwrap();
    repo.upsert_discovered_model(&discovered_model("claude-sonnet", "claude", "2.0"))
        .unwrap();
    repo.upsert_discovered_model(&discovered_model("claude-sonnet", "codex", "1.0"))
        .unwrap();

    assert_eq!(repo.list_discovered_models(None).unwrap().len(), 2);
    assert_eq!(
        repo.list_discovered_models(Some("claude")).unwrap()[0].cli_version,
        "2.0"
    );
}

/// Risk: T8/T17 (stale discovery deletion is provider-scoped)
/// Source: proposal §8 T8/T17; contract §5 DiscoveryRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn discovery_repository_delete_stale_models_deletes_only_requested_provider_version_mismatches() {
    let fixture = StateRepoFixture::new();
    fixture.seed_discovered_model_row("claude-old", "claude", "1.0");
    fixture.seed_discovered_model_row("claude-current", "claude", "2.0");
    fixture.seed_discovered_model_row("codex-old", "codex", "1.0");
    let db = fixture.open_db();
    let repo: &dyn DiscoveryRepository = &db;

    let deleted = repo.delete_stale_models("claude", "2.0").unwrap();

    assert_eq!(deleted, 1);
    assert_eq!(
        repo.list_discovered_models(Some("claude")).unwrap().len(),
        1
    );
    assert_eq!(repo.list_discovered_models(Some("codex")).unwrap().len(), 1);
}

/// Risk: T8/T17 (model parameter upsert/list semantics preserved)
/// Source: proposal §8 T8/T17; contract §5 DiscoveryRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn discovery_repository_model_parameters_upsert_by_model_provider_and_name() {
    let fixture = StateRepoFixture::new();
    let db = fixture.open_db();
    let repo: &dyn DiscoveryRepository = &db;

    repo.upsert_model_parameter("claude-sonnet", "claude", &model_parameter("effort"))
        .unwrap();
    repo.upsert_model_parameter("claude-sonnet", "claude", &model_parameter("effort"))
        .unwrap();

    assert_eq!(
        repo.list_model_parameters("claude-sonnet", "claude")
            .unwrap()
            .len(),
        1
    );
    assert!(
        repo.list_model_parameters("missing", "claude")
            .unwrap()
            .is_empty()
    );
}

/// Risk: T1/T2/T11 (state opener preserves read-write and read-only semantics)
/// Source: proposal §8 T1/T2/T11; contract §5 StateDbOpener
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn state_db_opener_open_creates_parent_directories_and_initializes_schema() {
    let fixture = StateRepoFixture::new();
    let opener = agent_runner_lib::state::DefaultStateDbOpener::default();
    let repo: &dyn StateDbOpener = &opener;

    let _db = repo.open(fixture.db_path()).unwrap();

    assert!(fixture.db_path().exists());
    assert_eq!(fixture.count("invocations"), 0);
}

/// Risk: T1/T2/T11 (read-only opener must not create missing state)
/// Source: proposal §8 T1/T2/T11; contract §5 StateDbOpener
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn state_db_opener_open_read_only_missing_returns_missing_without_creating_file() {
    let fixture = StateRepoFixture::new();
    let opener = agent_runner_lib::state::DefaultStateDbOpener::default();
    let repo: &dyn StateDbOpener = &opener;
    let missing = fixture.missing_db_path();

    let err = repo.open_read_only(&missing).unwrap_err();

    assert!(matches!(err, ReadOnlyOpenError::Missing { .. }));
    assert!(!missing.exists());
}

/// Risk: T1/T2/T11 (read-only opener classifies corrupt files)
/// Source: proposal §8 T1/T2/T11; contract §5 StateDbOpener
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn state_db_opener_open_read_only_corrupt_file_returns_not_a_database() {
    let fixture = StateRepoFixture::new();
    let opener = agent_runner_lib::state::DefaultStateDbOpener::default();
    let repo: &dyn StateDbOpener = &opener;

    let err = repo.open_read_only(&fixture.corrupt_db_path()).unwrap_err();

    assert!(matches!(err, ReadOnlyOpenError::NotADatabase { .. }));
}

/// Risk: T1/T2/T11 (default opener path is isolated and delegates to open)
/// Source: proposal §8 T1/T2/T11; contract §5 StateDbOpener
/// Level: particular-integration
/// Fixture source: src-tauri/tests/fixtures/b1_state_repos.rs
#[test]
fn state_db_opener_default_path_and_open_default_use_current_data_dir_contract() {
    isolated_xdg_data_home(|| {
        let opener = agent_runner_lib::state::DefaultStateDbOpener::default();
        let repo: &dyn StateDbOpener = &opener;

        let path = repo.default_path().unwrap();
        let _db = repo.open_default().unwrap();

        assert!(path.ends_with("oulipoly-agent-runner/state.db"));
        assert!(path.exists());
    });
}

/// Risk: T2 (state module no longer depends on balancer/config loaders)
/// Source: proposal §8 T2; hookpoint research §5 service compile guard; assumption A1
/// Level: unit
/// Fixture source: source-file dependency guard in this test
#[test]
fn state_source_no_longer_imports_balancer_or_config_load_models() {
    let state_db = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/state/db.rs"),
    )
    .unwrap();

    assert!(!state_db.contains("crate::balancer::TransitionReason"));
    assert!(!state_db.contains("config::load_models"));
    assert!(!state_db.contains("load_models("));
}
