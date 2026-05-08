use oulipoly_config::SessionsConfig;
use oulipoly_runtime::services::{
    ProductionSessionLifecycleService, ServiceError, SessionLifecycleIngestMode,
    SessionLifecycleOutput, SessionLifecycleRequest, SessionLifecycleServicePort,
};
use oulipoly_state::{InvocationStart, StateDb};
use rusqlite::Connection;

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
            provider_name: "claude",
            invocation_row_id,
            invocation_uuid,
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
            provider_name: "claude",
            invocation_row_id,
            invocation_uuid,
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
            provider_name: "claude",
            invocation_row_id,
            invocation_uuid,
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
            provider_name: "claude",
            invocation_row_id: other_row_id,
            invocation_uuid,
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
