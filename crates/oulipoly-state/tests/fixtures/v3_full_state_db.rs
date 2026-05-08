use super::{
    RepresentativeSnapshot, create_full_state_schema, open, representative_snapshot,
    seed_representative_state_rows,
};
use oulipoly_state::schema::{CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION};
use rusqlite::Connection;
use std::path::Path;

pub fn fixture_schema_version() -> i32 {
    MINIMUM_SUPPORTED_SCHEMA_VERSION
}

pub fn build_v3_full_state_db(path: &Path) {
    let conn = open(path);
    create_full_state_schema(&conn, MINIMUM_SUPPORTED_SCHEMA_VERSION);
    seed_representative_state_rows(&conn);
}

pub fn build_current_full_state_db(path: &Path) {
    let conn = open(path);
    create_full_state_schema(&conn, CURRENT_SCHEMA_VERSION);
    seed_representative_state_rows(&conn);
}

pub fn assert_representative_state_rows_preserved(conn: &Connection) -> RepresentativeSnapshot {
    let snapshot = representative_snapshot(conn);
    assert_eq!(snapshot.invocations, 2);
    assert_eq!(snapshot.providers, 1);
    assert_eq!(snapshot.provider_quotas, 1);
    assert_eq!(snapshot.provider_quota_windows, 1);
    assert_eq!(snapshot.memory_nodes, 2);
    assert_eq!(snapshot.memory_edges, 1);
    assert_eq!(snapshot.setup_sessions, 1);
    assert_eq!(snapshot.setup_turns, 1);
    assert_eq!(snapshot.session_turns, 2);
    assert_eq!(snapshot.session_chains, 1);
    assert_eq!(snapshot.session_chain_segments, 1);
    assert_eq!(snapshot.root_status, "succeeded");
    assert_eq!(snapshot.child_parent_id, 1);
    assert_eq!(snapshot.provider_invocation_count, 2);
    assert_eq!(snapshot.quota_calls_since_refresh, 12);
    assert_eq!(
        snapshot.assistant_body.as_deref(),
        Some("{\"role\":\"assistant\"}")
    );
    assert_eq!(snapshot.segment_last_turn_id.as_deref(), Some("turn-child"));
    snapshot
}
