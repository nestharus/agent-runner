use chrono::{Duration, TimeZone, Utc};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_core::TransitionReason;
use oulipoly_state::repositories::{
    InvocationRepository, ProductionStateDbOpener, ProviderQuotaRepository,
    ProviderRoutingRepository, ResumeRepository, SchemaProbeRepository, SessionChainRepository,
    SessionTurnRepository, SetupRepository, StateDbOpener,
};
use oulipoly_state::{
    AccountRecord, AuthMethod, AuthStatus, CliMapping, CliProviderRecord, DiscoveredModel,
    InvocationStart, InvocationStatus, ModelParameter, ParamType, QuotaWindowInput,
    SessionTurnIngest, StateDb,
};
use std::path::{Path, PathBuf};

fn memory_db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

fn start_fixture(provider: &str) -> InvocationStart {
    InvocationStart {
        invocation_uuid: uuid::Uuid::new_v4().to_string(),
        model_name: "claude~high".to_string(),
        provider_name: provider.to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

fn model_with_provider(model_name: &str, provider_name: &str) -> ModelConfig {
    ModelConfig {
        name: model_name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::model_provider(provider_name, vec![])],
        inputs: vec![],
    }
}

fn fixed_time(offset_secs: i64) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap() + Duration::seconds(offset_secs)
}

fn seed_chain(db: &StateDb, chain_id: &str, model_name: &str, ts: chrono::DateTime<Utc>) {
    db.connection()
        .execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            rusqlite::params![chain_id, ts.to_rfc3339(), model_name],
        )
        .unwrap();
}

#[test]
fn invocation_repository_delegates_invocation_lifecycle_methods() {
    let direct_db = memory_db();
    let trait_db = memory_db();
    let direct_start = start_fixture("claude");
    let trait_start = start_fixture("claude");

    let direct_id = direct_db.start_invocation(&direct_start).unwrap();
    let trait_record =
        <StateDb as InvocationRepository>::start_invocation(&trait_db, trait_start.clone())
            .unwrap();

    let direct_record = direct_db
        .get_invocation_by_uuid(&direct_start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(trait_record.invocation_uuid, trait_start.invocation_uuid);
    assert_eq!(trait_record.model_name, direct_record.model_name);
    assert_eq!(trait_record.provider_name, direct_record.provider_name);
    assert_eq!(trait_record.status, InvocationStatus::Running);

    direct_db
        .update_session_capture(direct_id, Some("sess-direct"), "forced_flag_verified")
        .unwrap();
    direct_db
        .update_resume_acceptance(direct_id, "accepted", Some("direct evidence"))
        .unwrap();
    direct_db
        .finalize_invocation(direct_id, true, 0, None, Some("done"))
        .unwrap();

    <StateDb as InvocationRepository>::update_session_capture(
        &trait_db,
        &trait_start.invocation_uuid,
        Some("sess-trait"),
        "forced_flag_verified",
    )
    .unwrap();
    <StateDb as InvocationRepository>::update_resume_acceptance(
        &trait_db,
        &trait_start.invocation_uuid,
        "accepted",
        Some("trait evidence"),
    )
    .unwrap();
    <StateDb as InvocationRepository>::finalize_invocation(
        &trait_db,
        &trait_start.invocation_uuid,
        InvocationStatus::Succeeded,
        0,
        None,
        Some("done"),
    )
    .unwrap();

    let finalized = <StateDb as InvocationRepository>::get_invocation_by_uuid(
        &trait_db,
        &trait_start.invocation_uuid,
    )
    .unwrap()
    .unwrap();
    assert_eq!(finalized.status, InvocationStatus::Succeeded);
    assert_eq!(finalized.success, Some(true));
    assert_eq!(finalized.exit_code, Some(0));
    assert_eq!(finalized.session_id.as_deref(), Some("sess-trait"));
    assert_eq!(
        finalized.session_capture_method.as_deref(),
        Some("forced_flag_verified")
    );
    assert_eq!(
        finalized.resume_acceptance_status.as_deref(),
        Some("accepted")
    );
    assert_eq!(
        finalized.resume_acceptance_evidence.as_deref(),
        Some("trait evidence")
    );

    let parent = <StateDb as InvocationRepository>::start_invocation(
        &trait_db,
        InvocationStart {
            parent_invocation_id: None,
            ..start_fixture("claude")
        },
    )
    .unwrap();
    let child_uuid = uuid::Uuid::new_v4().to_string();
    <StateDb as InvocationRepository>::start_invocation(
        &trait_db,
        InvocationStart {
            invocation_uuid: child_uuid.clone(),
            parent_invocation_id: Some(parent.id),
            ..start_fixture("claude")
        },
    )
    .unwrap();
    let children = <StateDb as InvocationRepository>::list_invocation_children(
        &trait_db,
        &parent.invocation_uuid,
    )
    .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].invocation_uuid, child_uuid);
}

#[test]
fn provider_routing_repository_delegates_provider_quota_and_turn_reads() {
    let db = memory_db();
    let start = start_fixture("claude");
    let row_id = db.start_invocation(&start).unwrap();
    db.finalize_invocation(row_id, false, 2, Some("quota"), Some("quota exhausted"))
        .unwrap();
    db.upsert_quota_refresh(
        "claude",
        &[QuotaWindowInput {
            used_percent: 0.42,
            resets_at: fixed_time(3600),
        }],
    )
    .unwrap();
    db.ingest_session_turns_batch(
        "claude",
        &[SessionTurnIngest {
            session_id: "sess-routing".to_string(),
            turn_id: "turn-1".to_string(),
            timestamp: fixed_time(0),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        }],
    )
    .unwrap();

    let direct_provider = db
        .get_provider("claude~high", "claude")
        .unwrap()
        .expect("direct provider aggregate");
    let trait_provider =
        <StateDb as ProviderRoutingRepository>::get_provider(&db, "claude~high", "claude")
            .unwrap()
            .expect("trait provider aggregate");
    assert_eq!(trait_provider.model_name, direct_provider.model_name);
    assert_eq!(trait_provider.provider_name, direct_provider.provider_name);
    assert_eq!(
        trait_provider.invocation_count,
        direct_provider.invocation_count
    );
    assert_eq!(trait_provider.error_count, direct_provider.error_count);

    assert_eq!(
        <StateDb as ProviderRoutingRepository>::recent_error_count(
            &db,
            "claude~high",
            "claude",
            60
        )
        .unwrap(),
        db.recent_error_count("claude~high", "claude", 60).unwrap()
    );
    assert_eq!(
        <StateDb as ProviderRoutingRepository>::get_quota(&db, "claude")
            .unwrap()
            .unwrap()
            .provider_name,
        db.get_quota("claude").unwrap().unwrap().provider_name
    );
    assert_eq!(
        <StateDb as ProviderRoutingRepository>::get_windows(&db, "claude")
            .unwrap()
            .len(),
        db.get_windows("claude").unwrap().len()
    );
    assert_eq!(
        <StateDb as ProviderRoutingRepository>::count_assistant_turns_since(&db, "claude", None)
            .unwrap(),
        db.count_assistant_turns_since("claude", None).unwrap()
    );
}

#[test]
fn provider_quota_repository_delegates_quota_mutations() {
    let direct_db = memory_db();
    let trait_db = memory_db();
    let windows = vec![QuotaWindowInput {
        used_percent: 0.25,
        resets_at: fixed_time(600),
    }];

    direct_db.upsert_quota_refresh("codex", &windows).unwrap();
    <StateDb as ProviderQuotaRepository>::upsert_quota_refresh(&trait_db, "codex", &windows)
        .unwrap();
    direct_db.increment_calls_since_refresh("codex").unwrap();
    <StateDb as ProviderQuotaRepository>::increment_calls_since_refresh(&trait_db, "codex")
        .unwrap();
    direct_db.record_topology_probe("codex").unwrap();
    <StateDb as ProviderQuotaRepository>::record_topology_probe(&trait_db, "codex").unwrap();
    direct_db.mark_exhausted("codex").unwrap();
    <StateDb as ProviderQuotaRepository>::mark_exhausted(&trait_db, "codex").unwrap();

    let direct_quota = direct_db.get_quota("codex").unwrap().unwrap();
    let trait_quota = trait_db.get_quota("codex").unwrap().unwrap();
    assert_eq!(trait_quota.provider_name, direct_quota.provider_name);
    assert_eq!(
        trait_quota.calls_since_refresh,
        direct_quota.calls_since_refresh
    );
    assert!(trait_quota.exhausted_at.is_some());
    assert!(trait_quota.last_topology_probe_at.is_some());
    assert_eq!(trait_db.get_windows("codex").unwrap().len(), 1);
}

#[test]
fn session_turn_repository_delegates_ingest_counts_and_compaction_boundary() {
    let direct_db = memory_db();
    let trait_db = memory_db();
    let turns = vec![
        SessionTurnIngest {
            session_id: "sess-turns".to_string(),
            turn_id: "u1".to_string(),
            timestamp: fixed_time(0),
            role: "user".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: Some("hello".to_string()),
        },
        SessionTurnIngest {
            session_id: "sess-turns".to_string(),
            turn_id: "a1".to_string(),
            timestamp: fixed_time(1),
            role: "assistant".to_string(),
            parent_turn_id: Some("u1".to_string()),
            is_sidechain: true,
            is_compaction_boundary: false,
            body: Some("hi".to_string()),
        },
    ];

    direct_db
        .ingest_session_turns_batch("claude", &turns)
        .unwrap();
    <StateDb as SessionTurnRepository>::ingest_session_turns_batch(&trait_db, "claude", &turns)
        .unwrap();

    assert_eq!(
        <StateDb as SessionTurnRepository>::count_session_turns(&trait_db, "claude", "sess-turns")
            .unwrap(),
        direct_db
            .count_session_turns("claude", "sess-turns")
            .unwrap()
    );
    assert!(
        <StateDb as SessionTurnRepository>::flag_compaction_boundary(
            &trait_db,
            "claude",
            "sess-turns",
            "a1"
        )
        .unwrap()
    );
    let boundary = <StateDb as SessionTurnRepository>::latest_compaction_boundary(
        &trait_db,
        "claude",
        "sess-turns",
    )
    .unwrap()
    .unwrap();
    assert_eq!(boundary.0, "a1");
}

#[test]
fn session_chain_repository_delegates_chain_segment_operations() {
    let db = memory_db();
    let ts = fixed_time(0);
    let chain_id = uuid::Uuid::new_v4().to_string();
    let conflicting_chain = uuid::Uuid::new_v4().to_string();
    seed_chain(&db, &chain_id, "claude~high", ts);
    seed_chain(&db, &conflicting_chain, "claude~high", ts);

    let segment_id = <StateDb as SessionChainRepository>::open_chain_segment(
        &db,
        &chain_id,
        "claude",
        "sess-chain",
        &ts,
        TransitionReason::Initial,
    )
    .unwrap();
    <StateDb as SessionChainRepository>::open_chain_segment(
        &db,
        &conflicting_chain,
        "claude",
        "sess-chain",
        &ts,
        TransitionReason::Manual,
    )
    .unwrap();

    assert_eq!(
        <StateDb as SessionChainRepository>::chain_id_for_segment(&db, "claude", "sess-chain")
            .unwrap()
            .as_deref(),
        Some(conflicting_chain.as_str())
    );
    assert_eq!(
        <StateDb as SessionChainRepository>::active_segment_id_for_chain_provider_session(
            &db,
            &chain_id,
            "claude",
            "sess-chain",
        )
        .unwrap(),
        Some(segment_id)
    );
    assert_eq!(
        <StateDb as SessionChainRepository>::find_conflicting_active_segment(
            &db,
            "claude",
            "sess-chain",
            &chain_id,
        )
        .unwrap()
        .as_deref(),
        Some(conflicting_chain.as_str())
    );

    <StateDb as SessionChainRepository>::update_chain_last_used(&db, &chain_id).unwrap();
    assert_eq!(
        <StateDb as SessionChainRepository>::close_active_segment_returning(
            &db,
            &chain_id,
            &fixed_time(10)
        )
        .unwrap(),
        Some(segment_id)
    );

    <StateDb as SessionTurnRepository>::ingest_session_turns_batch(
        &db,
        "claude",
        &[SessionTurnIngest {
            session_id: "window-session".to_string(),
            turn_id: "w1".to_string(),
            timestamp: fixed_time(5),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        }],
    )
    .unwrap();
    assert_eq!(
        <StateDb as SessionChainRepository>::find_session_for_invocation_window(
            &db,
            "claude",
            &fixed_time(4),
            &fixed_time(6),
        )
        .unwrap()
        .as_deref(),
        Some("window-session")
    );

    <StateDb as SessionChainRepository>::mint_imported_chain_if_absent(
        &db,
        "codex",
        "imported-session",
        &ts,
        "codex~low",
    )
    .unwrap();
    assert!(
        <StateDb as SessionChainRepository>::chain_id_for_segment(&db, "codex", "imported-session")
            .unwrap()
            .is_some()
    );
}

#[test]
fn resume_repository_delegates_resume_resolution_and_previews() {
    let db = memory_db();
    let chain_id = uuid::Uuid::new_v4().to_string();
    seed_chain(&db, &chain_id, "claude~high", fixed_time(0));
    db.open_chain_segment(
        &chain_id,
        "claude",
        "resume-session",
        &fixed_time(0),
        TransitionReason::Initial,
    )
    .unwrap();
    db.ingest_session_turns_batch(
        "claude",
        &[SessionTurnIngest {
            session_id: "resume-session".to_string(),
            turn_id: "r1".to_string(),
            timestamp: fixed_time(1),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        }],
    )
    .unwrap();
    let models = std::collections::HashMap::from([(
        "claude~high".to_string(),
        model_with_provider("claude~high", "claude"),
    )]);

    let direct = db.resolve_resume(&models, &chain_id, None).unwrap();
    let via_trait =
        <StateDb as ResumeRepository>::resolve_resume(&db, &models, &chain_id, None).unwrap();
    assert_eq!(via_trait.chain_id, direct.chain_id);
    assert_eq!(via_trait.model_name, direct.model_name);
    assert_eq!(via_trait.active_provider, direct.active_provider);
    assert_eq!(via_trait.active_session_id, direct.active_session_id);
    assert_eq!(
        <StateDb as ResumeRepository>::chain_id_for_segment(&db, "claude", "resume-session")
            .unwrap()
            .as_deref(),
        Some(chain_id.as_str())
    );
    let previews = <StateDb as ResumeRepository>::resume_previews(&db, &chain_id).unwrap();
    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].active_session_id, "resume-session");
}

#[test]
fn setup_repository_delegates_setup_crud() {
    let db = memory_db();
    let provider = CliProviderRecord {
        cli_name: "codex".to_string(),
        display_name: "Codex".to_string(),
        installed: true,
        version: Some("1.2.3".to_string()),
        config_dir: Some("/tmp/codex".to_string()),
        last_synced: Some("2026-01-02T03:04:05Z".to_string()),
    };
    <StateDb as SetupRepository>::upsert_cli_provider(&db, &provider).unwrap();
    assert_eq!(
        <StateDb as SetupRepository>::get_cli_provider(&db, "codex")
            .unwrap()
            .unwrap()
            .display_name,
        db.get_cli_provider("codex").unwrap().unwrap().display_name
    );
    assert_eq!(
        <StateDb as SetupRepository>::list_cli_providers(&db)
            .unwrap()
            .len(),
        1
    );

    let account = AccountRecord {
        id: "acct-1".to_string(),
        provider: "codex".to_string(),
        profile_name: "default".to_string(),
        auth_method: AuthMethod::OAuth,
        auth_status: AuthStatus::Valid,
        created_at: "2026-01-02T03:04:05Z".to_string(),
    };
    <StateDb as SetupRepository>::insert_account(&db, &account).unwrap();
    assert_eq!(
        <StateDb as SetupRepository>::list_accounts(&db, Some("codex"))
            .unwrap()
            .len(),
        1
    );
    assert!(<StateDb as SetupRepository>::delete_account(&db, "acct-1", "codex").unwrap());

    let discovered = DiscoveredModel {
        canonical_name: "codex-1".to_string(),
        provider: "codex".to_string(),
        discovered_at: "2026-01-02T03:04:05Z".to_string(),
        cli_version: "1.2.3".to_string(),
    };
    <StateDb as SetupRepository>::upsert_discovered_model(&db, &discovered).unwrap();
    assert_eq!(
        <StateDb as SetupRepository>::list_discovered_models(&db, Some("codex"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        <StateDb as SetupRepository>::delete_stale_models(&db, "codex", "9.9.9").unwrap(),
        1
    );

    let param = ModelParameter {
        name: "effort".to_string(),
        display_name: "Reasoning effort".to_string(),
        param_type: ParamType::Enum {
            options: vec!["low".to_string(), "high".to_string()],
        },
        description: "Effort".to_string(),
        cli_mapping: CliMapping {
            flag: "--effort".to_string(),
            value_template: "{value}".to_string(),
        },
    };
    <StateDb as SetupRepository>::upsert_model_parameter(&db, "codex-1", "codex", &param).unwrap();
    assert_eq!(
        <StateDb as SetupRepository>::list_model_parameters(&db, "codex-1", "codex")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn schema_probe_repository_delegates_open_db_inspection() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let db = StateDb::open(&db_path).unwrap();

    let direct_state =
        oulipoly_state::schema_probe::inspect_schema(db.connection(), PathBuf::from(&db_path))
            .unwrap();
    let direct_report = oulipoly_state::schema_probe::report_from_state_db(direct_state);
    let trait_report =
        <StateDb as SchemaProbeRepository>::inspect_open_db(&db, PathBuf::from(&db_path)).unwrap();

    assert_eq!(trait_report.state_db.path, direct_report.state_db.path);
    assert_eq!(trait_report.state_db.exists, direct_report.state_db.exists);
    assert_eq!(
        trait_report.state_db.compatible,
        direct_report.state_db.compatible
    );
    assert_eq!(trait_report.state_db.tables, direct_report.state_db.tables);
    assert_eq!(trait_report.features, direct_report.features);
}

#[test]
fn state_db_opener_delegates_open_at_and_in_memory_policies() {
    let opener = ProductionStateDbOpener;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("nested").join("state.db");

    let file_db = <ProductionStateDbOpener as StateDbOpener>::open_at(&opener, &db_path).unwrap();
    let start = start_fixture("claude");
    let record =
        <StateDb as InvocationRepository>::start_invocation(&file_db, start.clone()).unwrap();
    assert_eq!(record.invocation_uuid, start.invocation_uuid);
    assert!(db_path.exists());

    let memory = <ProductionStateDbOpener as StateDbOpener>::open_in_memory(&opener);
    let memory_start = start_fixture("codex");
    let memory_record =
        <StateDb as InvocationRepository>::start_invocation(&memory, memory_start.clone()).unwrap();
    assert_eq!(memory_record.invocation_uuid, memory_start.invocation_uuid);
}

#[test]
fn lifecycle_emission_does_not_break_existing_rows() {
    let db = memory_db();
    let parent_start = start_fixture("claude");
    let parent =
        <StateDb as InvocationRepository>::start_invocation(&db, parent_start.clone()).unwrap();

    assert_eq!(parent.invocation_uuid, parent_start.invocation_uuid);
    assert_eq!(parent.model_name, parent_start.model_name);
    assert_eq!(parent.provider_name.as_deref(), Some("claude"));
    assert_eq!(parent.status, InvocationStatus::Running);

    <StateDb as InvocationRepository>::update_session_capture(
        &db,
        &parent.invocation_uuid,
        Some("lifecycle-preserved-session"),
        "forced_flag_verified",
    )
    .unwrap();
    <StateDb as InvocationRepository>::finalize_invocation(
        &db,
        &parent.invocation_uuid,
        InvocationStatus::Succeeded,
        0,
        None,
        Some("done"),
    )
    .unwrap();

    let finalized =
        <StateDb as InvocationRepository>::get_invocation_by_uuid(&db, &parent.invocation_uuid)
            .unwrap()
            .unwrap();
    assert_eq!(finalized.status, InvocationStatus::Succeeded);
    assert_eq!(finalized.success, Some(true));
    assert_eq!(finalized.exit_code, Some(0));
    assert_eq!(
        finalized.session_id.as_deref(),
        Some("lifecycle-preserved-session")
    );
    assert_eq!(
        finalized.session_capture_method.as_deref(),
        Some("forced_flag_verified")
    );

    let child_uuid = uuid::Uuid::new_v4().to_string();
    <StateDb as InvocationRepository>::start_invocation(
        &db,
        InvocationStart {
            invocation_uuid: child_uuid.clone(),
            parent_invocation_id: Some(parent.id),
            ..start_fixture("claude")
        },
    )
    .unwrap();
    let children =
        <StateDb as InvocationRepository>::list_invocation_children(&db, &parent.invocation_uuid)
            .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].invocation_uuid, child_uuid);
}
