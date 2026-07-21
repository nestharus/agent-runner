#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06::*;
use oulipoly_runtime::session_metadata::{
    MetadataError, SessionMetadata, SessionStorageType, locate_resume_session_metadata,
    locate_session_metadata,
};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

fn locate(prepared: &PreparedLocate) -> Result<SessionMetadata, MetadataError> {
    ensure_scripts_on_path();
    let db = prepared.fixture.open_db();
    locate_session_metadata(
        &db,
        &prepared.fixture.models(),
        &prepared.fixture.providers_config(),
        &prepared.fixture.sessions_config(),
        &prepared.session_id,
    )
}

fn locate_for_resume(prepared: &PreparedLocate) -> Result<SessionMetadata, MetadataError> {
    ensure_scripts_on_path();
    let db = prepared.fixture.open_db();
    locate_resume_session_metadata(
        &db,
        &prepared.fixture.models(),
        &prepared.fixture.providers_config(),
        &prepared.fixture.sessions_config(),
        &prepared.session_id,
    )
}

fn ensure_scripts_on_path() {
    static SCRIPTS_PATH: OnceLock<()> = OnceLock::new();
    SCRIPTS_PATH.get_or_init(|| {
        let scripts_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("scripts");
        let existing_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(scripts_dir).chain(std::env::split_paths(&existing_path)),
        )
        .unwrap();
        unsafe {
            std::env::set_var("PATH", path);
        }
    });
}

fn assert_session_not_found(result: Result<SessionMetadata, MetadataError>, input: &str) {
    match result {
        Err(MetadataError::SessionNotFound { input: actual }) => assert_eq!(actual, input),
        other => panic!("expected SessionNotFound, got {other:?}"),
    }
}

fn assert_unsupported(result: Result<SessionMetadata, MetadataError>, expected: &[&str]) {
    match result {
        Err(MetadataError::UnsupportedStorage { reason, .. }) => {
            for fragment in expected {
                assert!(
                    reason.contains(fragment),
                    "reason {reason:?} should contain {fragment:?}"
                );
            }
        }
        other => panic!("expected UnsupportedStorage, got {other:?}"),
    }
}

fn unsupported_reason(result: Result<SessionMetadata, MetadataError>) -> String {
    match result {
        Err(MetadataError::UnsupportedStorage { reason, .. }) => reason,
        other => panic!("expected UnsupportedStorage, got {other:?}"),
    }
}

fn assert_canonical_eq(actual: &Path, expected: &Path) {
    assert_eq!(actual, expected.canonicalize().unwrap());
}

fn valid_cwd_script(workspace_root: &Path) -> String {
    format!(
        "printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n'",
        workspace_root.display()
    )
}

fn component_script_storage_fixture(
    transcript_script: Option<&str>,
    cwd_script: &str,
) -> PreparedLocate {
    let fixture = LocateFixture::new();
    let workspace_root = fixture.root().join("script-workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let jsonl_path = fixture.root().join("script-session.jsonl");
    fs::write(
        &jsonl_path,
        format!(
            "{{\"sessionId\":\"{SESSION_A}\",\"type\":\"assistant\",\"message\":\"fixture\"}}\n"
        ),
    )
    .unwrap();
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_script_provider(
        CLAUDE_PROVIDER,
        cwd_script,
        transcript_script,
        transcript_script.map(|_| "claude_code"),
        true,
    );
    fixture.write_sessions_without_locator(CLAUDE_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root,
        jsonl_path,
    }
}

/// Risk: T2 — D1 ambiguity mirrors non-resume metadata resolver.
/// Level: component.
/// Source: contract §6 row T2; A2.
/// Observable: AmbiguousSession only for non-resume resolver ambiguity.
/// Residual: time-window edges bounded by deterministic fixture timestamps.
#[test]
fn locate_ambiguous_recent_multi_chain_returns_ambiguous_session() {
    let prepared = component_ambiguous_session_fixture();

    match locate(&prepared) {
        Err(MetadataError::AmbiguousSession { input }) => {
            assert_eq!(input, prepared.session_id);
        }
        other => panic!("expected AmbiguousSession, got {other:?}"),
    }
}

/// Risk: T2 — D1 recency cannot select among distinct native lineages.
/// Level: component.
/// Source: contract §6 row T2; A2.
/// Observable: resume metadata lookup preserves multi-chain ambiguity.
/// Residual: time-window edges bounded by deterministic fixture timestamps.
#[test]
fn locate_resume_recent_multi_chain_returns_ambiguous_session() {
    let prepared = component_ambiguous_session_fixture();

    match locate_for_resume(&prepared) {
        Err(MetadataError::AmbiguousSession { input }) => {
            assert_eq!(input, prepared.session_id);
        }
        other => panic!("expected AmbiguousSession, got {other:?}"),
    }
}

/// Risk: T2 — D1 stale sibling lineages remain distinct.
/// Level: component.
/// Source: contract §6 row T2; A2.
/// Observable: age differences do not collapse multi-chain ambiguity.
/// Residual: time-window edges bounded by deterministic fixture timestamps.
#[test]
fn locate_stale_sibling_lineage_returns_ambiguous_session() {
    let prepared = component_recency_collapsed_fixture();

    match locate(&prepared) {
        Err(MetadataError::AmbiguousSession { input }) => {
            assert_eq!(input, prepared.session_id);
        }
        other => panic!("expected AmbiguousSession, got {other:?}"),
    }
}

/// Risk: T3 — D2 storage mapping is stable at type level.
/// Level: unit.
/// Source: contract §6 row T3; A5.
/// Observable: mapping returns all three variants and serde names.
/// Residual: does not validate future storage variants.
#[test]
fn storage_type_mapping_covers_claude_codex_and_other() {
    let (claude, codex, other) = storage_mapping_inputs();

    let cases = [
        (SessionStorageType::from(&claude), "claude_code"),
        (SessionStorageType::from(&codex), "codex_session"),
        (SessionStorageType::from(&other), "other"),
    ];

    assert_eq!(cases[0].0, SessionStorageType::ClaudeCode);
    assert_eq!(cases[1].0, SessionStorageType::CodexSession);
    assert_eq!(cases[2].0, SessionStorageType::Other);
    for (storage_type, serialized) in cases {
        assert_eq!(serde_json::to_value(storage_type).unwrap(), serialized);
    }
}

/// Risk: T5 — D3 mutable true requires all reachable success conditions.
/// Level: component.
/// Source: contract §6 row T5; A8, A9.
/// Observable: fully located resumable Claude session returns mutable true.
/// Residual: condition 2 is structurally unreachable in v1 success path.
#[test]
fn locate_mutable_true_when_all_reachable_conditions_hold() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);

    let metadata = locate(&prepared).unwrap();

    assert!(metadata.mutable);
}

/// Risk: T5 — D3 mutable excludes providers without resume strategy.
/// Level: component.
/// Source: contract §6 row T5; A8.
/// Observable: successful locate with missing resume returns mutable false.
/// Residual: does not prove future pause-handshake lock semantics.
#[test]
fn locate_mutable_false_when_provider_resume_is_missing() {
    let prepared = component_claude_without_resume_fixture();

    let metadata = locate(&prepared).unwrap();

    assert!(!metadata.mutable);
}

/// Risk: T5 — D3 mutable does not consult quota exhaustion.
/// Level: component.
/// Source: contract §6 row T5; A8, A9.
/// Observable: exhausted provider quota does not flip mutable false.
/// Residual: does not prove future pause-handshake lock semantics.
#[test]
fn locate_mutable_ignores_provider_quota_exhaustion() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .seed_provider_quota_exhausted(CLAUDE_PROVIDER);

    let metadata = locate(&prepared).unwrap();

    assert!(metadata.mutable);
}

/// Risk: T5 — D3 transcript availability failure is not partial success.
/// Level: component.
/// Source: contract §6 row T5; A8.
/// Observable: missing locator plus missing transcript returns UnsupportedStorage before mutable output.
/// Residual: condition 4 failure has no partial success JSON.
#[test]
fn locate_mutable_matrix_missing_transcript_returns_unsupported_storage() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    std::fs::remove_file(&prepared.jsonl_path).unwrap();
    prepared
        .fixture
        .write_sessions_without_locator(CLAUDE_PROVIDER);

    assert_unsupported(locate(&prepared), &["claude_storage_scan_not_found"]);
}

/// Risk: T5 — D3 workspace-root failure is not partial success.
/// Level: component.
/// Source: contract §6 row T5; A8.
/// Observable: missing workspace returns UnsupportedStorage before mutable output.
/// Residual: condition 5 failure has no partial success JSON.
#[test]
fn locate_mutable_matrix_missing_workspace_returns_unsupported_storage() {
    let prepared = component_claude_missing_workspace_fixture();

    assert_unsupported(locate(&prepared), &["no_existing"]);
}

/// Risk: T5 — D3 no-storage is structurally unreachable on success.
/// Level: component.
/// Source: contract §6 row T5; A5, A8.
/// Observable: no-storage fixture returns UnsupportedStorage, not Other + mutable false.
/// Residual: condition 2 is structurally unreachable in v1 success path.
#[test]
fn locate_mutable_matrix_no_storage_returns_unsupported_storage() {
    let prepared = component_no_storage_fixture(true);

    assert_unsupported(locate(&prepared), &["workspace_root", "other"]);
}

/// Risk: T5 — D3 active-segment absence is not a partial mutable result.
/// Level: component.
/// Source: contract §6 row T5; A8.
/// Observable: segmentless session returns SessionNotFound.
/// Residual: overlaps T6 to pin matrix condition 1 explicitly.
#[test]
fn locate_mutable_matrix_missing_active_segment_returns_session_not_found() {
    let prepared = component_partial_db_fixture();

    assert_session_not_found(locate(&prepared), &prepared.session_id);
}

/// Risk: T6 — D4 partial DB invisible.
/// Level: component.
/// Source: contract §6 row T6; A7.
/// Observable: segmentless session_turns row returns SessionNotFound.
/// Residual: open-time backfill side effects are controlled by fixture setup after open.
#[test]
fn locate_segmentless_session_turn_is_session_not_found() {
    let prepared = component_partial_db_fixture();

    assert_session_not_found(locate(&prepared), &prepared.session_id);
}

/// Risk: T10 — D6 missing locator falls back to provider storage scan.
/// Level: component.
/// Source: contract §6 row T10; A3.
/// Observable: direct storage scan resolves the transcript and workspace.
/// Residual: locator timeout behavior is covered by existing locate_transcript tests.
#[test]
fn locate_without_locator_uses_provider_storage_scan() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .write_sessions_without_locator(CLAUDE_PROVIDER);

    let metadata = locate(&prepared).unwrap();

    assert_canonical_eq(&metadata.workspace_root, &prepared.workspace_root);
    assert_canonical_eq(&metadata.jsonl_path, &prepared.jsonl_path);
}

/// Risk: T10 — D6 missing JSONL maps to unsupported storage.
/// Level: component.
/// Source: contract §6 row T10; A3.
/// Observable: no partial success; UnsupportedStorage reason includes missing.
/// Residual: locator timeout behavior is covered by existing locate_transcript tests.
#[test]
fn locate_rejects_missing_jsonl_transcript_state() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    let missing = prepared.fixture.root().join("missing.jsonl");
    prepared
        .fixture
        .write_sessions_with_locator_path(CLAUDE_PROVIDER, &missing);

    assert_unsupported(locate(&prepared), &["missing"]);
}

/// Risk: T10 — D6 relative JSONL path maps to unsupported storage.
/// Level: component.
/// Source: contract §6 row T10; A3.
/// Observable: no partial success; UnsupportedStorage reason includes relative.
/// Residual: locator timeout behavior is covered by existing locate_transcript tests.
#[test]
fn locate_rejects_relative_jsonl_transcript_state() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .write_sessions_locator_returns_relative(CLAUDE_PROVIDER);

    assert_unsupported(locate(&prepared), &["relative"]);
}

/// Risk: T10 — D6 locator errors map to unsupported storage.
/// Level: component.
/// Source: contract §6 row T10; A3.
/// Observable: no partial success; UnsupportedStorage reason includes locator_error.
/// Residual: locator timeout behavior is covered by existing locate_transcript tests.
#[test]
fn locate_rejects_locator_error_transcript_state() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .write_sessions_locator_errors(CLAUDE_PROVIDER);

    assert_unsupported(locate(&prepared), &["locator_error"]);
}

/// Risk: T11 — D7 Claude path-hash inversion success.
/// Level: component.
/// Source: contract §6 row T11; A4.
/// Observable: canonical workspace_root equals fixture workspace.
/// Residual: real upstream path encoding drift can invalidate A4.
#[test]
fn locate_claude_path_hash_inversion_returns_canonical_workspace() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);

    let metadata = locate(&prepared).unwrap();

    assert_canonical_eq(&metadata.workspace_root, &prepared.workspace_root);
    assert_canonical_eq(&metadata.jsonl_path, &prepared.jsonl_path);
}

/// Risk: T12 — D7 Claude path-hash succeeds with exactly one decomposition.
/// Level: component.
/// Source: contract §6 row T12; A4.
/// Observable: one existing candidate succeeds.
/// Residual: specific tiebreaker only.
#[test]
fn locate_claude_path_hash_one_existing_decomposition_succeeds() {
    let prepared = component_claude_single_hyphen_workspace_fixture();

    let metadata = locate(&prepared).unwrap();

    assert_canonical_eq(&metadata.workspace_root, &prepared.workspace_root);
}

/// Risk: T12 — D7 Claude path-hash rejects zero decompositions.
/// Level: component.
/// Source: contract §6 row T12; A4.
/// Observable: zero existing candidates return UnsupportedStorage.
/// Residual: specific tiebreaker only.
#[test]
fn locate_claude_path_hash_zero_existing_decompositions_is_unsupported() {
    let prepared = component_claude_missing_workspace_fixture();

    assert_unsupported(locate(&prepared), &["no_existing"]);
}

/// Risk: T12 — D7 Claude path-hash rejects multiple decompositions.
/// Level: component.
/// Source: contract §6 row T12; A4.
/// Observable: multiple existing candidates return UnsupportedStorage.
/// Residual: specific tiebreaker only.
#[test]
fn locate_claude_path_hash_multiple_existing_decompositions_is_unsupported() {
    let prepared = component_claude_ambiguous_workspace_fixture();

    assert_unsupported(locate(&prepared), &["ambiguous_path_hash"]);
}

/// Risk: T13 — D7 Codex payload.cwd success.
/// Level: component.
/// Source: contract §6 row T13; A4.
/// Observable: canonical workspace_root equals Codex session_meta payload.cwd.
/// Residual: real Codex schema drift can invalidate A4.
#[test]
fn locate_codex_session_meta_payload_cwd_returns_canonical_workspace() {
    let prepared = component_codex_success_fixture(CODEX_PROVIDER);

    let metadata = locate(&prepared).unwrap();

    assert_canonical_eq(&metadata.workspace_root, &prepared.workspace_root);
    assert_eq!(metadata.storage_type, SessionStorageType::CodexSession);
}

/// Risk: T14 — D7 Codex missing session_meta failure mode.
/// Level: component.
/// Source: contract §6 row T14; A4.
/// Observable: UnsupportedStorage reason distinguishes session_meta absence.
/// Residual: multi-record edge follows first-match semantics.
#[test]
fn locate_codex_missing_session_meta_is_unsupported() {
    let prepared = component_codex_failure_fixture(
        "{\"type\":\"response\",\"payload\":{\"text\":\"no meta\"}}\n",
    );

    assert_unsupported(locate(&prepared), &["codex", "session_meta"]);
}

/// Risk: T14 — D7 Codex absent payload.cwd failure mode.
/// Level: component.
/// Source: contract §6 row T14; A4.
/// Observable: UnsupportedStorage reason distinguishes cwd absence.
/// Residual: multi-record edge follows first-match semantics.
#[test]
fn locate_codex_absent_payload_cwd_is_unsupported() {
    let prepared = component_codex_failure_fixture(&format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\"}}}}\n"
    ));

    assert_unsupported(locate(&prepared), &["codex", "cwd"]);
}

/// Risk: T14 — D7 Codex non-absolute cwd failure mode.
/// Level: component.
/// Source: contract §6 row T14; A4.
/// Observable: UnsupportedStorage reason distinguishes non-absolute cwd.
/// Residual: multi-record edge follows first-match semantics.
#[test]
fn locate_codex_non_absolute_payload_cwd_is_unsupported() {
    let prepared = component_codex_failure_fixture(&format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"relative/workspace\"}}}}\n"
    ));

    assert_unsupported(locate(&prepared), &["codex", "absolute"]);
}

/// Risk: T14 — D7 Codex non-existing cwd failure mode.
/// Level: component.
/// Source: contract §6 row T14; A4.
/// Observable: UnsupportedStorage reason distinguishes missing cwd.
/// Residual: multi-record edge follows first-match semantics.
#[test]
fn locate_codex_non_existing_payload_cwd_is_unsupported() {
    let prepared = component_codex_failure_fixture(&format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"/definitely/missing/oulipoly/workspace\"}}}}\n"
    ));

    assert_unsupported(locate(&prepared), &["codex", "missing"]);
}

/// Risk: T14 — D7 Codex non-UTF-8 canonical cwd failure mode.
/// Level: component.
/// Source: contract §6 row T14; A4.
/// Observable: UnsupportedStorage reason distinguishes non_utf8 canonical cwd.
/// Residual: JSON itself remains UTF-8; fixture uses a symlink to a non-UTF-8 target.
#[test]
fn locate_codex_non_utf8_canonical_payload_cwd_is_unsupported() {
    let prepared = component_codex_non_utf8_cwd_fixture();

    assert_unsupported(locate(&prepared), &["codex", "non_utf8"]);
}

/// Risk: T16 — JSON shape stability for UTF-8 paths and punctuation.
/// Level: component.
/// Source: contract §6 row T16; A3, A4.
/// Observable: required fields exist; UTF-8 paths round-trip as strings.
/// Residual: non-UTF-8 OS paths intentionally unsupported, not fuzzed.
#[test]
fn locate_json_shape_round_trips_unicode_paths_and_provider_punctuation() {
    let prepared = component_unicode_json_shape_fixture();

    let metadata = locate(&prepared).unwrap();
    let json: Value = serde_json::to_value(&metadata).unwrap();

    for field in required_success_fields() {
        assert!(json.get(field).is_some(), "missing {field} in {json}");
    }
    assert_eq!(
        json.as_object().unwrap().len(),
        required_success_fields().len()
    );
    assert_eq!(json["session_id"], prepared.session_id);
    assert_eq!(json["chain_id"], prepared.chain_id);
    assert_eq!(json["provider_name"], prepared.provider_name);
    assert_eq!(json["storage_type"], "claude_code");
    assert_eq!(json["transcript_state"], "available");
    assert_eq!(
        json["workspace_root"].as_str().unwrap(),
        prepared
            .workspace_root
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
}

/// Risk: AGE-137/B016 — transcript script process failures must stay controlled.
/// Level: component.
/// Source: Step 6a §5 row B016.
/// Observable: missing executable maps to an UnsupportedStorage reason with the transcript script failure stem.
/// Residual: POSIX shell reports a missing command as exit 127 rather than spawn failure.
#[test]
fn transcript_script_missing_executable_maps_to_unsupported_storage() {
    let cwd_script = valid_cwd_script(&tempfile::tempdir().unwrap().path().join("unused"));
    let prepared = component_script_storage_fixture(
        Some("/definitely/missing/oulipoly-transcript-script"),
        &cwd_script,
    );

    let reason = unsupported_reason(locate(&prepared));

    assert!(
        reason.contains("transcript_script_spawn_failed")
            || reason.contains("transcript_script_exit_127"),
        "unexpected reason {reason:?}"
    );
}

/// Risk: AGE-137/B017 — transcript script non-zero exits must retain useful stderr.
/// Level: component.
/// Source: Step 6a §5 row B017.
/// Observable: reason includes `transcript_script_exit_<code>` and stderr.
/// Residual: exact shell quoting around stderr is platform-owned.
#[test]
fn transcript_script_nonzero_exit_includes_kind_code_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    let cwd_script = valid_cwd_script(&workspace);
    let fixture = LocateFixture::new();
    let script = fixture.write_script(
        "transcript-nonzero.sh",
        "printf 'transcript exploded\\n' >&2\nexit 42",
    );
    let prepared =
        component_script_storage_fixture(Some(&script.display().to_string()), &cwd_script);

    let reason = unsupported_reason(locate(&prepared));

    assert!(reason.contains("transcript_script_exit_42"), "{reason}");
    assert!(reason.contains("transcript exploded"), "{reason}");
}

/// Risk: AGE-137/B018 — transcript script stdout cardinality is part of the contract.
/// Level: component.
/// Source: Step 6a §5 row B018.
/// Observable: empty stdout and multi-line stdout receive distinct reason stems.
/// Residual: whitespace-only stdout is represented by the empty-stdout case.
#[test]
fn transcript_script_empty_or_multiline_stdout_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let cwd_script = valid_cwd_script(&dir.path().join("workspace"));
    let empty_fixture = LocateFixture::new();
    let empty_script = empty_fixture.write_script("transcript-empty.sh", "true");
    let empty =
        component_script_storage_fixture(Some(&empty_script.display().to_string()), &cwd_script);

    assert_eq!(
        unsupported_reason(locate(&empty)),
        "transcript_script_empty_stdout"
    );

    let multiline_fixture = LocateFixture::new();
    let multiline_script = multiline_fixture.write_script(
        "transcript-multiline.sh",
        "printf '/tmp/one.jsonl\\n/tmp/two.jsonl\\n'",
    );
    let multiline = component_script_storage_fixture(
        Some(&multiline_script.display().to_string()),
        &cwd_script,
    );

    assert_eq!(
        unsupported_reason(locate(&multiline)),
        "transcript_script_stdout_not_single_line"
    );
}

/// Risk: AGE-137/B019 — normal locate mode still requires an existing transcript file.
/// Level: component.
/// Source: Step 6a §5 row B019.
/// Observable: absolute missing JSONL from transcript_script maps to `missing_jsonl_path`.
/// Residual: export allow-missing is covered separately by CT-001.
#[test]
fn transcript_script_missing_jsonl_path_rejected_in_require_existing() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.jsonl");
    let cwd_script = valid_cwd_script(&dir.path().join("workspace"));
    let fixture = LocateFixture::new();
    let script = fixture.write_script(
        "transcript-missing-jsonl.sh",
        &format!("printf '{}\\n'", missing.display()),
    );
    let prepared =
        component_script_storage_fixture(Some(&script.display().to_string()), &cwd_script);

    let reason = unsupported_reason(locate(&prepared));

    assert!(reason.starts_with("missing_jsonl_path:"), "{reason}");
    assert!(reason.contains(&missing.display().to_string()), "{reason}");
}

/// Risk: AGE-137/B020 — script storage without a transcript script must fail closed.
/// Level: component.
/// Source: Step 6a §5 row B020.
/// Observable: reason equals `no_transcript_script_for_script_storage`.
/// Residual: provider config validation for malformed script storage is covered elsewhere.
#[test]
fn script_storage_without_transcript_script_returns_no_transcript_script() {
    let dir = tempfile::tempdir().unwrap();
    let cwd_script = valid_cwd_script(&dir.path().join("workspace"));
    let prepared = component_script_storage_fixture(None, &cwd_script);

    assert_eq!(
        unsupported_reason(locate(&prepared)),
        "no_transcript_script_for_script_storage"
    );
}

/// Risk: AGE-137/B022 — Claude storage root failures are a user-visible fallback reason.
/// Level: component.
/// Source: Step 6a §5 row B022.
/// Observable: reason starts with `claude_projects_dir_unavailable`.
/// Residual: exact OS error text is platform-owned.
#[test]
fn claude_missing_projects_dir_reports_projects_dir_unavailable() {
    let fixture = LocateFixture::new();
    let projects_dir = fixture.root().join("missing-claude-projects");
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(
        CLAUDE_PROVIDER,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_without_locator(CLAUDE_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    let prepared = PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root: Path::new("/unused").to_path_buf(),
        jsonl_path: Path::new("/unused").to_path_buf(),
    };

    let reason = unsupported_reason(locate(&prepared));

    assert!(
        reason.starts_with("claude_projects_dir_unavailable:"),
        "{reason}"
    );
}

/// Risk: AGE-137/B024 — Claude fallback ambiguity must not silently pick a transcript.
/// Level: component.
/// Source: Step 6a §5 row B024.
/// Observable: reason equals `claude_storage_scan_ambiguous`.
/// Residual: candidates are not asserted because the public facade exposes only the reason string.
#[test]
fn claude_two_matching_transcripts_report_ambiguity() {
    let fixture = LocateFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_a = fixture.root().join("workspace-a");
    let workspace_b = fixture.root().join("workspace-b");
    fixture.stage_claude_transcript_for_encoded_path(&projects_dir, &workspace_a, SESSION_A);
    fixture.stage_claude_transcript_for_encoded_path(&projects_dir, &workspace_b, SESSION_A);
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(
        CLAUDE_PROVIDER,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_without_locator(CLAUDE_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    let prepared = PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CLAUDE_PROVIDER.to_string(),
        workspace_root: workspace_a,
        jsonl_path: Path::new("/unused").to_path_buf(),
    };

    assert_eq!(
        unsupported_reason(locate(&prepared)),
        "claude_storage_scan_ambiguous"
    );
}

/// Risk: AGE-137/B027 — Codex fallback direct rollout lookup remains available.
/// Level: component.
/// Source: Step 6a §5 row B027.
/// Observable: direct rollout resolves to the canonical located path.
/// Residual: nested traversal is covered by B028.
#[test]
fn codex_direct_rollout_resolves_through_contract() {
    let prepared = component_codex_success_fixture(CODEX_PROVIDER);
    prepared
        .fixture
        .write_sessions_without_locator(CODEX_PROVIDER);

    let metadata = locate(&prepared).unwrap();

    assert_canonical_eq(&metadata.jsonl_path, &prepared.jsonl_path);
    assert_eq!(metadata.storage_type, SessionStorageType::CodexSession);
}

/// Risk: AGE-137/B028 — Codex fallback recurses nested session directories.
/// Level: component.
/// Source: Step 6a §5 row B028.
/// Observable: nested rollout resolves to the canonical located path.
/// Residual: recursion depth boundary is covered by B029.
#[test]
fn codex_nested_rollout_resolves_through_contract() {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    let nested_dir = sessions_dir.join("2026").join("05").join("10");
    let workspace_root = fixture.root().join("codex-workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let jsonl_path = fixture.stage_codex_rollout(
        &nested_dir,
        &format!("rollout-2026-05-10T00-00-00-{SESSION_A}.jsonl"),
        &format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"{}\"}}}}\n",
            workspace_root.display()
        ),
    );
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_without_locator(CODEX_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    let prepared = PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        workspace_root,
        jsonl_path,
    };

    let metadata = locate(&prepared).unwrap();

    assert_canonical_eq(&metadata.jsonl_path, &prepared.jsonl_path);
}

/// Risk: AGE-137/B029 — Codex fallback recursion depth is intentionally bounded.
/// Level: component.
/// Source: Step 6a §5 row B029.
/// Observable: depth-4 rollout wins while depth-5 rollout is ignored.
/// Residual: performance on wider trees is not measured here.
#[test]
fn codex_depth_4_eligible_depth_5_ignored() {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    let depth_4 = sessions_dir.join("d1").join("d2").join("d3").join("d4");
    let depth_5 = depth_4.join("d5");
    let workspace_root = fixture.root().join("codex-workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let body = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"{}\"}}}}\n",
        workspace_root.display()
    );
    let expected = fixture.stage_codex_rollout(
        &depth_4,
        &format!("rollout-depth4-{SESSION_A}.jsonl"),
        &body,
    );
    fixture.stage_codex_rollout(
        &depth_5,
        &format!("rollout-depth5-{SESSION_A}.jsonl"),
        &body,
    );
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_without_locator(CODEX_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    let prepared = PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        workspace_root,
        jsonl_path: expected,
    };

    let metadata = locate(&prepared).unwrap();

    assert_canonical_eq(&metadata.jsonl_path, &prepared.jsonl_path);
}

/// Risk: AGE-137/B030 — Codex storage root failures are a user-visible fallback reason.
/// Level: component.
/// Source: Step 6a §5 row B030.
/// Observable: reason starts with `codex_sessions_dir_unavailable`.
/// Residual: exact OS error text is platform-owned.
#[test]
fn codex_missing_sessions_dir_reports_unavailable() {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("missing-codex-sessions");
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_without_locator(CODEX_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    let prepared = PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        workspace_root: Path::new("/unused").to_path_buf(),
        jsonl_path: Path::new("/unused").to_path_buf(),
    };

    let reason = unsupported_reason(locate(&prepared));

    assert!(
        reason.starts_with("codex_sessions_dir_unavailable:"),
        "{reason}"
    );
}

/// Risk: AGE-137/B031 — Codex no-match behavior remains explicit.
/// Level: component.
/// Source: Step 6a §5 row B031.
/// Observable: reason equals `codex_storage_scan_not_found`.
/// Residual: content fallback is not part of the Rust storage scan contract.
#[test]
fn codex_no_match_reports_not_found() {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_without_locator(CODEX_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    let prepared = PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        workspace_root: Path::new("/unused").to_path_buf(),
        jsonl_path: Path::new("/unused").to_path_buf(),
    };

    assert_eq!(
        unsupported_reason(locate(&prepared)),
        "codex_storage_scan_not_found"
    );
}

/// Risk: AGE-137/B032 — Codex fallback ambiguity must not silently pick a rollout.
/// Level: component.
/// Source: Step 6a §5 row B032.
/// Observable: reason equals `codex_storage_scan_ambiguous`.
/// Residual: candidates are not asserted because the public facade exposes only the reason string.
#[test]
fn codex_two_rollout_matches_report_ambiguity() {
    let fixture = LocateFixture::new();
    let sessions_dir = fixture.root().join("codex-sessions");
    let workspace_root = fixture.root().join("codex-workspace");
    fs::create_dir_all(&workspace_root).unwrap();
    let body = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{SESSION_A}\",\"cwd\":\"{}\"}}}}\n",
        workspace_root.display()
    );
    fixture.stage_codex_rollout(
        &sessions_dir,
        &format!("rollout-a-{SESSION_A}.jsonl"),
        &body,
    );
    fixture.stage_codex_rollout(
        &sessions_dir,
        &format!("rollout-b-{SESSION_A}.jsonl"),
        &body,
    );
    fixture.write_model(MODEL, &[CODEX_PROVIDER]);
    fixture.write_provider(
        CODEX_PROVIDER,
        StorageKind::Codex {
            sessions_dir: &sessions_dir,
        },
        true,
        None,
    );
    fixture.write_sessions_without_locator(CODEX_PROVIDER);
    fixture.seed_active_chain(
        CHAIN_A,
        CODEX_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    let prepared = PreparedLocate {
        fixture,
        session_id: SESSION_A.to_string(),
        chain_id: CHAIN_A.to_string(),
        provider_name: CODEX_PROVIDER.to_string(),
        workspace_root,
        jsonl_path: Path::new("/unused").to_path_buf(),
    };

    assert_eq!(
        unsupported_reason(locate(&prepared)),
        "codex_storage_scan_ambiguous"
    );
}

/// Risk: AGE-137/CT-002 — public facade preservation under the locator contract.
/// Level: component.
/// Source: Step 6a §5 row CT-002.
/// Observable: success metadata still carries facade fields and a representative error variant.
/// Residual: the surrounding component matrix covers the exhaustive edge rows.
#[test]
fn session_metadata_public_facade_preservation_under_locator_contract() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    let metadata = locate(&prepared).unwrap();

    assert_eq!(metadata.session_id, prepared.session_id);
    assert_eq!(metadata.chain_id, prepared.chain_id);
    assert_eq!(metadata.provider_name, prepared.provider_name);
    assert_eq!(metadata.storage_type, SessionStorageType::ClaudeCode);
    assert_eq!(
        serde_json::to_value(metadata.transcript_state).unwrap(),
        "available"
    );
    assert!(metadata.active_segment_id > 0);
    assert!(metadata.mutable);

    let missing = component_no_storage_fixture(false);
    assert_eq!(unsupported_reason(locate(&missing)), "no_locator");
}
