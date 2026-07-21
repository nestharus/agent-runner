//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/resume_resolution_tests_1.rs
//!     role: intrinsic-surface
//!     Domain: resume-resolution-tests-1-persistence
//!     Owns:
//!       - StateDb resume-resolution-tests-1 persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: CHAIN_A, CHAIN_B, CHAIN_C, SESSION_A, SESSION_B, resolver_model_store, seed_chain_row, seed_invocation_for_session, seed_segment_row, seed_test_chain, test_db
//! ```

use super::common::*;
use super::*;
#[test]
fn resolve_resume_returns_active_segment_for_single_chain() {
    let db = test_db();
    seed_chain_row(&db, CHAIN_A, "provider-a-opus", "2026-04-17T09:00:00Z");
    seed_segment_row(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        Some("2026-04-17T08:30:00Z"),
        "initial",
    );
    seed_segment_row(
        &db,
        CHAIN_A,
        "provider-a2",
        SESSION_B,
        "2026-04-17T08:31:00Z",
        None,
        "quota_threshold",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, CHAIN_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_A);
    assert_eq!(resolved.active_provider, "provider-a2");
    assert_eq!(resolved.active_session_id, SESSION_B);
    assert_eq!(resolved.model_name.as_deref(), Some("provider-a-opus"));
}

#[test]
fn resolve_resume_classification_prefers_exact_chain_over_native_match() {
    let db = test_db();
    seed_test_chain(
        &db,
        SESSION_A,
        "provider-a",
        SESSION_B,
        "provider-a-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "provider-a",
        SESSION_A,
        "provider-a-opus",
        "2026-04-17T09:00:00Z",
    );
    let classified = db.classify_resume_input(SESSION_A).unwrap();

    assert_eq!(
        classified,
        ResumeInputMatch::ExactChain {
            chain_id: SESSION_A.to_string()
        }
    );
}

#[test]
fn resolve_resume_classification_preserves_provider_scoped_native_candidates() {
    let db = test_db();
    seed_chain_row(&db, CHAIN_A, "provider-a-opus", "2026-04-17T08:00:00Z");
    seed_segment_row(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        Some("2026-04-17T08:30:00Z"),
        "initial",
    );
    seed_segment_row(
        &db,
        CHAIN_A,
        "provider-a2",
        SESSION_A,
        "2026-04-17T08:31:00Z",
        None,
        "quota_threshold",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "provider-a2",
        SESSION_A,
        "provider-a-opus",
        "2026-04-17T09:00:00Z",
    );

    let classified = db.classify_resume_input(SESSION_A).unwrap();

    assert_eq!(
        classified,
        ResumeInputMatch::NativeSession {
            candidates: vec![
                ResumeNativeCandidate {
                    chain_id: CHAIN_A.to_string(),
                    matching_provider: "provider-a".to_string(),
                },
                ResumeNativeCandidate {
                    chain_id: CHAIN_A.to_string(),
                    matching_provider: "provider-a2".to_string(),
                },
                ResumeNativeCandidate {
                    chain_id: CHAIN_B.to_string(),
                    matching_provider: "provider-a2".to_string(),
                },
            ]
        }
    );
}

#[test]
fn resolve_resume_treats_multiple_matching_segments_in_one_chain_as_one_lineage() {
    let db = test_db();
    seed_chain_row(&db, CHAIN_A, "provider-a-opus", "2026-04-17T09:00:00Z");
    seed_segment_row(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        Some("2026-04-17T08:30:00Z"),
        "initial",
    );
    seed_segment_row(
        &db,
        CHAIN_A,
        "provider-a2",
        SESSION_A,
        "2026-04-17T08:31:00Z",
        None,
        "quota_threshold",
    );

    let resolved = db
        .resolve_resume(&resolver_model_store(), SESSION_A, None)
        .unwrap();

    assert_eq!(resolved.chain_id, CHAIN_A);
    assert_eq!(resolved.active_provider, "provider-a2");
    assert_eq!(resolved.active_session_id, SESSION_A);
}

#[test]
fn resolve_resume_rejects_multiple_native_chains_without_ordering_them() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "provider-a-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "provider-a2",
        SESSION_A,
        "provider-a-opus",
        "2026-04-17T10:00:00Z",
    );
    let models = resolver_model_store();

    let error = db.resolve_resume(&models, SESSION_A, None).unwrap_err();

    match error {
        ResumeError::Ambiguous { input, previews } => {
            assert_eq!(input, SESSION_A);
            assert_eq!(previews.len(), 2);
            assert!(previews.iter().any(|preview| preview.chain_id == CHAIN_A));
            assert!(previews.iter().any(|preview| preview.chain_id == CHAIN_B));
        }
        other => panic!("expected ambiguous native resume, got {other:?}"),
    }
}

#[test]
fn resolve_resume_infers_model_from_latest_invocation() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "<unknown>",
        "2026-04-17T08:00:00Z",
    );
    seed_invocation_for_session(
        &db,
        "provider-a-haiku",
        "provider-a",
        SESSION_A,
        "2026-04-17T08:00:00Z",
    );
    seed_invocation_for_session(
        &db,
        "provider-a-opus",
        "provider-a",
        SESSION_A,
        "2026-04-17T09:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name.as_deref(), Some("provider-a-opus"));
}

#[test]
fn resolve_resume_falls_back_to_chain_model_name_when_no_invocations() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "provider-a-haiku",
        "2026-04-17T08:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name.as_deref(), Some("provider-a-haiku"));
}

#[test]
fn resolve_resume_returns_none_model_when_no_inference_source() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "<unknown>",
        "2026-04-17T08:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name, None);
    assert!(resolved.model.is_none());
}

#[test]
fn resolve_resume_validates_provider_in_model_pool() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a2",
        SESSION_A,
        "provider-a-haiku",
        "2026-04-17T08:00:00Z",
    );
    let models = resolver_model_store();

    let err = db.resolve_resume(&models, SESSION_A, None).unwrap_err();

    match err {
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            suggestions,
        } => {
            assert_eq!(model_name, "provider-a-haiku");
            assert_eq!(active_provider, "provider-a2");
            assert!(suggestions.contains(&"provider-a-opus".to_string()));
        }
        other => panic!("expected provider/model mismatch, got {other:?}"),
    }
}

#[test]
fn preserve_compaction_boundary_for_session_copies_only_latest_marker_to_fresh_session() {
    let db = test_db();
    let ts = |value: &str| {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    db.ingest_session_turns_batch(
        "provider-a",
        &[
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "pre-boundary".to_string(),
                timestamp: ts("2026-04-17T07:59:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some("pre-boundary history".to_string()),
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "boundary-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: Some("pre-boundary".to_string()),
                is_sidechain: false,
                is_compaction_boundary: true,
                body: Some("compact summary".to_string()),
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "post-boundary".to_string(),
                timestamp: ts("2026-04-17T08:01:00Z"),
                role: "user".to_string(),
                parent_turn_id: Some("boundary-turn".to_string()),
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some("post-boundary prompt".to_string()),
            },
        ],
    )
    .unwrap();

    let preserved = db
        .preserve_compaction_boundary_for_session("provider-a", SESSION_A, "provider-a", SESSION_B)
        .unwrap();

    assert!(
        preserved,
        "expected boundary marker to be inserted for the fresh session"
    );
    assert_eq!(
        db.latest_compaction_boundary("provider-a", SESSION_B)
            .unwrap()
            .map(|(turn_id, _)| turn_id),
        Some("boundary-turn".to_string())
    );
    assert_eq!(
        db.count_session_turns("provider-a", SESSION_A)
            .unwrap()
            .total,
        3
    );
    assert_eq!(
        db.count_session_turns("provider-a", SESSION_B)
            .unwrap()
            .total,
        1,
        "fresh session should receive only the boundary marker metadata, not pre-boundary history"
    );
    assert_eq!(
        db.connection()
            .query_row(
                "SELECT body FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
                rusqlite::params!["provider-a", SESSION_B, "boundary-turn"],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
            .as_deref(),
        Some("compact summary")
    );
}
