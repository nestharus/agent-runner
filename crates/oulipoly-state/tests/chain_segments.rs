use chrono::{DateTime, Utc};
use oulipoly_core::TransitionReason;
use oulipoly_state::StateDb;

const SESSION_A: &str = "session-a";
const SESSION_B: &str = "session-b";

fn ts(value: &str) -> DateTime<Utc> {
    value.parse::<DateTime<Utc>>().unwrap()
}

#[test]
fn list_chain_segments_desc_returns_segments_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    db.mint_imported_chain_if_absent(
        "provider-retired",
        SESSION_A,
        &ts("2026-04-17T08:00:00Z"),
        "model-opus",
    )
    .unwrap();
    let chain_id = db
        .chain_id_for_segment("provider-retired", SESSION_A)
        .unwrap()
        .unwrap();
    db.open_chain_segment(
        &chain_id,
        "provider-live",
        SESSION_B,
        &ts("2026-04-17T09:00:00Z"),
        TransitionReason::Manual,
    )
    .unwrap();

    let segments = db.list_chain_segments_desc(&chain_id).unwrap();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].provider_name, "provider-live");
    assert_eq!(segments[0].session_id, SESSION_B);
    assert_eq!(segments[1].provider_name, "provider-retired");
    assert_eq!(segments[1].session_id, SESSION_A);
}
