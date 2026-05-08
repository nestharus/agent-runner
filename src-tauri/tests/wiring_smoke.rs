#[path = "../src/wiring.rs"]
mod wiring;

use oulipoly_config::repositories::{
    AgentConfigRepository, AppConfigRepository, ModelConfigRepository, ProvidersConfigRepository,
    SessionsConfigRepository,
};
use oulipoly_runtime::ports::{Clock, ProcessRunner, ProcessSpec, UuidGenerator};
use oulipoly_state::InvocationStart;
use oulipoly_state::repositories::{InvocationRepository, StateDbOpener};
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
fn production_wiring_builds_foundation_repositories_and_ports() {
    let dir = tempfile::tempdir().unwrap();
    let paths = runtime_paths(dir.path());

    let services = AgentRuntimeServices::production(paths.clone()).unwrap();

    let db = services
        .state_db_opener
        .open_at(&paths.state_db_path)
        .unwrap();
    let record = <_ as InvocationRepository>::start_invocation(
        &db,
        InvocationStart {
            invocation_uuid: services.uuid_gen.new_v4().to_string(),
            model_name: "claude~high".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        },
    )
    .unwrap();
    assert_eq!(record.model_name, "claude~high");

    let app_config = services
        .app_config
        .load_app_config(&paths.config_root.join("config.toml"))
        .unwrap();
    assert!(app_config.default_provider.is_none());
    assert!(
        services
            .agent_config
            .load_agents(&paths.agents_dir)
            .unwrap()
            .is_empty()
    );
    assert!(
        services
            .model_config
            .load_models(&paths.models_dir)
            .unwrap()
            .is_empty()
    );
    assert!(
        services
            .providers_config
            .load_providers(&paths.config_root.join("providers.toml"))
            .unwrap()
            .entries
            .is_empty()
    );
    assert!(
        services
            .sessions_config
            .load_sessions(&paths.config_root.join("sessions.toml"))
            .unwrap()
            .entries
            .is_empty()
    );

    let _ = services.clock.now();
    let first_uuid = services.uuid_gen.new_v4();
    let second_uuid = services.uuid_gen.new_v4();
    assert_ne!(first_uuid, second_uuid);

    let process = services
        .process_runner
        .run(ProcessSpec {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            current_dir: Some(paths.working_dir),
            env: Vec::new(),
            stdin: None,
        })
        .unwrap();
    assert_eq!(process.exit_code, 0);
    assert_eq!(process.stdout, b"hello\n");

    assert!(std::sync::Arc::strong_count(&services.stdout_sink) >= 1);
    assert!(std::sync::Arc::strong_count(&services.stderr_sink) >= 1);
}
