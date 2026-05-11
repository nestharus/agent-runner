use chrono::Utc;
use oulipoly_config::{ProviderEntry, ProvidersConfig, SessionStorage, SessionsConfig};
use oulipoly_runtime::services::{
    ProductionSessionLifecycleService, ServiceError, SessionLifecycleIngestMode,
    SessionLifecycleOutput, SessionLifecycleRequest, SessionLifecycleServicePort,
};
use oulipoly_state::{InvocationStart, SessionTurnIngest, StateDb};
use rusqlite::Connection;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;

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
