#[path = "../src/wiring.rs"]
mod wiring;

use oulipoly_runtime::services::{
    SessionExportServicePort, SessionLockServicePort, SessionReplaceServicePort, TraceServicePort,
};
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

#[test]
fn production_wiring_builds_age37_services() {
    let dir = tempfile::tempdir().unwrap();
    let paths = runtime_paths(dir.path());

    let services = AgentRuntimeServices::production(paths).unwrap();

    let _: Arc<dyn TraceServicePort> = Arc::clone(&services.trace_service);
    let _: Arc<dyn SessionExportServicePort> = Arc::clone(&services.session_export_service);
    let _: Arc<dyn SessionReplaceServicePort> = Arc::clone(&services.session_replace_service);
    let _: Arc<dyn SessionLockServicePort> = Arc::clone(&services.session_lock_service);
    assert!(Arc::strong_count(&services.routing_service) >= 1);
    assert!(Arc::strong_count(&services.invocation_lifecycle_service) >= 1);
    assert!(Arc::strong_count(&services.provider_registry) >= 1);
    assert!(
        services
            .provider_registry
            .configured_artifact_keys()
            .is_empty(),
        "provider registry construction must not discover ambient providers"
    );
}
