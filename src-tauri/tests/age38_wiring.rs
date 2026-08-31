//! ## Declared roles
//!
//! `accessor`, `validator`, `orchestration`, `mapper`, `parser`
//!
#[path = "../src/wiring.rs"]
mod wiring;

use oulipoly_runtime::services::{DiagnosticsServicePort, ExecutorServicePort, QuotaServicePort};
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use wiring::{AgentRuntimeServices, RuntimePaths};

struct RuntimePathEnvGuard {
    previous_data_dir: Option<OsString>,
    previous_config_home: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl RuntimePathEnvGuard {
    fn set(root: &Path) -> Self {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let lock = ENV_LOCK.lock().unwrap();
        let previous_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        let previous_config_home = std::env::var_os("OULIPOLY_CONFIG_HOME");
        // SAFETY: this test serializes both environment mutations through ENV_LOCK.
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", root.join("data"));
            std::env::set_var("OULIPOLY_CONFIG_HOME", root.join("config"));
        }
        Self {
            previous_data_dir,
            previous_config_home,
            _lock: lock,
        }
    }
}

impl Drop for RuntimePathEnvGuard {
    fn drop(&mut self) {
        // SAFETY: the guard still owns ENV_LOCK while restoring both values.
        unsafe {
            restore_env("OULIPOLY_DATA_DIR", self.previous_data_dir.take());
            restore_env("OULIPOLY_CONFIG_HOME", self.previous_config_home.take());
        }
    }
}

unsafe fn restore_env(name: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}

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
    let open = source_block_open_index(source, needle);
    let close = source_block_close_index(source, open, needle);
    source[open..=open + close].to_string()
}

fn source_block_open_index(source: &str, needle: &str) -> usize {
    let start = source_marker_index(source, needle);
    source[start..]
        .find('{')
        .map(|idx| start + idx)
        .unwrap_or_else(|| panic!("missing opening brace for {needle}"))
}

fn source_marker_index(source: &str, needle: &str) -> usize {
    source
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"))
}

fn source_block_close_index(source: &str, open: usize, needle: &str) -> usize {
    let mut depth = 0usize;
    source[open..]
        .char_indices()
        .find_map(|(offset, ch)| {
            depth = next_source_block_depth(depth, ch);
            source_block_close_offset(depth, ch, offset)
        })
        .unwrap_or_else(|| panic!("missing closing brace for {needle}"))
}

fn next_source_block_depth(depth: usize, ch: char) -> usize {
    match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    }
}

fn source_block_close_offset(depth: usize, ch: char, offset: usize) -> Option<usize> {
    (ch == '}' && depth == 0).then_some(offset)
}

fn accept_gui_cutover_service_ports() {
    fn accept_executor_service(_: Option<Arc<dyn ExecutorServicePort>>) {}
    fn accept_quota_service(_: Option<Arc<dyn QuotaServicePort>>) {}
    fn accept_diagnostics_service(_: Option<Arc<dyn DiagnosticsServicePort>>) {}

    accept_executor_service(None);
    accept_quota_service(None);
    accept_diagnostics_service(None);
}

fn wiring_source() -> String {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/wiring.rs");
    fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()))
}

fn assert_service_port_fields(source: &str) {
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
    assert!(
        source.contains("pub provider_registry: Arc<") && source.contains("ProviderRegistry"),
        "AgentRuntimeServices must expose a neutral provider_registry field"
    );
}

fn construct_runtime_services() {
    let dir = tempfile::tempdir().unwrap();
    let _env = RuntimePathEnvGuard::set(dir.path());
    let _cli_services = AgentRuntimeServices::cli_defaults().expect("CLI services");
    let _production_services =
        AgentRuntimeServices::production(runtime_paths(dir.path())).expect("production services");
}

fn constructor_markers() -> [&'static str; 2] {
    [
        "pub fn cli_defaults() -> Result<Self, String>",
        "pub fn production(paths: RuntimePaths)",
    ]
}

fn assert_constructor_body_initializes_runtime_services(constructor: &str, body: &str) {
    assert!(
        body.contains("executor_service: Arc::new(RuntimeExecutorService::new())")
            || body.contains("executor_service: Arc::new(RuntimeExecutorService)")
            || body.contains(
                "executor_service: Arc::new(RuntimeExecutorService::with_registry_handle("
            ),
        "{constructor} must initialize RuntimeExecutorService"
    );
    assert!(
        body.contains("quota_service: Arc::new(RuntimeQuotaService::new())")
            || body.contains("quota_service: Arc::new(RuntimeQuotaService)")
            || body.contains("quota_service: Arc::new(RuntimeQuotaService::with_registry_handle("),
        "{constructor} must initialize RuntimeQuotaService"
    );
    assert!(
        body.contains("diagnostics_service: Arc::new(RuntimeDiagnosticsService::new())")
            || body.contains("diagnostics_service: Arc::new(RuntimeDiagnosticsService)")
            || body.contains(
                "diagnostics_service: Arc::new(RuntimeDiagnosticsService::with_registry_handle(",
            ),
        "{constructor} must initialize RuntimeDiagnosticsService"
    );
}

fn assert_constructor_body_initializes_provider_registry(constructor: &str, body: &str) {
    assert!(
        body.contains("let provider_registry =")
            && body.contains("Arc::new(")
            && body.contains("let provider_registry_handle = ProviderRegistryHandle::new(")
            && (body.contains("provider_registry_handle,")
                || body.contains("provider_registry_handle: provider_registry_handle.clone(),")),
        "{constructor} must initialize the provider registry and shared handle"
    );
}

fn assert_constructor_body_avoids_provider_invocation(constructor: &str, body: &str) {
    assert!(
        !body.contains("describe_model_provider")
            && !body.contains(".describe(")
            && !body.contains("invoke_typed")
            && !body.contains("ProviderClient::new"),
        "{constructor} must not resolve artifacts, call describe, or spawn provider processes"
    );
}

fn assert_constructor_body(source: &str, constructor: &str) {
    let body = source_block_after(source, constructor);
    assert_constructor_body_initializes_runtime_services(constructor, &body);
    assert_constructor_body_initializes_provider_registry(constructor, &body);
    assert_constructor_body_avoids_provider_invocation(constructor, &body);
}

#[test]
fn age38_wiring_declares_gui_cutover_service_ports() {
    accept_gui_cutover_service_ports();
    let source = wiring_source();
    assert_service_port_fields(&source);
}

#[test]
fn age38_wiring_constructors_initialize_gui_cutover_service_ports() {
    construct_runtime_services();
    let source = wiring_source();
    for constructor in constructor_markers() {
        assert_constructor_body(&source, constructor);
    }
}
