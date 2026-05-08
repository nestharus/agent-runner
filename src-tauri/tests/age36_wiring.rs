#[path = "../src/wiring.rs"]
mod wiring;

use oulipoly_runtime::services::{
    MigrationServicePort, ResumeServicePort, SessionLifecycleServicePort,
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
fn agent_runtime_services_production_wires_age_36_services() {
    fn accept_resume_service(_: Arc<dyn ResumeServicePort>) {}
    fn accept_session_lifecycle_service(_: Arc<dyn SessionLifecycleServicePort>) {}
    fn accept_migration_service(_: Arc<dyn MigrationServicePort>) {}

    let dir = tempfile::tempdir().unwrap();
    let services = AgentRuntimeServices::production(runtime_paths(dir.path())).unwrap();

    accept_resume_service(services.resume_service.clone());
    accept_session_lifecycle_service(services.session_lifecycle_service.clone());
    accept_migration_service(services.migration_service.clone());
}
