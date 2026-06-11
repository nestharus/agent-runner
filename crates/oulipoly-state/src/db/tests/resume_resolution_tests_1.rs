//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn resolve_resume_returns_active_segment_for_single_chain() {
    let db = test_db();
    seed_chain_row(&db, CHAIN_A, "claude-opus", "2026-04-17T09:00:00Z");
    seed_segment_row(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        Some("2026-04-17T08:30:00Z"),
        "initial",
    );
    seed_segment_row(
        &db,
        CHAIN_A,
        "claude2",
        SESSION_B,
        "2026-04-17T08:31:00Z",
        None,
        "quota_threshold",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, CHAIN_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_A);
    assert_eq!(resolved.active_provider, "claude2");
    assert_eq!(resolved.active_session_id, SESSION_B);
    assert_eq!(resolved.model_name.as_deref(), Some("claude-opus"));
}

#[test]
fn resolve_resume_chooses_most_recent_chain_when_two_chains_share_session_id() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T09:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_B);
}

#[test]
fn resolve_resume_chooses_most_recent_chain_without_ambiguous_halt() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "claude2",
        SESSION_A,
        "claude-opus",
        "2026-04-17T09:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_C,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T10:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_C);
}

#[test]
fn resolve_resume_breaks_equal_last_used_tie_by_latest_segment_start() {
    let db = test_db();
    let last_used_at = "2026-04-17T10:00:00Z";
    seed_chain_row(&db, CHAIN_A, "claude-opus", last_used_at);
    seed_segment_row(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        None,
        "initial",
    );
    seed_chain_row(&db, CHAIN_B, "claude-opus", last_used_at);
    seed_segment_row(
        &db,
        CHAIN_B,
        "claude2",
        SESSION_A,
        "2026-04-17T09:00:00Z",
        None,
        "initial",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_B);
}

#[test]
fn resolve_resume_infers_model_from_latest_invocation() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "<unknown>",
        "2026-04-17T08:00:00Z",
    );
    seed_invocation_for_session(
        &db,
        "claude-haiku",
        "claude",
        SESSION_A,
        "2026-04-17T08:00:00Z",
    );
    seed_invocation_for_session(
        &db,
        "claude-opus",
        "claude",
        SESSION_A,
        "2026-04-17T09:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name.as_deref(), Some("claude-opus"));
}

#[test]
fn resolve_resume_falls_back_to_chain_model_name_when_no_invocations() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-haiku",
        "2026-04-17T08:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name.as_deref(), Some("claude-haiku"));
}

#[test]
fn resolve_resume_returns_none_model_when_no_inference_source() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
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
        "claude2",
        SESSION_A,
        "claude-haiku",
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
            assert_eq!(model_name, "claude-haiku");
            assert_eq!(active_provider, "claude2");
            assert!(suggestions.contains(&"claude-opus".to_string()));
        }
        other => panic!("expected provider/model mismatch, got {other:?}"),
    }
}
