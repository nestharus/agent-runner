#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06::*;
use oulipoly_runtime::session_metadata::{
    MetadataError, SessionMetadata, SessionStorageType, locate_session_metadata,
};
use serde_json::Value;
use std::path::Path;

fn locate(prepared: &PreparedLocate) -> Result<SessionMetadata, MetadataError> {
    let db = prepared.fixture.open_db();
    locate_session_metadata(
        &db,
        &prepared.fixture.models(),
        &prepared.fixture.providers_config(),
        &prepared.fixture.sessions_config(),
        &prepared.session_id,
    )
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

fn assert_canonical_eq(actual: &Path, expected: &Path) {
    assert_eq!(actual, expected.canonicalize().unwrap());
}

/// Risk: T2 — D1 ambiguity mirrors resolver.
/// Level: component.
/// Source: contract §6 row T2; A2.
/// Observable: AmbiguousSession only for resolver ambiguity.
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

/// Risk: T2 — D1 recency collapse does not become synthetic ambiguity.
/// Level: component.
/// Source: contract §6 row T2; A2.
/// Observable: recency-collapsed multi-chain input succeeds.
/// Residual: time-window edges bounded by deterministic fixture timestamps.
#[test]
fn locate_recency_collapsed_multi_chain_succeeds() {
    let prepared = component_recency_collapsed_fixture();

    let metadata = locate(&prepared).unwrap();

    assert_eq!(metadata.chain_id, prepared.chain_id);
    assert_eq!(metadata.session_id, prepared.session_id);
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
/// Observable: missing locator returns UnsupportedStorage before mutable output.
/// Residual: condition 4 failure has no partial success JSON.
#[test]
fn locate_mutable_matrix_missing_transcript_returns_unsupported_storage() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .write_sessions_without_locator(CLAUDE_PROVIDER);

    assert_unsupported(locate(&prepared), &["no_locator"]);
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

/// Risk: T10 — D6 no locator maps to unsupported storage.
/// Level: component.
/// Source: contract §6 row T10; A3.
/// Observable: no partial success; UnsupportedStorage reason includes no_locator.
/// Residual: locator timeout behavior is covered by existing locate_transcript tests.
#[test]
fn locate_rejects_no_locator_transcript_state() {
    let prepared = component_claude_success_fixture(CLAUDE_PROVIDER, true);
    prepared
        .fixture
        .write_sessions_without_locator(CLAUDE_PROVIDER);

    assert_unsupported(locate(&prepared), &["no_locator"]);
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
