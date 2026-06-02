use agent_runner_lib::{AppState, commands::models::reload_models_inner};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use std::collections::HashMap;

const RELOAD_SOURCE: &str = include_str!("../src/commands/models/reload.rs");

fn model(name: &str, command: &str) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::new(command.to_string(), vec![])],
        inputs: vec![],
        provider: None,
    }
}

#[test]
fn reload_models_inner_replaces_cache_from_models_dir_and_refreshes_settings() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(
        models_dir.join("fresh.toml"),
        r#"
[[providers]]
name = "codex"
args = []
"#,
    )
    .unwrap();

    let state = AppState::test_default(
        models_dir,
        HashMap::from([("stale".to_string(), model("stale", "claude"))]),
    );

    reload_models_inner(&state).expect("reload should refresh model cache");

    let models = state.models.lock().unwrap();
    assert!(models.contains_key("fresh"));
    assert!(!models.contains_key("stale"));
}

#[test]
fn reload_models_inner_falls_back_to_empty_cache_on_load_error() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(models_dir.join("invalid.toml"), "not valid toml =").unwrap();

    let state = AppState::test_default(
        models_dir,
        HashMap::from([("stale".to_string(), model("stale", "claude"))]),
    );

    reload_models_inner(&state).expect("reload should tolerate model load errors");

    let models = state.models.lock().unwrap();
    assert!(models.is_empty());
}

#[test]
fn reload_models_inner_preserves_reload_sequence_contracts() {
    assert!(RELOAD_SOURCE.contains(
        "app_paths::load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config)"
    ));
    assert!(
        RELOAD_SOURCE.contains(
            "config::load_models(&state.models_dir, Some(&providers)).unwrap_or_default()"
        )
    );

    let lock = RELOAD_SOURCE
        .find("let mut models = state.models.lock().map_err(|e| e.to_string())?;")
        .expect("reload should lock the model cache");
    let replace = RELOAD_SOURCE
        .find("*models = fresh;")
        .expect("reload should replace the model cache");
    let drop_lock = RELOAD_SOURCE
        .find("drop(models);")
        .expect("reload should drop the model-cache lock before refresh");
    let refresh = RELOAD_SOURCE
        .find("provider_settings::refresh_provider_settings_host(state)?;")
        .expect("reload should refresh provider settings");

    assert!(lock < replace);
    assert!(replace < drop_lock);
    assert!(drop_lock < refresh);
}
