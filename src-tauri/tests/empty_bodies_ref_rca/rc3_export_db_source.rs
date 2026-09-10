use super::RcaFixture;

/// RC-3 — explicit provider authority owns export and does not fall back to
/// DB-stored bodies when provider storage is unavailable.
#[test]
fn session_export_reports_unsupported_storage_when_provider_jsonl_is_missing() {
    // risk: export regression; level: end-to-end; source: contract §4 T3 / ~/projects/agent-runner/planning/trunk/research/12-empty-bodies-ref-rca.md RC-3.
    let fixture = RcaFixture::new();
    let _db = fixture.open_db();
    fixture.seed_chain();
    fixture.seed_body_turns();
    fixture.write_cli_config_with_missing_transcript();

    let output = fixture.run_export();

    assert_eq!(output.status.code(), Some(12), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "unsupported-storage");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("sessions directory unavailable")),
        "{error}"
    );
}
