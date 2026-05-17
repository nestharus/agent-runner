use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::services::{
    ProductionResumeService, ResumeAcceptanceOutput, ResumeAcceptanceRequest, ResumeServiceOutput,
    ResumeServicePort, ResumeServiceRequest,
};
use oulipoly_state::repositories::ResumeRepository;
use oulipoly_state::{InvocationStart, ModelStore, ResolvedResume, ResumeError, StateDb};
use rusqlite::{Connection, params};
use std::path::PathBuf;

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

struct Fixture {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        Self { _dir: dir, db_path }
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path).unwrap()
    }

    fn conn(&self) -> Connection {
        Connection::open(&self.db_path).unwrap()
    }

    fn seed_active_chain(&self, chain_id: &str, provider: &str, session_id: &str, model: &str) {
        let _db = self.open_db();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![chain_id, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![chain_id, provider, session_id],
        )
        .unwrap();
    }
}

fn model(name: &str, providers: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: providers
            .iter()
            .map(|provider| ProviderConfig::model_provider(*provider, Vec::new()))
            .collect(),
        inputs: Vec::new(),
    }
}

fn model_store(models: Vec<ModelConfig>) -> ModelStore {
    models
        .into_iter()
        .map(|model| (model.name.clone(), model))
        .collect()
}

fn assert_resolved_same(actual: &ResolvedResume, expected: &ResolvedResume) {
    assert_eq!(actual.chain_id, expected.chain_id);
    assert_eq!(actual.model_name, expected.model_name);
    assert_eq!(actual.active_provider, expected.active_provider);
    assert_eq!(actual.active_session_id, expected.active_session_id);
    assert_eq!(
        actual.model.as_ref().map(|model| &model.name),
        expected.model.as_ref().map(|model| &model.name)
    );
}

fn assert_resolved_expected_segment_only_shape(
    actual: &ResolvedResume,
    expected_chain_id: &str,
    expected_model_name: Option<&str>,
    expected_model: Option<&str>,
    expected_provider: &str,
    expected_session_id: &str,
) {
    let ResolvedResume {
        chain_id,
        model_name,
        model,
        active_provider,
        active_session_id,
    } = actual;

    assert_eq!(chain_id, expected_chain_id);
    assert_eq!(model_name.as_deref(), expected_model_name);
    assert_eq!(
        model.as_ref().map(|model| model.name.as_str()),
        expected_model
    );
    assert_eq!(active_provider, expected_provider);
    assert_eq!(active_session_id, expected_session_id);
}

fn start_invocation(db: &StateDb, uuid: &str) -> i64 {
    db.start_invocation(&InvocationStart {
        invocation_uuid: uuid.to_string(),
        model_name: "claude-opus".to_string(),
        provider_name: "claude".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
    .unwrap()
}

#[test]
fn resume_service_resolve_resume_preserves_typed_rejections() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "claude-a", SESSION_A, "claude-opus");
    let db = fixture.open_db();
    let models = model_store(vec![
        model("claude-opus", &["claude-a"]),
        model("codex-low", &["codex"]),
    ]);
    let service = ProductionResumeService::new();

    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            input: SESSION_A,
            model_override: Some("codex-low"),
        })
        .expect("domain rejection crosses as Ok");

    match output {
        ResumeServiceOutput::ResumeRejected {
            error:
                ResumeError::ProviderModelMismatch {
                    model_name,
                    active_provider,
                    suggestions,
                },
        } => {
            assert_eq!(model_name, "codex-low");
            assert_eq!(active_provider, "claude-a");
            assert_eq!(suggestions, vec!["claude-opus"]);
        }
        other => panic!("expected typed provider/model mismatch rejection, got {other:?}"),
    }
}

#[test]
fn resume_service_resolve_resume_matches_repository_resolve_resume() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "claude-a", SESSION_A, "claude-opus");
    let db = fixture.open_db();
    let models = model_store(vec![model("claude-opus", &["claude-a"])]);
    let expected =
        <StateDb as ResumeRepository>::resolve_resume(&db, &models, SESSION_A, None).unwrap();
    let service = ProductionResumeService::new();

    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            input: SESSION_A,
            model_override: None,
        })
        .expect("service resolution succeeds");

    match output {
        ResumeServiceOutput::ResumeResolved { resolved } => {
            assert_resolved_same(&resolved, &expected);
        }
        other => panic!("expected resolved resume, got {other:?}"),
    }
}

#[test]
fn resume_service_resolve_resume_remains_expected_segment_only() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "claude-a", SESSION_A, "claude-opus");
    let db = fixture.open_db();
    let models = model_store(vec![model("claude-opus", &["claude-a"])]);

    let direct = StateDb::resolve_resume(&db, &models, SESSION_A, Some("claude-opus")).unwrap();
    let repository =
        <StateDb as ResumeRepository>::resolve_resume(&db, &models, SESSION_A, Some("claude-opus"))
            .unwrap();
    assert_resolved_expected_segment_only_shape(
        &direct,
        CHAIN_A,
        Some("claude-opus"),
        Some("claude-opus"),
        "claude-a",
        SESSION_A,
    );
    assert_resolved_same(&repository, &direct);

    let service = ProductionResumeService::new();
    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            input: SESSION_A,
            model_override: Some("claude-opus"),
        })
        .expect("service resolution succeeds");

    match output {
        ResumeServiceOutput::ResumeResolved { resolved } => {
            assert_resolved_expected_segment_only_shape(
                &resolved,
                CHAIN_A,
                Some("claude-opus"),
                Some("claude-opus"),
                "claude-a",
                SESSION_A,
            );
            assert_resolved_same(&resolved, &direct);
            assert_resolved_same(&resolved, &repository);
        }
        other => panic!("expected resolved resume, got {other:?}"),
    }
}

#[test]
fn resume_service_records_acceptance_matches_state_db_update_resume_acceptance() {
    let direct_fixture = Fixture::new();
    let direct_db = direct_fixture.open_db();
    let direct_id = start_invocation(&direct_db, "11111111-1111-4111-8111-111111111111");
    direct_db
        .update_resume_acceptance(direct_id, "accepted", Some("matched session id"))
        .unwrap();
    let expected = direct_db
        .get_invocation_by_uuid("11111111-1111-4111-8111-111111111111")
        .unwrap()
        .unwrap();

    let service_fixture = Fixture::new();
    let service_db = service_fixture.open_db();
    let service_id = start_invocation(&service_db, "22222222-2222-4222-8222-222222222222");
    let service = ProductionResumeService::new();

    let output = service
        .record_acceptance(ResumeAcceptanceRequest {
            state: &service_db,
            invocation_row_id: service_id,
            status: "accepted",
            evidence: Some("matched session id"),
        })
        .expect("service records resume acceptance");

    assert_eq!(output, ResumeAcceptanceOutput);
    let actual = service_db
        .get_invocation_by_uuid("22222222-2222-4222-8222-222222222222")
        .unwrap()
        .unwrap();
    assert_eq!(
        actual.resume_acceptance_status,
        expected.resume_acceptance_status
    );
    assert_eq!(
        actual.resume_acceptance_evidence,
        expected.resume_acceptance_evidence
    );
}

#[test]
fn resume_service_repository_parity_matches_state_db_resolve_resume() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "claude-a", SESSION_B, "claude-opus");
    let db = fixture.open_db();
    let models = model_store(vec![model("claude-opus", &["claude-a"])]);
    let direct = StateDb::resolve_resume(&db, &models, SESSION_B, Some("claude-opus")).unwrap();
    let repository =
        <StateDb as ResumeRepository>::resolve_resume(&db, &models, SESSION_B, Some("claude-opus"))
            .unwrap();
    assert_resolved_same(&repository, &direct);

    let service = ProductionResumeService::new();
    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            input: SESSION_B,
            model_override: Some("claude-opus"),
        })
        .expect("service resolution succeeds");

    match output {
        ResumeServiceOutput::ResumeResolved { resolved } => {
            assert_resolved_same(&resolved, &direct);
            assert_resolved_same(&resolved, &repository);
        }
        other => panic!("expected resolved resume, got {other:?}"),
    }
}
