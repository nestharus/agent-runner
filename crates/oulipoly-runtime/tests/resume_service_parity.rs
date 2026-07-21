//! ## Declared roles
//! orchestration, mapper, validator, accessor
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/tests/resume_service_parity.rs
//!     role: intrinsic-surface
//!     Domain: resume-service-parity-test-domain
//!     Owns:
//!       - oulipoly_runtime::services::ProductionResumeService
//!       - oulipoly_runtime::services ResumeAcceptanceOutput, ResumeAcceptanceRequest, ResumeServiceOutput, ResumeServicePort, ResumeServiceRequest
//!       - oulipoly_state::repositories::ResumeRepository
//!       - oulipoly_state InvocationStart, ModelStore, ResolvedResume, ResumeError, StateDb
//!       - Fixture fixture surface
//!       - assert_resolved_same assertion validator
//!       - assert_resolved_expected_segment_only_shape assertion validator
//!       - model and model_store mappers
//!       - start_invocation accessor

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEntry, ProvidersConfig, ResumeKind,
    ResumeStrategy, SessionStorage,
};
use oulipoly_runtime::services::{
    ProductionResumeService, ResumeAcceptanceOutput, ResumeAcceptanceRequest, ResumeServiceOutput,
    ResumeServicePort, ResumeServiceRejection, ResumeServiceRequest,
};
use oulipoly_state::repositories::ResumeRepository;
use oulipoly_state::{InvocationStart, ModelStore, ResolvedResume, ResumeError, StateDb};
use rusqlite::{Connection, params};
use std::path::PathBuf;

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const CHAIN_C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

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

    fn seed_migrated_chain(
        &self,
        chain_id: &str,
        matching_provider: &str,
        input_session_id: &str,
        active_provider: &str,
        active_session_id: &str,
        model: &str,
    ) {
        self.seed_active_chain(chain_id, matching_provider, input_session_id, model);
        let conn = self.conn();
        conn.execute(
            "UPDATE session_chain_segments SET ended_at = '2026-04-17T09:00:00Z'
             WHERE chain_id = ?1 AND provider_name = ?2 AND session_id = ?3",
            params![chain_id, matching_provider, input_session_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T09:00:01Z', 'quota_threshold')",
            params![chain_id, active_provider, active_session_id],
        )
        .unwrap();
    }
}

fn providers_config(entries: &[(&str, &str)]) -> ProvidersConfig {
    let mut config = ProvidersConfig::default();
    for (provider_name, response) in entries {
        insert_provider(
            &mut config,
            provider_name,
            format!("printf %s {}; :", shell_quote(response)),
        );
    }
    config
}

fn insert_provider(config: &mut ProvidersConfig, provider_name: &str, cwd_script: String) {
    config.entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            command: Some(provider_name.to_string()),
            resume: Some(ResumeStrategy {
                kind: ResumeKind::Flag,
                flag: Some("--session".to_string()),
                subcommand: None,
            }),
            session_storage: Some(SessionStorage::Script {
                cwd_script,
                transcript_script: None,
                storage_type: None,
            }),
            ..ProviderEntry::default()
        },
    );
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
        provider: None,
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
    let providers_cfg = ProvidersConfig::default();

    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
            input: SESSION_A,
            model_override: Some("codex-low"),
        })
        .expect("domain rejection crosses as Ok");

    match output {
        ResumeServiceOutput::ResumeRejected {
            error:
                ResumeServiceRejection::State(ResumeError::ProviderModelMismatch {
                    model_name,
                    active_provider,
                    suggestions,
                }),
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
    let providers_cfg = ProvidersConfig::default();

    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
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
    let providers_cfg = ProvidersConfig::default();
    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
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
fn resume_service_bypasses_ownership_for_exact_and_single_native_inputs() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "provider-a", SESSION_A, "shared");
    let db = fixture.open_db();
    let models = model_store(vec![model("shared", &["provider-a"])]);
    let mut providers_cfg = ProvidersConfig::default();
    insert_provider(&mut providers_cfg, "provider-a", "exit 93".to_string());
    let service = ProductionResumeService::new();

    for input in [CHAIN_A, SESSION_A] {
        let output = service
            .resolve_resume(ResumeServiceRequest {
                state: &db,
                models: &models,
                providers_cfg: &providers_cfg,
                input,
                model_override: None,
            })
            .unwrap();
        assert!(
            matches!(output, ResumeServiceOutput::ResumeResolved { ref resolved } if resolved.chain_id == CHAIN_A),
            "exact and single-native inputs must not execute the failing ownership script: {output:?}"
        );
    }
}

#[test]
fn resume_service_selects_unique_storage_owner_using_original_native_id() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "provider-a", SESSION_A, "shared");
    fixture.seed_active_chain(CHAIN_B, "provider-b", SESSION_A, "shared");
    let db = fixture.open_db();
    let models = model_store(vec![model("shared", &["provider-a", "provider-b"])]);
    let record = fixture._dir.path().join("ownership-probes.txt");
    let mut providers_cfg = ProvidersConfig::default();
    insert_provider(
        &mut providers_cfg,
        "provider-a",
        format!(
            "printf '%s\\n' \"$1\" >> {}; printf '%s\\n' '{{\"owned\":false}}'; :",
            shell_quote(&record.display().to_string())
        ),
    );
    insert_provider(
        &mut providers_cfg,
        "provider-b",
        format!(
            "printf '%s\\n' \"$1\" >> {}; printf '%s\\n' '{{\"owned\":true}}'; :",
            shell_quote(&record.display().to_string())
        ),
    );

    let output = ProductionResumeService::new()
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
            input: SESSION_A,
            model_override: None,
        })
        .unwrap();

    assert!(
        matches!(output, ResumeServiceOutput::ResumeResolved { resolved } if resolved.chain_id == CHAIN_B && resolved.active_provider == "provider-b")
    );
    assert_eq!(
        std::fs::read_to_string(record).unwrap(),
        format!("{SESSION_A}\n{SESSION_A}\n")
    );
}

#[test]
fn resume_service_owner_on_old_segment_finalizes_current_active_segment() {
    let fixture = Fixture::new();
    fixture.seed_migrated_chain(
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "provider-c",
        SESSION_B,
        "shared",
    );
    fixture.seed_active_chain(CHAIN_B, "provider-b", SESSION_A, "shared");
    let db = fixture.open_db();
    let models = model_store(vec![model(
        "shared",
        &["provider-a", "provider-b", "provider-c"],
    )]);
    let providers_cfg = providers_config(&[
        ("provider-a", "{\"owned\":true}\n"),
        ("provider-b", "{\"owned\":false}\n"),
    ]);

    let output = ProductionResumeService::new()
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
            input: SESSION_A,
            model_override: None,
        })
        .unwrap();

    assert!(matches!(
        output,
        ResumeServiceOutput::ResumeResolved { resolved }
            if resolved.chain_id == CHAIN_A
                && resolved.active_provider == "provider-c"
                && resolved.active_session_id == SESSION_B
    ));
}

#[test]
fn resume_service_distinguishes_ownership_rejections() {
    for (name, a, b, expected) in [
        (
            "no owner",
            "{\"owned\":false}\n",
            "{\"owned\":false}\n",
            "not-found",
        ),
        (
            "multiple owners",
            "{\"owned\":true}\n",
            "{\"owned\":true}\n",
            "ambiguous",
        ),
        (
            "positive and indeterminate",
            "{\"owned\":true}\n",
            "{\"found\":false}\n",
            "indeterminate",
        ),
        (
            "zero positive and indeterminate",
            "{\"owned\":false}\n",
            "{\"found\":false}\n",
            "indeterminate",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.seed_active_chain(CHAIN_A, "provider-a", SESSION_A, "shared");
        fixture.seed_active_chain(CHAIN_B, "provider-b", SESSION_A, "shared");
        let db = fixture.open_db();
        let models = model_store(vec![model("shared", &["provider-a", "provider-b"])]);
        let providers_cfg = providers_config(&[("provider-a", a), ("provider-b", b)]);
        let output = ProductionResumeService::new()
            .resolve_resume(ResumeServiceRequest {
                state: &db,
                models: &models,
                providers_cfg: &providers_cfg,
                input: SESSION_A,
                model_override: None,
            })
            .unwrap();

        let matched = matches!(
            (&output, expected),
            (
                ResumeServiceOutput::ResumeRejected {
                    error: ResumeServiceRejection::StorageOwnerNotFound { .. }
                },
                "not-found"
            ) | (
                ResumeServiceOutput::ResumeRejected {
                    error: ResumeServiceRejection::StorageOwnershipAmbiguous { .. }
                },
                "ambiguous"
            ) | (
                ResumeServiceOutput::ResumeRejected {
                    error: ResumeServiceRejection::StorageOwnershipIndeterminate { .. }
                },
                "indeterminate"
            )
        );
        assert!(matched, "{name}: unexpected output {output:?}");
    }
}

#[test]
fn resume_service_multiple_known_owners_win_over_additional_probe_failure() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "provider-a", SESSION_A, "shared");
    fixture.seed_active_chain(CHAIN_B, "provider-b", SESSION_A, "shared");
    fixture.seed_active_chain(CHAIN_C, "provider-c", SESSION_A, "shared");
    let db = fixture.open_db();
    let models = model_store(vec![model(
        "shared",
        &["provider-a", "provider-b", "provider-c"],
    )]);
    let providers_cfg = providers_config(&[
        ("provider-a", "{\"owned\":true}\n"),
        ("provider-b", "{\"owned\":true}\n"),
        ("provider-c", "{\"found\":false}\n"),
    ]);

    let output = ProductionResumeService::new()
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
            input: SESSION_A,
            model_override: None,
        })
        .unwrap();

    assert!(matches!(
        output,
        ResumeServiceOutput::ResumeRejected {
            error: ResumeServiceRejection::StorageOwnershipAmbiguous { owners, .. }
        } if owners.len() == 2
            && owners.iter().any(|owner| owner.matching_provider == "provider-a")
            && owners.iter().any(|owner| owner.matching_provider == "provider-b")
    ));
}

#[test]
fn resume_service_rejects_one_owner_associated_with_multiple_chains() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "provider-a", SESSION_A, "shared");
    fixture.seed_active_chain(CHAIN_B, "provider-a", SESSION_A, "shared");
    fixture.seed_active_chain(CHAIN_C, "provider-b", SESSION_A, "shared");
    let db = fixture.open_db();
    let models = model_store(vec![model("shared", &["provider-a", "provider-b"])]);
    let providers_cfg = providers_config(&[
        ("provider-a", "{\"owned\":true}\n"),
        ("provider-b", "{\"owned\":false}\n"),
    ]);

    let output = ProductionResumeService::new()
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
            input: SESSION_A,
            model_override: None,
        })
        .unwrap();

    assert!(matches!(
        output,
        ResumeServiceOutput::ResumeRejected {
            error: ResumeServiceRejection::StorageOwnerChainAmbiguous {
                provider_name,
                chain_ids,
                ..
            }
        } if provider_name == "provider-a" && chain_ids == vec![CHAIN_A.to_string(), CHAIN_B.to_string()]
    ));
}

#[test]
fn resume_service_validates_model_only_after_owner_selection() {
    let fixture = Fixture::new();
    fixture.seed_active_chain(CHAIN_A, "provider-a", SESSION_A, "shared");
    fixture.seed_active_chain(CHAIN_B, "provider-b", SESSION_A, "shared");
    let db = fixture.open_db();
    let models = model_store(vec![
        model("shared", &["provider-a", "provider-b"]),
        model("provider-b-only", &["provider-b"]),
    ]);
    let providers_cfg = providers_config(&[
        ("provider-a", "{\"owned\":true}\n"),
        ("provider-b", "{\"owned\":false}\n"),
    ]);

    let output = ProductionResumeService::new()
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
            input: SESSION_A,
            model_override: Some("provider-b-only"),
        })
        .unwrap();

    assert!(matches!(
        output,
        ResumeServiceOutput::ResumeRejected {
            error: ResumeServiceRejection::State(ResumeError::ProviderModelMismatch {
                active_provider,
                ..
            })
        } if active_provider == "provider-a"
    ));
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
    let providers_cfg = ProvidersConfig::default();
    let output = service
        .resolve_resume(ResumeServiceRequest {
            state: &db,
            models: &models,
            providers_cfg: &providers_cfg,
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
