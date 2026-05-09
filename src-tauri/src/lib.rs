pub mod setup;

use oulipoly_config as config;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_runtime::services::{
    ProductionRoutingService, RoutingServicePort, RoutingServiceRequest,
};
use oulipoly_runtime::{diagnostics, discovery, executor, quota};
use oulipoly_setup as setup_core;
use oulipoly_setup::actions::{SetupEvent, UserResponse};
use oulipoly_state as state;
use oulipoly_state::StateDb;
use oulipoly_state::{AccountRecord, AuthMethod, AuthStatus, CliProviderRecord};
use oulipoly_state::{DiscoveredModel, ModelParameter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

pub type AppConfig = oulipoly_config::app::AppConfig;

pub fn load_app_config() -> AppConfig {
    let config_dir = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));

    let config_path = config_dir.join("config.toml");
    oulipoly_config::app::AppConfig::load(&config_path).unwrap_or_default()
}

#[derive(Serialize, Clone)]
pub struct TestModelResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Serialize)]
pub struct ModelSummary {
    pub name: String,
    pub prompt_mode: config::PromptMode,
    pub provider_count: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct PoolSummary {
    pub commands: Vec<String>,
    pub model_count: usize,
    pub model_names: Vec<String>,
}

fn derive_pools(models: &HashMap<String, config::ModelConfig>) -> Vec<PoolSummary> {
    let mut groups: HashMap<Vec<String>, Vec<String>> = HashMap::new();

    for model in models.values() {
        let mut cmds: Vec<String> = model.providers.iter().map(|p| p.name.clone()).collect();
        cmds.sort();
        cmds.dedup();
        groups.entry(cmds).or_default().push(model.name.clone());
    }

    let mut pools: Vec<PoolSummary> = groups
        .into_iter()
        .map(|(commands, mut model_names)| {
            model_names.sort();
            PoolSummary {
                model_count: model_names.len(),
                commands,
                model_names,
            }
        })
        .collect();

    pools.sort_by(|a, b| a.commands.cmp(&b.commands));
    pools
}

pub struct AppState {
    pub models: Mutex<HashMap<String, config::ModelConfig>>,
    pub models_dir: PathBuf,
    pub setup_input_tx: Mutex<Option<mpsc::Sender<UserResponse>>>,
    /// Tracks quota-refresh calls in flight so duplicate callers collapse.
    pub quota_in_flight: quota::InFlight,
}

fn load_providers_for_models_dir(models_dir: &Path) -> config::ProvidersConfig {
    let config_root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    config::ProvidersConfig::load(&config_root.join("providers.toml")).unwrap_or_default()
}

impl AppState {
    fn db_path(&self) -> PathBuf {
        self.models_dir
            .parent()
            .unwrap_or(&self.models_dir)
            .join("state.db")
    }
}

#[tauri::command]
fn check_setup_needed(state: tauri::State<AppState>) -> Result<bool, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    if models.is_empty() {
        return Ok(true);
    }
    // Check if claude CLI is available
    let output = std::process::Command::new("which").arg("claude").output();
    match output {
        Ok(o) if o.status.success() => Ok(false),
        _ => Ok(true),
    }
}

#[tauri::command]
async fn start_setup(
    state: tauri::State<'_, AppState>,
    on_event: Channel<SetupEvent>,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel::<UserResponse>(16);

    {
        let mut guard = state.setup_input_tx.lock().map_err(|e| e.to_string())?;
        *guard = Some(tx);
    }

    let sid = session_id.clone();
    let db_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");

    tauri::async_runtime::spawn(async move {
        let memory = match setup_core::memory::MemoryGraph::open(&db_path) {
            Ok(m) => m,
            Err(e) => {
                let _ = on_event.send(SetupEvent::Error {
                    message: format!("Failed to open memory store: {e}"),
                    recoverable: false,
                });
                return;
            }
        };

        let flow = setup::flow::SetupFlow::new(on_event, rx, memory, sid);
        flow.run().await;
    });

    Ok(session_id)
}

#[tauri::command]
fn setup_respond(state: tauri::State<AppState>, response: UserResponse) -> Result<(), String> {
    let guard = state.setup_input_tx.lock().map_err(|e| e.to_string())?;
    if let Some(ref tx) = *guard {
        tx.blocking_send(response)
            .map_err(|e| format!("Failed to send response: {e}"))
    } else {
        Err("No active setup session".to_string())
    }
}

#[tauri::command]
fn cancel_setup(state: tauri::State<AppState>) -> Result<(), String> {
    let mut guard = state.setup_input_tx.lock().map_err(|e| e.to_string())?;
    *guard = None; // Dropping sender closes channel, wakes flow
    Ok(())
}

#[tauri::command]
async fn start_cli_setup(
    state: tauri::State<'_, AppState>,
    cli_name: String,
    on_event: Channel<SetupEvent>,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel::<UserResponse>(16);

    {
        let mut guard = state.setup_input_tx.lock().map_err(|e| e.to_string())?;
        *guard = Some(tx);
    }

    let sid = session_id.clone();
    let db_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");
    let cli = cli_name.clone();

    tauri::async_runtime::spawn(async move {
        let memory = match setup_core::memory::MemoryGraph::open(&db_path) {
            Ok(m) => m,
            Err(e) => {
                let _ = on_event.send(SetupEvent::Error {
                    message: format!("Failed to open memory store: {e}"),
                    recoverable: false,
                });
                return;
            }
        };

        let flow = setup::flow::SetupFlow::new(on_event, rx, memory, sid);
        flow.run_for_cli(&cli).await;
    });

    Ok(session_id)
}

#[tauri::command]
fn reload_models(state: tauri::State<AppState>) -> Result<(), String> {
    let providers = load_providers_for_models_dir(&state.models_dir);
    let fresh = config::load_models(&state.models_dir, Some(&providers)).unwrap_or_default();
    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    *models = fresh;
    Ok(())
}

#[tauri::command]
fn detect_clis() -> Result<setup_core::detection::DetectionReport, String> {
    Ok(setup_core::detection::detect_all())
}

#[tauri::command]
fn get_memory_graph(
    state: tauri::State<AppState>,
) -> Result<setup_core::memory::MemorySnapshot, String> {
    let db_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");
    let graph = setup_core::memory::MemoryGraph::open(&db_path)?;
    graph.snapshot()
}

#[tauri::command]
fn list_models(state: tauri::State<AppState>) -> Result<Vec<ModelSummary>, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    let mut summaries: Vec<ModelSummary> = models
        .values()
        .map(|m| ModelSummary {
            name: m.name.clone(),
            prompt_mode: m.prompt_mode,
            provider_count: m.providers.len(),
        })
        .collect();
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summaries)
}

#[tauri::command]
fn get_model(state: tauri::State<AppState>, name: String) -> Result<ModelConfig, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    models
        .get(&name)
        .cloned()
        .ok_or_else(|| format!("Model '{}' not found", name))
}

#[tauri::command]
fn save_model(state: tauri::State<AppState>, model: ModelConfig) -> Result<(), String> {
    save_model_inner(&state, model)
}

fn save_model_inner(state: &AppState, model: ModelConfig) -> Result<(), String> {
    if model.name.is_empty() {
        return Err("Model name cannot be empty".to_string());
    }
    if model.providers.is_empty() {
        return Err("Model must have at least one provider".to_string());
    }
    for (i, p) in model.providers.iter().enumerate() {
        if p.name.is_empty() {
            return Err(format!("Provider {} has empty name", i + 1));
        }
    }
    let providers = load_providers_for_models_dir(&state.models_dir);
    let toml_content = config::render_validated_model_toml(&model, Some(&providers))?;
    let path = state.models_dir.join(format!("{}.toml", model.name));

    std::fs::create_dir_all(&state.models_dir)
        .map_err(|e| format!("Failed to create models directory: {e}"))?;
    std::fs::write(&path, &toml_content).map_err(|e| format!("Failed to write model file: {e}"))?;

    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    models.insert(model.name.clone(), model);
    Ok(())
}

#[tauri::command]
fn delete_model(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    let path = state.models_dir.join(format!("{}.toml", name));
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete model file: {e}"))?;
    }
    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    models.remove(&name);
    Ok(())
}

#[tauri::command]
fn list_pools(state: tauri::State<AppState>) -> Result<Vec<PoolSummary>, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    Ok(derive_pools(&models))
}

#[derive(Serialize)]
pub struct QuotaRefreshWindow {
    pub used_percent: f64,
    pub resets_at: String,
}

#[derive(Serialize)]
pub struct QuotaRefreshEntry {
    pub provider_name: String,
    pub status: String,
    pub windows: Vec<QuotaRefreshWindow>,
    pub message: Option<String>,
}

/// Refresh quotas for every distinct provider that participates in at least
/// one multi-provider model. Single-provider models are skipped since there's
/// no load-balancing decision to inform.
#[tauri::command]
async fn refresh_quotas(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<QuotaRefreshEntry>, String> {
    refresh_quotas_inner(&state)
}

fn refresh_quotas_inner(state: &AppState) -> Result<Vec<QuotaRefreshEntry>, String> {
    // Collect the set of provider names that can actually benefit from a
    // quota refresh — i.e. names that appear in any model with >1 provider.
    let candidates: Vec<String> = {
        let models = state.models.lock().map_err(|e| e.to_string())?;
        let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for m in models.values() {
            if m.providers.len() > 1 {
                for p in &m.providers {
                    set.insert(p.name.clone());
                }
            }
        }
        set.into_iter().collect()
    };

    let providers_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("providers.toml");
    let providers_cfg = config::ProvidersConfig::load(&providers_path).unwrap_or_default();

    let db_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");

    let db = state::StateDb::open(&db_path).map_err(|e| format!("Failed to open state DB: {e}"))?;
    let in_flight = &state.quota_in_flight;
    let mut results = Vec::with_capacity(candidates.len());

    for provider_name in candidates {
        if !quota::is_stale(&db, &provider_name) {
            results.push(QuotaRefreshEntry {
                provider_name,
                status: "fresh".into(),
                windows: vec![],
                message: None,
            });
            continue;
        }

        let outcome = quota::refresh_provider(&provider_name, &providers_cfg, in_flight, &db);
        results.push(match outcome {
            quota::RefreshOutcome::Updated { windows } => QuotaRefreshEntry {
                provider_name,
                status: "updated".into(),
                windows: windows
                    .into_iter()
                    .map(|w| QuotaRefreshWindow {
                        used_percent: w.used_percent,
                        resets_at: w.resets_at.to_rfc3339(),
                    })
                    .collect(),
                message: None,
            },
            quota::RefreshOutcome::NoScript => QuotaRefreshEntry {
                provider_name,
                status: "no_script".into(),
                windows: vec![],
                message: None,
            },
            quota::RefreshOutcome::AlreadyInFlight => QuotaRefreshEntry {
                provider_name,
                status: "in_flight".into(),
                windows: vec![],
                message: None,
            },
            quota::RefreshOutcome::Failed(msg) => QuotaRefreshEntry {
                provider_name,
                status: "failed".into(),
                windows: vec![],
                message: Some(msg),
            },
        });
    }

    Ok(results)
}

#[tauri::command]
fn update_pool(
    state: tauri::State<AppState>,
    original_commands: Vec<String>,
    new_commands: Vec<String>,
) -> Result<(), String> {
    update_pool_inner(&state, original_commands, new_commands)
}

fn update_pool_inner(
    state: &AppState,
    original_commands: Vec<String>,
    new_commands: Vec<String>,
) -> Result<(), String> {
    if new_commands.is_empty() {
        return Err("Pool must have at least one command".to_string());
    }

    let mut orig_sorted = original_commands.clone();
    orig_sorted.sort();
    orig_sorted.dedup();

    let mut new_sorted = new_commands.clone();
    new_sorted.sort();
    new_sorted.dedup();

    let providers = load_providers_for_models_dir(&state.models_dir);
    let mut models_guard = state.models.lock().map_err(|e| e.to_string())?;

    // Find models matching the original command set (using provider names)
    let matching_names: Vec<String> = models_guard
        .values()
        .filter(|m| {
            let mut cmds: Vec<String> = m.providers.iter().map(|p| p.name.clone()).collect();
            cmds.sort();
            cmds.dedup();
            cmds == orig_sorted
        })
        .map(|m| m.name.clone())
        .collect();

    if matching_names.is_empty() {
        return Err("No models found with the specified command set".to_string());
    }

    let mut updates = Vec::new();

    // Compute added and removed provider names
    let removed: Vec<&String> = orig_sorted
        .iter()
        .filter(|c| !new_sorted.contains(c))
        .collect();
    let added: Vec<&String> = new_sorted
        .iter()
        .filter(|c| !orig_sorted.contains(c))
        .collect();

    for name in &matching_names {
        let mut model = models_guard.get(name).unwrap().clone();

        // Remove providers whose extracted provider name is in the removed set
        model.providers.retain(|p| !removed.contains(&&p.name));

        // Add providers with empty args for new commands
        for cmd in &added {
            model.providers.push(config::ProviderConfig::model_provider(
                (*cmd).clone(),
                vec![],
            ));
        }

        if model.providers.is_empty() {
            return Err(format!("Model '{}' would end up with zero providers", name));
        }

        let toml_content = config::render_validated_model_toml(&model, Some(&providers))?;
        updates.push((name.clone(), model, toml_content));
    }

    for (name, model, toml_content) in updates {
        let path = state.models_dir.join(format!("{}.toml", name));
        std::fs::write(&path, &toml_content)
            .map_err(|e| format!("Failed to write model file for '{}': {e}", name))?;
        models_guard.insert(name, model);
    }

    Ok(())
}

#[tauri::command]
async fn test_model(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<TestModelResult, String> {
    let model = {
        let models = state.models.lock().map_err(|e| e.to_string())?;
        models
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("Model '{}' not found", name))?
    };

    let models_dir = state.models_dir.clone();
    let db_path = models_dir.parent().unwrap_or(&models_dir).join("state.db");

    let result = tauri::async_runtime::spawn_blocking(move || {
        let routing_service = ProductionRoutingService;
        test_model_with_db_path(
            &routing_service,
            model,
            models_dir,
            db_path,
            "Say hello in one sentence.",
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

fn test_model_with_db_path(
    routing_service: &dyn RoutingServicePort,
    model: ModelConfig,
    models_dir: PathBuf,
    db_path: PathBuf,
    prompt: &str,
) -> Result<TestModelResult, String> {
    let db = state::StateDb::open(&db_path).map_err(|e| e.to_string())?;
    let provider_index = routing_service
        .select_route(RoutingServiceRequest {
            model: &model,
            state: &db,
            ctx: None,
        })
        .map_err(|error| error.to_string())?
        .provider_index;
    let providers_path = models_dir
        .parent()
        .unwrap_or(&models_dir)
        .join("providers.toml");
    let providers_cfg = config::ProvidersConfig::load(&providers_path).unwrap_or_default();
    let (provider, prompt_mode) =
        effective_provider_for_model_provider(&model, provider_index, &providers_cfg)?;
    let extra_inputs = HashMap::new();
    let result =
        executor::execute_effective_with_inputs_and_env(executor::cli::EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index,
            prompt_mode,
            prompt,
            working_dir: None,
            extra_inputs: &extra_inputs,
            parent_invocation_env: None,
        })?;
    if result.exit_code != 0 && diagnostics::classify_exhaustion(&result.stderr) {
        db.mark_exhausted(&provider.name)?;
    }
    Ok(TestModelResult {
        success: result.exit_code == 0,
        stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
        stderr: result.stderr,
        exit_code: result.exit_code,
    })
}

#[cfg(test)]
pub(crate) fn test_model_for_test(
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
    name: &str,
) -> Result<TestModelResult, String> {
    let model = models
        .get(name)
        .cloned()
        .ok_or_else(|| format!("Model '{}' not found", name))?;
    let db_path = models_dir.parent().unwrap_or(&models_dir).join("state.db");
    let routing_service = ProductionRoutingService;
    test_model_with_db_path(
        &routing_service,
        model,
        models_dir,
        db_path,
        "Say hello in one sentence.",
    )
}

pub fn effective_provider_for_model_provider(
    model: &ModelConfig,
    provider_index: usize,
    providers_cfg: &ProvidersConfig,
) -> Result<(ProviderConfig, PromptMode), String> {
    let provider = model
        .providers
        .get(provider_index)
        .ok_or_else(|| "provider_index out of range".to_string())?;
    match providers_cfg.effective_provider(provider) {
        Ok(effective) => Ok(effective),
        Err(_) if !provider.command.trim().is_empty() => Ok((provider.clone(), model.prompt_mode)),
        Err(err) => Err(err),
    }
}

// --- Provider & Account commands ---

/// Helper to open the state DB from AppState.
fn open_state_db(state: &AppState) -> Result<StateDb, String> {
    StateDb::open(&state.db_path())
}

#[tauri::command]
fn list_cli_providers(state: tauri::State<AppState>) -> Result<Vec<CliProviderRecord>, String> {
    list_cli_providers_inner(&state)
}

fn list_cli_providers_inner(state: &AppState) -> Result<Vec<CliProviderRecord>, String> {
    let db = open_state_db(state)?;
    db.list_cli_providers()
}

#[tauri::command]
fn get_cli_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    get_cli_provider_inner(&state, cli_name)
}

fn get_cli_provider_inner(state: &AppState, cli_name: String) -> Result<CliProviderRecord, String> {
    let db = open_state_db(state)?;
    db.get_cli_provider(&cli_name)?
        .ok_or_else(|| format!("Provider '{}' not found", cli_name))
}

#[tauri::command]
fn list_accounts(
    state: tauri::State<AppState>,
    provider: Option<String>,
) -> Result<Vec<AccountRecord>, String> {
    list_accounts_inner(&state, provider)
}

fn list_accounts_inner(
    state: &AppState,
    provider: Option<String>,
) -> Result<Vec<AccountRecord>, String> {
    let db = open_state_db(state)?;
    db.list_accounts(provider.as_deref())
}

/// Input payload for adding a new account.
#[derive(Deserialize)]
pub struct AddAccountInput {
    pub id: String,
    pub provider: String,
    pub profile_name: String,
    pub auth_method: AuthMethod,
}

#[tauri::command]
fn add_account(
    state: tauri::State<AppState>,
    account: AddAccountInput,
) -> Result<AccountRecord, String> {
    add_account_inner(&state, account)
}

fn add_account_inner(state: &AppState, account: AddAccountInput) -> Result<AccountRecord, String> {
    if account.id.is_empty() {
        return Err("Account id cannot be empty".to_string());
    }
    if account.provider.is_empty() {
        return Err("Account provider cannot be empty".to_string());
    }
    if account.profile_name.is_empty() {
        return Err("Account profile_name cannot be empty".to_string());
    }

    let db = open_state_db(state)?;

    // Verify the provider exists
    db.get_cli_provider(&account.provider)?
        .ok_or_else(|| format!("Provider '{}' not found", account.provider))?;

    let now = chrono::Utc::now().to_rfc3339();
    let record = AccountRecord {
        id: account.id,
        provider: account.provider,
        profile_name: account.profile_name,
        auth_method: account.auth_method,
        auth_status: AuthStatus::Unknown,
        created_at: now,
    };

    db.insert_account(&record)?;
    Ok(record)
}

#[tauri::command]
fn remove_account(
    state: tauri::State<AppState>,
    id: String,
    provider: String,
) -> Result<bool, String> {
    remove_account_inner(&state, id, provider)
}

fn remove_account_inner(state: &AppState, id: String, provider: String) -> Result<bool, String> {
    let db = open_state_db(state)?;
    db.delete_account(&id, &provider)
}

#[tauri::command]
fn sync_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    // Detect the current state of this CLI using the existing detection module
    let cli_info = setup_core::detection::detect_single_cli(&cli_name);
    let record = sync_provider_record_from_cli_info(&cli_name, cli_info);

    sync_provider_persist_record(&state, &record)?;
    Ok(record)
}

fn sync_provider_display_name(cli_name: &str) -> &str {
    match cli_name {
        "claude" => "Anthropic",
        "codex" => "OpenAI",
        "gemini" => "Google",
        "opencode" => "OpenCode",
        _ => cli_name,
    }
}

fn sync_provider_record_from_cli_info(
    cli_name: &str,
    cli_info: setup_core::detection::CliInfo,
) -> CliProviderRecord {
    let now = chrono::Utc::now().to_rfc3339();
    CliProviderRecord {
        cli_name: cli_info.name,
        display_name: sync_provider_display_name(cli_name).to_string(),
        installed: cli_info.installed,
        version: cli_info.version,
        config_dir: cli_info.config_dir.map(|p| p.to_string_lossy().to_string()),
        last_synced: Some(now),
    }
}

fn sync_provider_persist_record(
    state: &AppState,
    record: &CliProviderRecord,
) -> Result<(), String> {
    let db = open_state_db(state)?;
    db.upsert_cli_provider(record)
}

// --- Discovery commands ---

#[tauri::command]
async fn discover_models_cmd(
    state: tauri::State<'_, AppState>,
    cli_name: String,
) -> Result<Vec<DiscoveredModel>, String> {
    let db_path = state.db_path();

    tauri::async_runtime::spawn_blocking(move || {
        let result = discovery::discover_models(&cli_name)?;
        persist_discovery_result(&db_path, &cli_name, result)
    })
    .await
    .map_err(|e| format!("Discovery task failed: {e}"))?
}

fn persist_discovery_result(
    db_path: &Path,
    cli_name: &str,
    result: discovery::DiscoveryResult,
) -> Result<Vec<DiscoveredModel>, String> {
    let db = StateDb::open(db_path)?;

    // Clean out models from older CLI versions
    if !result.models.is_empty() {
        db.delete_stale_models(cli_name, &result.cli_version)?;
    }

    // Store discovered models
    for model in &result.models {
        db.upsert_discovered_model(model)?;
    }

    // Store discovered parameters
    for (model_name, param) in &result.parameters {
        db.upsert_model_parameter(model_name, cli_name, param)?;
    }

    Ok(result.models)
}

#[tauri::command]
fn list_discovered_models(
    state: tauri::State<AppState>,
    provider: Option<String>,
) -> Result<Vec<DiscoveredModel>, String> {
    list_discovered_models_inner(&state, provider)
}

fn list_discovered_models_inner(
    state: &AppState,
    provider: Option<String>,
) -> Result<Vec<DiscoveredModel>, String> {
    let db = open_state_db(state)?;
    db.list_discovered_models(provider.as_deref())
}

#[tauri::command]
fn get_model_parameters(
    state: tauri::State<AppState>,
    model_name: String,
    provider: String,
) -> Result<Vec<ModelParameter>, String> {
    get_model_parameters_inner(&state, model_name, provider)
}

fn get_model_parameters_inner(
    state: &AppState,
    model_name: String,
    provider: String,
) -> Result<Vec<ModelParameter>, String> {
    let db = open_state_db(state)?;
    db.list_model_parameters(&model_name, &provider)
}

pub fn run_tauri() {
    let models_dir = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"));

    let providers = load_providers_for_models_dir(&models_dir);
    let models = config::load_models(&models_dir, Some(&providers)).unwrap_or_default();

    tauri::Builder::default()
        .manage(AppState {
            models: Mutex::new(models),
            models_dir: models_dir.clone(),
            setup_input_tx: Mutex::new(None),
            quota_in_flight: quota::InFlight::new(),
        })
        .invoke_handler(tauri::generate_handler![
            check_setup_needed,
            start_setup,
            start_cli_setup,
            reload_models,
            setup_respond,
            cancel_setup,
            detect_clis,
            get_memory_graph,
            list_models,
            get_model,
            save_model,
            delete_model,
            list_pools,
            refresh_quotas,
            update_pool,
            test_model,
            list_cli_providers,
            get_cli_provider,
            list_accounts,
            add_account,
            remove_account,
            sync_provider,
            discover_models_cmd,
            list_discovered_models,
            get_model_parameters,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{ModelConfig, PromptMode, ProviderConfig};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    fn write_executable(path: &std::path::Path, body: &str) {
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn make_model(name: &str, commands: &[&str]) -> ModelConfig {
        ModelConfig {
            name: name.to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: commands
                .iter()
                .map(|c| ProviderConfig::new(c.to_string(), vec![]))
                .collect(),
            inputs: vec![],
        }
    }

    fn model_with_provider_args(name: &str, provider_name: &str, args: &[&str]) -> ModelConfig {
        ModelConfig {
            name: name.to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider(
                provider_name,
                args.iter().map(|arg| (*arg).to_string()).collect(),
            )],
            inputs: vec![],
        }
    }

    fn test_state(models_dir: PathBuf, models: HashMap<String, ModelConfig>) -> AppState {
        AppState {
            models: Mutex::new(models),
            models_dir,
            setup_input_tx: Mutex::new(None),
            quota_in_flight: quota::InFlight::new(),
        }
    }

    fn write_codex_providers(root: &Path) {
        std::fs::write(
            root.join("providers.toml"),
            r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
interactive_args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
        )
        .unwrap();
    }

    fn cli_provider(cli_name: &str, display_name: &str) -> CliProviderRecord {
        CliProviderRecord {
            cli_name: cli_name.to_string(),
            display_name: display_name.to_string(),
            installed: true,
            version: Some("1.2.3".to_string()),
            config_dir: Some("/tmp/config".to_string()),
            last_synced: Some("2026-05-08T12:00:00Z".to_string()),
        }
    }

    fn discovered_model(provider: &str, name: &str, cli_version: &str) -> DiscoveredModel {
        DiscoveredModel {
            canonical_name: name.to_string(),
            provider: provider.to_string(),
            discovered_at: "2026-05-08T12:00:00Z".to_string(),
            cli_version: cli_version.to_string(),
        }
    }

    fn model_parameter(name: &str) -> ModelParameter {
        ModelParameter {
            name: name.to_string(),
            display_name: name.to_string(),
            param_type: state::ParamType::String,
            description: format!("{name} parameter"),
            cli_mapping: state::CliMapping {
                flag: format!("--{name}"),
                value_template: "{value}".to_string(),
            },
        }
    }

    #[test]
    fn app_state_db_path_returns_models_parent_state_db() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let state = test_state(models_dir, HashMap::new());

        assert_eq!(state.db_path(), dir.path().join("state.db"));
    }

    #[test]
    fn open_state_db_opens_models_parent_state_db_and_returns_state_db() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let state = test_state(models_dir, HashMap::new());

        let db = super::open_state_db(&state).unwrap();
        db.upsert_cli_provider(&cli_provider("codex", "OpenAI"))
            .unwrap();
        drop(db);

        assert!(dir.path().join("state.db").exists());
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();
        assert_eq!(
            db.get_cli_provider("codex").unwrap().unwrap().display_name,
            "OpenAI"
        );
    }

    #[test]
    fn load_providers_for_models_dir_loads_parent_providers_and_defaults_errors() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(
            dir.path().join("providers.toml"),
            r#"
[codex]
command = "codex"
args = ["exec"]
"#,
        )
        .unwrap();

        let providers = super::load_providers_for_models_dir(&models_dir);
        assert!(providers.entries.contains_key("codex"));

        std::fs::write(dir.path().join("providers.toml"), "not = [valid").unwrap();
        let providers = super::load_providers_for_models_dir(&models_dir);
        assert!(providers.entries.is_empty());

        std::fs::remove_file(dir.path().join("providers.toml")).unwrap();
        let providers = super::load_providers_for_models_dir(&models_dir);
        assert!(providers.entries.is_empty());
    }

    #[test]
    fn effective_provider_for_model_provider_rejects_out_of_range_index() {
        let model = make_model("gpt-high", &["codex"]);

        let err =
            super::effective_provider_for_model_provider(&model, 1, &ProvidersConfig::default())
                .unwrap_err();

        assert_eq!(err, "provider_index out of range");
    }

    #[test]
    fn effective_provider_for_model_provider_rejects_unresolved_empty_command() {
        let model = ModelConfig {
            name: "gpt-high".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider("missing-provider", vec![])],
            inputs: vec![],
        };

        let err =
            super::effective_provider_for_model_provider(&model, 0, &ProvidersConfig::default())
                .unwrap_err();

        assert_eq!(
            err,
            "provider missing-provider is missing from providers.toml"
        );
    }

    #[test]
    fn refresh_quotas_filters_to_multi_provider_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let models = HashMap::from([
            (
                "single".to_string(),
                make_model("single", &["single-provider"]),
            ),
            (
                "multi".to_string(),
                make_model("multi", &["multi-a", "multi-b"]),
            ),
        ]);
        let state = test_state(models_dir, models);

        let mut results = super::refresh_quotas_inner(&state).unwrap();
        results.sort_by(|a, b| a.provider_name.cmp(&b.provider_name));

        assert_eq!(
            results
                .iter()
                .map(|entry| entry.provider_name.as_str())
                .collect::<Vec<_>>(),
            vec!["multi-a", "multi-b"]
        );
        assert!(results.iter().all(|entry| entry.status == "no_script"));
    }

    #[test]
    fn refresh_quotas_skips_fresh_providers() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let state = test_state(
            models_dir,
            HashMap::from([(
                "multi".to_string(),
                make_model("multi", &["fresh-provider", "stale-provider"]),
            )]),
        );
        let db = StateDb::open(&state.db_path()).unwrap();
        db.upsert_quota_refresh(
            "fresh-provider",
            &[state::QuotaWindowInput {
                used_percent: 0.20,
                resets_at: chrono::Utc::now() + chrono::Duration::hours(24),
            }],
        )
        .unwrap();
        drop(db);

        let mut results = super::refresh_quotas_inner(&state).unwrap();
        results.sort_by(|a, b| a.provider_name.cmp(&b.provider_name));

        let fresh = results
            .iter()
            .find(|entry| entry.provider_name == "fresh-provider")
            .unwrap();
        assert_eq!(fresh.status, "fresh");
        assert!(fresh.windows.is_empty());
        let stale = results
            .iter()
            .find(|entry| entry.provider_name == "stale-provider")
            .unwrap();
        assert_eq!(stale.status, "no_script");
    }

    #[cfg(unix)]
    #[test]
    fn refresh_quotas_marks_in_flight_providers() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let script = dir.path().join("quota.sh");
        write_executable(
            &script,
            r#"#!/usr/bin/env bash
printf '%s\n' '{"windows":[{"used_percent":42,"resets_at":"2099-01-01T00:00:00Z"}]}'
"#,
        );
        std::fs::write(
            dir.path().join("providers.toml"),
            format!(
                r#"[in-flight-provider]
quota_script = "{}"
"#,
                script.display()
            ),
        )
        .unwrap();
        let state = test_state(
            models_dir,
            HashMap::from([(
                "multi".to_string(),
                make_model("multi", &["in-flight-provider", "other-provider"]),
            )]),
        );
        let _guard = state
            .quota_in_flight
            .try_claim("in-flight-provider")
            .unwrap();

        let results = super::refresh_quotas_inner(&state).unwrap();

        let entry = results
            .iter()
            .find(|entry| entry.provider_name == "in-flight-provider")
            .unwrap();
        assert_eq!(entry.status, "in_flight");
        assert!(entry.windows.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn refresh_quotas_maps_refresh_outcome_to_dto() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let script = dir.path().join("quota.sh");
        write_executable(
            &script,
            r#"#!/usr/bin/env bash
printf '%s\n' '{"windows":[{"used_percent":42,"resets_at":"2099-01-01T00:00:00Z"}]}'
"#,
        );
        std::fs::write(
            dir.path().join("providers.toml"),
            format!(
                r#"[updated-provider]
quota_script = "{}"

[no-script-provider]
"#,
                script.display()
            ),
        )
        .unwrap();
        let state = test_state(
            models_dir,
            HashMap::from([(
                "multi".to_string(),
                make_model("multi", &["updated-provider", "no-script-provider"]),
            )]),
        );

        let results = super::refresh_quotas_inner(&state).unwrap();

        let updated = results
            .iter()
            .find(|entry| entry.provider_name == "updated-provider")
            .unwrap();
        assert_eq!(updated.status, "updated");
        assert_eq!(updated.windows.len(), 1);
        assert!((updated.windows[0].used_percent - 0.42).abs() < 1e-6);
        assert_eq!(updated.windows[0].resets_at, "2099-01-01T00:00:00+00:00");

        let no_script = results
            .iter()
            .find(|entry| entry.provider_name == "no-script-provider")
            .unwrap();
        assert_eq!(no_script.status, "no_script");
        assert!(no_script.windows.is_empty());
        assert!(no_script.message.is_none());
    }

    #[test]
    fn provider_account_commands_validate_and_persist_through_state_db() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().join("models"), HashMap::new());
        let db = StateDb::open(&state.db_path()).unwrap();
        db.upsert_cli_provider(&cli_provider("codex", "OpenAI"))
            .unwrap();
        drop(db);

        assert_eq!(
            super::add_account_inner(
                &state,
                AddAccountInput {
                    id: String::new(),
                    provider: "codex".to_string(),
                    profile_name: "default".to_string(),
                    auth_method: AuthMethod::OAuth,
                },
            )
            .unwrap_err(),
            "Account id cannot be empty"
        );
        assert_eq!(
            super::add_account_inner(
                &state,
                AddAccountInput {
                    id: "acct-1".to_string(),
                    provider: String::new(),
                    profile_name: "default".to_string(),
                    auth_method: AuthMethod::OAuth,
                },
            )
            .unwrap_err(),
            "Account provider cannot be empty"
        );
        assert_eq!(
            super::add_account_inner(
                &state,
                AddAccountInput {
                    id: "acct-1".to_string(),
                    provider: "codex".to_string(),
                    profile_name: String::new(),
                    auth_method: AuthMethod::OAuth,
                },
            )
            .unwrap_err(),
            "Account profile_name cannot be empty"
        );
        assert_eq!(
            super::add_account_inner(
                &state,
                AddAccountInput {
                    id: "acct-1".to_string(),
                    provider: "missing".to_string(),
                    profile_name: "default".to_string(),
                    auth_method: AuthMethod::OAuth,
                },
            )
            .unwrap_err(),
            "Provider 'missing' not found"
        );

        let added = super::add_account_inner(
            &state,
            AddAccountInput {
                id: "acct-1".to_string(),
                provider: "codex".to_string(),
                profile_name: "default".to_string(),
                auth_method: AuthMethod::OAuth,
            },
        )
        .unwrap();

        assert_eq!(added.auth_status, AuthStatus::Unknown);
        chrono::DateTime::parse_from_rfc3339(&added.created_at).unwrap();
        let accounts = super::list_accounts_inner(&state, Some("codex".to_string())).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct-1");
        assert_eq!(super::list_cli_providers_inner(&state).unwrap().len(), 1);
        assert_eq!(
            super::get_cli_provider_inner(&state, "codex".to_string())
                .unwrap()
                .display_name,
            "OpenAI"
        );
        assert!(
            super::remove_account_inner(&state, "acct-1".to_string(), "codex".to_string()).unwrap()
        );
        assert!(
            !super::remove_account_inner(&state, "acct-1".to_string(), "codex".to_string())
                .unwrap()
        );
    }

    #[test]
    fn sync_provider_maps_display_name_and_persists_with_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().join("models"), HashMap::new());
        let cli_info = setup_core::detection::CliInfo {
            name: "codex".to_string(),
            installed: true,
            path: Some("/tmp/bin/codex".to_string()),
            version: Some("codex 1.2.3".to_string()),
            authenticated: false,
            config_dir: Some(dir.path().join("codex-config")),
            profiles: vec![],
            version_changed: None,
            previous_version: None,
        };

        let record = super::sync_provider_record_from_cli_info("codex", cli_info);
        super::sync_provider_persist_record(&state, &record).unwrap();

        assert_eq!(record.display_name, "OpenAI");
        let last_synced = record.last_synced.as_deref().unwrap();
        chrono::DateTime::parse_from_rfc3339(last_synced).unwrap();
        let stored = StateDb::open(&state.db_path())
            .unwrap()
            .get_cli_provider("codex")
            .unwrap()
            .unwrap();
        assert_eq!(stored.display_name, "OpenAI");
        assert_eq!(stored.version.as_deref(), Some("codex 1.2.3"));
        assert_eq!(stored.last_synced, record.last_synced);

        assert_eq!(super::sync_provider_display_name("claude"), "Anthropic");
        assert_eq!(super::sync_provider_display_name("gemini"), "Google");
        assert_eq!(super::sync_provider_display_name("opencode"), "OpenCode");
        assert_eq!(super::sync_provider_display_name("custom"), "custom");
    }

    #[test]
    fn discover_models_cmd_persists_models_and_parameters_and_guards_stale_delete() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        let db = StateDb::open(&db_path).unwrap();
        db.upsert_discovered_model(&discovered_model("codex", "old-gpt", "old-version"))
            .unwrap();
        drop(db);

        let empty_result = discovery::DiscoveryResult {
            cli_name: "codex".to_string(),
            cli_version: "new-version".to_string(),
            models: vec![],
            parameters: vec![],
        };
        let returned = super::persist_discovery_result(&db_path, "codex", empty_result).unwrap();
        assert!(returned.is_empty());
        assert_eq!(
            StateDb::open(&db_path)
                .unwrap()
                .list_discovered_models(Some("codex"))
                .unwrap()
                .len(),
            1
        );

        let new_model = discovered_model("codex", "gpt-new", "new-version");
        let parameter = model_parameter("model");
        let result = discovery::DiscoveryResult {
            cli_name: "codex".to_string(),
            cli_version: "new-version".to_string(),
            models: vec![new_model.clone()],
            parameters: vec![("gpt-new".to_string(), parameter.clone())],
        };
        let returned = super::persist_discovery_result(&db_path, "codex", result).unwrap();

        assert_eq!(returned.len(), 1);
        assert_eq!(returned[0].canonical_name, "gpt-new");
        let db = StateDb::open(&db_path).unwrap();
        let models = db.list_discovered_models(Some("codex")).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].canonical_name, "gpt-new");
        let params = db.list_model_parameters("gpt-new", "codex").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, parameter.name);
    }

    #[test]
    fn list_discovered_models_filters_by_provider() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().join("models"), HashMap::new());
        let db = StateDb::open(&state.db_path()).unwrap();
        db.upsert_discovered_model(&discovered_model("codex", "gpt-5", "codex-1"))
            .unwrap();
        db.upsert_discovered_model(&discovered_model("claude", "sonnet", "claude-1"))
            .unwrap();
        drop(db);

        let models =
            super::list_discovered_models_inner(&state, Some("codex".to_string())).unwrap();

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider, "codex");
        assert_eq!(models[0].canonical_name, "gpt-5");
    }

    #[test]
    fn get_model_parameters_filters_by_provider_and_model() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().join("models"), HashMap::new());
        let db = StateDb::open(&state.db_path()).unwrap();
        db.upsert_model_parameter("gpt-5", "codex", &model_parameter("model"))
            .unwrap();
        db.upsert_model_parameter("gpt-5", "claude", &model_parameter("max_tokens"))
            .unwrap();
        db.upsert_model_parameter("gpt-4", "codex", &model_parameter("temperature"))
            .unwrap();
        drop(db);

        let params =
            super::get_model_parameters_inner(&state, "gpt-5".to_string(), "codex".to_string())
                .unwrap();

        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "model");
        assert_eq!(params[0].cli_mapping.flag, "--model");
    }

    #[test]
    fn save_model_inner_rejects_duplicate_codex_args_without_disk_or_memory_update() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        write_codex_providers(dir.path());
        let state = test_state(models_dir.clone(), HashMap::new());
        let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);

        let err = super::save_model_inner(&state, model).unwrap_err();

        assert!(err.contains("duplicates root [codex].args"), "{err}");
        assert!(!models_dir.join("gpt-high.toml").exists());
        assert!(!state.models.lock().unwrap().contains_key("gpt-high"));
    }

    #[test]
    fn save_model_inner_accepts_clean_model_and_provider_aware_reload() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        write_codex_providers(dir.path());
        let providers = config::ProvidersConfig::load(&dir.path().join("providers.toml")).unwrap();
        let state = test_state(models_dir.clone(), HashMap::new());
        let model = model_with_provider_args(
            "gpt-high",
            "codex",
            &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
        );

        super::save_model_inner(&state, model).unwrap();

        assert!(models_dir.join("gpt-high.toml").exists());
        let loaded = config::load_models(&models_dir, Some(&providers)).unwrap();
        assert!(loaded.contains_key("gpt-high"));
    }

    #[test]
    fn save_model_inner_accepts_duplicate_shape_without_sibling_providers() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let state = test_state(models_dir.clone(), HashMap::new());
        let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);

        super::save_model_inner(&state, model).unwrap();

        assert!(models_dir.join("gpt-high.toml").exists());
        assert!(
            config::load_models(&models_dir, None)
                .unwrap()
                .contains_key("gpt-high")
        );
    }

    #[test]
    fn save_model_inner_preserves_existing_basic_validation_errors() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path().join("models"), HashMap::new());

        let empty_name = model_with_provider_args("", "codex", &["-m", "gpt-5.5"]);
        assert_eq!(
            super::save_model_inner(&state, empty_name).unwrap_err(),
            "Model name cannot be empty"
        );

        let no_providers = ModelConfig {
            name: "gpt-high".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![],
            inputs: vec![],
        };
        assert_eq!(
            super::save_model_inner(&state, no_providers).unwrap_err(),
            "Model must have at least one provider"
        );

        let empty_provider_name = model_with_provider_args("gpt-high", "", &["-m", "gpt-5.5"]);
        assert_eq!(
            super::save_model_inner(&state, empty_provider_name).unwrap_err(),
            "Provider 1 has empty name"
        );
    }

    #[test]
    fn update_pool_inner_rejects_duplicate_preserving_rewrite_without_file_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        write_codex_providers(dir.path());
        let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);
        let model_path = models_dir.join("gpt-high.toml");
        std::fs::write(&model_path, "sentinel").unwrap();
        let state = test_state(models_dir, HashMap::from([(model.name.clone(), model)]));

        let err = super::update_pool_inner(
            &state,
            vec!["codex".to_string()],
            vec!["codex".to_string(), "claude".to_string()],
        )
        .unwrap_err();

        assert!(err.contains("duplicates root [codex].args"), "{err}");
        assert_eq!(std::fs::read_to_string(&model_path).unwrap(), "sentinel");
    }

    #[test]
    fn update_pool_inner_accepts_clean_rewrite_and_added_provider_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        write_codex_providers(dir.path());
        let providers = config::ProvidersConfig::load(&dir.path().join("providers.toml")).unwrap();
        let model = model_with_provider_args("claude-high", "claude", &["--model", "sonnet"]);
        std::fs::write(models_dir.join("claude-high.toml"), model.to_toml()).unwrap();
        let state = test_state(
            models_dir.clone(),
            HashMap::from([(model.name.clone(), model)]),
        );

        super::update_pool_inner(
            &state,
            vec!["claude".to_string()],
            vec!["claude".to_string(), "codex".to_string()],
        )
        .unwrap();

        let loaded = config::load_models(&models_dir, Some(&providers)).unwrap();
        let codex = loaded["claude-high"]
            .providers
            .iter()
            .find(|provider| provider.name == "codex")
            .expect("codex provider was added");
        assert!(codex.args.is_empty());
    }

    #[test]
    fn update_pool_inner_accepts_duplicate_preserving_rewrite_without_sibling_providers() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);
        std::fs::write(models_dir.join("gpt-high.toml"), model.to_toml()).unwrap();
        let state = test_state(
            models_dir.clone(),
            HashMap::from([(model.name.clone(), model)]),
        );

        super::update_pool_inner(
            &state,
            vec!["codex".to_string()],
            vec!["codex".to_string(), "claude".to_string()],
        )
        .unwrap();

        assert!(
            config::load_models(&models_dir, None)
                .unwrap()
                .contains_key("gpt-high")
        );
    }

    #[test]
    fn update_pool_inner_preserves_existing_command_errors() {
        let dir = tempfile::tempdir().unwrap();
        let model = model_with_provider_args("claude-high", "claude", &["--model", "sonnet"]);
        let state = test_state(
            dir.path().join("models"),
            HashMap::from([(model.name.clone(), model)]),
        );

        assert_eq!(
            super::update_pool_inner(&state, vec!["claude".to_string()], vec![]).unwrap_err(),
            "Pool must have at least one command"
        );
        assert_eq!(
            super::update_pool_inner(
                &state,
                vec!["codex".to_string()],
                vec!["codex".to_string(), "claude".to_string()],
            )
            .unwrap_err(),
            "No models found with the specified command set"
        );
    }

    #[test]
    fn derive_pools_groups_by_command_set() {
        let mut models = HashMap::new();
        models.insert("a".into(), make_model("a", &["claude", "codex"]));
        models.insert("b".into(), make_model("b", &["claude", "codex"]));
        models.insert("c".into(), make_model("c", &["gemini"]));

        let pools = derive_pools(&models);
        assert_eq!(pools.len(), 2);

        let pool_claude = pools
            .iter()
            .find(|p| p.commands.contains(&"claude".to_string()))
            .unwrap();
        assert_eq!(pool_claude.model_count, 2);
        assert!(pool_claude.model_names.contains(&"a".to_string()));
        assert!(pool_claude.model_names.contains(&"b".to_string()));

        let pool_gemini = pools
            .iter()
            .find(|p| p.commands.contains(&"gemini".to_string()))
            .unwrap();
        assert_eq!(pool_gemini.model_count, 1);
        assert_eq!(pool_gemini.model_names, vec!["c".to_string()]);
    }

    #[test]
    fn derive_pools_deduplicates_commands() {
        let mut models = HashMap::new();
        // Model with duplicate commands should deduplicate
        models.insert(
            "x".into(),
            ModelConfig {
                name: "x".to_string(),
                prompt_mode: PromptMode::Stdin,
                providers: vec![
                    ProviderConfig::new("claude", vec![]),
                    ProviderConfig::new("claude", vec!["-p".to_string()]),
                ],
                inputs: vec![],
            },
        );
        models.insert("y".into(), make_model("y", &["claude"]));

        let pools = derive_pools(&models);
        // Both should be in the same pool since deduped command set is ["claude"]
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].model_count, 2);
    }

    #[test]
    fn derive_pools_groups_by_provider_name() {
        let mut models = HashMap::new();
        models.insert(
            "a".into(),
            ModelConfig {
                name: "a".to_string(),
                prompt_mode: PromptMode::Stdin,
                providers: vec![ProviderConfig::model_provider("claude", vec![])],
                inputs: vec![],
            },
        );
        models.insert("b".into(), make_model("b", &["claude"]));

        let pools = derive_pools(&models);
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].commands, vec!["claude".to_string()]);
        assert_eq!(pools[0].model_count, 2);
    }

    #[cfg(unix)]
    #[test]
    fn test_model_marks_provider_exhausted_on_quota_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model = ModelConfig {
            name: "quota-model".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new(
                "sh",
                vec![
                    "-c".to_string(),
                    "echo quota exhausted >&2; exit 7".to_string(),
                ],
            )],
            inputs: vec![],
        };
        let mut models = HashMap::new();
        models.insert(model.name.clone(), model);

        let db_path = dir.path().join("state.db");
        let db = StateDb::open(&db_path).unwrap();
        db.upsert_quota_refresh("sh", &[]).unwrap();
        drop(db);

        let result = test_model_for_test(models, models_dir, "quota-model").unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, 7);
        assert!(result.stderr.contains("quota exhausted"));
        let db = StateDb::open(&db_path).unwrap();
        let quota = db.get_quota("sh").unwrap().unwrap();
        assert!(quota.exhausted_at.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn test_model_migrated_provider_uses_providers_toml_effective_provider() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let argv_dump = dir.path().join("test-model-argv.txt");
        let stdin_dump = dir.path().join("test-model-stdin.txt");
        let script = dir.path().join("test-model-provider.sh");
        write_executable(
            &script,
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "{argv_dump}"
cat > "{stdin_dump}"
printf 'test-model stdout\n'
printf 'test-model stderr\n' >&2
"#,
                argv_dump = argv_dump.display(),
                stdin_dump = stdin_dump.display()
            ),
        );
        std::fs::write(
            dir.path().join("providers.toml"),
            format!(
                r#"[test-model-provider]
command = "{}"
args = ["--provider"]
prompt_mode = "arg"
"#,
                script.display()
            ),
        )
        .unwrap();
        let success_model = ModelConfig {
            name: "test-model".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider(
                "test-model-provider",
                vec!["--model".to_string()],
            )],
            inputs: vec![],
        };
        let mut models = HashMap::new();
        models.insert(success_model.name.clone(), success_model);

        let result = test_model_for_test(models, models_dir.clone(), "test-model").unwrap();

        assert!(result.success);
        assert_eq!(result.stdout, "test-model stdout\n");
        assert_eq!(result.stderr, "test-model stderr\n");
        assert_eq!(result.exit_code, 0);
        assert_eq!(std::fs::read_to_string(&stdin_dump).unwrap(), "");
        let argv = std::fs::read_to_string(&argv_dump).unwrap();
        assert!(argv.contains("--provider\n"), "{argv}");
        assert!(argv.contains("--model\n"), "{argv}");
        assert!(argv.ends_with("Say hello in one sentence.\n"), "{argv}");

        let quota_script = dir.path().join("test-model-provider-quota.sh");
        write_executable(
            &quota_script,
            r#"#!/usr/bin/env bash
set -euo pipefail
printf 'quota exhausted from effective provider\n' >&2
exit 7
"#,
        );
        std::fs::write(
            dir.path().join("providers.toml"),
            format!(
                r#"[quota-effective-provider]
command = "{}"
args = []
prompt_mode = "arg"
"#,
                quota_script.display()
            ),
        )
        .unwrap();
        let quota_model = ModelConfig {
            name: "quota-model".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider(
                "quota-effective-provider",
                vec![],
            )],
            inputs: vec![],
        };
        let mut quota_models = HashMap::new();
        quota_models.insert(quota_model.name.clone(), quota_model);

        let quota_result = test_model_for_test(quota_models, models_dir, "quota-model").unwrap();

        assert!(!quota_result.success);
        assert_eq!(quota_result.exit_code, 7);
        assert!(
            quota_result
                .stderr
                .contains("quota exhausted from effective provider"),
            "{}",
            quota_result.stderr
        );
        assert!(
            !quota_result.stderr.contains("Empty command"),
            "{}",
            quota_result.stderr
        );
        let db = StateDb::open(&dir.path().join("state.db")).unwrap();
        let quota = db.get_quota("quota-effective-provider").unwrap().unwrap();
        assert!(quota.exhausted_at.is_some());
    }

    // risk: exhaustive surfaces 38-39; level: Rust unit; source: contract § 5.8, A8, A10
    #[cfg(unix)]
    #[test]
    fn test_model_raw_sigterm_returns_unified_signal_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model = ModelConfig {
            name: "sigterm-model".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new(
                "sh",
                vec!["-c".to_string(), "kill -TERM $$; sleep 1".to_string()],
            )],
            inputs: vec![],
        };
        let mut models = HashMap::new();
        models.insert(model.name.clone(), model);

        let result = test_model_for_test(models, models_dir, "sigterm-model").unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, 143);
    }
}
