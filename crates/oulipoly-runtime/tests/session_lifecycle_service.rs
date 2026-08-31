use chrono::Utc;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_config::{
    ProviderEntry, ProvidersConfig, SessionSourceEntry, SessionStorage, SessionsConfig,
};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::services::{
    ProductionSessionLifecycleService, ServiceError, SessionLifecycleIngestMode,
    SessionLifecycleOutput, SessionLifecycleRequest, SessionLifecycleServicePort,
    SessionServiceExternalProviderIdentity,
};
use oulipoly_state::{
    InvocationStart, SessionTurnIngest, SessionTurnIngestStreamKey, SessionTurnStreamProjection,
    StateDb,
};
use rusqlite::Connection;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PROVIDER_A_MODEL: &str = "provider-a-model";
const PROVIDER_A_ACCOUNT: &str = "provider-a-account";
const PROVIDER_A_INSTANCE: &str = "provider-a-instance";
const PROVIDER_A_SETTINGS: &str = "provider-a-test-settings";

fn legacy_provider_name() -> String {
    ["clau", "de"].concat()
}

fn legacy_model_name() -> String {
    format!("{}-opus", legacy_provider_name())
}

struct Fixture {
    dir: tempfile::TempDir,
    state: StateDb,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        Self { dir, state }
    }

    fn conn(&self) -> Connection {
        Connection::open(self.dir.path().join("state.db")).unwrap()
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn start_and_finalize_invocation(&self, uuid: &str) -> i64 {
        let id = self
            .state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid.to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        self.state
            .finalize_invocation(id, true, 0, None, Some("completed"))
            .unwrap();
        id
    }

    fn start_and_finalize_provider_a_invocation(&self, uuid: &str) -> i64 {
        let id = self
            .state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid.to_string(),
                model_name: PROVIDER_A_MODEL.to_string(),
                provider_name: PROVIDER_A_ACCOUNT.to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        self.state
            .finalize_invocation(id, true, 0, None, Some("completed"))
            .unwrap();
        id
    }

    fn seed_chain(&self, chain_id: &str, provider_name: &str, session_id: &str, model_name: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', ?2)",
            (chain_id, model_name),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-05-01T00:00:00Z', 'initial')",
            (chain_id, provider_name, session_id),
        )
        .unwrap();
    }

    fn session_turn_rows(&self) -> Vec<(String, String, String, String)> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT provider_name, session_id, turn_id, role
                 FROM session_turns ORDER BY provider_name, session_id, turn_id",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
    }
}

fn stderr_text(stderr: Vec<u8>) -> String {
    String::from_utf8(stderr).unwrap()
}

fn turn(session_id: &str, turn_id: &str, role: &str) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        timestamp: Utc::now(),
        role: role.to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: None,
    }
}

struct ProviderAFixture {
    fixture: Fixture,
    provider_path: PathBuf,
    record_path: PathBuf,
}

impl ProviderAFixture {
    fn new(mode: &str) -> Self {
        let fixture = Fixture::new();
        let mode_path = fixture.path().join("provider-a-mode.txt");
        let record_path = fixture.path().join("provider-a-records.jsonl");
        std::fs::write(&mode_path, mode).unwrap();
        std::fs::write(&record_path, "").unwrap();
        let provider_path =
            write_provider_a_lifecycle_script(fixture.path(), &mode_path, &record_path);
        Self {
            fixture,
            provider_path,
            record_path,
        }
    }

    fn registry_handle(&self) -> ProviderRegistryHandle {
        let registry = ProviderRegistry::from_model_configs(
            &[provider_a_model(PROVIDER_A_MODEL, &self.provider_path)],
            ProviderRegistryOptions::default(),
        )
        .unwrap();
        ProviderRegistryHandle::new(Arc::new(registry))
    }

    fn records(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.record_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn provider_a_model(name: &str, provider_path: &Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            PROVIDER_A_ACCOUNT,
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(provider_path.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn external_provider_identity() -> SessionServiceExternalProviderIdentity {
    SessionServiceExternalProviderIdentity {
        model_name: PROVIDER_A_MODEL.to_string(),
        provider_name: PROVIDER_A_ACCOUNT.to_string(),
        provider_instance_id: Some(PROVIDER_A_INSTANCE.to_string()),
        settings_id: PROVIDER_A_SETTINGS.to_string(),
    }
}

fn assert_canonical_stream_queued(state: &StateDb, session_id: &str) {
    let stream = state
        .session_turn_ingest_stream(&SessionTurnIngestStreamKey {
            provider_name: PROVIDER_A_ACCOUNT.to_string(),
            provider_instance_id: PROVIDER_A_INSTANCE.to_string(),
            settings_id: PROVIDER_A_SETTINGS.to_string(),
            session_id: session_id.to_string(),
            projection: SessionTurnStreamProjection::CanonicalIngest,
        })
        .unwrap()
        .expect("canonical turn stream queued");
    assert_eq!(stream.status, "ready");
    assert_eq!(stream.checkpoint_generation, 0);
    assert_eq!(stream.committed_page_count, 0);
    assert_eq!(stream.committed_turn_count, 0);
}

#[test]
fn session_lifecycle_unpinned_no_match_emits_nothing_and_preserves_one_shot_fallback() {
    let fixture = Fixture::new();
    let invocation_uuid = "11111111-1111-4111-8111-111111111111";
    let invocation_row_id = fixture.start_and_finalize_invocation(invocation_uuid);
    let mut stderr = Vec::new();
    let service = ProductionSessionLifecycleService::new();

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: "claude",
            external_provider: None,
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "stdout_json_event".to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("unpinned no-match is not an infrastructure failure");

    assert_eq!(
        output,
        SessionLifecycleOutput {
            emitted: false,
            session_id: None,
        }
    );
    assert!(stderr.is_empty());
    let row = fixture
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id, None);
    assert_eq!(row.session_capture_method, None);
}

#[test]
fn session_lifecycle_uses_effective_cwd_to_disambiguate_window_candidates() {
    let fixture = Fixture::new();
    let invocation_uuid = "12121212-1212-4212-8212-121212121212";
    let correct_session = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let wrong_session = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let correct_workspace = fixture.dir.path().join("rfq");
    let wrong_workspace = fixture.dir.path().join("other");
    std::fs::create_dir_all(&correct_workspace).unwrap();
    std::fs::create_dir_all(&wrong_workspace).unwrap();

    let cwd_script = fixture.dir.path().join("cwd-script.sh");
    std::fs::write(
        &cwd_script,
        format!(
            "#!/usr/bin/env bash\ncase \"$SESSION_ID\" in\n  {correct_session}) printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n' ;;\n  {wrong_session}) printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n' ;;\n  *) printf '{{\"found\":false}}\\n' ;;\nesac\n",
            correct_workspace.display(),
            wrong_workspace.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&cwd_script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&cwd_script, perms).unwrap();
    let providers_cfg = ProvidersConfig {
        entries: HashMap::from([(
            "claude".to_string(),
            ProviderEntry {
                session_storage: Some(SessionStorage::Script {
                    cwd_script: cwd_script.display().to_string(),
                    transcript_script: None,
                    storage_type: None,
                }),
                ..ProviderEntry::default()
            },
        )]),
    };

    let invocation_row_id = fixture
        .state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "claude-haiku".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    fixture
        .state
        .ingest_session_turns_batch(
            "claude",
            &[
                turn(wrong_session, "wrong-1", "assistant"),
                turn(wrong_session, "wrong-2", "user"),
                turn(correct_session, "correct-1", "assistant"),
            ],
        )
        .unwrap();
    fixture
        .state
        .finalize_invocation(invocation_row_id, true, 0, None, Some("completed"))
        .unwrap();

    let mut stderr = Vec::new();
    let service = ProductionSessionLifecycleService::new();
    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: Some(&providers_cfg),
            provider_name: "claude",
            external_provider: None,
            invocation_row_id,
            invocation_uuid,
            effective_cwd: Some(&correct_workspace),
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "turn_script".to_string(),
            },
            stderr: &mut stderr,
        })
        .unwrap();

    assert_eq!(
        output,
        SessionLifecycleOutput {
            emitted: true,
            session_id: Some(correct_session.to_string()),
        }
    );
    let stderr = stderr_text(stderr);
    assert!(stderr.contains(correct_session));
    assert!(!stderr.contains(wrong_session));
    let row = fixture
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.provider_session_id.as_deref(), Some(correct_session));
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("turn_script")
    );
}

#[test]
fn session_lifecycle_ingest_preserves_find_window_warning() {
    let fixture = Fixture::new();
    let invocation_uuid = "22222222-2222-4222-8222-222222222222";
    let invocation_row_id = fixture.start_and_finalize_invocation(invocation_uuid);
    fixture
        .conn()
        .execute_batch("DROP TABLE session_turns;")
        .unwrap();
    let mut stderr = Vec::new();
    let service = ProductionSessionLifecycleService::new();

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: "claude",
            external_provider: None,
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "stdout_json_event".to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("find-window failures are warning-only");

    assert_eq!(
        output,
        SessionLifecycleOutput {
            emitted: false,
            session_id: None,
        }
    );
    let stderr = stderr_text(stderr);
    assert!(stderr.contains(
        "Warning: Failed to resolve session for invocation 22222222-2222-4222-8222-222222222222"
    ));
    assert!(!stderr.contains("OULIPOLY_SESSION="));
    let row = fixture
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id, None);
    assert_eq!(row.session_capture_method, None);
}

#[test]
fn session_lifecycle_ingest_preserves_mint_failure_emission() {
    let fixture = Fixture::new();
    let invocation_uuid = "44444444-4444-4444-8444-444444444444";
    let invocation_row_id = fixture.start_and_finalize_invocation(invocation_uuid);
    fixture
        .conn()
        .execute_batch("DROP TABLE session_chain_segments;")
        .unwrap();
    let mut stderr = Vec::new();
    let service = ProductionSessionLifecycleService::new();

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: "claude",
            external_provider: None,
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Pinned {
                resume_target: "55555555-5555-4555-8555-555555555555".to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("mint failure is warning-only after capture update");

    assert_eq!(
        output,
        SessionLifecycleOutput {
            emitted: true,
            session_id: Some("55555555-5555-4555-8555-555555555555".to_string()),
        }
    );
    let stderr = stderr_text(stderr);
    assert!(!stderr.contains("Warning: Failed to resolve session for invocation"));
    assert!(stderr.contains("Warning: Failed to mint session chain:"));
    assert!(stderr.contains("OULIPOLY_SESSION="));
    assert!(stderr.contains("55555555-5555-4555-8555-555555555555"));
}

#[test]
fn session_lifecycle_rejects_mismatched_invocation_identifiers() {
    let fixture = Fixture::new();
    let invocation_uuid = "66666666-6666-4666-8666-666666666666";
    let invocation_row_id = fixture.start_and_finalize_invocation(invocation_uuid);
    let other_row_id =
        fixture.start_and_finalize_invocation("77777777-7777-4777-8777-777777777777");
    let mut stderr = Vec::new();
    let service = ProductionSessionLifecycleService::new();

    let err = service
        .ingest_session(SessionLifecycleRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: "claude",
            external_provider: None,
            invocation_row_id: other_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Pinned {
                resume_target: "88888888-8888-4888-8888-888888888888".to_string(),
            },
            stderr: &mut stderr,
        })
        .unwrap_err();

    match err {
        ServiceError::InvalidRequest { message } => {
            assert!(message.contains(&other_row_id.to_string()));
            assert!(message.contains(invocation_uuid));
        }
        other => panic!("expected invalid request, got {other:?}"),
    }
    assert!(stderr.is_empty());
    let requested = fixture
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    let other = fixture
        .state
        .get_invocation_by_uuid("77777777-7777-4777-8777-777777777777")
        .unwrap()
        .unwrap();
    assert_eq!(requested.id, invocation_row_id);
    assert_eq!(requested.session_id, None);
    assert_eq!(other.session_id, None);
}

#[test]
fn session_lifecycle_external_capture_persists_identity_and_queues_bounded_ingest() {
    let provider = ProviderAFixture::new("capture_success");
    let invocation_uuid = "99999999-9999-4999-8999-999999999999";
    let effective_cwd = provider.fixture.path().join("workspace");
    std::fs::create_dir_all(&effective_cwd).unwrap();
    let invocation_row_id = provider
        .fixture
        .start_and_finalize_provider_a_invocation(invocation_uuid);
    let mut stderr = Vec::new();
    let service =
        ProductionSessionLifecycleService::with_registry_handle(provider.registry_handle());

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &provider.fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: PROVIDER_A_ACCOUNT,
            external_provider: Some(external_provider_identity()),
            invocation_row_id,
            invocation_uuid,
            effective_cwd: Some(&effective_cwd),
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "provider_session_capture".to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("external lifecycle dispatch");

    assert_eq!(
        output,
        SessionLifecycleOutput {
            emitted: true,
            session_id: Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()),
        }
    );
    let stderr = stderr_text(stderr);
    assert!(stderr.starts_with("OULIPOLY_SESSION="), "{stderr}");
    assert!(
        stderr.contains("\"provider_name\":\"provider-a-account\""),
        "{stderr}"
    );
    assert!(
        stderr.contains("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        "{stderr}"
    );
    assert!(provider.fixture.session_turn_rows().is_empty());
    assert_canonical_stream_queued(
        &provider.fixture.state,
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
    );
    let row = provider
        .fixture
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.provider_session_id.as_deref(),
        Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
    );
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("provider_session_capture")
    );
    assert_provider_lifecycle_dispatch_shape(
        provider.records(),
        invocation_uuid,
        invocation_row_id,
        Some(&effective_cwd),
        None,
        None,
    );
}

#[test]
fn session_lifecycle_external_capture_preserves_start_bound_session_over_provider_fact() {
    let provider = ProviderAFixture::new("capture_conflict");
    let invocation_uuid = "aaaaaaaa-0000-4000-8000-000000000000";
    let invocation_row_id = provider
        .fixture
        .start_and_finalize_provider_a_invocation(invocation_uuid);
    provider
        .fixture
        .state
        .update_session_capture(
            invocation_row_id,
            Some("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
            "forced_flag_verified",
        )
        .unwrap();
    provider.fixture.seed_chain(
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        PROVIDER_A_ACCOUNT,
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        PROVIDER_A_MODEL,
    );
    let mut stderr = Vec::new();
    let service =
        ProductionSessionLifecycleService::with_registry_handle(provider.registry_handle());

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &provider.fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: PROVIDER_A_ACCOUNT,
            external_provider: Some(external_provider_identity()),
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "provider_session_capture".to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("external lifecycle dispatch");

    assert_eq!(
        output.session_id.as_deref(),
        Some("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")
    );
    let stderr = stderr_text(stderr);
    assert!(stderr.contains("OULIPOLY_SESSION="), "{stderr}");
    assert!(
        stderr.contains("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("\"provider_session_id\":\"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa\""),
        "provider capture fact must not override start-bound session: {stderr}"
    );
    assert_provider_lifecycle_dispatch_shape(
        provider.records(),
        invocation_uuid,
        invocation_row_id,
        None,
        None,
        Some("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
    );
}

#[test]
fn session_lifecycle_external_pinned_capture_preserves_resume_target_over_provider_fact() {
    let provider = ProviderAFixture::new("capture_conflict");
    let invocation_uuid = "abababab-0000-4000-8000-000000000000";
    let pinned_session = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
    let provider_session = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let invocation_row_id = provider
        .fixture
        .start_and_finalize_provider_a_invocation(invocation_uuid);
    provider.fixture.seed_chain(
        "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
        PROVIDER_A_ACCOUNT,
        pinned_session,
        PROVIDER_A_MODEL,
    );
    let mut stderr = Vec::new();
    let service =
        ProductionSessionLifecycleService::with_registry_handle(provider.registry_handle());

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &provider.fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: PROVIDER_A_ACCOUNT,
            external_provider: Some(external_provider_identity()),
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Pinned {
                resume_target: pinned_session.to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("external pinned lifecycle dispatch");

    assert_eq!(output.session_id.as_deref(), Some(pinned_session));
    let stderr = stderr_text(stderr);
    assert!(stderr.contains("OULIPOLY_SESSION="), "{stderr}");
    assert!(stderr.contains(pinned_session), "{stderr}");
    assert!(
        !stderr.contains(provider_session),
        "provider capture fact must not override pinned marker bytes: {stderr}"
    );
    let row = provider
        .fixture
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(pinned_session));
    assert_eq!(row.resume_input_id.as_deref(), Some(pinned_session));
    assert_ne!(row.provider_session_id.as_deref(), Some(provider_session));
    let subcommands = provider
        .records()
        .iter()
        .map(|record| record["subcommand"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        subcommands.contains(&"session.capture".to_string()),
        "external pinned lifecycle must prove precedence over provider capture facts: {subcommands:?}"
    );
    assert_provider_lifecycle_dispatch_shape(
        provider.records(),
        invocation_uuid,
        invocation_row_id,
        None,
        Some(pinned_session),
        None,
    );
}

#[test]
fn provider_ref_lifecycle_resume_captures_queues_and_preserves_pinned_target() {
    let provider = ProviderAFixture::new("capture_success");
    let invocation_uuid = "cdcdcdcd-0000-4000-8000-000000000000";
    let pinned_session = "edededed-eded-4ede-8ede-edededededed";
    let provider_session = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let invocation_row_id = provider
        .fixture
        .start_and_finalize_provider_a_invocation(invocation_uuid);
    provider.fixture.seed_chain(
        "fefefefe-fefe-4fef-8fef-fefefefefefe",
        PROVIDER_A_ACCOUNT,
        pinned_session,
        PROVIDER_A_MODEL,
    );
    let scan_record_path = provider.fixture.path().join("local-scan-record.txt");
    let sessions_cfg = SessionsConfig {
        entries: HashMap::from([(
            PROVIDER_A_ACCOUNT.to_string(),
            SessionSourceEntry {
                turn_script: format!(
                    "printf local-scan >> {:?}",
                    scan_record_path.display().to_string()
                ),
                transcript_locator: None,
                state_dir: Some(provider.fixture.path().join("local-scan-state")),
            },
        )]),
    };
    let mut stderr = Vec::new();
    let service =
        ProductionSessionLifecycleService::with_registry_handle(provider.registry_handle());

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &provider.fixture.state,
            sessions_cfg: &sessions_cfg,
            providers_cfg: None,
            provider_name: PROVIDER_A_ACCOUNT,
            external_provider: Some(external_provider_identity()),
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Pinned {
                resume_target: pinned_session.to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("external pinned lifecycle dispatch");

    assert!(
        !scan_record_path.exists(),
        "provider-ref lifecycle must not run local sessions scan"
    );
    assert_eq!(output.session_id.as_deref(), Some(pinned_session));
    assert!(provider.fixture.session_turn_rows().is_empty());
    assert_canonical_stream_queued(&provider.fixture.state, pinned_session);
    let row = provider
        .fixture
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some(pinned_session));
    assert_eq!(row.resume_input_id.as_deref(), Some(pinned_session));
    assert_ne!(row.provider_session_id.as_deref(), Some(provider_session));
    let stderr = stderr_text(stderr);
    assert!(stderr.contains("OULIPOLY_SESSION="), "{stderr}");
    assert!(stderr.contains(pinned_session), "{stderr}");
    assert!(!stderr.contains(provider_session), "{stderr}");
    assert_provider_lifecycle_dispatch_shape(
        provider.records(),
        invocation_uuid,
        invocation_row_id,
        None,
        Some(pinned_session),
        None,
    );
}

#[test]
fn provider_ref_lifecycle_empty_capture_then_window_match_uses_script_cwd() {
    let provider = ProviderAFixture::new("empty_capture");
    let invocation_uuid = "34343434-3434-4434-8434-343434343434";
    let correct_session = "45454545-4545-4454-8454-454545454545";
    let wrong_session = "56565656-5656-4656-8656-565656565656";
    let correct_workspace = provider.fixture.path().join("rfq");
    let wrong_workspace = provider.fixture.path().join("other");
    std::fs::create_dir_all(&correct_workspace).unwrap();
    std::fs::create_dir_all(&wrong_workspace).unwrap();
    let cwd_record = provider.fixture.path().join("cwd-record.txt");
    let cwd_script = provider.fixture.path().join("cwd-script.sh");
    let correct_response = serde_json::json!({
        "found": true,
        "cwd": correct_workspace,
    })
    .to_string();
    let wrong_response = serde_json::json!({
        "found": true,
        "cwd": wrong_workspace,
    })
    .to_string();
    std::fs::write(
        &cwd_script,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nline_count=$(wc -l < {:?} | tr -d ' ')\ncase \"$SESSION_ID\" in\n  {correct_session}) printf '%s|%s\\n' \"$SESSION_ID\" \"$line_count\" >> {:?}; printf '%s\\n' {:?} ;;\n  {wrong_session}) printf '%s\\n' {:?} ;;\n  *) printf '%s\\n' '{{\"found\":false}}' ;;\nesac\n",
            provider.record_path.display().to_string(),
            cwd_record.display().to_string(),
            correct_response,
            wrong_response,
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&cwd_script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&cwd_script, perms).unwrap();
    let providers_cfg = ProvidersConfig {
        entries: HashMap::from([(
            PROVIDER_A_ACCOUNT.to_string(),
            ProviderEntry {
                command: Some("provider-command-that-must-not-run".to_string()),
                session_storage: Some(SessionStorage::Script {
                    cwd_script: cwd_script.display().to_string(),
                    transcript_script: None,
                    storage_type: None,
                }),
                ..ProviderEntry::default()
            },
        )]),
    };
    let invocation_row_id = provider
        .fixture
        .state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: PROVIDER_A_MODEL.to_string(),
            provider_name: PROVIDER_A_ACCOUNT.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    provider
        .fixture
        .state
        .ingest_session_turns_batch(
            PROVIDER_A_ACCOUNT,
            &[
                turn(wrong_session, "wrong-1", "assistant"),
                turn(wrong_session, "wrong-2", "user"),
                turn(correct_session, "correct-1", "assistant"),
            ],
        )
        .unwrap();
    provider
        .fixture
        .state
        .finalize_invocation(invocation_row_id, true, 0, None, Some("completed"))
        .unwrap();
    let mut stderr = Vec::new();
    let service =
        ProductionSessionLifecycleService::with_registry_handle(provider.registry_handle());

    let output = service
        .ingest_session(SessionLifecycleRequest {
            state: &provider.fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: Some(&providers_cfg),
            provider_name: PROVIDER_A_ACCOUNT,
            external_provider: Some(external_provider_identity()),
            invocation_row_id,
            invocation_uuid,
            effective_cwd: Some(&correct_workspace),
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "empty_capture".to_string(),
            },
            stderr: &mut stderr,
        })
        .expect("external lifecycle dispatch with window match");

    assert_eq!(output.session_id.as_deref(), Some(correct_session));
    let stderr = stderr_text(stderr);
    assert!(stderr.contains(correct_session), "{stderr}");
    assert!(!stderr.contains(wrong_session), "{stderr}");
    let records = provider.records();
    assert_provider_lifecycle_dispatch_shape(
        records.clone(),
        invocation_uuid,
        invocation_row_id,
        Some(&correct_workspace),
        None,
        None,
    );
    assert_eq!(provider_lifecycle_subcommands(&records).len(), 2);
    assert_canonical_stream_queued(&provider.fixture.state, correct_session);
    let cwd_records = std::fs::read_to_string(&cwd_record).unwrap();
    assert!(
        cwd_records
            .lines()
            .any(|line| line == format!("{correct_session}|2")),
        "cwd records: {cwd_records}"
    );
    assert!(
        !cwd_records.contains(wrong_session),
        "wrong session appeared in cwd records: {cwd_records}"
    );
    assert!(
        cwd_records.lines().all(|line| line.ends_with("|2")),
        "cwd ran before provider records were complete: {cwd_records}"
    );
}

#[test]
fn session_lifecycle_no_ref_registry_path_preserves_marker_warnings_and_state() {
    let baseline = seeded_no_ref_marker_fixture();
    let dispatch = seeded_no_ref_marker_fixture();
    let unrelated_provider = ProviderAFixture::new("capture_success");
    let service = ProductionSessionLifecycleService::new();
    let registry_service = ProductionSessionLifecycleService::with_registry_handle(
        unrelated_provider.registry_handle(),
    );
    let invocation_uuid = "12121212-1212-4212-8212-121212121212";
    let invocation_row_id = 1;
    let mut baseline_stderr = Vec::new();
    let mut dispatch_stderr = Vec::new();
    let provider_name = legacy_provider_name();

    let baseline_output = service
        .ingest_session(SessionLifecycleRequest {
            state: &baseline.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: &provider_name,
            external_provider: None,
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "turn_script".to_string(),
            },
            stderr: &mut baseline_stderr,
        })
        .unwrap();
    let dispatch_output = registry_service
        .ingest_session(SessionLifecycleRequest {
            state: &dispatch.state,
            sessions_cfg: &SessionsConfig::default(),
            providers_cfg: None,
            provider_name: &provider_name,
            external_provider: None,
            invocation_row_id,
            invocation_uuid,
            effective_cwd: None,
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: "turn_script".to_string(),
            },
            stderr: &mut dispatch_stderr,
        })
        .unwrap();

    assert_eq!(dispatch_output, baseline_output);
    assert_eq!(dispatch_stderr, baseline_stderr);
    assert_eq!(dispatch.session_turn_rows(), baseline.session_turn_rows());
    assert!(unrelated_provider.records().is_empty());
    let baseline_row = baseline
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    let dispatch_row = dispatch
        .state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(dispatch_row.session_id, baseline_row.session_id);
    assert_eq!(
        dispatch_row.session_capture_method,
        baseline_row.session_capture_method
    );
    assert_eq!(
        dispatch_row.provider_session_id,
        baseline_row.provider_session_id
    );
    assert_eq!(
        dispatch_row.provider_session_capture_method,
        baseline_row.provider_session_capture_method
    );
}

fn seeded_no_ref_marker_fixture() -> Fixture {
    let fixture = Fixture::new();
    let invocation_uuid = "12121212-1212-4212-8212-121212121212";
    let invocation_row_id = fixture.start_and_finalize_invocation(invocation_uuid);
    assert_eq!(invocation_row_id, 1, "fresh fixture should use row id 1");
    let provider_name = legacy_provider_name();
    let model_name = legacy_model_name();
    fixture.seed_chain(
        "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        &provider_name,
        "99999999-9999-4999-8999-999999999999",
        &model_name,
    );
    fixture
        .state
        .ingest_session_turns_batch(
            &provider_name,
            &[turn(
                "99999999-9999-4999-8999-999999999999",
                "turn-no-ref-1",
                "assistant",
            )],
        )
        .unwrap();
    fixture
}

fn assert_provider_lifecycle_dispatch_shape(
    records: Vec<serde_json::Value>,
    invocation_uuid: &str,
    invocation_row_id: i64,
    effective_cwd: Option<&Path>,
    pinned_target: Option<&str>,
    start_bound_provider_session_id: Option<&str>,
) {
    assert_provider_lifecycle_subcommands(&records);
    for request in provider_lifecycle_requests(&records) {
        assert_provider_lifecycle_request(
            request,
            invocation_uuid,
            invocation_row_id,
            effective_cwd,
            pinned_target,
            start_bound_provider_session_id,
        );
    }
}

fn assert_provider_lifecycle_subcommands(records: &[serde_json::Value]) {
    assert_eq!(
        provider_lifecycle_subcommands(records),
        vec!["describe", "session.capture"]
    );
}

fn provider_lifecycle_subcommands(records: &[serde_json::Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| record["subcommand"].as_str().unwrap().to_string())
        .collect()
}

fn provider_lifecycle_requests(records: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    records
        .iter()
        .filter(|record| record["subcommand"] != "describe")
        .map(|record| &record["request"])
        .collect()
}

fn assert_provider_lifecycle_request(
    request: &serde_json::Value,
    invocation_uuid: &str,
    invocation_row_id: i64,
    effective_cwd: Option<&Path>,
    pinned_target: Option<&str>,
    start_bound_provider_session_id: Option<&str>,
) {
    assert_eq!(request["provider_instance_id"], PROVIDER_A_INSTANCE);
    assert_eq!(request["params"]["settings_id"], PROVIDER_A_SETTINGS);
    assert_eq!(request["params"]["model_name"], PROVIDER_A_MODEL);
    assert_eq!(request["params"]["provider_name"], PROVIDER_A_ACCOUNT);
    assert_eq!(request["params"]["invocation_uuid"], invocation_uuid);
    assert_eq!(request["params"]["invocation_row_id"], invocation_row_id);
    assert_effective_cwd(request, effective_cwd);
    assert_pinned_target(request, pinned_target);
    assert_start_bound_provider_session_id(request, start_bound_provider_session_id);
    assert_request_does_not_expose_state_db(request);
}

fn assert_effective_cwd(request: &serde_json::Value, effective_cwd: Option<&Path>) {
    if let Some(effective_cwd) = effective_cwd {
        assert_eq!(
            request["params"]["effective_cwd"],
            effective_cwd.display().to_string()
        );
    }
}

fn assert_pinned_target(request: &serde_json::Value, pinned_target: Option<&str>) {
    if let Some(pinned_target) = pinned_target {
        assert_eq!(request["params"]["pinned_target"], pinned_target);
    }
}

fn assert_start_bound_provider_session_id(
    request: &serde_json::Value,
    start_bound_provider_session_id: Option<&str>,
) {
    if let Some(start_bound_provider_session_id) = start_bound_provider_session_id {
        assert_eq!(
            request["params"]["start_bound_provider_session_id"],
            start_bound_provider_session_id
        );
    }
}

fn assert_request_does_not_expose_state_db(request: &serde_json::Value) {
    assert!(
        !request_json_for_message(request).contains("state.db"),
        "provider lifecycle requests must not expose host state.db paths"
    );
}

fn request_json_for_message(request: &serde_json::Value) -> String {
    request.to_string()
}

fn write_provider_a_lifecycle_script(dir: &Path, mode_path: &Path, record_path: &Path) -> PathBuf {
    let script = dir.join("provider-a-lifecycle.py");
    std::fs::write(&script, provider_a_lifecycle_script(mode_path, record_path)).unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
    script
}

fn provider_a_lifecycle_script(mode_path: &Path, record_path: &Path) -> String {
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
mode = pathlib.Path({mode_path}).read_text().strip()
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
with pathlib.Path({record_path}).open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": True,
        "result": result,
    }}

def describe():
    return envelope({{
        "provider_id": "provider-a",
        "display_name": "Provider A",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
    }})

def capture():
    if mode == "empty_capture":
        return envelope({{
            "provider_session_id": "",
            "state": {{"cursor": "empty"}},
            "artifacts": [],
        }})
    return envelope({{
        "provider_session_id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "state": {{"cursor": "provider-owned"}},
        "artifacts": [],
    }})

if subcommand == "describe":
    response = describe()
elif subcommand == "session.capture":
    response = capture()
else:
    response = {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": False,
        "error": {{"category": "unsupported", "code": "unsupported", "message": subcommand, "retryable": False}},
    }}
print(json.dumps(response))
"#,
        mode_path = serde_json::to_string(&mode_path.display().to_string()).unwrap(),
        record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
    )
}
