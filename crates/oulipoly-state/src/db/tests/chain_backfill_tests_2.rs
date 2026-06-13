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
//!   - component: crates/oulipoly-state/src/db/tests/chain_backfill_tests_2.rs
//!     role: intrinsic-surface
//!     Domain: chain-backfill-tests-2-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::common::*;
use super::*;
#[test]
fn agent_session_chain_records_initial_reason_even_if_ingestion_minted_first() {
    let db = test_db();
    db.mint_imported_chain_if_absent(
        "provider-a",
        SESSION_A,
        &ts("2026-04-17T08:00:00Z"),
        "<unknown>",
    )
    .unwrap();
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "provider-a-opus".to_string(),
            provider_name: "provider-a".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(id, Some(SESSION_A), "fixture")
        .unwrap();

    db.mint_chain_for_invocation_session(id).unwrap();

    let reason = chain_segment_transition_reason(&db, "provider-a", SESSION_A);
    assert_eq!(reason, "initial");
}

#[test]
fn imported_session_stays_imported_when_no_agent_mint_fires() {
    let db = test_db();

    db.mint_imported_chain_if_absent(
        "provider-a",
        SESSION_A,
        &ts("2026-04-17T08:00:00Z"),
        "<unknown>",
    )
    .unwrap();

    let reason = chain_segment_transition_reason(&db, "provider-a", SESSION_A);
    assert_eq!(reason, "imported");
}
