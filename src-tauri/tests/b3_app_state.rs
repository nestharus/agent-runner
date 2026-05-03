#![cfg(unix)]

use agent_runner_lib::config::{
    FilesystemModelConfigRepository, ModelConfig, ModelConfigRepository, PromptMode, ProviderConfig,
};
use agent_runner_lib::state::StateDb;
use agent_runner_lib::{AppState, configure_tauri_app};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn app_with_models(
    models_dir: PathBuf,
    models: HashMap<String, ModelConfig>,
) -> tauri::App<tauri::test::MockRuntime> {
    configure_tauri_app(
        tauri::test::mock_builder(),
        AppState {
            models: Mutex::new(models),
            models_dir,
            setup_input_tx: Mutex::new(None),
            quota_in_flight: agent_runner_lib::quota::InFlight::new(),
        },
    )
    .build(tauri::test::mock_context(tauri::test::noop_assets()))
    .unwrap()
}

fn webview(
    app: &tauri::App<tauri::test::MockRuntime>,
) -> tauri::WebviewWindow<tauri::test::MockRuntime> {
    tauri::WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .unwrap()
}

fn invoke_json(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    tauri::test::get_ipc_response(
        webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().unwrap())
}

fn model(name: &str, providers: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: providers
            .iter()
            .map(|provider| ProviderConfig::model_provider(*provider, vec![]))
            .collect(),
        inputs: vec![],
    }
}

fn executable_model(name: &str, provider_name: &str, command: &Path) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig {
            name: provider_name.to_string(),
            command: command.to_string_lossy().to_string(),
            args: vec![],
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
        }],
        inputs: vec![],
    }
}

fn write_model(models_dir: &Path, model: &ModelConfig) {
    std::fs::create_dir_all(models_dir).unwrap();
    std::fs::write(
        models_dir.join(format!("{}.toml", model.name)),
        model.to_toml(),
    )
    .unwrap();
}

fn load_models(models_dir: &Path) -> HashMap<String, ModelConfig> {
    FilesystemModelConfigRepository::new(models_dir.to_path_buf())
        .load_models()
        .unwrap()
}

fn write_providers_toml(models_dir: &Path, body: &str) {
    let config_dir = models_dir.parent().unwrap();
    std::fs::create_dir_all(config_dir).unwrap();
    std::fs::write(config_dir.join("providers.toml"), body).unwrap();
}

fn state_db(models_dir: &Path) -> StateDb {
    StateDb::open(&models_dir.parent().unwrap().join("state.db")).unwrap()
}

fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn prepend_path<'a>(bin_dir: &Path, guard: &'a MutexGuard<'a, ()>) -> PathEnvGuard<'a> {
    let prior = std::env::var_os("PATH");
    let mut paths = vec![bin_dir.to_path_buf()];
    if let Some(prior) = prior.as_ref() {
        paths.extend(std::env::split_paths(prior));
    }
    let joined = std::env::join_paths(paths).unwrap();
    unsafe {
        std::env::set_var("PATH", joined);
    }
    PathEnvGuard {
        prior,
        _guard: guard,
    }
}

struct PathEnvGuard<'a> {
    prior: Option<std::ffi::OsString>,
    _guard: &'a MutexGuard<'a, ()>,
}

impl Drop for PathEnvGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            match self.prior.as_ref() {
                Some(prior) => std::env::set_var("PATH", prior),
                None => std::env::remove_var("PATH"),
            }
        }
    }
}

/// Risk: T8 (AppState model command is covered through the real Tauri handler)
/// Source: proposal §8 T8; B3 contract §3 AppState command rules
/// Level: component
#[test]
fn app_state_list_models_invokes_real_tauri_command_handler() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("config").join("models");
    write_model(&models_dir, &model("alpha", &["claude"]));
    let app = app_with_models(models_dir.clone(), load_models(&models_dir));
    let webview = webview(&app);

    let models = invoke_json(&webview, "list_models", json!({})).unwrap();

    assert_eq!(models.as_array().unwrap().len(), 1);
    assert_eq!(models[0]["name"], "alpha");
}

/// Risk: T14 (refresh_quotas command uses the production Tauri command path)
/// Source: proposal §8 T14; B3 contract §3 AppState command rules
/// Level: component
#[test]
fn app_state_refresh_quotas_real_handler_persists_updated_status() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("config").join("models");
    let model = model("alpha", &["claude", "codex"]);
    write_model(&models_dir, &model);
    write_providers_toml(
        &models_dir,
        r#"[claude]
quota_script = "printf '%s\n' '{\"windows\":[{\"used_percent\":12,\"resets_at\":\"2099-01-01T00:00:00Z\"}]}'"
"#,
    );
    let app = app_with_models(models_dir.clone(), load_models(&models_dir));
    let webview = webview(&app);

    let response = invoke_json(&webview, "refresh_quotas", json!({})).unwrap();

    let claude = response
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["provider_name"] == "claude")
        .unwrap();
    assert_eq!(claude["status"], "updated");
    assert_eq!(
        state_db(&models_dir).get_windows("claude").unwrap().len(),
        1
    );
}

/// Risk: T15 (test_model command uses production balancer/executor/quota path)
/// Source: proposal §8 T15; B3 contract §3 AppState command rules
/// Level: component
#[test]
fn app_state_test_model_real_handler_marks_selected_provider_exhausted_on_quota_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("config").join("models");
    let bin_dir = dir.path().join("bin");
    let cli = write_executable(
        &bin_dir,
        "quota-cli",
        "#!/bin/sh\nprintf '%s\\n' 'quota exceeded' >&2\nexit 1\n",
    );
    let mut models = HashMap::new();
    models.insert(
        "alpha".to_string(),
        executable_model("alpha", "claude", &cli),
    );
    let app = app_with_models(models_dir.clone(), models);
    let webview = webview(&app);

    let response = invoke_json(&webview, "test_model", json!({ "name": "alpha" })).unwrap();

    assert_eq!(response["success"], false);
    assert!(
        state_db(&models_dir)
            .get_quota("claude")
            .unwrap()
            .and_then(|quota| quota.exhausted_at)
            .is_some()
    );
}

/// Risk: T16 (sync_provider command upserts the real detection result)
/// Source: proposal §8 T16; B3 contract §3 AppState command rules
/// Level: component
#[test]
fn app_state_sync_provider_real_handler_persists_detected_cli() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("config").join("models");
    let bin_dir = dir.path().join("bin");
    write_executable(
        &bin_dir,
        "fakecli",
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '%s\\n' 'fakecli 1.2.3'; fi\n",
    );
    let _path_guard = prepend_path(&bin_dir, &_env_guard);
    let app = app_with_models(models_dir.clone(), HashMap::new());
    let webview = webview(&app);

    let response = invoke_json(&webview, "sync_provider", json!({ "cliName": "fakecli" })).unwrap();

    assert_eq!(response["cli_name"], "fakecli");
    assert_eq!(response["installed"], true);
    assert_eq!(response["version"], "fakecli 1.2.3");
    assert!(
        state_db(&models_dir)
            .get_cli_provider("fakecli")
            .unwrap()
            .is_some()
    );
}

/// Risk: T17 (discover_models_cmd keeps stale deletion conditional in AppState wiring)
/// Source: proposal §8 T17; B3 contract §5 discover_models_cmd hookpoint
/// Level: component
#[test]
fn app_state_discover_models_real_handler_deletes_stale_rows_after_non_empty_discovery() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("config").join("models");
    let bin_dir = dir.path().join("bin");
    write_executable(
        &bin_dir,
        "claude",
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' 'claude 2.0.0'
elif [ "$1" = "models" ] && [ "$2" = "list" ]; then
  printf '%s\n' 'claude-3-opus'
else
  exit 1
fi
"#,
    );
    let _path_guard = prepend_path(&bin_dir, &_env_guard);
    state_db(&models_dir)
        .upsert_discovered_model(&agent_runner_lib::state::DiscoveredModel {
            canonical_name: "old-model".to_string(),
            provider: "claude".to_string(),
            discovered_at: "2026-05-02T00:00:00Z".to_string(),
            cli_version: "old-version".to_string(),
        })
        .unwrap();
    let app = app_with_models(models_dir.clone(), HashMap::new());
    let webview = webview(&app);

    let response = invoke_json(
        &webview,
        "discover_models_cmd",
        json!({ "cliName": "claude" }),
    )
    .unwrap();

    assert_eq!(response.as_array().unwrap().len(), 1);
    let db = state_db(&models_dir);
    let discovered = db.list_discovered_models(Some("claude")).unwrap();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].canonical_name, "claude-3-opus");
}
