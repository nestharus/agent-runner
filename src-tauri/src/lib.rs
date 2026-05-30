//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`, `formatter`, `accessor`, `validator`

mod app_paths;
mod app_state;
#[path = "commands/provider_settings.rs"]
pub(crate) mod provider_settings;
mod run_tauri;
pub mod setup;
pub mod terminal_outcome_adapter;
#[path = "commands/test_model/mod.rs"]
mod test_model_command;
pub mod usage;
mod wiring;
pub mod zero_turn_orchestration;

pub use app_paths::{
    AppConfig, load_app_config, load_providers_for_models_dir, load_providers_for_models_dir_with,
};
pub use app_state::AppState;
#[cfg(test)]
pub(crate) use app_state::AppStateTestServices;
pub use run_tauri::run_tauri;
pub use test_model_command::{TestModelResult, effective_provider_for_model_provider};
#[cfg(test)]
pub(crate) use test_model_command::{
    TestModelServices, test_model_for_test, test_model_with_db_path,
};

use oulipoly_config as config;
use oulipoly_config::ModelConfig;
use oulipoly_runtime::services::QuotaServiceRequest;
use oulipoly_runtime::{discovery, quota};
use oulipoly_setup as setup_core;
use oulipoly_setup::actions::{SetupEvent, UserResponse};
#[cfg(test)]
use oulipoly_state as state;
use oulipoly_state::StateDb;
use oulipoly_state::repositories::SetupRepository;
use oulipoly_state::{AccountRecord, AuthMethod, AuthStatus, CliProviderRecord};
use oulipoly_state::{DiscoveredModel, ModelParameter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

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
    let providers = load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config);
    let fresh = config::load_models(&state.models_dir, Some(&providers)).unwrap_or_default();
    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    *models = fresh;
    drop(models);
    provider_settings::refresh_provider_settings_host(&state)?;
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
    let providers = load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config);
    let toml_content = config::render_validated_model_toml(&model, Some(&providers))?;
    let path = state.models_dir.join(format!("{}.toml", model.name));

    std::fs::create_dir_all(&state.models_dir)
        .map_err(|e| format!("Failed to create models directory: {e}"))?;
    std::fs::write(&path, &toml_content).map_err(|e| format!("Failed to write model file: {e}"))?;

    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    models.insert(model.name.clone(), model);
    drop(models);
    provider_settings::refresh_provider_settings_host(state)?;
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
    drop(models);
    provider_settings::refresh_provider_settings_host(&state)?;
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
        let mut candidates: Vec<String> = set.into_iter().collect();
        candidates.sort();
        candidates
    };

    let providers_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("providers.toml");
    let providers_cfg = state
        .providers_config
        .load_providers(&providers_path)
        .unwrap_or_default();

    let db_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");

    let db = state
        .state_db_opener
        .open_at(&db_path)
        .map_err(|e| format!("Failed to open state DB: {e}"))?;
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

        let outcome = state
            .quota_service
            .refresh_quota(QuotaServiceRequest {
                provider_name: provider_name.clone(),
                providers_cfg: &providers_cfg,
                in_flight,
                state: &db,
            })
            .map_err(|error| error.to_string())?
            .outcome;
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

    let providers = load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config);
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

// --- Provider & Account commands ---

/// Helper to open the state DB from AppState.
fn open_state_db(state: &AppState) -> Result<StateDb, String> {
    state.state_db_opener.open_at(&state.db_path())
}

fn with_setup_repository<T>(
    state: &AppState,
    f: impl FnOnce(&dyn SetupRepository) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(test)]
    if let Some(repo) = state.setup_repository.as_ref() {
        return f(repo.as_ref());
    }

    let db = open_state_db(state)?;
    f(&db)
}

#[tauri::command]
fn list_cli_providers(state: tauri::State<AppState>) -> Result<Vec<CliProviderRecord>, String> {
    list_cli_providers_inner(&state)
}

fn list_cli_providers_inner(state: &AppState) -> Result<Vec<CliProviderRecord>, String> {
    with_setup_repository(state, |repo| repo.list_cli_providers())
}

#[tauri::command]
fn get_cli_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    get_cli_provider_inner(&state, cli_name)
}

fn get_cli_provider_inner(state: &AppState, cli_name: String) -> Result<CliProviderRecord, String> {
    with_setup_repository(state, |repo| {
        repo.get_cli_provider(&cli_name)?
            .ok_or_else(|| format!("Provider '{}' not found", cli_name))
    })
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
    with_setup_repository(state, |repo| repo.list_accounts(provider.as_deref()))
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

    with_setup_repository(state, |repo| {
        // Verify the provider exists
        repo.get_cli_provider(&account.provider)?
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

        repo.insert_account(&record)?;
        Ok(record)
    })
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
    with_setup_repository(state, |repo| repo.delete_account(&id, &provider))
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
    with_setup_repository(state, |repo| repo.upsert_cli_provider(record))
}

// --- Discovery commands ---

#[tauri::command]
async fn discover_models_cmd(
    state: tauri::State<'_, AppState>,
    cli_name: String,
) -> Result<Vec<DiscoveredModel>, String> {
    let db_path = state.db_path();
    let state_db_opener = Arc::clone(&state.state_db_opener);

    tauri::async_runtime::spawn_blocking(move || {
        let result = discovery::discover_models(&cli_name)?;
        let db = state_db_opener.open_at(&db_path)?;
        persist_discovery_result(&db, &cli_name, result)
    })
    .await
    .map_err(|e| format!("Discovery task failed: {e}"))?
}

fn persist_discovery_result(
    repo: &dyn SetupRepository,
    cli_name: &str,
    result: discovery::DiscoveryResult,
) -> Result<Vec<DiscoveredModel>, String> {
    // Clean out models from older CLI versions
    if !result.models.is_empty() {
        repo.delete_stale_models(cli_name, &result.cli_version)?;
    }

    // Store discovered models
    for model in &result.models {
        repo.upsert_discovered_model(model)?;
    }

    // Store discovered parameters
    for (model_name, param) in &result.parameters {
        repo.upsert_model_parameter(model_name, cli_name, param)?;
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
    with_setup_repository(state, |repo| {
        repo.list_discovered_models(provider.as_deref())
    })
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
    with_setup_repository(state, |repo| {
        repo.list_model_parameters(&model_name, &provider)
    })
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;
    use config::{ModelConfig, PromptMode, ProviderConfig};
    use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
    use oulipoly_config::repositories::ProvidersConfigRepository;
    use oulipoly_config::{
        ClaudeRestrictions, CodexRestrictions, ProviderEntry, ProvidersConfig, ToolRestrictionKind,
        ToolRestrictions,
    };
    use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind as TestTerminalSignalKind;
    use oulipoly_runtime::executor::{
        CapturedChildInvocation, ExecutionResult, ResumeAcceptanceResult, SessionCaptureMethod,
        SessionCaptureResult, TerminalSignal,
    };
    use oulipoly_runtime::services::{
        DiagnosticsServiceOutput, DiagnosticsServicePort, DiagnosticsServiceRequest,
        ExecutorServiceOutput, ExecutorServicePort, ExecutorServiceRequest, QuotaServiceOutput,
        QuotaServicePort, QuotaServiceRequest, RoutingServiceOutput, RoutingServicePort,
        RoutingServiceRequest, ServiceError,
    };
    use oulipoly_state::repositories::{SetupRepository, StateDbOpener};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

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
            provider: None,
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
            provider: None,
        }
    }

    fn model_with_provider_artifact(name: &str, provider_name: &str, path: &Path) -> ModelConfig {
        ModelConfig {
            name: name.to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(provider_name, Vec::new())],
            inputs: vec![],
            provider: Some(ProviderImplementationRef {
                path: Some(path.display().to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        }
    }

    fn test_state(models_dir: PathBuf, models: HashMap<String, ModelConfig>) -> AppState {
        AppState::test_default(models_dir, models)
    }

    struct StubProvidersConfigRepository {
        calls: Mutex<Vec<PathBuf>>,
        response: Mutex<Result<ProvidersConfig, String>>,
    }

    impl Default for StubProvidersConfigRepository {
        fn default() -> Self {
            Self::returning(Ok(ProvidersConfig::default()))
        }
    }

    impl StubProvidersConfigRepository {
        fn returning(response: Result<ProvidersConfig, String>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response: Mutex::new(response),
            }
        }

        fn calls(&self) -> Vec<PathBuf> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ProvidersConfigRepository for StubProvidersConfigRepository {
        fn load_providers(&self, path: &Path) -> Result<ProvidersConfig, String> {
            self.calls.lock().unwrap().push(path.to_path_buf());
            self.response.lock().unwrap().clone()
        }

        fn get<'a>(&self, config: &'a ProvidersConfig, name: &str) -> Option<&'a ProviderEntry> {
            config.get(name)
        }

        fn effective_provider(
            &self,
            config: &ProvidersConfig,
            model_provider: &ProviderConfig,
        ) -> Result<(ProviderConfig, PromptMode), String> {
            config.effective_provider(model_provider)
        }

        fn runtime_provider(
            &self,
            config: &ProvidersConfig,
            name: &str,
        ) -> Result<(ProviderConfig, PromptMode), String> {
            config.runtime_provider(name)
        }
    }

    struct StubStateDbOpener {
        calls: Mutex<Vec<PathBuf>>,
        response: Mutex<Result<PathBuf, String>>,
    }

    impl StubStateDbOpener {
        fn opening(path: PathBuf) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response: Mutex::new(Ok(path)),
            }
        }

        fn failing(message: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                response: Mutex::new(Err(message.to_string())),
            }
        }

        fn calls(&self) -> Vec<PathBuf> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl StateDbOpener for StubStateDbOpener {
        fn open_default(&self) -> Result<StateDb, String> {
            let path = self.response.lock().unwrap().clone()?;
            StateDb::open(&path)
        }

        fn open_at(&self, path: &Path) -> Result<StateDb, String> {
            self.calls.lock().unwrap().push(path.to_path_buf());
            let path = self.response.lock().unwrap().clone()?;
            StateDb::open(&path)
        }

        fn open_in_memory(&self) -> StateDb {
            StateDb::open(Path::new(":memory:")).unwrap()
        }
    }

    #[derive(Default)]
    struct StubSetupRepository {
        calls: Mutex<Vec<String>>,
        providers: Mutex<Vec<CliProviderRecord>>,
        accounts: Mutex<Vec<AccountRecord>>,
        discovered_models: Mutex<Vec<DiscoveredModel>>,
        parameters: Mutex<Vec<ModelParameter>>,
        delete_account_result: Mutex<bool>,
    }

    impl StubSetupRepository {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn with_provider(provider: CliProviderRecord) -> Self {
            Self {
                providers: Mutex::new(vec![provider]),
                delete_account_result: Mutex::new(true),
                ..Self::default()
            }
        }

        fn set_discovery_fixture(&self, model: DiscoveredModel, parameter: ModelParameter) {
            self.discovered_models.lock().unwrap().push(model);
            self.parameters.lock().unwrap().push(parameter);
        }
    }

    impl SetupRepository for StubSetupRepository {
        fn upsert_cli_provider(&self, provider: &CliProviderRecord) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("upsert_cli_provider:{}", provider.cli_name));
            self.providers.lock().unwrap().push(provider.clone());
            Ok(())
        }

        fn list_cli_providers(&self) -> Result<Vec<CliProviderRecord>, String> {
            self.calls
                .lock()
                .unwrap()
                .push("list_cli_providers".to_string());
            Ok(self.providers.lock().unwrap().clone())
        }

        fn get_cli_provider(&self, cli_name: &str) -> Result<Option<CliProviderRecord>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("get_cli_provider:{cli_name}"));
            Ok(self
                .providers
                .lock()
                .unwrap()
                .iter()
                .find(|provider| provider.cli_name == cli_name)
                .cloned())
        }

        fn insert_account(&self, account: &AccountRecord) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!(
                "insert_account:{}:{}",
                account.provider, account.id
            ));
            self.accounts.lock().unwrap().push(account.clone());
            Ok(())
        }

        fn list_accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("list_accounts:{provider:?}"));
            Ok(self
                .accounts
                .lock()
                .unwrap()
                .iter()
                .filter(|account| provider.is_none_or(|p| p == account.provider))
                .cloned()
                .collect())
        }

        fn delete_account(&self, id: &str, provider: &str) -> Result<bool, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("delete_account:{provider}:{id}"));
            Ok(*self.delete_account_result.lock().unwrap())
        }

        fn upsert_discovered_model(&self, model: &DiscoveredModel) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!(
                "upsert_discovered_model:{}:{}",
                model.provider, model.canonical_name
            ));
            self.discovered_models.lock().unwrap().push(model.clone());
            Ok(())
        }

        fn list_discovered_models(
            &self,
            provider: Option<&str>,
        ) -> Result<Vec<DiscoveredModel>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("list_discovered_models:{provider:?}"));
            Ok(self
                .discovered_models
                .lock()
                .unwrap()
                .iter()
                .filter(|model| provider.is_none_or(|p| p == model.provider))
                .cloned()
                .collect())
        }

        fn delete_stale_models(&self, provider: &str, cli_version: &str) -> Result<u64, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("delete_stale_models:{provider}:{cli_version}"));
            Ok(0)
        }

        fn upsert_model_parameter(
            &self,
            model_name: &str,
            provider: &str,
            parameter: &ModelParameter,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!(
                "upsert_model_parameter:{provider}:{model_name}:{}",
                parameter.name
            ));
            self.parameters.lock().unwrap().push(parameter.clone());
            Ok(())
        }

        fn list_model_parameters(
            &self,
            model_name: &str,
            provider: &str,
        ) -> Result<Vec<ModelParameter>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("list_model_parameters:{provider}:{model_name}"));
            Ok(self.parameters.lock().unwrap().clone())
        }
    }

    struct StubQuotaService {
        calls: Mutex<Vec<String>>,
        output: Mutex<quota::RefreshOutcome>,
    }

    impl StubQuotaService {
        fn updated() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                output: Mutex::new(quota::RefreshOutcome::Updated {
                    windows: vec![state::QuotaWindowInput {
                        used_percent: 0.73,
                        resets_at: chrono::DateTime::parse_from_rfc3339("2099-01-01T00:00:00Z")
                            .unwrap()
                            .with_timezone(&chrono::Utc),
                    }],
                }),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl QuotaServicePort for StubQuotaService {
        fn refresh_quota(
            &self,
            request: QuotaServiceRequest<'_>,
        ) -> Result<QuotaServiceOutput, ServiceError> {
            self.calls.lock().unwrap().push(request.provider_name);
            let outcome = std::mem::replace(
                &mut *self.output.lock().unwrap(),
                quota::RefreshOutcome::NoScript,
            );
            Ok(QuotaServiceOutput { outcome })
        }
    }

    struct StubExecutorService {
        calls: Mutex<Vec<String>>,
        output: Mutex<Option<ExecutionResult>>,
    }

    impl StubExecutorService {
        fn with_exit(exit_code: i32, stdout: &[u8], stderr: &str) -> Self {
            Self::with_optional_signal(exit_code, stdout, stderr, None)
        }

        fn with_signal(
            exit_code: i32,
            stdout: &[u8],
            stderr: &str,
            kind: TestTerminalSignalKind,
            provider_name: &str,
        ) -> Self {
            Self::with_optional_signal(
                exit_code,
                stdout,
                stderr,
                Some(TerminalSignal {
                    kind,
                    provider_name: provider_name.to_string(),
                    evidence: format!("age156 stub {:?}", kind),
                    observed_at: std::time::SystemTime::UNIX_EPOCH,
                }),
            )
        }

        fn with_optional_signal(
            exit_code: i32,
            stdout: &[u8],
            stderr: &str,
            terminal_signal: Option<TerminalSignal>,
        ) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                output: Mutex::new(Some(ExecutionResult {
                    stdout: stdout.to_vec(),
                    stderr: stderr.to_string(),
                    exit_code,
                    provider_index: 0,
                    session_capture: SessionCaptureResult {
                        session_id: None,
                        method: SessionCaptureMethod::None,
                    },
                    resume_acceptance: None::<ResumeAcceptanceResult>,
                    terminal_reason: None,
                    terminal_signal,
                    captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
                    returned_artifacts: Vec::new(),
                })),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ExecutorServicePort for StubExecutorService {
        fn execute(
            &self,
            request: ExecutorServiceRequest,
        ) -> Result<ExecutorServiceOutput, ServiceError> {
            match request {
                ExecutorServiceRequest::Effective {
                    provider,
                    extra_inputs,
                    working_dir,
                    parent_invocation_env,
                    ..
                }
                | ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
                    provider,
                    extra_inputs,
                    working_dir,
                    parent_invocation_env,
                    ..
                } => {
                    self.calls.lock().unwrap().push(format!(
                        "effective:{}:{}:{}:{}",
                        provider.name,
                        extra_inputs.len(),
                        working_dir.is_none(),
                        parent_invocation_env.is_none()
                    ));
                }
                ExecutorServiceRequest::Facade { .. } => {
                    self.calls.lock().unwrap().push("facade".to_string());
                }
            }
            Ok(ExecutorServiceOutput {
                result: self
                    .output
                    .lock()
                    .unwrap()
                    .take()
                    .expect("stub executor output should be consumed once"),
            })
        }
    }

    #[derive(Default)]
    struct PolicyRecordingExecutorService {
        providers: Mutex<Vec<ProviderConfig>>,
    }

    impl PolicyRecordingExecutorService {
        fn providers(&self) -> Vec<ProviderConfig> {
            self.providers.lock().unwrap().clone()
        }
    }

    impl ExecutorServicePort for PolicyRecordingExecutorService {
        fn execute(
            &self,
            request: ExecutorServiceRequest,
        ) -> Result<ExecutorServiceOutput, ServiceError> {
            let provider = match request {
                ExecutorServiceRequest::Effective { provider, .. }
                | ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
                    provider,
                    ..
                } => provider,
                ExecutorServiceRequest::Facade { .. } => {
                    return Err(ServiceError::Dependency {
                        message: "test_model must use effective provider requests".to_string(),
                    });
                }
            };
            self.providers.lock().unwrap().push(provider);
            Ok(ExecutorServiceOutput {
                result: ExecutionResult {
                    stdout: b"ok".to_vec(),
                    stderr: String::new(),
                    exit_code: 0,
                    provider_index: 0,
                    session_capture: SessionCaptureResult {
                        session_id: None,
                        method: SessionCaptureMethod::None,
                    },
                    resume_acceptance: None::<ResumeAcceptanceResult>,
                    terminal_reason: None,
                    terminal_signal: None,
                    captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
                    returned_artifacts: Vec::new(),
                },
            })
        }
    }

    struct StubDiagnosticsService {
        calls: Mutex<Vec<String>>,
        exhausted: bool,
    }

    impl StubDiagnosticsService {
        fn returning(exhausted: bool) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                exhausted,
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl DiagnosticsServicePort for StubDiagnosticsService {
        fn diagnose(
            &self,
            request: DiagnosticsServiceRequest,
        ) -> Result<DiagnosticsServiceOutput, ServiceError> {
            match request {
                DiagnosticsServiceRequest::ClassifyExhaustion { stderr } => {
                    self.calls
                        .lock()
                        .unwrap()
                        .push(format!("classify:{stderr}"));
                    Ok(DiagnosticsServiceOutput::ExhaustionClassification {
                        is_exhausted: self.exhausted,
                    })
                }
                DiagnosticsServiceRequest::DiagnoseError { .. } => {
                    self.calls
                        .lock()
                        .unwrap()
                        .push("diagnose_error".to_string());
                    Ok(DiagnosticsServiceOutput::Diagnosis {
                        diagnosis: oulipoly_runtime::diagnostics::Diagnosis {
                            category: oulipoly_runtime::diagnostics::ErrorCategory::Unknown,
                            summary: "unused".to_string(),
                        },
                    })
                }
            }
        }
    }

    struct StubRoutingService {
        calls: Mutex<Vec<String>>,
        provider_index: usize,
    }

    impl StubRoutingService {
        fn selecting(provider_index: usize) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                provider_index,
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl RoutingServicePort for StubRoutingService {
        fn select_route(
            &self,
            request: RoutingServiceRequest<'_>,
        ) -> Result<RoutingServiceOutput, ServiceError> {
            self.calls.lock().unwrap().push(format!(
                "select:{}:{}",
                request.model.name,
                request.ctx.is_none()
            ));
            Ok(RoutingServiceOutput {
                provider_index: self.provider_index,
            })
        }
    }

    fn services(
        providers_config: Arc<dyn ProvidersConfigRepository + Send + Sync>,
        state_db_opener: Arc<dyn StateDbOpener + Send + Sync>,
        setup_repository: Arc<dyn SetupRepository + Send + Sync>,
        quota_service: Arc<dyn QuotaServicePort>,
        executor_service: Arc<dyn ExecutorServicePort>,
        diagnostics_service: Arc<dyn DiagnosticsServicePort>,
    ) -> AppStateTestServices {
        AppStateTestServices {
            providers_config,
            state_db_opener,
            setup_repository,
            quota_service,
            executor_service,
            diagnostics_service,
        }
    }

    fn default_services(root: &Path) -> AppStateTestServices {
        let db_path = root.join("state.db");
        services(
            Arc::new(StubProvidersConfigRepository::default()),
            Arc::new(StubStateDbOpener::opening(db_path)),
            Arc::new(StubSetupRepository::default()),
            Arc::new(StubQuotaService::updated()),
            Arc::new(StubExecutorService::with_exit(0, b"stub stdout", "")),
            Arc::new(StubDiagnosticsService::returning(false)),
        )
    }

    #[test]
    fn age38_load_providers_for_models_dir_with_routes_through_stub_and_defaults_errors() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let repo =
            StubProvidersConfigRepository::returning(Err("sentinel provider failure".to_string()));

        let providers = super::load_providers_for_models_dir_with(&models_dir, &repo);

        assert_eq!(repo.calls(), vec![dir.path().join("providers.toml")]);
        assert!(providers.entries.is_empty());
    }

    #[test]
    fn age38_open_state_db_routes_through_injected_state_db_opener() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let opened_db_path = dir.path().join("opened-by-stub.db");
        let opener = Arc::new(StubStateDbOpener::opening(opened_db_path.clone()));
        let state = AppState::with_services(
            models_dir,
            HashMap::new(),
            services(
                Arc::new(StubProvidersConfigRepository::default()),
                opener.clone(),
                Arc::new(StubSetupRepository::default()),
                Arc::new(StubQuotaService::updated()),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );

        let db = super::open_state_db(&state).unwrap();
        db.upsert_cli_provider(&cli_provider("codex", "OpenAI"))
            .unwrap();
        drop(db);

        assert_eq!(opener.calls(), vec![dir.path().join("state.db")]);
        assert!(opened_db_path.exists());
        assert!(!dir.path().join("state.db").exists());
    }

    #[test]
    fn age38_open_state_db_returns_injected_opener_error() {
        let dir = tempfile::tempdir().unwrap();
        let opener = Arc::new(StubStateDbOpener::failing("sentinel opener failure"));
        let state = AppState::with_services(
            dir.path().join("models"),
            HashMap::new(),
            services(
                Arc::new(StubProvidersConfigRepository::default()),
                opener.clone(),
                Arc::new(StubSetupRepository::default()),
                Arc::new(StubQuotaService::updated()),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );

        let err = match super::open_state_db(&state) {
            Ok(_) => panic!("open_state_db should return the injected opener error"),
            Err(err) => err,
        };

        assert_eq!(err, "sentinel opener failure");
        assert_eq!(opener.calls(), vec![dir.path().join("state.db")]);
    }

    #[test]
    fn age38_refresh_quotas_routes_load_open_and_refresh_through_stubs() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let providers = Arc::new(StubProvidersConfigRepository::returning(Ok(
            ProvidersConfig::default(),
        )));
        let opener = Arc::new(StubStateDbOpener::opening(dir.path().join("stub-state.db")));
        let quota_service = Arc::new(StubQuotaService::updated());
        let state = AppState::with_services(
            models_dir,
            HashMap::from([(
                "multi".to_string(),
                make_model("multi", &["age38-a", "age38-b"]),
            )]),
            services(
                providers.clone(),
                opener.clone(),
                Arc::new(StubSetupRepository::default()),
                quota_service.clone(),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );

        let results = super::refresh_quotas_inner(&state).unwrap();

        assert_eq!(providers.calls(), vec![dir.path().join("providers.toml")]);
        assert_eq!(opener.calls(), vec![dir.path().join("state.db")]);
        let mut quota_calls = quota_service.calls();
        quota_calls.sort();
        assert_eq!(quota_calls, vec!["age38-a", "age38-b"]);
        assert!(
            results
                .iter()
                .any(|entry| entry.provider_name == "age38-a" && entry.status == "updated"),
            "stub quota output should be mapped to an updated DTO"
        );
    }

    #[test]
    fn age38_refresh_quotas_keeps_fresh_gate_before_quota_service() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let opened_db_path = dir.path().join("stub-state.db");
        let db = StateDb::open(&opened_db_path).unwrap();
        db.upsert_quota_refresh(
            "fresh-provider",
            &[state::QuotaWindowInput {
                used_percent: 0.10,
                resets_at: chrono::Utc::now() + chrono::Duration::hours(24),
            }],
        )
        .unwrap();
        drop(db);
        let opener = Arc::new(StubStateDbOpener::opening(opened_db_path));
        let quota_service = Arc::new(StubQuotaService::updated());
        let state = AppState::with_services(
            models_dir,
            HashMap::from([(
                "multi".to_string(),
                make_model("multi", &["fresh-provider", "stale-provider"]),
            )]),
            services(
                Arc::new(StubProvidersConfigRepository::default()),
                opener,
                Arc::new(StubSetupRepository::default()),
                quota_service.clone(),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );

        let results = super::refresh_quotas_inner(&state).unwrap();

        assert!(
            !quota_service
                .calls()
                .contains(&"fresh-provider".to_string())
        );
        let fresh = results
            .iter()
            .find(|entry| entry.provider_name == "fresh-provider")
            .unwrap();
        assert_eq!(fresh.status, "fresh");
    }

    #[test]
    fn age38_refresh_quotas_wraps_injected_db_open_error_and_skips_quota_service() {
        let dir = tempfile::tempdir().unwrap();
        let quota_service = Arc::new(StubQuotaService::updated());
        let state = AppState::with_services(
            dir.path().join("models"),
            HashMap::from([(
                "multi".to_string(),
                make_model("multi", &["age38-a", "age38-b"]),
            )]),
            services(
                Arc::new(StubProvidersConfigRepository::default()),
                Arc::new(StubStateDbOpener::failing("sentinel opener failure")),
                Arc::new(StubSetupRepository::default()),
                quota_service.clone(),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );

        let err = match super::refresh_quotas_inner(&state) {
            Ok(_) => panic!("refresh_quotas_inner should return the injected opener error"),
            Err(err) => err,
        };

        assert_eq!(err, "Failed to open state DB: sentinel opener failure");
        assert!(quota_service.calls().is_empty());
    }

    #[test]
    fn age38_provider_account_commands_route_through_setup_repository() {
        let dir = tempfile::tempdir().unwrap();
        let state_db_path = dir.path().join("state.db");
        let db = StateDb::open(&state_db_path).unwrap();
        db.upsert_cli_provider(&cli_provider("codex", "OpenAI"))
            .unwrap();
        drop(db);
        let setup = Arc::new(StubSetupRepository::with_provider(cli_provider(
            "codex",
            "Stub OpenAI",
        )));
        let state = AppState::with_services(
            dir.path().join("models"),
            HashMap::new(),
            services(
                Arc::new(StubProvidersConfigRepository::default()),
                Arc::new(StubStateDbOpener::opening(state_db_path)),
                setup.clone(),
                Arc::new(StubQuotaService::updated()),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );

        let providers = super::list_cli_providers_inner(&state).unwrap();
        let fetched = super::get_cli_provider_inner(&state, "codex".to_string()).unwrap();
        let account = super::add_account_inner(
            &state,
            AddAccountInput {
                id: "acct-1".to_string(),
                provider: "codex".to_string(),
                profile_name: "default".to_string(),
                auth_method: AuthMethod::OAuth,
            },
        )
        .unwrap();
        let _ = super::list_accounts_inner(&state, Some("codex".to_string())).unwrap();
        let removed =
            super::remove_account_inner(&state, "acct-1".to_string(), "codex".to_string()).unwrap();

        assert_eq!(providers[0].display_name, "Stub OpenAI");
        assert_eq!(fetched.display_name, "Stub OpenAI");
        assert_eq!(account.auth_status, AuthStatus::Unknown);
        assert!(removed);
        assert_eq!(
            setup.calls(),
            vec![
                "list_cli_providers",
                "get_cli_provider:codex",
                "get_cli_provider:codex",
                "insert_account:codex:acct-1",
                "list_accounts:Some(\"codex\")",
                "delete_account:codex:acct-1",
            ]
        );
    }

    #[test]
    fn age38_sync_provider_persist_record_routes_through_setup_repository() {
        let dir = tempfile::tempdir().unwrap();
        let setup = Arc::new(StubSetupRepository::default());
        let state = AppState::with_services(
            dir.path().join("models"),
            HashMap::new(),
            services(
                Arc::new(StubProvidersConfigRepository::default()),
                Arc::new(StubStateDbOpener::opening(dir.path().join("state.db"))),
                setup.clone(),
                Arc::new(StubQuotaService::updated()),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );
        let record = cli_provider("claude", "Anthropic");

        super::sync_provider_persist_record(&state, &record).unwrap();

        assert_eq!(setup.calls(), vec!["upsert_cli_provider:claude"]);
    }

    #[test]
    fn age38_discovery_reads_route_through_setup_repository() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        let db = StateDb::open(&db_path).unwrap();
        db.upsert_discovered_model(&discovered_model("codex", "gpt-5", "codex-1"))
            .unwrap();
        db.upsert_model_parameter("gpt-5", "codex", &model_parameter("temperature"))
            .unwrap();
        drop(db);
        let setup = Arc::new(StubSetupRepository::default());
        setup.set_discovery_fixture(
            discovered_model("codex", "stub-gpt", "stub-version"),
            model_parameter("stub-param"),
        );
        let state = AppState::with_services(
            dir.path().join("models"),
            HashMap::new(),
            services(
                Arc::new(StubProvidersConfigRepository::default()),
                Arc::new(StubStateDbOpener::opening(db_path)),
                setup.clone(),
                Arc::new(StubQuotaService::updated()),
                Arc::new(StubExecutorService::with_exit(0, b"", "")),
                Arc::new(StubDiagnosticsService::returning(false)),
            ),
        );

        let models =
            super::list_discovered_models_inner(&state, Some("codex".to_string())).unwrap();
        let params =
            super::get_model_parameters_inner(&state, "gpt-5".to_string(), "codex".to_string())
                .unwrap();

        assert_eq!(models[0].canonical_name, "stub-gpt");
        assert_eq!(params[0].name, "stub-param");
        assert_eq!(
            setup.calls(),
            vec![
                "list_discovered_models:Some(\"codex\")",
                "list_model_parameters:codex:gpt-5",
            ]
        );
    }

    #[test]
    fn age38_discovery_persistence_source_routes_through_setup_repository_in_order() {
        let source = include_str!("lib.rs");
        let persist_start = source
            .find("fn persist_discovery_result(")
            .expect("persist_discovery_result helper exists");
        let persist_end = source[persist_start..]
            .find("#[tauri::command]\nfn list_discovered_models")
            .map(|idx| persist_start + idx)
            .expect("list_discovered_models follows persist_discovery_result");
        let persist_body = &source[persist_start..persist_end];
        let delete = persist_body
            .find("delete_stale_models")
            .expect("delete stale models through SetupRepository");
        let upsert_model = persist_body
            .find("upsert_discovered_model")
            .expect("upsert discovered models through SetupRepository");
        let upsert_param = persist_body
            .find("upsert_model_parameter")
            .expect("upsert model parameters through SetupRepository");

        assert!(
            persist_body.contains("SetupRepository"),
            "persist_discovery_result must call the SetupRepository trait, not inherent StateDb methods"
        );
        assert!(
            delete < upsert_model && upsert_model < upsert_param,
            "discovery persistence must delete stale rows before model and parameter upserts"
        );
    }

    #[test]
    fn age38_discovery_persistence_routes_through_setup_repository_with_stub_calls() {
        let empty_setup = StubSetupRepository::default();
        let empty_result = discovery::DiscoveryResult {
            cli_name: "codex".to_string(),
            cli_version: "1.2.3".to_string(),
            models: vec![],
            parameters: vec![],
        };

        let returned = super::persist_discovery_result(&empty_setup, "codex", empty_result)
            .expect("empty discovery result should persist");

        assert!(returned.is_empty());
        assert!(empty_setup.calls().is_empty());

        let setup = StubSetupRepository::default();
        let model = discovered_model("codex", "gpt-5", "1.2.3");
        let parameter = model_parameter("temperature");
        let result = discovery::DiscoveryResult {
            cli_name: "codex".to_string(),
            cli_version: "1.2.3".to_string(),
            models: vec![model.clone()],
            parameters: vec![("gpt-5".to_string(), parameter)],
        };

        let returned = super::persist_discovery_result(&setup, "codex", result)
            .expect("non-empty discovery result should persist");

        assert_eq!(returned.len(), 1);
        assert_eq!(returned[0].canonical_name, model.canonical_name);
        assert_eq!(returned[0].provider, model.provider);
        assert_eq!(returned[0].cli_version, model.cli_version);
        assert_eq!(
            setup.calls(),
            vec![
                "delete_stale_models:codex:1.2.3",
                "upsert_discovered_model:codex:gpt-5",
                "upsert_model_parameter:codex:gpt-5:temperature",
            ]
        );
    }

    #[test]
    fn age38_test_model_success_routes_effective_request_through_stub_ports() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let stdout = b"ok \xF0\x28\x8C\x28";
        let model = make_model("age38-model", &["age38-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_exit(0, stdout, "");
        let diagnostics = StubDiagnosticsService::returning(false);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result = super::test_model_with_db_path(
            services,
            model,
            models_dir.clone(),
            db_path.clone(),
            "hello",
        )
        .expect("successful model test should return a result");

        assert!(result.success);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, String::from_utf8_lossy(stdout).into_owned());
        assert_eq!(opener.calls(), vec![db_path]);
        assert_eq!(providers.calls(), vec![dir.path().join("providers.toml")]);
        assert_eq!(routing.calls(), vec!["select:age38-model:true"]);
        assert_eq!(
            executor.calls(),
            vec!["effective:age38-provider:0:true:true"]
        );
        assert!(diagnostics.calls().is_empty());
    }

    #[test]
    fn tauri_test_model_injects_policy() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let model = model_with_provider_args("age28-model", "claude", &["--model", "opus"]);
        let mut entries = HashMap::new();
        entries.insert(
            "claude".to_string(),
            ProviderEntry {
                command: Some("claude".to_string()),
                args: vec!["-p".to_string()],
                system_prompt_override: Some("AGE-28 test_model policy".to_string()),
                tool_restrictions: Some(ToolRestrictions {
                    kind: ToolRestrictionKind::Claude,
                    claude: ClaudeRestrictions {
                        disallowed_tools: vec!["Task".to_string()],
                        allowed_tools: Vec::new(),
                        disable_slash_commands: false,
                    },
                    codex: CodexRestrictions::default(),
                }),
                ..ProviderEntry::default()
            },
        );
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::returning(Ok(ProvidersConfig { entries }));
        let routing = StubRoutingService::selecting(0);
        let executor = PolicyRecordingExecutorService::default();
        let diagnostics = StubDiagnosticsService::returning(false);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result = super::test_model_with_db_path(
            services,
            model,
            models_dir,
            db_path,
            "Say hello in one sentence.",
        )
        .expect("test_model should execute through effective provider path");

        assert!(result.success);
        let captured = executor.providers();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].args, ["-p", "--model", "opus"]);
        assert_eq!(
            captured[0].system_prompt_override.as_deref(),
            Some("AGE-28 test_model policy")
        );
        assert_eq!(
            captured[0].tool_restrictions,
            Some(ToolRestrictions {
                kind: ToolRestrictionKind::Claude,
                claude: ClaudeRestrictions {
                    disallowed_tools: vec!["Task".to_string()],
                    allowed_tools: Vec::new(),
                    disable_slash_commands: false,
                },
                codex: CodexRestrictions::default(),
            })
        );
    }

    #[test]
    fn age38_test_model_nonzero_not_exhausted_classifies_without_marking_quota() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let db = StateDb::open(&db_path).unwrap();
        db.upsert_quota_refresh("age38-provider", &[]).unwrap();
        drop(db);
        let model = make_model("age38-model", &["age38-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_exit(7, b"", "quota warning");
        let diagnostics = StubDiagnosticsService::returning(false);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result =
            super::test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
                .expect("non-exhausted nonzero model test should return a result");

        assert!(!result.success);
        assert_eq!(result.exit_code, 7);
        assert_eq!(diagnostics.calls(), vec!["classify:quota warning"]);
        let quota = StateDb::open(&db_path)
            .unwrap()
            .get_quota("age38-provider")
            .unwrap()
            .expect("quota row should remain present");
        assert!(quota.exhausted_at.is_none());
    }

    #[test]
    fn age38_test_model_nonzero_exhausted_classifies_and_marks_quota() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let model = make_model("age38-model", &["age38-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_exit(7, b"", "quota exhausted");
        let diagnostics = StubDiagnosticsService::returning(true);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result =
            super::test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
                .expect("exhausted nonzero model test should return a result");

        assert!(!result.success);
        assert_eq!(result.exit_code, 7);
        assert_eq!(diagnostics.calls(), vec!["classify:quota exhausted"]);
        let quota = StateDb::open(&db_path)
            .unwrap()
            .get_quota("age38-provider")
            .unwrap()
            .expect("exhaustion marking should create a quota row");
        assert!(quota.exhausted_at.is_some());
    }

    #[test]
    fn age156_test_model_typed_rate_limited_signal_does_not_mark_exhausted_even_when_legacy_classifier_would()
     {
        // AGE-156: typed-signal precedence over legacy broad-string classifier.
        //
        // Scenario: the partitioned recognizer (AGE-162 WU-B) emits
        // `TerminalSignalKind::RateLimited` for a transient HTTP 429 / rate-limit
        // signature. The legacy `classify_exhaustion` classifier — left as the
        // degraded-mode fallback — still returns true for this stderr (it
        // matches the broad `"rate limit"` / `"too many requests"` patterns
        // historically). Under the AGE-156 consolidation, the typed signal is
        // AUTHORITATIVE, so the durable `exhausted_at` write MUST NOT happen
        // and the legacy classifier MUST NOT be consulted at all.
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let db = StateDb::open(&db_path).unwrap();
        db.upsert_quota_refresh("age156-provider", &[]).unwrap();
        drop(db);
        let model = make_model("age156-model", &["age156-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_signal(
            7,
            b"",
            "HTTP 429: too many requests. rate_limit_error encountered.",
            TestTerminalSignalKind::RateLimited,
            "age156-provider",
        );
        // Stub returns `true` to prove the typed signal wins even when the
        // legacy classifier would write `exhausted_at`. With the typed-signal
        // precedence in place, this stub must never be called.
        let diagnostics = StubDiagnosticsService::returning(true);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result =
            super::test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
                .expect("rate-limited typed signal should still return a result");

        assert!(!result.success);
        assert_eq!(result.exit_code, 7);
        assert!(
            diagnostics.calls().is_empty(),
            "typed terminal signal must short-circuit the legacy classifier; \
             diagnostics calls observed: {:?}",
            diagnostics.calls()
        );
        let quota = StateDb::open(&db_path)
            .unwrap()
            .get_quota("age156-provider")
            .unwrap()
            .expect("quota row should remain present");
        assert!(
            quota.exhausted_at.is_none(),
            "transient RateLimited typed signal must NOT flip exhausted_at — \
             the AGE-156 consolidation gates the durable write behind the \
             persistent-quota typed kind only"
        );
    }

    #[test]
    fn test_model_maybe_signal_is_non_durable() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let db = StateDb::open(&db_path).unwrap();
        db.upsert_quota_refresh("age166-provider", &[]).unwrap();
        drop(db);
        let model = make_model("age166-model", &["age166-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_signal(
            7,
            b"maybe stdout",
            "quota-looking text must not drive durability",
            TestTerminalSignalKind::MaybeQuotaExhausted,
            "age166-provider",
        );
        let diagnostics = StubDiagnosticsService::returning(true);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result =
            super::test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
                .expect("maybe quota signal should still return a test-model result");

        assert!(!result.success);
        assert_eq!(result.exit_code, 7);
        assert!(
            diagnostics.calls().is_empty(),
            "MaybeQuotaExhausted must not invoke legacy diagnostics in test_model"
        );
        assert_eq!(
            StateDb::open(&db_path)
                .unwrap()
                .get_quota("age166-provider")
                .unwrap()
                .unwrap()
                .exhausted_at,
            None
        );
    }

    #[test]
    fn age156_test_model_typed_quota_exhausted_inband_signal_marks_exhausted_even_when_legacy_classifier_would_not()
     {
        // AGE-156 branch-coverage companion to the rate-limit case above.
        //
        // Scenario: the partitioned recognizer emits
        // `TerminalSignalKind::QuotaExhaustedInband` for a canonical persistent
        // quota signature. The legacy classifier stub is rigged to return
        // `false` (i.e., would have refused to mark exhausted). The typed
        // signal MUST still drive the durable `exhausted_at` write, and the
        // legacy classifier MUST NOT be consulted.
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let model = make_model("age156-model", &["age156-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_signal(
            7,
            b"",
            "Claude usage limit reached for your account.",
            TestTerminalSignalKind::QuotaExhaustedInband,
            "age156-provider",
        );
        let diagnostics = StubDiagnosticsService::returning(false);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result =
            super::test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
                .expect("quota-exhausted typed signal should return a result");

        assert!(!result.success);
        assert!(
            diagnostics.calls().is_empty(),
            "typed terminal signal must short-circuit the legacy classifier; \
             diagnostics calls observed: {:?}",
            diagnostics.calls()
        );
        let quota = StateDb::open(&db_path)
            .unwrap()
            .get_quota("age156-provider")
            .unwrap()
            .expect("exhaustion marking should create a quota row");
        assert!(
            quota.exhausted_at.is_some(),
            "persistent QuotaExhaustedInband typed signal must flip exhausted_at"
        );
    }

    #[test]
    fn test_model_nonzero_stdout_exhausted_classifies_and_marks_quota() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let db_path = dir.path().join("state.db");
        let model = make_model("age38-model", &["age38-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_exit(
            7,
            br#"{"api_error_status":429,"result":"You've hit your limit"}"#,
            "",
        );
        let diagnostics = StubDiagnosticsService::returning(true);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result =
            super::test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
                .expect("stdout-exhausted nonzero model test should return a result");

        assert!(!result.success);
        assert_eq!(
            diagnostics.calls(),
            vec![r#"classify:{"api_error_status":429,"result":"You've hit your limit"}"#]
        );
        let quota = StateDb::open(&db_path)
            .unwrap()
            .get_quota("age38-provider")
            .unwrap()
            .expect("exhaustion marking should create a quota row");
        assert!(quota.exhausted_at.is_some());
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
            provider: None,
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
        let db = StateDb::open(&db_path).unwrap();
        let returned = super::persist_discovery_result(&db, "codex", empty_result).unwrap();
        drop(db);
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
        let db = StateDb::open(&db_path).unwrap();
        let returned = super::persist_discovery_result(&db, "codex", result).unwrap();
        drop(db);

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
            provider: None,
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
                provider: None,
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
                provider: None,
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
        let db_path = dir.path().join("state.db");
        let db = StateDb::open(&db_path).unwrap();
        db.upsert_quota_refresh("quota-provider", &[]).unwrap();
        drop(db);
        let model = make_model("quota-model", &["quota-provider"]);
        let opener = StubStateDbOpener::opening(db_path.clone());
        let providers = StubProvidersConfigRepository::default();
        let routing = StubRoutingService::selecting(0);
        let executor = StubExecutorService::with_signal(
            7,
            b"",
            "typed quota signal",
            TestTerminalSignalKind::QuotaExhaustedInband,
            "quota-provider",
        );
        let diagnostics = StubDiagnosticsService::returning(false);
        let services = TestModelServices {
            state_db_opener: &opener,
            providers_repository: &providers,
            routing_service: &routing,
            executor_service: &executor,
            diagnostics_service: &diagnostics,
        };

        let result =
            super::test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
                .unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, 7);
        assert!(result.stderr.contains("typed quota signal"));
        let db = StateDb::open(&db_path).unwrap();
        let quota = db.get_quota("quota-provider").unwrap().unwrap();
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
            provider: None,
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
            provider: None,
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
        // AGE-166: substring quota detection was removed in PR #126; this
        // effective-provider subcase still proves the providers.toml command is
        // used, but quota-looking stderr is no longer durable by itself.
        let quota = db.get_quota("quota-effective-provider").unwrap();
        assert!(
            quota.and_then(|quota| quota.exhausted_at).is_none(),
            "quota-looking stderr without a typed signal must not mark exhausted"
        );
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
            provider: None,
        };
        let mut models = HashMap::new();
        models.insert(model.name.clone(), model);

        let result = test_model_for_test(models, models_dir, "sigterm-model").unwrap();

        assert!(!result.success);
        assert_eq!(result.exit_code, 143);
    }

    // risk: Cross-language IPC drift; level: Rust Tauri command; source: contract "Argument casing must be stable for TypeScript callers"
    #[test]
    fn provider_settings_command_args_deserialize_camel_case_ipc_payloads() {
        let schema_args: provider_settings::GetProviderSettingsSchemaArgs =
            serde_json::from_value(serde_json::json!({
                "modelName": "example-model",
                "schemaId": "example.settings/v1",
            }))
            .expect("camelCase schema payload should deserialize for Tauri IPC");
        assert_eq!(schema_args.model_name, "example-model");
        assert_eq!(schema_args.schema_id, "example.settings/v1");

        let update_args: provider_settings::UpdateProviderSettingsArgs =
            serde_json::from_value(serde_json::json!({
                "modelName": "example-model",
                "id": "record",
                "version": "opaque-version",
                "values": {"endpoint": "https://example.test", "enabled": true},
            }))
            .expect("camelCase update payload should deserialize for Tauri IPC");
        assert_eq!(update_args.model_name, "example-model");
        assert_eq!(update_args.id, "record");
        assert_eq!(update_args.version, "opaque-version");
        assert_eq!(
            update_args.values["endpoint"],
            serde_json::json!("https://example.test")
        );

        let migrate_args: provider_settings::MigrateProviderSettingsArgs =
            serde_json::from_value(serde_json::json!({
                "modelName": "example-model",
                "dryRun": true,
                "legacy": {"providers": {"provider-a": {"command": "example"}}},
            }))
            .expect("camelCase migrate payload should deserialize for Tauri IPC");
        assert_eq!(migrate_args.model_name, "example-model");
        assert!(migrate_args.dry_run);
        assert_eq!(
            migrate_args.legacy["providers"]["provider-a"]["command"],
            "example"
        );
    }

    // risk: Provider diagnostics loss and opaque version data loss; level: Rust Tauri command; source: contract "Structured IPC DTOs"
    #[test]
    fn provider_settings_command_preserves_structured_conflict_and_transport_errors() {
        let mut harness = provider_settings::ProviderSettingsCommandHarness::new();
        harness.fail_update_with_provider_error(provider_settings::ProviderSettingsErrorDto {
            category: "conflict".to_string(),
            code: Some("settings_conflict".to_string()),
            message: "record changed".to_string(),
            retryable: Some(false),
            details: Some(serde_json::json!({"remoteVersion": "remote-version"})),
            diagnostics: vec![provider_settings::ProviderDiagnosticDto {
                severity: "warning".to_string(),
                message: "Reload before saving".to_string(),
                path: Some("/endpoint".to_string()),
                code: Some("stale".to_string()),
            }],
            process_status: Some(provider_settings::ProviderProcessStatusDto {
                exit_code: Some(17),
                signal: None,
                kind: "exited".to_string(),
            }),
        });

        let conflict = provider_settings::update_provider_settings_inner(
            harness.state(),
            provider_settings::UpdateProviderSettingsArgs {
                model_name: "example-model".to_string(),
                id: "record".to_string(),
                version: "stale-version".to_string(),
                values: serde_json::json!({"endpoint": "https://example.test"}),
            },
        )
        .expect_err("provider conflict must surface as structured conflict DTO");

        assert_eq!(conflict.category, "conflict");
        assert_eq!(conflict.code.as_deref(), Some("settings_conflict"));
        assert_eq!(conflict.retryable, Some(false));
        assert_eq!(conflict.details.unwrap()["remoteVersion"], "remote-version");
        assert_eq!(conflict.diagnostics[0].path.as_deref(), Some("/endpoint"));
        assert_eq!(
            conflict.process_status.and_then(|status| status.exit_code),
            Some(17)
        );

        harness.fail_validate_with_transport_error("transport", "provider process failed");
        let transport = provider_settings::validate_provider_settings_inner(
            harness.state(),
            provider_settings::ValidateProviderSettingsArgs {
                model_name: "example-model".to_string(),
                values: serde_json::json!({"endpoint": "https://example.test"}),
            },
        )
        .expect_err("transport/capability failures must surface as structured error DTOs");
        assert_eq!(transport.category, "transport");
        assert_eq!(transport.message, "provider process failed");
        assert!(transport.diagnostics.is_empty());
    }

    // risk: Provider diagnostics loss; level: Rust Tauri command; source: contract "settings.migrate diagnostics must survive the real provider-host mapper"
    #[cfg(unix)]
    #[test]
    fn provider_settings_command_preserves_migration_diagnostics_from_real_host() {
        let dir = tempfile::tempdir().unwrap();
        let provider_path = dir.path().join("provider-settings.py");
        write_executable(
            &provider_path,
            provider_settings_diagnostic_provider_script(),
        );
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model = model_with_provider_artifact("example-model", "provider-a", &provider_path);
        let state = test_state(models_dir, HashMap::from([(model.name.clone(), model)]));

        let migrated = provider_settings::migrate_provider_settings_inner(
            &state,
            provider_settings::MigrateProviderSettingsArgs {
                model_name: "example-model".to_string(),
                dry_run: true,
                legacy: serde_json::json!({"providers": {"provider-a": {"command": "example"}}}),
            },
        )
        .expect("real provider settings host should map provider migration output");

        assert_eq!(
            migrated.actions,
            vec![serde_json::json!({"kind": "would-write", "target": "record"})]
        );
        assert_eq!(migrated.warnings, vec!["review before apply"]);
        assert!(migrated.requires_user_input);
        assert_eq!(migrated.diagnostics.len(), 1);
        assert_eq!(migrated.diagnostics[0].severity, "warning");
        assert_eq!(migrated.diagnostics[0].message, "Legacy field needs review");
        assert_eq!(
            migrated.diagnostics[0].path.as_deref(),
            Some("/providers/provider-a")
        );
        assert_eq!(
            migrated.diagnostics[0].code.as_deref(),
            Some("legacy_field")
        );
    }

    // risk: Supported-surface target listing failure; level: Rust Tauri command; source: contract "central-config-only models must not break provider settings targets"
    #[cfg(unix)]
    #[test]
    fn provider_settings_targets_skip_central_config_only_models() {
        let dir = tempfile::tempdir().unwrap();
        let provider_path = dir.path().join("provider-settings.py");
        write_executable(
            &provider_path,
            provider_settings_diagnostic_provider_script(),
        );
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let artifact_model =
            model_with_provider_artifact("artifact-model", "provider-a", &provider_path);
        let central_model = make_model("central-only-model", &["provider-a"]);
        let state = test_state(
            models_dir,
            HashMap::from([
                (artifact_model.name.clone(), artifact_model),
                (central_model.name.clone(), central_model),
            ]),
        );

        let targets = provider_settings::list_provider_settings_targets_inner(&state)
            .expect("mixed central and artifact models should list configured targets");

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].model_name, "artifact-model");
        assert_eq!(targets[0].provider_id, "provider-a");
    }

    #[cfg(unix)]
    fn provider_settings_diagnostic_provider_script() -> &'static str {
        r#"#!/usr/bin/env python3
import json
import sys

subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{}")

def envelope(result):
    return {
        "contract": request.get("contract"),
        "request_id": request.get("request_id"),
        "ok": True,
        "result": result,
    }

if subcommand == "describe":
    response = envelope({
        "provider_id": "provider-a",
        "display_name": "Provider A",
        "contract_versions": [request.get("contract")],
        "preferred_contract": request.get("contract"),
        "capabilities": {
            "launch": True,
            "policy": False,
            "quota": False,
            "session": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": True,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        },
        "settings_schema_id": "example.settings/v1",
    })
elif subcommand == "settings.migrate":
    response = envelope({
        "actions": [{"kind": "would-write", "target": "record"}],
        "warnings": ["review before apply"],
        "requires_user_input": True,
        "diagnostics": [{
            "severity": "warning",
            "message": "Legacy field needs review",
            "path": "/providers/provider-a",
            "code": "legacy_field",
        }],
    })
else:
    response = {
        "contract": request.get("contract"),
        "request_id": request.get("request_id"),
        "ok": False,
        "error": {
            "category": "unsupported",
            "code": "unsupported_subcommand",
            "message": "unsupported",
            "retryable": False,
        },
    }

print(json.dumps(response))
"#
    }

    // risk: Migration mutating or interpreting central config; level: Rust Tauri command; source: contract "settings.migrate receives opaque legacy central-config blocks"
    #[cfg(unix)]
    #[test]
    fn provider_settings_migration_packages_central_config_blocks_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let provider_path = dir.path().join("provider-settings.py");
        let record_path = dir.path().join("migration-record.jsonl");
        write_executable(
            &provider_path,
            &provider_settings_migration_recording_provider_script(&record_path),
        );
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model_path = models_dir.join("example-model.toml");
        let providers_path = dir.path().join("providers.toml");
        let provider_path_toml =
            serde_json::to_string(&provider_path.display().to_string()).unwrap();
        let model_toml = format!(
            r#"
name = "example-model"
prompt_mode = "arg"

[[providers]]
name = "provider-a"
args = ["--profile", "record"]

[provider]
path = {provider_path_toml}
"#
        );
        let providers_toml = r#"
[provider-a]
command = "example"
args = ["--endpoint", "https://example.test"]
prompt_mode = "arg"
"#;
        std::fs::write(&model_path, model_toml).unwrap();
        std::fs::write(&providers_path, providers_toml).unwrap();
        let providers = super::load_providers_for_models_dir(&models_dir);
        let models = config::load_models(&models_dir, Some(&providers)).unwrap();
        let state = test_state(models_dir.clone(), models);
        let before_model = std::fs::read_to_string(&model_path).unwrap();
        let before_providers = std::fs::read_to_string(&providers_path).unwrap();

        let legacy = provider_settings::package_migration_legacy_payload(&state, "example-model")
            .expect("migration legacy packaging should read central config");

        assert_eq!(
            legacy["models"]["example-model"]["providers"][0]["name"],
            "provider-a"
        );
        assert_eq!(
            legacy["models"]["example-model"]["providers"][0]["args"][0],
            "--profile"
        );
        assert_eq!(
            legacy["models"]["example-model"]["provider"]["path"],
            provider_path.display().to_string()
        );
        assert_eq!(legacy["providers"]["provider-a"]["command"], "example");
        assert_eq!(
            legacy["providers"]["provider-a"]["args"][1],
            "https://example.test"
        );
        assert_eq!(std::fs::read_to_string(&model_path).unwrap(), before_model);
        assert_eq!(
            std::fs::read_to_string(&providers_path).unwrap(),
            before_providers
        );

        let migrated = provider_settings::migrate_provider_settings_inner(
            &state,
            provider_settings::MigrateProviderSettingsArgs {
                model_name: "example-model".to_string(),
                dry_run: false,
                legacy: serde_json::Value::Null,
            },
        )
        .expect("real provider migrate should receive packaged central config");
        assert_eq!(
            migrated.actions,
            vec![serde_json::json!({"kind": "would-write", "target": "record"})]
        );
        assert_eq!(migrated.warnings, vec!["review"]);

        let recorded = read_provider_settings_migration_record(&record_path);
        assert_eq!(recorded["params"]["dry_run"], false);
        assert_eq!(
            recorded["params"]["legacy"]["models"]["example-model"]["providers"][0]["name"],
            "provider-a"
        );
        assert_eq!(
            recorded["params"]["legacy"]["providers"]["provider-a"]["command"],
            "example"
        );
        assert_eq!(std::fs::read_to_string(&model_path).unwrap(), before_model);
        assert_eq!(
            std::fs::read_to_string(&providers_path).unwrap(),
            before_providers
        );
    }

    #[cfg(unix)]
    fn provider_settings_migration_recording_provider_script(record_path: &Path) -> String {
        let record_path = serde_json::to_string(&record_path.display().to_string()).unwrap();
        format!(
            r#"#!/usr/bin/env python3
import json
import pathlib
import sys

record_path = pathlib.Path({record_path})
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")

def envelope(result):
    return {{
        "contract": request.get("contract"),
        "request_id": request.get("request_id"),
        "ok": True,
        "result": result,
    }}

if subcommand == "describe":
    response = envelope({{
        "provider_id": "provider-a",
        "display_name": "Provider A",
        "contract_versions": [request.get("contract")],
        "preferred_contract": request.get("contract"),
        "capabilities": {{
            "launch": True,
            "policy": False,
            "quota": False,
            "session": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": True,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
        "settings_schema_id": "example.settings/v1",
    }})
elif subcommand == "settings.migrate":
    with record_path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(request, sort_keys=True) + "\n")
    response = envelope({{
        "actions": [{{"kind": "would-write", "target": "record"}}],
        "warnings": ["review"],
        "requires_user_input": False,
        "diagnostics": [],
    }})
else:
    response = {{
        "contract": request.get("contract"),
        "request_id": request.get("request_id"),
        "ok": False,
        "error": {{
            "category": "unsupported",
            "code": "unsupported_subcommand",
            "message": "unsupported",
            "retryable": False,
        }},
    }}

print(json.dumps(response))
"#,
            record_path = record_path
        )
    }

    #[cfg(unix)]
    fn read_provider_settings_migration_record(record_path: &Path) -> serde_json::Value {
        std::fs::read_to_string(record_path)
            .expect("settings.migrate record should exist")
            .lines()
            .map(|line| serde_json::from_str(line).expect("recorded request should parse"))
            .next()
            .expect("settings.migrate request should be recorded")
    }
}
