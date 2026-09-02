#![cfg(unix)]
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProviderEndpointConfig, ProviderEntry,
    ProvidersConfig, SessionSourceEntry, SessionsConfig,
    provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_provider::client::{CancellationToken, ProviderClientOptions};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::session_metadata::{
    LocatedTranscript, SessionStorageType, TranscriptLookupMode,
};
use oulipoly_runtime::session_provider::{
    self, SessionProviderCaptureRequest, SessionProviderEnumerateRequest, SessionProviderIdentity,
    SessionProviderLocateRequest, SessionProviderPageCursor, SessionProviderReadPageRequest,
    SessionProviderTurnProjection, SessionTurnIngestQuantumRequest,
};
use oulipoly_state::{InvocationStart, SessionTurnStreamProjection, StateDb};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MODEL: &str = "provider-a-model";
const UNRELATED_MODEL: &str = "provider-a-unrelated-model";
const PROVIDER_NAME: &str = "provider-a-account";
const PROVIDER_INSTANCE_ID: &str = "provider-a-instance";
const SETTINGS_ID: &str = "provider-a-test-settings";
const SESSION_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const NATIVE_SESSION_ID: &str = "native-session-opaque-id";
const LEASE_OWNER: &str = "age243-worker";

struct Fixture {
    dir: tempfile::TempDir,
    provider_path: PathBuf,
    mode_path: PathBuf,
    record_path: PathBuf,
    state: StateDb,
    transcript_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SqliteSnapshot {
    invocations: Vec<InvocationSnapshotRow>,
    session_turns: Vec<SessionTurnSnapshotRow>,
    session_chains: Vec<(String, String)>,
    session_chain_segments: Vec<(String, String, String)>,
}

type InvocationSnapshotRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

type SessionTurnSnapshotRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    i64,
);

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = StateDb::open(&dir.path().join("state.db")).expect("state db");
        let transcript_path = dir.path().join("provider-a-session.jsonl");
        fs::write(&transcript_path, "{}\n").expect("transcript");
        let mode_path = dir.path().join("mode.txt");
        let record_path = dir.path().join("records.jsonl");
        fs::write(&mode_path, "describe_only").expect("mode");
        fs::write(&record_path, "").expect("records");
        let provider_path =
            write_fake_provider(dir.path(), &mode_path, &record_path, &transcript_path);
        Self {
            dir,
            provider_path,
            mode_path,
            record_path,
            state,
            transcript_path,
        }
    }

    fn set_mode(&self, mode: &str) {
        fs::write(&self.mode_path, mode).expect("write mode");
    }

    fn registry(&self) -> ProviderRegistry {
        ProviderRegistry::from_configs(
            &[external_model(MODEL, &self.provider_path)],
            &self.providers(),
            ProviderRegistryOptions::default()
                .with_config_root(self.dir.path().join("config-root"))
                .with_data_root(self.dir.path().join("data-root")),
        )
        .expect("registry")
    }

    fn hostile_registry(&self) -> ProviderRegistry {
        ProviderRegistry::from_configs(
            &[external_model(MODEL, &self.provider_path)],
            &self.providers(),
            ProviderRegistryOptions::default()
                .with_config_root(self.dir.path().join("hostile-config-root"))
                .with_data_root(self.dir.path().join("hostile-data-root")),
        )
        .expect("registry")
    }

    fn timeout_registry(&self) -> ProviderRegistry {
        ProviderRegistry::from_configs(
            &[external_model(MODEL, &self.provider_path)],
            &self.providers(),
            ProviderRegistryOptions::default().with_client_options(
                ProviderClientOptions::default()
                    .with_timeout(Duration::from_millis(150))
                    .with_kill_after_grace(Duration::from_millis(10)),
            ),
        )
        .expect("registry")
    }

    fn unrelated_registry(&self) -> ProviderRegistry {
        ProviderRegistry::from_configs(
            &[external_model(UNRELATED_MODEL, &self.provider_path)],
            &self.providers(),
            ProviderRegistryOptions::default()
                .with_config_root(self.dir.path().join("config-root"))
                .with_data_root(self.dir.path().join("data-root")),
        )
        .expect("registry")
    }

    fn providers(&self) -> ProvidersConfig {
        ProvidersConfig {
            entries: HashMap::from([(
                PROVIDER_NAME.to_string(),
                ProviderEntry {
                    implementation: Some(ProviderEndpointConfig {
                        family: "provider-a-family".to_string(),
                        executable: self.provider_path.display().to_string(),
                    }),
                    settings_id: Some(SETTINGS_ID.to_string()),
                    ..ProviderEntry::default()
                },
            )]),
        }
    }

    fn records(&self) -> Vec<Value> {
        provider_record_values(&provider_record_text(&self.record_path))
    }

    fn request_records_for(&self, subcommand: &str) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|record| record["subcommand"] == subcommand)
            .collect()
    }

    fn snapshot(&self) -> SqliteSnapshot {
        sqlite_snapshot(&self.conn())
    }

    fn state_path(&self) -> PathBuf {
        self.dir.path().join("state.db")
    }

    fn conn(&self) -> Connection {
        Connection::open(self.state_path()).expect("sqlite")
    }

    fn seed_finalized_invocation(&self, uuid: &str) -> i64 {
        let row_id = self
            .state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid.to_string(),
                model_name: MODEL.to_string(),
                provider_name: PROVIDER_NAME.to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .expect("start");
        self.state
            .finalize_invocation(row_id, true, 0, None, Some("completed"))
            .expect("finalize");
        row_id
    }

    fn seed_chain(&self, chain_id: &str, provider_name: &str, session_id: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z', ?2)",
            params![chain_id, MODEL],
        )
        .expect("chain");
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-05-01T00:00:00Z', 'initial')",
            params![chain_id, provider_name, session_id],
        )
        .expect("segment");
    }
}

fn provider_record_text(record_path: &Path) -> String {
    fs::read_to_string(record_path).expect("records")
}

fn provider_record_values(records: &str) -> Vec<Value> {
    records.lines().map(provider_record_value).collect()
}

fn provider_record_value(line: &str) -> Value {
    serde_json::from_str(line).expect("record json")
}

fn external_model(name: &str, provider_path: &Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(PROVIDER_NAME, Vec::new())],
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

fn provider_identity() -> SessionProviderIdentity {
    SessionProviderIdentity {
        model_name: MODEL.to_string(),
        provider_name: PROVIDER_NAME.to_string(),
        provider_instance_id: Some(PROVIDER_INSTANCE_ID.to_string()),
        settings_id: SETTINGS_ID.to_string(),
    }
}

fn locate_request<'a>(
    registry: &'a ProviderRegistry,
    session_id: &'a str,
    mode: TranscriptLookupMode,
) -> SessionProviderLocateRequest<'a> {
    SessionProviderLocateRequest {
        registry,
        identity: provider_identity(),
        session_id,
        lookup_mode: mode,
        effective_cwd: None,
        purpose: None::<&'a str>,
        tail_bytes_hint: None::<usize>,
    }
}

fn capture_request<'a>(
    registry: &'a ProviderRegistry,
    invocation_uuid: &'a str,
) -> SessionProviderCaptureRequest<'a> {
    SessionProviderCaptureRequest {
        registry,
        identity: provider_identity(),
        invocation_uuid,
        effective_cwd: None,
    }
}

fn page_request<'a>(
    registry: &'a ProviderRegistry,
    cancellation: &'a CancellationToken,
) -> SessionProviderReadPageRequest<'a> {
    SessionProviderReadPageRequest {
        registry,
        identity: provider_identity(),
        session_id: SESSION_ID,
        effective_cwd: None,
        projection: SessionProviderTurnProjection::CanonicalIngest,
        expected_delivery_nonce: None,
        cursor: SessionProviderPageCursor::Beginning { after_token: None },
        expected_page_index: 0,
        expected_turn_sequence: 0,
        max_turns: 16,
        max_response_bytes: 16 * 1024,
        max_source_bytes: 64 * 1024,
        max_inline_body_bytes: 1024,
        cancellation,
        timeout: Duration::from_secs(2),
    }
}

fn enumerate_request<'a>(registry: &'a ProviderRegistry) -> SessionProviderEnumerateRequest<'a> {
    SessionProviderEnumerateRequest {
        registry,
        identity: provider_identity(),
        limit: Some(100),
        cursor: None,
        include_cwd: true,
        include_turn_count: false,
        since_unix_ms: Some(1_782_000_000_000),
        effective_cwd: None,
    }
}

#[test]
fn no_ref_dispatch_aware_lifecycle_path_preserves_session_capture_marker_and_sqlite_bytes() {
    let baseline = NoRefDispatchProofFixture::new();
    let dispatch = NoRefDispatchProofFixture::new();
    let unrelated = Fixture::new();
    let unrelated_registry = ProviderRegistryHandle::new(Arc::new(unrelated.unrelated_registry()));

    assert_no_ref_dispatch_fixture_row_id(&baseline);
    assert_no_ref_dispatch_fixture_row_id(&dispatch);

    let baseline_output = session_provider::dispatch_aware_no_ref_lifecycle_proof(
        baseline.request_without_registry(),
    )
    .expect("baseline no-ref proof");
    let dispatch_output = session_provider::dispatch_aware_no_ref_lifecycle_proof(
        dispatch.request_with_registry(unrelated_registry),
    )
    .expect("dispatch-aware no-ref proof");

    assert_eq!(
        dispatch_output.lifecycle_stderr, baseline_output.lifecycle_stderr,
        "warnings and OULIPOLY_SESSION marker bytes must remain byte-identical"
    );
    assert_eq!(dispatch_output.lifecycle, baseline_output.lifecycle);
    assert!(
        dispatch_output.lifecycle.emitted,
        "runtime no-ref proof must exercise marker/session capture emission"
    );
    assert_eq!(
        dispatch.snapshot(),
        baseline.snapshot(),
        "dispatch-aware no-ref path must leave relevant SQLite rows byte-identical"
    );
    assert!(
        unrelated.records().is_empty(),
        "populated unrelated registry must not be described or invoked by no-ref dispatch"
    );
}

#[test]
fn external_provider_locate_dispatch_maps_success_and_request_identity() {
    let fixture = Fixture::new();
    fixture.set_mode("locate_success");
    let registry = fixture.registry();

    let located = session_provider::locate_transcript(locate_request(
        &registry,
        SESSION_ID,
        TranscriptLookupMode::RequireExisting,
    ))
    .expect("locate dispatch");

    assert_eq!(
        located,
        LocatedTranscript {
            path: fixture.transcript_path.clone(),
            storage_classification: SessionStorageType::Other,
            require_existing_observed: true,
        }
    );
    let records = fixture.request_records_for("session.locate_transcript");
    assert_request_shape(&records, "session.locate_transcript", Some(SESSION_ID));
    let request = &records[0]["request"];
    assert_eq!(
        request["host"]["config_root"],
        fixture.dir.path().join("config-root").display().to_string()
    );
    assert_eq!(
        request["host"]["data_root"],
        fixture.dir.path().join("data-root").display().to_string()
    );
    assert_eq!(fixture.request_records_for("describe").len(), 1);
}

#[test]
fn external_provider_locate_failure_matrix_does_not_fall_back_to_private_layouts() {
    for (mode, expected_token) in [
        ("locate_missing", "session_locate_missing"),
        ("locate_true_without_path", "session_locate_missing_path"),
        ("locate_empty_path", "session_locate_empty_path"),
        (
            "locate_missing_require_existing_observed",
            "session_locate_require_existing_unobserved",
        ),
        ("locate_relative_path", "session_locate_invalid_path"),
        ("session_capability_disabled", "session_capability_missing"),
        ("provider_error", "provider_error_mode"),
        ("malformed_json", "invalid_json"),
        ("empty_stdout", "empty_stdout"),
        ("leading_stdout_text", "leading_stdout_text"),
        ("multiple_json_objects", "multiple_json_objects"),
        ("trailing_junk", "trailing_non_whitespace"),
        ("schema_invalid", "schema_invalid_response"),
        ("schema_invalid_error", "schema_invalid_error_response"),
        ("nonzero", "nonzero_mode"),
        ("nonzero_no_envelope", "provider_process_nonzero"),
    ] {
        let fixture = Fixture::new();
        fixture.set_mode(mode);
        let registry = fixture.registry();
        let before = fixture.snapshot();

        let err = session_provider::locate_transcript(locate_request(
            &registry,
            SESSION_ID,
            TranscriptLookupMode::RequireExisting,
        ))
        .expect_err("mode should fail");

        assert_eq!(
            fixture.snapshot(),
            before,
            "locate failure {mode} must not mutate host SQLite"
        );
        assert_error_token(&err, expected_token);
        assert!(
            fixture
                .request_records_for("session.locate_transcript")
                .len()
                <= 1,
            "locate dispatch should make at most one provider locate attempt for {mode}"
        );
    }
}

#[test]
fn external_provider_locate_unknown_format_maps_to_other_storage_class() {
    let fixture = Fixture::new();
    fixture.set_mode("locate_unknown_format");
    let registry = fixture.registry();

    let located = session_provider::locate_transcript(locate_request(
        &registry,
        SESSION_ID,
        TranscriptLookupMode::RequireExisting,
    ))
    .expect("unknown provider format should be preserved as Other");

    assert_eq!(
        located,
        LocatedTranscript {
            path: fixture.transcript_path.clone(),
            storage_classification: SessionStorageType::Other,
            require_existing_observed: true,
        }
    );
    assert_request_shape(
        &fixture.request_records_for("session.locate_transcript"),
        "session.locate_transcript",
        Some(SESSION_ID),
    );
}

#[test]
fn external_provider_enumerate_maps_provider_native_sessions_without_mutating_sqlite() {
    let fixture = Fixture::new();
    fixture.set_mode("enumerate_success");
    let registry = fixture.registry();
    let before = fixture.snapshot();

    let result = session_provider::enumerate_sessions(enumerate_request(&registry))
        .expect("enumerate sessions");

    assert_eq!(result.sessions.len(), 1);
    let entry = &result.sessions[0];
    assert_eq!(entry.provider_session_id, NATIVE_SESSION_ID);
    assert_eq!(entry.title.as_deref(), Some("Native session"));
    assert_eq!(entry.cwd.as_deref(), Some(fixture.dir.path()));
    assert_eq!(entry.created_unix_ms, Some(1_782_000_000_000));
    assert_eq!(entry.updated_unix_ms, Some(1_782_000_010_000));
    assert_eq!(entry.turn_count, None);
    assert_eq!(entry.source.kind, "provider_native_list");
    assert_eq!(entry.source.detail.as_deref(), Some("fixture session list"));
    assert!(result.complete);
    assert_eq!(result.next_cursor, None);
    assert_eq!(result.warnings, vec!["fixture warning".to_string()]);
    assert_eq!(fixture.snapshot(), before);
    assert_enumerate_request_shape(&fixture.request_records_for("session.enumerate"));
    assert_eq!(fixture.request_records_for("describe").len(), 1);
}

#[test]
fn external_provider_enumerate_without_capability_is_clear_unsupported_noop() {
    let fixture = Fixture::new();
    fixture.set_mode("locate_success");
    let registry = fixture.registry();
    let before = fixture.snapshot();

    let err = session_provider::enumerate_sessions(enumerate_request(&registry))
        .expect_err("enumerate should require fine-grained capability");

    assert_error_token(&err, "session_enumerate_capability_missing");
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.request_records_for("describe").len(), 1);
    assert!(
        fixture.request_records_for("session.enumerate").is_empty(),
        "missing capability must not invoke the provider enumerate subcommand"
    );
}

#[test]
fn external_provider_page_dispatch_enforces_bounded_request_and_maps_one_page() {
    let fixture = Fixture::new();
    fixture.set_mode("page_success");
    let registry = fixture.registry();
    let cancellation = CancellationToken::new();
    let before = fixture.snapshot();

    let result = session_provider::read_turn_page(page_request(&registry, &cancellation))
        .expect("bounded page");

    assert_eq!(result.session_id, SESSION_ID);
    assert_eq!(result.page_index, 0);
    assert_eq!(result.page_start_sequence, 0);
    assert_eq!(result.page_turn_count, 1);
    assert_eq!(result.turns.len(), 1);
    assert_eq!(result.turns[0].turn_id, "turn-user-1");
    assert!(result.turns[0].canonical_text_digest_verified);
    assert!(result.snapshot_complete);
    assert_eq!(result.resume_token.as_deref(), Some("resume-1"));
    assert_eq!(fixture.snapshot(), before);

    let records = fixture.request_records_for("session.read_turns");
    assert_eq!(records.len(), 1);
    let params = &records[0]["request"]["params"];
    assert_eq!(params["read_protocol"], "oulipoly.session_turn_pages/v1");
    assert_eq!(params["turn_projection"], "canonical_ingest");
    assert!(params.get("expected_delivery_nonce").is_none());
    assert_eq!(params["start_mode"], "beginning");
    assert_eq!(params["max_turns"], 16);
    assert_eq!(params["max_response_bytes"], 16 * 1024);
    assert_eq!(params["max_source_bytes"], 64 * 1024);
    assert_eq!(params["max_inline_body_bytes"], 1024);
    assert!(records[0]["request"]["host"]["deadline_unix_ms"].is_u64());
}

#[test]
fn user_observation_uses_an_empty_tail_anchor_then_reads_only_after_that_token() {
    let fixture = Fixture::new();
    fixture.set_mode("page_success");
    let registry = fixture.registry();
    let cancellation = CancellationToken::new();
    let delivery_nonce = "a".repeat(64);
    let mut request = page_request(&registry, &cancellation);
    request.projection = SessionProviderTurnProjection::UserObservation;
    request.expected_delivery_nonce = Some(&delivery_nonce);
    request.cursor = SessionProviderPageCursor::Tail;
    request.max_inline_body_bytes = 0;

    let anchor = session_provider::read_turn_page(request).expect("tail anchor");
    assert!(anchor.turns.is_empty());
    assert!(anchor.snapshot_complete);
    assert_eq!(anchor.resume_token.as_deref(), Some("observation-anchor"));

    let mut request = page_request(&registry, &cancellation);
    request.projection = SessionProviderTurnProjection::UserObservation;
    request.expected_delivery_nonce = Some(&delivery_nonce);
    request.cursor = SessionProviderPageCursor::Beginning {
        after_token: anchor.resume_token,
    };
    request.max_inline_body_bytes = 0;
    let observed = session_provider::read_turn_page(request).expect("post-anchor page");
    assert_eq!(observed.turns.len(), 1);
    assert_eq!(observed.turns[0].turn_id, "turn-user-1");
    assert_eq!(
        observed.turns[0].canonical_text_sha256.as_deref(),
        Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
    );
    assert!(observed.turns[0].body.is_none());

    let records = fixture.request_records_for("session.read_turns");
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0]["request"]["params"]["turn_projection"],
        "user_observation"
    );
    assert_eq!(records[0]["request"]["params"]["start_mode"], "tail");
    assert_eq!(
        records[0]["request"]["params"]["expected_delivery_nonce"],
        delivery_nonce
    );
    assert_eq!(
        records[1]["request"]["params"]["expected_delivery_nonce"],
        delivery_nonce
    );
    assert_eq!(records[1]["request"]["params"]["start_mode"], "beginning");
    assert_eq!(
        records[1]["request"]["params"]["after_token"],
        "observation-anchor"
    );
}

#[test]
fn one_page_quantum_commits_turn_and_checkpoint_once() {
    let fixture = Fixture::new();
    fixture.set_mode("page_success");
    let registry = fixture.registry();
    let identity = provider_identity();
    let key = session_provider::canonical_stream_key(&identity, SESSION_ID)
        .expect("provider identity should produce an ingest key");
    fixture
        .state
        .enqueue_session_turn_ingest_stream(&key)
        .expect("queue stream");
    lease_stream(&fixture.state);
    let cancellation = CancellationToken::new();

    let outcome =
        session_provider::ingest_one_canonical_turn_page(SessionTurnIngestQuantumRequest {
            state: &fixture.state,
            registry: &registry,
            lease_owner: LEASE_OWNER,
            identity,
            session_id: SESSION_ID,
            effective_cwd: None,
            cancellation: &cancellation,
            timeout: Duration::from_secs(2),
            max_turns: 16,
            max_response_bytes: 16 * 1024,
            max_source_bytes: 64 * 1024,
            max_inline_body_bytes: 1024,
        })
        .expect("one page quantum");

    assert_eq!(outcome.inserted_turns, 1);
    assert_eq!(outcome.checkpoint_generation, 1);
    let stream = fixture
        .state
        .session_turn_ingest_stream(&key)
        .unwrap()
        .unwrap();
    assert_eq!(stream.status, "caught_up");
    assert_eq!(stream.after_token.as_deref(), Some("resume-1"));
    assert_eq!(stream.committed_page_count, 1);
    assert_eq!(stream.committed_turn_count, 1);
    assert_eq!(
        fixture
            .state
            .count_session_turns(PROVIDER_NAME, SESSION_ID)
            .unwrap()
            .total,
        1
    );
}

#[test]
fn provider_page_failure_leaves_checkpoint_unchanged() {
    let fixture = Fixture::new();
    fixture.set_mode("page_provider_error");
    let registry = fixture.registry();
    let identity = provider_identity();
    let key = session_provider::canonical_stream_key(&identity, SESSION_ID)
        .expect("provider identity should produce an ingest key");
    fixture
        .state
        .enqueue_session_turn_ingest_stream(&key)
        .expect("queue stream");
    lease_stream(&fixture.state);
    let cancellation = CancellationToken::new();

    let error = session_provider::ingest_one_canonical_turn_page(SessionTurnIngestQuantumRequest {
        state: &fixture.state,
        registry: &registry,
        lease_owner: LEASE_OWNER,
        identity,
        session_id: SESSION_ID,
        effective_cwd: None,
        cancellation: &cancellation,
        timeout: Duration::from_secs(2),
        max_turns: 16,
        max_response_bytes: 16 * 1024,
        max_source_bytes: 64 * 1024,
        max_inline_body_bytes: 1024,
    })
    .expect_err("provider page failure");

    assert_error_token(&error, "provider_page_failed");
    let stream = fixture
        .state
        .session_turn_ingest_stream(&key)
        .unwrap()
        .unwrap();
    assert_eq!(stream.checkpoint_generation, 0);
    assert_eq!(stream.committed_page_count, 0);
    assert_eq!(stream.committed_turn_count, 0);
    assert_eq!(
        fixture
            .state
            .count_session_turns(PROVIDER_NAME, SESSION_ID)
            .unwrap()
            .total,
        0
    );
}

#[test]
fn bounded_worker_leases_and_applies_exactly_one_ready_page() {
    let fixture = Fixture::new();
    fixture.set_mode("page_success");
    let registry = fixture.registry();
    let key = session_provider::canonical_stream_key(&provider_identity(), SESSION_ID)
        .expect("provider identity should produce an ingest key");
    fixture
        .state
        .enqueue_session_turn_ingest_stream(&key)
        .expect("queue stream");
    let cancellation = CancellationToken::new();

    let outcome = session_provider::run_one_session_turn_ingest_quantum(
        session_provider::SessionTurnIngestDriverRequest {
            state: &fixture.state,
            registry: &registry,
            lease_owner: LEASE_OWNER,
            effective_cwd: None,
            cancellation: &cancellation,
            now: Utc::now(),
        },
    )
    .expect("worker quantum");

    assert!(matches!(
        outcome,
        session_provider::SessionTurnIngestQuantumOutcome::Applied {
            inserted_turns: 1,
            duplicate_turns: 0,
            checkpoint_generation: 1,
            ..
        }
    ));
    let idle = session_provider::run_one_session_turn_ingest_quantum(
        session_provider::SessionTurnIngestDriverRequest {
            state: &fixture.state,
            registry: &registry,
            lease_owner: LEASE_OWNER,
            effective_cwd: None,
            cancellation: &cancellation,
            now: Utc::now(),
        },
    )
    .expect("idle worker quantum");
    assert_eq!(
        idle,
        session_provider::SessionTurnIngestQuantumOutcome::Idle
    );
}

#[test]
fn bounded_worker_schedules_per_stream_retry_without_advancing_checkpoint() {
    let fixture = Fixture::new();
    fixture.set_mode("page_provider_error");
    let registry = fixture.registry();
    let key = session_provider::canonical_stream_key(&provider_identity(), SESSION_ID)
        .expect("provider identity should produce an ingest key");
    fixture
        .state
        .enqueue_session_turn_ingest_stream(&key)
        .expect("queue stream");
    let cancellation = CancellationToken::new();
    let now = Utc::now();

    let outcome = session_provider::run_one_session_turn_ingest_quantum(
        session_provider::SessionTurnIngestDriverRequest {
            state: &fixture.state,
            registry: &registry,
            lease_owner: LEASE_OWNER,
            effective_cwd: None,
            cancellation: &cancellation,
            now,
        },
    )
    .expect("worker retry quantum");

    assert!(matches!(
        outcome,
        session_provider::SessionTurnIngestQuantumOutcome::RetryScheduled { ref error, .. }
            if error == "provider_page_failed"
    ));
    let stream = fixture
        .state
        .session_turn_ingest_stream(&key)
        .unwrap()
        .unwrap();
    assert_eq!(stream.status, "retry_wait");
    assert_eq!(stream.checkpoint_generation, 0);
    assert_eq!(stream.retry_count, 1);
    assert_eq!(stream.lease_owner, None);
    assert_eq!(
        session_provider::run_one_session_turn_ingest_quantum(
            session_provider::SessionTurnIngestDriverRequest {
                state: &fixture.state,
                registry: &registry,
                lease_owner: LEASE_OWNER,
                effective_cwd: None,
                cancellation: &cancellation,
                now,
            },
        )
        .unwrap(),
        session_provider::SessionTurnIngestQuantumOutcome::Idle
    );
}

#[test]
fn bounded_worker_marks_unpaged_provider_unsupported_without_fallback() {
    let fixture = Fixture::new();
    fixture.set_mode("capture_success");
    let registry = fixture.registry();
    let key = session_provider::canonical_stream_key(&provider_identity(), SESSION_ID)
        .expect("provider identity should produce an ingest key");
    fixture
        .state
        .enqueue_session_turn_ingest_stream(&key)
        .expect("queue stream");
    let cancellation = CancellationToken::new();

    let outcome = session_provider::run_one_session_turn_ingest_quantum(
        session_provider::SessionTurnIngestDriverRequest {
            state: &fixture.state,
            registry: &registry,
            lease_owner: LEASE_OWNER,
            effective_cwd: None,
            cancellation: &cancellation,
            now: Utc::now(),
        },
    )
    .expect("unsupported worker quantum");

    assert!(matches!(
        outcome,
        session_provider::SessionTurnIngestQuantumOutcome::Unsupported { ref error, .. }
            if error == "session_turn_pages_capability_missing"
    ));
    let stream = fixture
        .state
        .session_turn_ingest_stream(&key)
        .unwrap()
        .unwrap();
    assert_eq!(stream.status, "unsupported");
    assert_eq!(stream.checkpoint_generation, 0);
    assert_eq!(fixture.request_records_for("session.read_turns").len(), 0);
}

fn lease_stream(state: &StateDb) {
    let now = Utc::now();
    let leased = state
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            LEASE_OWNER,
            now,
            now + chrono::Duration::seconds(30),
        )
        .expect("lease stream")
        .expect("ready stream");
    assert_eq!(leased.key.session_id, SESSION_ID);
}

#[test]
fn external_provider_capture_maps_facts_without_mutating_capture_rows() {
    for (mode, expected_session, artifact_count) in [
        ("capture_success", Some(SESSION_ID), 1),
        ("capture_empty", None, 0),
    ] {
        let fixture = Fixture::new();
        fixture.set_mode(mode);
        let invocation_uuid = "11111111-1111-4111-8111-111111111111";
        fixture.seed_finalized_invocation(invocation_uuid);
        let registry = fixture.registry();
        let before = fixture.snapshot();

        let result =
            session_provider::capture(capture_request(&registry, invocation_uuid)).expect(mode);

        assert_eq!(result.provider_session_id.as_deref(), expected_session);
        assert_eq!(result.artifacts.len(), artifact_count);
        assert_eq!(
            fixture.snapshot(),
            before,
            "provider capture returns evidence only; lifecycle/marker owns SQLite mutation for {mode}"
        );
        assert_request_shape(
            &fixture.request_records_for("session.capture"),
            "session.capture",
            None,
        );
    }
}

#[test]
fn external_provider_capture_provider_transport_and_schema_failures_do_not_mutate_sqlite() {
    for (mode, expected_token) in [
        ("capture_provider_error", "provider_capture_failed"),
        ("capture_malformed_json", "invalid_json"),
        ("capture_empty_stdout", "empty_stdout"),
        ("leading_stdout_text", "leading_stdout_text"),
        ("multiple_json_objects", "multiple_json_objects"),
        ("trailing_junk", "trailing_non_whitespace"),
        ("capture_schema_invalid", "schema_invalid_response"),
        ("schema_invalid_error", "schema_invalid_error_response"),
        ("capture_nonzero", "capture_nonzero_mode"),
        ("nonzero_no_envelope", "provider_process_nonzero"),
    ] {
        let fixture = Fixture::new();
        let invocation_uuid = "12121212-1212-4212-8212-121212121212";
        fixture.seed_finalized_invocation(invocation_uuid);
        let registry = fixture.registry();
        registry
            .preflight_account(PROVIDER_NAME)
            .expect("capture failure fixture endpoint should preflight");
        fixture.set_mode(mode);
        let before = fixture.snapshot();

        let err =
            session_provider::capture(capture_request(&registry, invocation_uuid)).expect_err(mode);

        assert_eq!(
            fixture.snapshot(),
            before,
            "capture failure {mode} must not mutate host SQLite"
        );
        assert_error_token(&err, expected_token);
        assert_eq!(
            fixture.request_records_for("session.capture").len(),
            1,
            "capture failure {mode} must exercise provider capture dispatch exactly once"
        );
    }
}

#[test]
fn external_provider_timeout_is_stable_transport_token_without_mutation() {
    let fixture = Fixture::new();
    fixture.set_mode("provider_timeout");
    let registry = fixture.timeout_registry();
    let before = fixture.snapshot();

    let err = session_provider::locate_transcript(locate_request(
        &registry,
        SESSION_ID,
        TranscriptLookupMode::RequireExisting,
    ))
    .expect_err("provider timeout should fail");

    assert_eq!(
        fixture.snapshot(),
        before,
        "timeout failure must not mutate host SQLite"
    );
    assert_error_token(&err, "host_timeout");
}

#[test]
fn external_provider_error_tokens_are_stable_by_failure_class() {
    for (mode, expected_token) in [
        ("session_capability_disabled", "session_capability_missing"),
        ("provider_error", "provider_error_mode"),
        ("nonzero_no_envelope", "provider_process_nonzero"),
        ("leading_stdout_text", "leading_stdout_text"),
        ("schema_invalid", "schema_invalid_response"),
        ("locate_relative_path", "session_locate_invalid_path"),
    ] {
        let fixture = Fixture::new();
        fixture.set_mode(mode);
        let registry = fixture.registry();

        let err = session_provider::locate_transcript(locate_request(
            &registry,
            SESSION_ID,
            TranscriptLookupMode::RequireExisting,
        ))
        .expect_err("representative failure should fail");

        assert_error_token(&err, expected_token);
    }
}

fn assert_error_token(error: &impl Display, expected_token: &str) {
    let message = error.to_string();
    assert!(
        message.contains(expected_token),
        "expected stable error token {expected_token:?} in {message:?}"
    );
}

struct NoRefDispatchProofFixture {
    fixture: Fixture,
    invocation_row_id: i64,
}

impl NoRefDispatchProofFixture {
    fn new() -> Self {
        let fixture = Fixture::new();
        let invocation_uuid = "33333333-3333-4333-8333-333333333333";
        let invocation_row_id = fixture.seed_finalized_invocation(invocation_uuid);
        fixture.seed_chain(
            "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
            PROVIDER_NAME,
            SESSION_ID,
        );
        Self {
            fixture,
            invocation_row_id,
        }
    }

    fn request_without_registry<'a>(&'a self) -> session_provider::NoRefProofRequest<'a> {
        session_provider::NoRefProofRequest {
            state: &self.fixture.state,
            registry: None,
            model_name: MODEL,
            provider_name: PROVIDER_NAME,
            session_id: SESSION_ID,
            invocation_row_id: self.invocation_row_id,
            invocation_uuid: "33333333-3333-4333-8333-333333333333",
        }
    }

    fn request_with_registry<'a>(
        &'a self,
        registry: ProviderRegistryHandle,
    ) -> session_provider::NoRefProofRequest<'a> {
        session_provider::NoRefProofRequest {
            registry: Some(registry),
            ..self.request_without_registry()
        }
    }

    fn snapshot(&self) -> SqliteSnapshot {
        self.fixture.snapshot()
    }
}

fn assert_no_ref_dispatch_fixture_row_id(fixture: &NoRefDispatchProofFixture) {
    assert_eq!(
        fixture.invocation_row_id, 1,
        "fresh proof fixture should use row id 1"
    );
}

fn hostile_marker(fixture: &Fixture, route: &str) -> PathBuf {
    fixture.dir.path().join(format!("hostile-mutated-{route}"))
}

struct DataDirOverride {
    previous: Option<OsString>,
}

impl DataDirOverride {
    fn install(path: &Path) -> Self {
        let previous = std::env::var_os("OULIPOLY_DATA_DIR");
        // SAFETY: this test holds its data-dir override for the synchronous
        // provider dispatch and restores the process environment on drop.
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", path);
        }
        Self { previous }
    }
}

impl Drop for DataDirOverride {
    fn drop(&mut self) {
        // SAFETY: restore the process env to its pre-test value.
        unsafe {
            match &self.previous {
                Some(previous) => std::env::set_var("OULIPOLY_DATA_DIR", previous),
                None => std::env::remove_var("OULIPOLY_DATA_DIR"),
            }
        }
    }
}

fn hostile_markers_json(dir: &Path) -> String {
    let marker = |route: &str| dir.join(format!("hostile-mutated-{route}"));
    serde_json::to_string(&serde_json::json!({
        "request-json": marker("request-json").display().to_string(),
        "cwd": marker("cwd").display().to_string(),
        "env": marker("env").display().to_string(),
        "data-root": marker("data-root").display().to_string(),
        "config-root": marker("config-root").display().to_string(),
    }))
    .expect("hostile marker json")
}

fn assert_request_shape(records: &[Value], subcommand: &str, session_id: Option<&str>) {
    assert_eq!(
        records.len(),
        1,
        "expected exactly one {subcommand} record, got {records:?}"
    );
    let request = &records[0]["request"];
    assert_eq!(records[0]["subcommand"], subcommand);
    assert_eq!(request["provider_instance_id"], PROVIDER_INSTANCE_ID);
    assert_eq!(request["params"]["settings_id"], SETTINGS_ID);
    assert_eq!(request["params"]["model_name"], MODEL);
    assert_eq!(request["params"]["provider_name"], PROVIDER_NAME);
    if let Some(session_id) = session_id {
        assert_eq!(request["params"]["session_id"], session_id);
    }
    assert!(
        request["params"].get("purpose").is_none(),
        "non-inspect session provider requests must omit purpose: {request}"
    );
    assert!(
        request["params"].get("tail_bytes_hint").is_none(),
        "non-inspect session provider requests must omit tail_bytes_hint: {request}"
    );
}

fn assert_enumerate_request_shape(records: &[Value]) {
    assert_eq!(
        records.len(),
        1,
        "expected exactly one session.enumerate record, got {records:?}"
    );
    let request = &records[0]["request"];
    assert_eq!(records[0]["subcommand"], "session.enumerate");
    assert_eq!(request["provider_instance_id"], PROVIDER_INSTANCE_ID);
    assert_eq!(request["params"]["settings_id"], SETTINGS_ID);
    assert_eq!(request["params"]["limit"], 100);
    assert_eq!(request["params"]["include_cwd"], true);
    assert_eq!(request["params"]["include_turn_count"], false);
    assert_eq!(request["params"]["since_unix_ms"], 1_782_000_000_000u64);
    assert!(request["params"].get("session_id").is_none());
    assert!(request["params"].get("model_name").is_none());
    assert!(request["params"].get("provider_name").is_none());
}

fn sqlite_snapshot(conn: &Connection) -> SqliteSnapshot {
    SqliteSnapshot {
        invocations: invocation_snapshot_rows(conn),
        session_turns: session_turn_snapshot_rows(conn),
        session_chains: session_chain_snapshot_rows(conn),
        session_chain_segments: session_chain_segment_snapshot_rows(conn),
    }
}

fn invocation_snapshot_rows(conn: &Connection) -> Vec<InvocationSnapshotRow> {
    query_rows(
        conn,
        "SELECT invocation_uuid, provider_session_id, provider_session_capture_method,
                session_id, session_capture_method
         FROM invocations ORDER BY invocation_uuid",
        invocation_snapshot_row,
    )
}

fn invocation_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationSnapshotRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn session_turn_snapshot_rows(conn: &Connection) -> Vec<SessionTurnSnapshotRow> {
    query_rows(
        conn,
        "SELECT provider_name, session_id, turn_id, timestamp, role,
                parent_turn_id, is_sidechain, is_compaction_boundary
         FROM session_turns ORDER BY provider_name, session_id, turn_id",
        session_turn_snapshot_row,
    )
}

fn session_turn_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionTurnSnapshotRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn session_chain_snapshot_rows(conn: &Connection) -> Vec<(String, String)> {
    query_rows(
        conn,
        "SELECT chain_id, model_name FROM session_chains ORDER BY chain_id",
        session_chain_snapshot_row,
    )
}

fn session_chain_snapshot_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String)> {
    Ok((row.get(0)?, row.get(1)?))
}

fn session_chain_segment_snapshot_rows(conn: &Connection) -> Vec<(String, String, String)> {
    query_rows(
        conn,
        "SELECT chain_id, provider_name, session_id
         FROM session_chain_segments ORDER BY chain_id, provider_name, session_id",
        session_chain_segment_snapshot_row,
    )
}

fn session_chain_segment_snapshot_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

fn query_rows<T, F>(conn: &Connection, sql: &str, mut mapper: F) -> Vec<T>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut stmt = conn.prepare(sql).expect(sql);
    stmt.query_map([], |row| mapper(row))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}

fn parse_ts(input: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(input)
        .expect("timestamp")
        .with_timezone(&Utc)
}

fn write_fake_provider(
    dir: &Path,
    mode_path: &Path,
    record_path: &Path,
    transcript_path: &Path,
) -> PathBuf {
    let script = dir.join("provider-a-session.py");
    fs::write(
        &script,
        fake_provider_body(dir, mode_path, record_path, transcript_path),
    )
    .expect("write provider");
    let mut perms = fs::metadata(&script).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod");
    script
}

fn fake_provider_body(
    dir: &Path,
    mode_path: &Path,
    record_path: &Path,
    transcript_path: &Path,
) -> String {
    format!(
        r#"#!/usr/bin/env python3
import json
import os
import pathlib
import sqlite3
import sys

CONTRACT = "oulipoly.provider/v1"
HOSTILE_MARKERS = {hostile_markers}
mode = pathlib.Path({mode_path}).read_text().strip()
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
raw = sys.stdin.read() or "{{}}"
request = json.loads(raw)
record_path = pathlib.Path({record_path})
with record_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": True,
        "result": result,
    }}

def error(code):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": False,
        "error": {{
            "category": "failed",
            "code": code,
            "message": code,
            "retryable": False,
        }},
    }}

def describe():
    session_enabled = mode != "session_capability_disabled"
    session_enumerate_enabled = mode.startswith("enumerate_")
    session_pages_enabled = mode.startswith("page_")
    return envelope({{
        "provider_id": "provider-a",
        "display_name": "Provider A",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": session_enabled,
            "session_enumerate": session_enumerate_enabled,
            "session_turn_pages_v1": session_pages_enabled,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
    }})

def enumerate_sessions():
    if mode == "enumerate_success":
        return envelope({{
            "sessions": [{{
                "provider_session_id": "{native_session_id}",
                "title": "Native session",
                "cwd": {fixture_dir},
                "created_unix_ms": 1782000000000,
                "updated_unix_ms": 1782000010000,
                "turn_count": None,
                "source": {{
                    "kind": "provider_native_list",
                    "detail": "fixture session list",
                }},
            }}],
            "complete": True,
            "next_cursor": None,
            "warnings": ["fixture warning"],
        }})
    return error("unexpected_enumerate_mode")

def locate():
    if mode == "locate_success":
        return envelope({{
            "located": True,
            "path": {transcript_path},
            "format_id": "jsonl",
            "source_id": "provider-a",
            "require_existing_observed": True,
        }})
    if mode == "locate_unknown_format":
        return envelope({{
            "located": True,
            "path": {transcript_path},
            "format_id": "provider-a/custom-format-v1",
            "source_id": "provider-a",
            "require_existing_observed": True,
        }})
    if mode == "locate_missing":
        return envelope({{"located": False}})
    if mode == "locate_true_without_path":
        return envelope({{"located": True}})
    if mode == "locate_empty_path":
        return envelope({{"located": True, "path": ""}})
    if mode == "locate_missing_require_existing_observed":
        return envelope({{
            "located": True,
            "path": {transcript_path},
            "format_id": "jsonl",
            "source_id": "provider-a",
        }})
    if mode == "locate_relative_path":
        return envelope({{"located": True, "path": "relative.jsonl"}})
    if mode == "schema_invalid":
        return envelope({{"located": True, "path": {transcript_path}, "artifacts": []}})
    return error("unexpected_locate_mode")

def turn_page():
    if mode == "page_provider_error":
        return error("provider_page_failed")
    params = request["params"]
    if params["turn_projection"] == "user_observation" and params["start_mode"] == "tail":
        return envelope({{
            "read_protocol": "oulipoly.session_turn_pages/v1",
            "provider_instance_id": "{provider_instance_id}",
            "settings_id": "{settings_id}",
            "session_id": "{session_id}",
            "turn_projection": "user_observation",
            "snapshot_id": "observation-tail",
            "page_index": 0,
            "page_start_sequence": 0,
            "turns": [],
            "page_turn_count": 0,
            "source_bytes_examined": 0,
            "scan_progress": False,
            "snapshot_complete": True,
            "next_page_token": None,
            "resume_token": "observation-anchor",
            "source_final": False,
            "warnings": [],
        }})
    body = [{{"type": "text", "text": "hello"}}]
    turn = {{
        "session_id": "{session_id}",
        "turn_id": "turn-user-1",
        "snapshot_sequence": 0,
        "timestamp": "2026-05-01T00:00:01Z",
        "role": "user",
        "parent_turn_id": None,
        "is_sidechain": False,
        "is_compaction_boundary": False,
        "body_state": "inline",
        "body": body,
        "body_bytes": 32,
        "body_sha256": "0f0fe295ef4aae213788b9539dad9a4ffc34333e769f5e2aa33a66c30e7353ea",
        "canonical_text_sha256": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    }}
    if params["turn_projection"] == "user_observation":
        turn["body_state"] = "omitted_oversize"
        turn["body"] = None
        turn["body_bytes"] = 5
        turn["body_sha256"] = None
    return envelope({{
        "read_protocol": "oulipoly.session_turn_pages/v1",
        "provider_instance_id": "{provider_instance_id}",
        "settings_id": "{settings_id}",
        "session_id": "{session_id}",
        "turn_projection": params["turn_projection"],
        "snapshot_id": "snapshot-1",
        "page_index": 0,
        "page_start_sequence": 0,
        "turns": [turn],
        "page_turn_count": 1,
        "source_bytes_examined": 32,
        "scan_progress": False,
        "snapshot_complete": True,
        "next_page_token": None,
        "resume_token": "resume-1",
        "source_final": False,
        "warnings": [],
    }})

def capture():
    if mode == "capture_success":
        return envelope({{
            "provider_session_id": "{session_id}",
            "state": {{"cursor": "provider-owned"}},
            "artifacts": [{{"kind": "transcript", "path": {transcript_path}}}],
        }})
    if mode == "capture_empty":
        return envelope({{"provider_session_id": None, "state": None, "artifacts": []}})
    if mode == "capture_provider_error":
        return error("provider_capture_failed")
    if mode == "capture_malformed_json":
        print("{{")
        raise SystemExit(0)
    if mode == "capture_empty_stdout":
        raise SystemExit(0)
    if mode == "capture_schema_invalid":
        return envelope({{"provider_session_id": "{session_id}", "state": {{}}, "artifacts": "not-an-array"}})
    if mode == "capture_nonzero":
        print(json.dumps(error("capture_nonzero_mode")))
        raise SystemExit(42)
    return error("unexpected_capture_mode")

if mode == "malformed_json":
    print("{{")
    raise SystemExit(0)
if mode == "empty_stdout":
    raise SystemExit(0)
if mode == "leading_stdout_text":
    print("provider log line before json")
    print(json.dumps(envelope({{"unexpected": True}})))
    raise SystemExit(0)
if mode == "multiple_json_objects":
    print(json.dumps(envelope({{"unexpected": True}})))
    print(json.dumps(envelope({{"unexpected": True}})))
    raise SystemExit(0)
if mode == "trailing_junk":
    print(json.dumps(envelope({{"unexpected": True}})) + " trailing-junk")
    raise SystemExit(0)
if mode == "schema_invalid_error":
    print(json.dumps({{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": False,
        "error": {{"category": "failed", "message": "missing code and retryable"}},
    }}))
    raise SystemExit(0)
if mode == "nonzero":
    print(json.dumps(error("nonzero_mode")))
    raise SystemExit(42)
if mode == "nonzero_no_envelope":
    raise SystemExit(42)
if mode == "provider_error":
    print(json.dumps(error("provider_error_mode")))
    raise SystemExit(0)
if mode == "provider_timeout":
    import time
    time.sleep(5)

if subcommand == "describe":
    response = describe()
elif subcommand == "session.locate_transcript":
    response = locate()
elif subcommand == "session.enumerate":
    response = enumerate_sessions()
elif subcommand == "session.read_turns":
    response = turn_page()
elif subcommand == "session.capture":
    response = capture()
else:
    response = error("unsupported_subcommand")
print(json.dumps(response))
"#,
        mode_path = json_string(mode_path),
        record_path = json_string(record_path),
        transcript_path = json_string(transcript_path),
        fixture_dir = json_string(dir),
        session_id = SESSION_ID,
        provider_instance_id = PROVIDER_INSTANCE_ID,
        settings_id = SETTINGS_ID,
        native_session_id = NATIVE_SESSION_ID,
        hostile_markers = hostile_markers_json(dir),
    )
}

fn json_string(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).expect("json path")
}

fn repo_script_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("scripts")
        .join(name)
}

fn opencode_sessions_config(
    script_path: &Path,
    opencode_bin: &Path,
    opencode_root: &Path,
    state_dir: &Path,
) -> SessionsConfig {
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let native_path = std::env::join_paths(
        std::iter::once(
            opencode_bin
                .parent()
                .expect("fake opencode parent")
                .to_path_buf(),
        )
        .chain(std::env::split_paths(&inherited_path)),
    )
    .expect("fake opencode PATH");
    SessionsConfig {
        entries: HashMap::from([(
            "opencode".to_string(),
            SessionSourceEntry {
                turn_script: format!(
                    "env -u OPENCODE_BIN PATH={} {} {}",
                    shell_single_quote_path(Path::new(&native_path)),
                    shell_single_quote_path(script_path),
                    shell_single_quote_path(opencode_root)
                ),
                transcript_locator: None,
                state_dir: Some(state_dir.to_path_buf()),
            },
        )]),
    }
}

fn write_fake_opencode(dir: &Path) -> PathBuf {
    let script = dir.join("opencode");
    fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json
import sys

args = sys.argv[1:]

if args == ["session", "list", "--format", "json"]:
    print(json.dumps([{"id": "ses_fixture"}], separators=(",", ":")))
    sys.exit(0)

if args != ["export", "ses_fixture"]:
    print("unexpected argv: " + repr(sys.argv[1:]), file=sys.stderr)
    sys.exit(3)

print(json.dumps({
    "sessionID": "ses_fixture",
    "messages": [
        {
            "id": "msg_user",
            "sessionID": "ses_fixture",
            "timestamp": "2026-05-01T00:00:01Z",
            "role": "user",
            "parts": [{"type": "text", "text": "hello"}],
        },
        {
            "id": "msg_assistant",
            "timestamp": "2026-05-01T00:00:02Z",
            "role": "assistant",
            "content": [{"type": "text", "text": "world"}],
        },
    ],
}, separators=(",", ":")))
"#,
    )
    .expect("write fake opencode");
    let mut perms = fs::metadata(&script).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).expect("chmod fake opencode");
    script
}

fn shell_single_quote_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
