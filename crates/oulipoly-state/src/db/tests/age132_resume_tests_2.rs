//! ## Declared roles
//!
//! - validator
//! - parser
//!
//! Role set: { validator, parser }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/age132_resume_tests_2.rs
//!     role: intrinsic-surface
//!     Domain: age132-resume-tests-2-persistence
//!     Owns:
//!       - StateDb age132-resume-tests-2 persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: CHAIN_A, SESSION_A, chain_segment_started_at_raw, exhausted_at, model_store_from_toml, quota_input, resolver_model_store, seed_chain_row, seed_segment_row, seed_test_chain, test_db
//! ```

use super::common::*;
use super::*;
#[test]
fn age132_resolve_resume_rejections_and_wrong_id_context_are_typed() {
    let models = resolver_model_store();
    assert!(matches!(
        test_db()
            .resolve_resume(&models, "not-a-uuid", None)
            .unwrap_err(),
        ResumeError::NoChainFound { .. }
    ));
    assert!(matches!(
        test_db()
            .resolve_resume(&models, "ses_ab", None)
            .unwrap_err(),
        ResumeError::NoChainFound { .. }
    ));
    assert!(matches!(
        test_db()
            .resolve_resume(&models, "77777777-7777-4777-8777-777777777777", None)
            .unwrap_err(),
        ResumeError::NoChainFound { .. }
    ));

    let unknown_model_db = test_db();
    seed_test_chain(
        &unknown_model_db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "missing-model",
        "2026-04-17T08:00:00Z",
    );
    assert!(matches!(
        unknown_model_db.resolve_resume(&models, SESSION_A, None).unwrap_err(),
        ResumeError::UnknownModel { ref model_name } if model_name == "missing-model"
    ));

    let missing_segment_db = test_db();
    seed_chain_row(
        &missing_segment_db,
        CHAIN_A,
        "provider-a-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_segment_row(
        &missing_segment_db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        Some("2026-04-17T08:30:00Z"),
        "initial",
    );
    assert!(matches!(
        missing_segment_db.resolve_resume(&models, SESSION_A, None).unwrap_err(),
        ResumeError::ActiveSegmentMissing { ref chain_id } if chain_id == CHAIN_A
    ));

    let wrong_id_db = test_db();
    let invocation_uuid = "88888888-8888-4888-8888-888888888888";
    let id = wrong_id_db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "provider-a-opus".to_string(),
            provider_name: "provider-a".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    wrong_id_db
        .bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: SESSION_A.to_string(),
                capture_method: "verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    match wrong_id_db
        .resolve_resume(&models, invocation_uuid, None)
        .unwrap_err()
    {
        ResumeError::WrongIdKind {
            provider_session_id,
            chain_id,
            provider_name,
            agent_runner_invocation_id,
            ..
        } => {
            assert_eq!(provider_session_id.as_deref(), Some(SESSION_A));
            assert!(chain_id.is_some());
            assert_eq!(provider_name.as_deref(), Some("provider-a"));
            assert_eq!(agent_runner_invocation_id, invocation_uuid);
        }
        other => panic!("expected wrong-id-kind rejection, got {other:?}"),
    }
}

#[test]
fn resolve_resume_rejects_only_invalid_opaque_input_before_lookup() {
    let models = resolver_model_store();

    for input in ["", " ", "abc\n123", "abc\u{0}123"] {
        assert!(matches!(
            test_db().resolve_resume(&models, input, None).unwrap_err(),
            ResumeError::InvalidResumeInput { .. }
        ));
    }

    let too_long = "x".repeat(RESUME_INPUT_MAX_LEN + 1);
    assert!(matches!(
        test_db()
            .resolve_resume(&models, &too_long, None)
            .unwrap_err(),
        ResumeError::InvalidResumeInput { .. }
    ));
}

#[test]
fn resolve_resume_accepts_provider_native_non_uuid_non_opencode_id() {
    let db = test_db();
    let models = model_store_from_toml(&[(
        "external-high",
        r#"
[[providers]]
name = "external"
interactive_args = ["--resume"]
"#,
    )]);
    seed_test_chain(
        &db,
        CHAIN_A,
        "external",
        "external-abc123xyz",
        "external-high",
        "2026-06-04T08:00:00Z",
    );

    let resolved = db
        .resolve_resume(&models, "external-abc123xyz", None)
        .unwrap();

    assert_eq!(resolved.chain_id, CHAIN_A);
    assert_eq!(resolved.active_provider, "external");
    assert_eq!(resolved.active_session_id, "external-abc123xyz");
    assert_eq!(resolved.model_name.as_deref(), Some("external-high"));
}

#[test]
fn resolve_resume_accepts_opencode_provider_session_id() {
    let db = test_db();
    let models = model_store_from_toml(&[(
        "gpt-high",
        r#"
[[providers]]
name = "opencode"
interactive_args = ["run"]
"#,
    )]);
    seed_test_chain(
        &db,
        CHAIN_A,
        "opencode",
        "ses_fixture",
        "gpt-high",
        "2026-06-04T08:00:00Z",
    );

    let resolved = db.resolve_resume(&models, "ses_fixture", None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_A);
    assert_eq!(resolved.active_provider, "opencode");
    assert_eq!(resolved.active_session_id, "ses_fixture");
    assert_eq!(resolved.model_name.as_deref(), Some("gpt-high"));
}

#[test]
fn age132_timestamp_policies_preserve_strict_forgiving_and_fallback_callers() {
    let db = test_db();
    db.upsert_quota_refresh("provider-a", &[quota_input(0.40, "2026-04-22T00:00:00Z")])
        .unwrap();
    db.conn
        .execute(
            "UPDATE provider_quotas
                 SET refreshed_at = 'bad-refreshed',
                     exhausted_at = 'bad-exhausted',
                     last_topology_probe_at = 'bad-probe'
                 WHERE provider_name = 'provider-a'",
            [],
        )
        .unwrap();
    let quota = db.get_quota("provider-a").unwrap().unwrap();
    assert_eq!(quota.refreshed_at, None);
    assert_eq!(quota.exhausted_at, None);
    assert_eq!(quota.last_topology_probe_at, None);
    db.conn
            .execute(
                "UPDATE provider_quota_windows SET resets_at = 'bad-window' WHERE provider_name = 'provider-a'",
                [],
            )
            .unwrap();
    assert!(db.get_windows("provider-a").is_err());

    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "provider-a-opus".to_string(),
            provider_name: "provider-a".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(id, Some(SESSION_A), "verified")
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations SET created_at = 'not-a-timestamp' WHERE id = ?1",
            sqlite::params![id],
        )
        .unwrap();
    let before = Utc::now();
    db.mint_chain_for_invocation_session(id).unwrap();
    let after = Utc::now();
    let raw_started = chain_segment_started_at_raw(&db, "provider-a", SESSION_A);
    let started_at = parse_test_timestamp_utc(&raw_started);
    assert!(started_at >= before - chrono::Duration::seconds(1));
    assert!(started_at <= after + chrono::Duration::seconds(1));
}

fn parse_test_timestamp_utc(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw)
        .unwrap()
        .with_timezone(&Utc)
}
