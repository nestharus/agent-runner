#[test]
fn s7a_production_ingest_callers_pass_external_provider_identity() {
    assert_source_contains(
        "src-tauri/src/run/balancing/mapper.rs",
        "external_provider: crate::session_ingest_cli::session_external_provider_identity(",
    );
    assert_source_contains(
        "src-tauri/src/run/resume/finalization.rs",
        "external_provider: crate::session_ingest_cli::session_external_provider_identity(",
    );
    assert_source_contains(
        "src-tauri/src/run/repl/finalization.rs",
        "external_provider: crate::session_ingest_cli::session_external_provider_identity(",
    );
    assert_source_contains(
        "src-tauri/src/session_ingest_cli.rs",
        "external_provider: input.external_provider,",
    );
}

fn assert_source_contains(relative: &str, needle: &str) {
    let source = read_source(relative);
    assert_source_has_needle(relative, &source, needle);
}

fn read_source(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn assert_source_has_needle(relative: &str, source: &str, needle: &str) {
    assert!(
        source.contains(needle),
        "{relative} must keep S7a production external-provider identity wiring"
    );
}

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
