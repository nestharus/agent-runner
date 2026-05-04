use super::RcaFixture;

/// RC-3 — `agents session export` reconstructs turn bodies from provider JSONL
/// instead of using DB-stored bodies.
///
/// Design-intent source: user report for Phase 0 says `state.db`, not provider
/// JSONL files, is the intended source of truth for turn bodies. This harness
/// seeds DB-stored canonical body payloads and deliberately points the transcript
/// locator at a missing JSONL file; export should still succeed from SQLite.
#[test]
fn session_export_emits_db_stored_bodies_when_jsonl_is_missing() {
    // risk: export regression; level: end-to-end; source: contract §4 T3 / ~/projects/agent-runner/planning/trunk/research/12-empty-bodies-ref-rca.md RC-3.
    let fixture = RcaFixture::new();
    let _db = fixture.open_db();
    fixture.seed_chain();
    fixture.seed_body_turns();
    fixture.write_cli_config_with_missing_transcript();

    let output = fixture.run_export();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("db stored assistant body"),
        "export must emit DB-stored turn content without reading provider JSONL; stdout={stdout:?}"
    );
    assert!(
        stdout.contains("db stored user body"),
        "export must emit all DB-stored turn content without reading provider JSONL; stdout={stdout:?}"
    );
    assert!(
        stdout.lines().any(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["source"]["storage_type"] == "state_db"
        }),
        "export must mark DB fallback records with state_db source sentinel; stdout={stdout:?}"
    );
}
