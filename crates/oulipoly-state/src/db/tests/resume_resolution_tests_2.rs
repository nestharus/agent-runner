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
//!   - component: crates/oulipoly-state/src/db/tests/resume_resolution_tests_2.rs
//!     role: intrinsic-surface
//!     Domain: resume-resolution-tests-2-persistence
//!     Owns:
//!       - StateDb resume-resolution-tests-2 persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: CHAIN_A, SESSION_A, chain_last_used_at_raw, db_without_table, seed_test_chain, test_db, ts
//! ```

use super::common::*;
use super::*;
#[test]
fn chain_last_used_at_updates_after_successful_invocation() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "provider-a-opus",
        "2026-04-17T08:00:00Z",
    );

    let before = Utc::now();
    db.update_chain_last_used(CHAIN_A).unwrap();
    let after = Utc::now();

    let last_used_raw = chain_last_used_at_raw(&db, CHAIN_A);
    let last_used = ts(&last_used_raw);
    assert!(last_used >= before - chrono::Duration::seconds(1));
    assert!(last_used <= after + chrono::Duration::seconds(1));
}

#[test]
fn chain_identity_helpers_report_sql_errors() {
    let segmentless = db_without_table("session_chain_segments");
    let segment_open_err = segmentless
        .open_chain_segment(
            CHAIN_A,
            "provider-a",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            oulipoly_core::TransitionReason::Initial,
        )
        .unwrap_err();
    assert!(
        segment_open_err.contains("session chain segment"),
        "{segment_open_err}"
    );

    let mint_err = db_without_table("session_chain_segments")
        .mint_imported_chain_if_absent(
            "provider-a",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            "provider-a-opus",
        )
        .unwrap_err();
    assert!(
        mint_err.contains("existing session chain segment"),
        "{mint_err}"
    );

    let update_err = db_without_table("session_chains")
        .update_chain_last_used(CHAIN_A)
        .unwrap_err();
    assert!(update_err.contains("last_used_at"), "{update_err}");

    let chain_lookup_err = db_without_table("session_chain_segments")
        .chain_id_for_segment("provider-a", SESSION_A)
        .unwrap_err();
    assert!(
        chain_lookup_err.contains("session chain id"),
        "{chain_lookup_err}"
    );
}

#[test]
fn compaction_and_preview_helpers_report_negative_paths() {
    let malformed_uuid = test_db().resume_previews("not-a-uuid").unwrap_err();
    assert!(malformed_uuid.contains("Invalid UUID"), "{malformed_uuid}");

    let db = test_db();
    db.conn
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_compaction_boundary)
                 VALUES ('provider-a', ?1, 'bad-boundary', 'not-a-timestamp', 'assistant', '', '2026-04-17T08:00:00Z', 1)",
                sqlite::params![SESSION_A],
            )
            .unwrap();

    let boundary_err = db
        .latest_compaction_boundary("provider-a", SESSION_A)
        .unwrap_err();
    assert!(
        boundary_err.contains("Bad compaction boundary timestamp"),
        "{boundary_err}"
    );
}
