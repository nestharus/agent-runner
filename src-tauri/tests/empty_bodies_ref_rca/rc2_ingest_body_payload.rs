use super::{PROVIDER, RcaFixture, SESSION_ID};
use agent_runner_lib::config::{SessionSourceEntry, SessionsConfig};
use agent_runner_lib::sessions::scan_provider;
use std::collections::HashMap;

/// RC-2 — the turn-script ingest boundary accepts only summary fields and drops
/// the body payload before SQLite.
///
/// Design-intent source: user report for Phase 0 says the DB should store the
/// content payload for every turn directly. This harness emits a turn-script
/// record with `content` and then expects that payload to be retrievable from
/// `session_turns`.
#[test]
fn turn_script_ingest_persists_body_payload_in_session_turns() {
    let fixture = RcaFixture::new();
    let script = fixture.write_script(
        "turns-with-content.sh",
        &format!(
            "printf '%s\\n' {}",
            super::sh_path(std::path::Path::new(&format!(
                r#"{{"session_id":"{SESSION_ID}","turn_id":"turn-with-content","timestamp":"2026-04-17T08:00:00Z","role":"assistant","content":[{{"type":"text","text":"ingested body payload"}}]}}"#
            )))
        ),
    );
    let mut entries = HashMap::new();
    entries.insert(
        PROVIDER.to_string(),
        SessionSourceEntry {
            turn_script: script.to_string_lossy().to_string(),
            transcript_locator: None,
            state_dir: Some(fixture.root().join("scan-state")),
        },
    );
    let sessions = SessionsConfig { entries };
    let db = fixture.open_db();

    let report = scan_provider(PROVIDER, &sessions, &db);

    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.new_turns, 1);
    let content = fixture
        .fetch_turn_content("turn-with-content")
        .expect("ingested turn body must be stored in state.db");
    assert_eq!(content[0]["text"], "ingested body payload");
}
