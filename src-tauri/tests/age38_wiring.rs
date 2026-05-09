#[path = "../src/wiring.rs"]
mod wiring;

use oulipoly_runtime::services::{DiagnosticsServicePort, ExecutorServicePort, QuotaServicePort};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use wiring::{AgentRuntimeServices, RuntimePaths};

fn runtime_paths(root: &std::path::Path) -> RuntimePaths {
    RuntimePaths {
        config_root: root.join("config"),
        models_dir: root.join("config").join("models"),
        agents_dir: root.join("config").join("agents"),
        data_root: root.join("data"),
        state_db_path: root.join("data").join("state.db"),
        lock_dir: root.join("data").join("locks"),
        working_dir: root.join("work"),
    }
}

fn source_block_after(source: &str, needle: &str) -> String {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"));
    let open = source[start..]
        .find('{')
        .map(|idx| start + idx)
        .unwrap_or_else(|| panic!("missing opening brace for {needle}"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[open..=open + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("missing closing brace for {needle}");
}

#[test]
fn age38_wiring_declares_gui_cutover_service_ports() {
    fn accept_executor_service(_: Option<Arc<dyn ExecutorServicePort>>) {}
    fn accept_quota_service(_: Option<Arc<dyn QuotaServicePort>>) {}
    fn accept_diagnostics_service(_: Option<Arc<dyn DiagnosticsServicePort>>) {}

    accept_executor_service(None);
    accept_quota_service(None);
    accept_diagnostics_service(None);

    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/wiring.rs");
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));

    assert!(
        source.contains("pub executor_service: Arc<dyn ExecutorServicePort>"),
        "AgentRuntimeServices must expose executor_service as Arc<dyn ExecutorServicePort>"
    );
    assert!(
        source.contains("pub quota_service: Arc<dyn QuotaServicePort>"),
        "AgentRuntimeServices must expose quota_service as Arc<dyn QuotaServicePort>"
    );
    assert!(
        source.contains("pub diagnostics_service: Arc<dyn DiagnosticsServicePort>"),
        "AgentRuntimeServices must expose diagnostics_service as Arc<dyn DiagnosticsServicePort>"
    );
}

#[test]
fn age38_wiring_constructors_initialize_gui_cutover_service_ports() {
    let dir = tempfile::tempdir().unwrap();
    let _cli_services = AgentRuntimeServices::cli_defaults();
    let _production_services =
        AgentRuntimeServices::production(runtime_paths(dir.path())).expect("production services");

    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/wiring.rs");
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));

    for constructor in [
        "pub fn cli_defaults() -> Self",
        "pub fn production(paths: RuntimePaths)",
    ] {
        let body = source_block_after(&source, constructor);
        assert!(
            body.contains("executor_service: Arc::new(RuntimeExecutorService::new())")
                || body.contains("executor_service: Arc::new(RuntimeExecutorService)"),
            "{constructor} must initialize RuntimeExecutorService"
        );
        assert!(
            body.contains("quota_service: Arc::new(RuntimeQuotaService::new())")
                || body.contains("quota_service: Arc::new(RuntimeQuotaService)"),
            "{constructor} must initialize RuntimeQuotaService"
        );
        assert!(
            body.contains("diagnostics_service: Arc::new(RuntimeDiagnosticsService::new())")
                || body.contains("diagnostics_service: Arc::new(RuntimeDiagnosticsService)"),
            "{constructor} must initialize RuntimeDiagnosticsService"
        );
    }
}
