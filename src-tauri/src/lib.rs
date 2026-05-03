pub mod balancer;
pub mod config;
pub mod diagnostics;
pub mod discovery;
pub mod executor;
pub mod migration;
pub mod process;
pub mod quota;
pub mod runtime;
pub mod schema_probe;
pub mod session_export;
pub mod session_lock;
pub mod session_metadata;
pub mod session_replace;
pub mod sessions;
pub mod setup;
pub mod state;
pub mod trace;

use config::{
    AgentConfigRepository, FilesystemAgentConfigRepository, FilesystemModelConfigRepository,
    FilesystemProviderConfigSource, FilesystemSessionsConfigSource, ModelConfig,
    ModelConfigRepository, ProviderConfigSource, SessionsConfigSource,
};
pub use runtime::{DefaultRuntimePaths, RuntimePaths};
use serde::{Deserialize, Serialize};
use session_lock::{FilesystemSessionLockProvider, SessionLockProvider};
use setup::actions::{SetupEvent, UserResponse};
#[allow(unused_imports)]
use state::StateDb;
use state::{AccountRecord, AuthMethod, AuthStatus, CliProviderRecord};
use state::{
    CliProviderRepository, DefaultStateDbOpener, DiscoveryRepository, QuotaRepository,
    StateDbOpener,
};
use state::{DiscoveredModel, ModelParameter};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

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
    pub paths: Arc<dyn RuntimePaths>,
    pub state_opener: Arc<dyn StateDbOpener + Send + Sync>,
    pub model_repo: Arc<dyn ModelConfigRepository + Send + Sync>,
    pub provider_source: Arc<dyn ProviderConfigSource + Send + Sync>,
    pub sessions_source: Arc<dyn SessionsConfigSource + Send + Sync>,
    pub agent_repo: Arc<dyn AgentConfigRepository + Send + Sync>,
    pub process_runner: Arc<dyn process::ProcessRunner>,
    pub lock_provider: Arc<dyn SessionLockProvider>,
    pub quota_in_flight: quota::InFlight,
    pub setup_input_tx: Mutex<Option<mpsc::Sender<UserResponse>>>,
}

#[tauri::command]
fn check_setup_needed(state: tauri::State<AppState>) -> Result<bool, String> {
    let models = state.model_repo.load_models().unwrap_or_default();
    check_setup_needed_with_runner(state.process_runner.as_ref(), models.is_empty())
}

pub fn check_setup_needed_with_runner(
    runner: &dyn process::ProcessRunner,
    models_empty: bool,
) -> Result<bool, String> {
    if models_empty {
        return Ok(true);
    }
    let output = runner.run(process::CommandSpec {
        program: "which".to_string(),
        args: vec!["claude".to_string()],
        cwd: None,
        env: Default::default(),
        stdin: process::StdinSpec::Null,
        stdout: process::OutputSpec::Capture,
        stderr: process::OutputSpec::Capture,
        timeout: None,
        description: "check claude cli".to_string(),
    });
    match output {
        Ok(o) if o.exit_code == 0 => Ok(false),
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
    let db_path = state.paths.state_db_path()?;
    let runner = Arc::clone(&state.process_runner);

    tauri::async_runtime::spawn(async move {
        let memory = match setup::memory::MemoryGraph::open(&db_path) {
            Ok(m) => m,
            Err(e) => {
                let _ = on_event.send(SetupEvent::Error {
                    message: format!("Failed to open memory store: {e}"),
                    recoverable: false,
                });
                return;
            }
        };

        let flow = setup::flow::SetupFlow::new_with_runner(on_event, rx, memory, sid, runner);
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
    let db_path = state.paths.state_db_path()?;
    let cli = cli_name.clone();
    let runner = Arc::clone(&state.process_runner);

    tauri::async_runtime::spawn(async move {
        let memory = match setup::memory::MemoryGraph::open(&db_path) {
            Ok(m) => m,
            Err(e) => {
                let _ = on_event.send(SetupEvent::Error {
                    message: format!("Failed to open memory store: {e}"),
                    recoverable: false,
                });
                return;
            }
        };

        let flow = setup::flow::SetupFlow::new_with_runner(on_event, rx, memory, sid, runner);
        flow.run_for_cli(&cli).await;
    });

    Ok(session_id)
}

#[tauri::command]
fn reload_models(state: tauri::State<AppState>) -> Result<(), String> {
    let _ = state.model_repo.load_models()?;
    Ok(())
}

#[tauri::command]
fn detect_clis(state: tauri::State<AppState>) -> Result<setup::detection::DetectionReport, String> {
    Ok(setup::detection::detect_all_with_runner(
        state.process_runner.as_ref(),
    ))
}

#[tauri::command]
fn get_memory_graph(
    state: tauri::State<AppState>,
) -> Result<setup::memory::MemorySnapshot, String> {
    let db_path = state.paths.state_db_path()?;
    let graph = setup::memory::MemoryGraph::open(&db_path)?;
    graph.snapshot()
}

#[tauri::command]
fn list_models(state: tauri::State<AppState>) -> Result<Vec<ModelSummary>, String> {
    let models = state.model_repo.load_models()?;
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
    let models = state.model_repo.load_models()?;
    models
        .get(&name)
        .cloned()
        .ok_or_else(|| format!("Model '{}' not found", name))
}

#[tauri::command]
fn save_model(state: tauri::State<AppState>, model: ModelConfig) -> Result<(), String> {
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
    state.model_repo.save_model(&model)?;
    Ok(())
}

#[tauri::command]
fn delete_model(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    state.model_repo.delete_model(&name)?;
    Ok(())
}

#[tauri::command]
fn list_pools(state: tauri::State<AppState>) -> Result<Vec<PoolSummary>, String> {
    let models = state.model_repo.load_models()?;
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
    providers: Option<Vec<String>>,
) -> Result<Vec<QuotaRefreshEntry>, String> {
    let candidates: Vec<String> = if let Some(providers) = providers {
        providers
    } else {
        let models = state.model_repo.load_models()?;
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

    let providers_cfg = state.provider_source.load_providers().unwrap_or_default();
    let db_path = state.paths.state_db_path()?;
    let db = state
        .state_opener
        .open(&db_path)
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

        let quota_repo: &dyn QuotaRepository = &db;
        let outcome = quota::refresh_provider(
            &provider_name,
            &providers_cfg,
            in_flight,
            quota_repo,
            state.process_runner.as_ref(),
        );
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
    if new_commands.is_empty() {
        return Err("Pool must have at least one command".to_string());
    }

    let mut orig_sorted = original_commands.clone();
    orig_sorted.sort();
    orig_sorted.dedup();

    let mut new_sorted = new_commands.clone();
    new_sorted.sort();
    new_sorted.dedup();

    let mut models = state.model_repo.load_models()?;

    // Find models matching the original command set (using provider names)
    let matching_names: Vec<String> = models
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
        let model = models.get_mut(name).unwrap();

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

        save_pool_model(state.model_repo.as_ref(), model)?;
    }

    Ok(())
}

fn save_pool_model(repo: &dyn ModelConfigRepository, model: &ModelConfig) -> Result<(), String> {
    repo.save_model(model).map_err(|e| {
        e.strip_prefix("Failed to write model file: ")
            .map(|source| format!("Failed to write model file for '{}': {source}", model.name))
            .unwrap_or(e)
    })
}

#[tauri::command]
async fn test_model(
    state: tauri::State<'_, AppState>,
    name: Option<String>,
    model_name: Option<String>,
) -> Result<TestModelResult, String> {
    let name = name
        .or(model_name)
        .ok_or_else(|| "Model name is required".to_string())?;
    let model = {
        let models = state.model_repo.load_models()?;
        models
            .get(&name)
            .cloned()
            .ok_or_else(|| format!("Model '{}' not found", name))?
    };

    let providers_cfg = state.provider_source.load_providers().unwrap_or_default();
    let db_path = state.paths.state_db_path()?;
    let opener = Arc::clone(&state.state_opener);
    let runner = Arc::clone(&state.process_runner);

    let result = tauri::async_runtime::spawn_blocking(move || {
        test_model_with_deps(
            model,
            &providers_cfg,
            opener.as_ref(),
            runner.as_ref(),
            db_path,
            "Say hello in one sentence.",
        )
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(result)
}

fn test_model_with_db_path(
    model: ModelConfig,
    db_path: PathBuf,
    prompt: &str,
) -> Result<TestModelResult, String> {
    let opener = DefaultStateDbOpener;
    let runner = process::OsProcessRunner;
    test_model_with_deps(
        model,
        &config::ProvidersConfig::default(),
        &opener,
        &runner,
        db_path,
        prompt,
    )
}

fn test_model_with_deps(
    model: ModelConfig,
    providers_cfg: &config::ProvidersConfig,
    opener: &dyn StateDbOpener,
    runner: &dyn process::ProcessRunner,
    db_path: PathBuf,
    prompt: &str,
) -> Result<TestModelResult, String> {
    let db = opener.open(&db_path).map_err(|e| e.to_string())?;
    let provider_index = balancer::select_provider(&model, &db, None);
    let result = match providers_cfg.effective_provider(&model.providers[provider_index]) {
        Ok((provider, prompt_mode)) => executor::cli::execute_effective_with_inputs_and_runner(
            runner,
            executor::cli::EffectiveExecuteWithInputsRequest {
                model: &model,
                provider: &provider,
                provider_index,
                prompt_mode,
                prompt,
                working_dir: None,
                extra_inputs: &HashMap::new(),
                parent_invocation_env: None,
            },
        )?,
        Err(_) => executor::execute_with_runner(
            runner,
            &model,
            provider_index,
            prompt,
            None,
            &HashMap::new(),
            None,
        )?,
    };
    if result.exit_code != 0 && diagnostics::classify_exhaustion(&result.stderr) {
        let quota_repo: &dyn QuotaRepository = &db;
        quota_repo.mark_exhausted(&model.providers[provider_index].name)?;
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
    test_model_with_db_path(model, db_path, "Say hello in one sentence.")
}

// --- Provider & Account commands ---

/// Helper to open the state DB from AppState.
fn open_state_db(state: &AppState) -> Result<StateDb, String> {
    let db_path = state.paths.state_db_path()?;
    state.state_opener.open(&db_path)
}

#[tauri::command]
fn list_cli_providers(state: tauri::State<AppState>) -> Result<Vec<CliProviderRecord>, String> {
    let db = open_state_db(&state)?;
    let repo: &dyn CliProviderRepository = &db;
    repo.list_cli_providers()
}

#[tauri::command]
fn get_cli_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    let db = open_state_db(&state)?;
    let repo: &dyn CliProviderRepository = &db;
    repo.get_cli_provider(&cli_name)?
        .ok_or_else(|| format!("Provider '{}' not found", cli_name))
}

#[tauri::command]
fn list_accounts(
    state: tauri::State<AppState>,
    provider: Option<String>,
) -> Result<Vec<AccountRecord>, String> {
    let db = open_state_db(&state)?;
    let repo: &dyn CliProviderRepository = &db;
    repo.list_accounts(provider.as_deref())
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
    if account.id.is_empty() {
        return Err("Account id cannot be empty".to_string());
    }
    if account.provider.is_empty() {
        return Err("Account provider cannot be empty".to_string());
    }
    if account.profile_name.is_empty() {
        return Err("Account profile_name cannot be empty".to_string());
    }

    let db = open_state_db(&state)?;
    let repo: &dyn CliProviderRepository = &db;

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
}

#[tauri::command]
fn remove_account(
    state: tauri::State<AppState>,
    id: String,
    provider: String,
) -> Result<bool, String> {
    let db = open_state_db(&state)?;
    let repo: &dyn CliProviderRepository = &db;
    repo.delete_account(&id, &provider)
}

#[tauri::command]
fn sync_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    let cli_info =
        setup::detection::detect_single_cli_with_runner(&cli_name, state.process_runner.as_ref());

    let display_name = match cli_name.as_str() {
        "claude" => "Anthropic",
        "codex" => "OpenAI",
        "gemini" => "Google",
        "opencode" => "OpenCode",
        _ => &cli_name,
    };

    let now = chrono::Utc::now().to_rfc3339();
    let record = CliProviderRecord {
        cli_name: cli_info.name,
        display_name: display_name.to_string(),
        installed: cli_info.installed,
        version: cli_info.version,
        config_dir: cli_info.config_dir.map(|p| p.to_string_lossy().to_string()),
        last_synced: Some(now),
    };

    let db = open_state_db(&state)?;
    let repo: &dyn CliProviderRepository = &db;
    repo.upsert_cli_provider(&record)?;
    Ok(record)
}

// --- Discovery commands ---

#[tauri::command]
async fn discover_models_cmd(
    state: tauri::State<'_, AppState>,
    cli_name: String,
) -> Result<Vec<DiscoveredModel>, String> {
    let db_path = state.paths.state_db_path()?;
    let opener = Arc::clone(&state.state_opener);
    let runner = Arc::clone(&state.process_runner);

    tauri::async_runtime::spawn_blocking(move || {
        let result = discovery::discover_models_with_runner(&cli_name, runner.as_ref())?;

        let db = opener.open(&db_path)?;
        let repo: &dyn DiscoveryRepository = &db;

        // Clean out models from older CLI versions
        if !result.models.is_empty() {
            repo.delete_stale_models(&cli_name, &result.cli_version)?;
        }

        // Store discovered models
        for model in &result.models {
            repo.upsert_discovered_model(model)?;
        }

        // Store discovered parameters
        for (model_name, param) in &result.parameters {
            repo.upsert_model_parameter(model_name, &cli_name, param)?;
        }

        Ok(result.models)
    })
    .await
    .map_err(|e| format!("Discovery task failed: {e}"))?
}

#[tauri::command]
fn list_discovered_models(
    state: tauri::State<AppState>,
    provider: Option<String>,
) -> Result<Vec<DiscoveredModel>, String> {
    let db = open_state_db(&state)?;
    let repo: &dyn DiscoveryRepository = &db;
    repo.list_discovered_models(provider.as_deref())
}

#[tauri::command]
fn get_model_parameters(
    state: tauri::State<AppState>,
    model_name: String,
    provider: String,
) -> Result<Vec<ModelParameter>, String> {
    let db = open_state_db(&state)?;
    let repo: &dyn DiscoveryRepository = &db;
    repo.list_model_parameters(&model_name, &provider)
}

pub fn run_tauri() {
    let paths = Arc::new(DefaultRuntimePaths::new()) as Arc<dyn RuntimePaths>;
    let model_repo = Arc::new(FilesystemModelConfigRepository::new(paths.models_dir()))
        as Arc<dyn ModelConfigRepository + Send + Sync>;
    let provider_source = Arc::new(FilesystemProviderConfigSource::new(paths.providers_path()))
        as Arc<dyn ProviderConfigSource + Send + Sync>;
    let sessions_source = Arc::new(FilesystemSessionsConfigSource::new(paths.sessions_path()))
        as Arc<dyn SessionsConfigSource + Send + Sync>;
    let agent_repo = Arc::new(FilesystemAgentConfigRepository::new(paths.agents_dir()))
        as Arc<dyn AgentConfigRepository + Send + Sync>;

    tauri::Builder::default()
        .manage(AppState {
            paths,
            state_opener: Arc::new(DefaultStateDbOpener),
            model_repo,
            provider_source,
            sessions_source,
            agent_repo,
            process_runner: Arc::new(process::OsProcessRunner),
            lock_provider: Arc::new(FilesystemSessionLockProvider),
            quota_in_flight: quota::InFlight::new(),
            setup_input_tx: Mutex::new(None),
        })
        .invoke_handler(configure_tauri_app())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub fn configure_tauri_app<R: tauri::Runtime>()
-> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
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
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{ModelConfig, PromptMode, ProviderConfig};
    use tauri::Manager;

    struct TestRuntimePaths {
        root: PathBuf,
        models_dir: PathBuf,
    }

    impl RuntimePaths for TestRuntimePaths {
        fn data_root(&self) -> Result<PathBuf, String> {
            Ok(self.root.clone())
        }

        fn config_root(&self) -> PathBuf {
            self.root.clone()
        }

        fn models_dir(&self) -> PathBuf {
            self.models_dir.clone()
        }

        fn agents_dir(&self) -> PathBuf {
            self.root.join("agents")
        }

        fn state_db_path(&self) -> Result<PathBuf, String> {
            Ok(self.root.join("state.db"))
        }

        fn providers_path(&self) -> PathBuf {
            self.root.join("providers.toml")
        }

        fn sessions_path(&self) -> PathBuf {
            self.root.join("sessions.toml")
        }

        fn lock_dir(&self) -> Result<PathBuf, String> {
            Ok(self.root.join("locks"))
        }

        fn replace_journal_dir(&self) -> Result<PathBuf, String> {
            Ok(self.root.join("replace_journal"))
        }
    }

    struct TestModelRepo {
        models: Mutex<HashMap<String, ModelConfig>>,
        models_dir: PathBuf,
    }

    impl ModelConfigRepository for TestModelRepo {
        fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String> {
            Ok(self.models.lock().unwrap().clone())
        }

        fn save_model(&self, model: &ModelConfig) -> Result<(), String> {
            std::fs::create_dir_all(&self.models_dir)
                .map_err(|e| format!("Failed to create models directory: {e}"))?;
            std::fs::write(
                self.models_dir.join(format!("{}.toml", model.name)),
                model.to_toml(),
            )
            .map_err(|e| format!("Failed to write model file: {e}"))?;
            self.models
                .lock()
                .unwrap()
                .insert(model.name.clone(), model.clone());
            Ok(())
        }

        fn delete_model(&self, name: &str) -> Result<(), String> {
            std::fs::remove_file(self.models_dir.join(format!("{name}.toml")))
                .map_err(|e| format!("Failed to delete model file: {e}"))?;
            self.models.lock().unwrap().remove(name);
            Ok(())
        }
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

    fn mock_app_with_state(
        models: HashMap<String, ModelConfig>,
        models_dir: PathBuf,
    ) -> tauri::App<tauri::test::MockRuntime> {
        let root = models_dir
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let paths = Arc::new(TestRuntimePaths {
            root: root.clone(),
            models_dir: models_dir.clone(),
        }) as Arc<dyn RuntimePaths>;
        tauri::test::mock_builder()
            .manage(AppState {
                paths,
                state_opener: Arc::new(DefaultStateDbOpener),
                model_repo: Arc::new(TestModelRepo {
                    models: Mutex::new(models),
                    models_dir,
                }),
                provider_source: Arc::new(FilesystemProviderConfigSource::new(
                    root.join("providers.toml"),
                )),
                sessions_source: Arc::new(FilesystemSessionsConfigSource::new(
                    root.join("sessions.toml"),
                )),
                agent_repo: Arc::new(FilesystemAgentConfigRepository::new(root.join("agents"))),
                process_runner: Arc::new(process::OsProcessRunner),
                lock_provider: Arc::new(FilesystemSessionLockProvider),
                setup_input_tx: Mutex::new(None),
                quota_in_flight: quota::InFlight::new(),
            })
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap()
    }

    fn assert_model_keys(app: &tauri::App<tauri::test::MockRuntime>, expected: &[&str]) {
        let state = app.state::<AppState>();
        let models = state.model_repo.load_models().unwrap();
        assert_eq!(models.len(), expected.len());
        for name in expected {
            assert!(models.contains_key(*name), "missing model key {name}");
        }
    }

    fn provider_names(model: &ModelConfig) -> Vec<String> {
        model.providers.iter().map(|p| p.name.clone()).collect()
    }

    fn assert_model_provider_names(
        app: &tauri::App<tauri::test::MockRuntime>,
        name: &str,
        expected: &[&str],
    ) {
        let state = app.state::<AppState>();
        let models = state.model_repo.load_models().unwrap();
        let model = models
            .get(name)
            .unwrap_or_else(|| panic!("missing model {name}"));
        let expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        assert_eq!(provider_names(model), expected);
    }

    fn read_model_from_disk(models_dir: &std::path::Path, name: &str) -> ModelConfig {
        let content = std::fs::read_to_string(models_dir.join(format!("{name}.toml"))).unwrap();
        ModelConfig::from_toml(name, &content).unwrap()
    }

    fn account_test_app(dir: &tempfile::TempDir) -> tauri::App<tauri::test::MockRuntime> {
        let models_dir = dir.path().join("models");
        mock_app_with_state(HashMap::new(), models_dir)
    }

    fn account_test_db(app: &tauri::App<tauri::test::MockRuntime>) -> StateDb {
        open_state_db(&app.state::<AppState>()).unwrap()
    }

    fn cli_provider(cli_name: &str) -> CliProviderRecord {
        CliProviderRecord {
            cli_name: cli_name.to_string(),
            display_name: format!("{cli_name} display"),
            installed: true,
            version: Some("1.0.0".to_string()),
            config_dir: Some(format!("/tmp/{cli_name}")),
            last_synced: None,
        }
    }

    fn seed_cli_provider(app: &tauri::App<tauri::test::MockRuntime>, cli_name: &str) {
        account_test_db(app)
            .upsert_cli_provider(&cli_provider(cli_name))
            .unwrap();
    }

    fn account_record(
        id: &str,
        provider: &str,
        profile_name: &str,
        auth_method: AuthMethod,
        auth_status: AuthStatus,
    ) -> AccountRecord {
        AccountRecord {
            id: id.to_string(),
            provider: provider.to_string(),
            profile_name: profile_name.to_string(),
            auth_method,
            auth_status,
            created_at: "2026-05-02T00:00:00Z".to_string(),
        }
    }

    fn seed_account(app: &tauri::App<tauri::test::MockRuntime>, account: AccountRecord) {
        account_test_db(app).insert_account(&account).unwrap();
    }

    fn persisted_accounts(app: &tauri::App<tauri::test::MockRuntime>) -> Vec<AccountRecord> {
        account_test_db(app).list_accounts(None).unwrap()
    }

    #[test]
    fn add_account_rejects_empty_account_id_without_inserting_row() {
        let dir = tempfile::tempdir().unwrap();
        let app = account_test_app(&dir);
        seed_cli_provider(&app, "claude");

        let result = add_account(
            app.state::<AppState>(),
            AddAccountInput {
                id: "".to_string(),
                provider: "claude".to_string(),
                profile_name: "work-profile".to_string(),
                auth_method: AuthMethod::OAuth,
            },
        );

        assert!(result.is_err());
        assert!(persisted_accounts(&app).is_empty());
    }

    #[test]
    fn add_account_rejects_empty_provider_id_without_inserting_row() {
        let dir = tempfile::tempdir().unwrap();
        let app = account_test_app(&dir);

        let result = add_account(
            app.state::<AppState>(),
            AddAccountInput {
                id: "work".to_string(),
                provider: "".to_string(),
                profile_name: "work-profile".to_string(),
                auth_method: AuthMethod::OAuth,
            },
        );

        assert!(result.is_err());
        assert!(persisted_accounts(&app).is_empty());
    }

    #[test]
    fn add_account_rejects_empty_profile_name_without_inserting_row() {
        let dir = tempfile::tempdir().unwrap();
        let app = account_test_app(&dir);
        seed_cli_provider(&app, "claude");

        let result = add_account(
            app.state::<AppState>(),
            AddAccountInput {
                id: "work".to_string(),
                provider: "claude".to_string(),
                profile_name: "".to_string(),
                auth_method: AuthMethod::OAuth,
            },
        );

        assert!(result.is_err());
        assert!(persisted_accounts(&app).is_empty());
    }

    #[test]
    fn add_account_rejects_unknown_provider_without_inserting_row() {
        let dir = tempfile::tempdir().unwrap();
        let app = account_test_app(&dir);

        let result = add_account(
            app.state::<AppState>(),
            AddAccountInput {
                id: "work".to_string(),
                provider: "claude".to_string(),
                profile_name: "work-profile".to_string(),
                auth_method: AuthMethod::OAuth,
            },
        );

        assert!(result.is_err());
        assert!(persisted_accounts(&app).is_empty());
    }

    #[test]
    fn add_account_inserts_returns_and_persists_valid_account_with_unknown_auth_status() {
        let dir = tempfile::tempdir().unwrap();
        let app = account_test_app(&dir);
        seed_cli_provider(&app, "claude");
        let auth_method = AuthMethod::ApiKey {
            env_var: "ANTHROPIC_API_KEY".to_string(),
            config_path: Some("/tmp/anthropic-key".to_string()),
        };

        let inserted = add_account(
            app.state::<AppState>(),
            AddAccountInput {
                id: "work".to_string(),
                provider: "claude".to_string(),
                profile_name: "work-profile".to_string(),
                auth_method: auth_method.clone(),
            },
        )
        .unwrap();

        assert_eq!(inserted.id, "work");
        assert_eq!(inserted.provider, "claude");
        assert_eq!(inserted.profile_name, "work-profile");
        assert_eq!(inserted.auth_method, auth_method);
        assert_eq!(inserted.auth_status, AuthStatus::Unknown);
        assert!(!inserted.created_at.is_empty());

        let accounts = persisted_accounts(&app);
        assert_eq!(accounts.len(), 1);
        let persisted = &accounts[0];
        assert_eq!(persisted.id, inserted.id);
        assert_eq!(persisted.provider, inserted.provider);
        assert_eq!(persisted.profile_name, inserted.profile_name);
        assert_eq!(persisted.auth_method, inserted.auth_method);
        assert_eq!(persisted.auth_status, AuthStatus::Unknown);
        assert_eq!(persisted.created_at, inserted.created_at);
    }

    #[test]
    fn remove_account_deletes_existing_id_provider_pair_and_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let app = account_test_app(&dir);
        seed_cli_provider(&app, "claude");
        seed_account(
            &app,
            account_record(
                "work",
                "claude",
                "work-profile",
                AuthMethod::ConfigFile {
                    path: "~/.claude/config".to_string(),
                },
                AuthStatus::Valid,
            ),
        );

        let removed = remove_account(
            app.state::<AppState>(),
            "work".to_string(),
            "claude".to_string(),
        )
        .unwrap();

        assert!(removed);
        assert!(persisted_accounts(&app).is_empty());
    }

    #[test]
    fn remove_account_returns_false_for_missing_pair_and_leaves_existing_rows_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let app = account_test_app(&dir);
        seed_cli_provider(&app, "claude");
        seed_cli_provider(&app, "codex");
        seed_account(
            &app,
            account_record(
                "work",
                "claude",
                "work-profile",
                AuthMethod::OAuth,
                AuthStatus::Valid,
            ),
        );
        seed_account(
            &app,
            account_record(
                "personal",
                "codex",
                "personal-profile",
                AuthMethod::ApiKey {
                    env_var: "OPENAI_API_KEY".to_string(),
                    config_path: None,
                },
                AuthStatus::Unknown,
            ),
        );

        let removed = remove_account(
            app.state::<AppState>(),
            "missing".to_string(),
            "claude".to_string(),
        )
        .unwrap();

        assert!(!removed);
        let accounts = persisted_accounts(&app);
        assert_eq!(accounts.len(), 2);
        assert!(accounts.iter().any(|account| {
            account.id == "work"
                && account.provider == "claude"
                && account.profile_name == "work-profile"
                && account.auth_method == AuthMethod::OAuth
                && account.auth_status == AuthStatus::Valid
        }));
        assert!(accounts.iter().any(|account| {
            account.id == "personal"
                && account.provider == "codex"
                && account.profile_name == "personal-profile"
                && account.auth_method
                    == (AuthMethod::ApiKey {
                        env_var: "OPENAI_API_KEY".to_string(),
                        config_path: None,
                    })
                && account.auth_status == AuthStatus::Unknown
        }));
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

    #[test]
    fn save_model_with_empty_name_returns_error_without_writing_or_mutating_models() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let mut models = HashMap::new();
        models.insert("existing".into(), make_model("existing", &["claude"]));
        let app = mock_app_with_state(models, models_dir.clone());

        let result = save_model(app.state::<AppState>(), make_model("", &["claude"]));

        assert!(result.is_err());
        assert!(!models_dir.join(".toml").exists());
        assert_model_keys(&app, &["existing"]);
    }

    #[test]
    fn save_model_with_no_providers_returns_error_without_writing_or_mutating_models() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let mut models = HashMap::new();
        models.insert("existing".into(), make_model("existing", &["claude"]));
        let app = mock_app_with_state(models, models_dir.clone());
        let model = ModelConfig {
            name: "no-providers".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![],
            inputs: vec![],
        };

        let result = save_model(app.state::<AppState>(), model);

        assert!(result.is_err());
        assert!(!models_dir.join("no-providers.toml").exists());
        assert_model_keys(&app, &["existing"]);
    }

    #[test]
    fn save_model_with_empty_provider_name_returns_error_without_writing_or_mutating_models() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let mut models = HashMap::new();
        models.insert("existing".into(), make_model("existing", &["claude"]));
        let app = mock_app_with_state(models, models_dir.clone());
        let model = ModelConfig {
            name: "empty-provider".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider("", vec![])],
            inputs: vec![],
        };

        let result = save_model(app.state::<AppState>(), model);

        assert!(result.is_err());
        assert!(!models_dir.join("empty-provider.toml").exists());
        assert_model_keys(&app, &["existing"]);
    }

    #[test]
    fn save_model_when_models_directory_cannot_be_created_returns_error_without_mutating_models() {
        let dir = tempfile::tempdir().unwrap();
        let blocking_file = dir.path().join("not-a-directory");
        std::fs::write(&blocking_file, "blocks directory creation").unwrap();
        let models_dir = blocking_file.join("models");
        let mut models = HashMap::new();
        models.insert("existing".into(), make_model("existing", &["claude"]));
        let app = mock_app_with_state(models, models_dir);

        let result = save_model(
            app.state::<AppState>(),
            make_model("write-fails", &["claude"]),
        );

        assert!(result.is_err());
        assert_model_keys(&app, &["existing"]);
    }

    #[test]
    fn delete_model_removes_persisted_config_and_model_entry() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let model = make_model("removable", &["claude"]);
        std::fs::write(models_dir.join("removable.toml"), model.to_toml()).unwrap();
        let mut models = HashMap::new();
        models.insert("removable".into(), model);
        models.insert("kept".into(), make_model("kept", &["codex"]));
        let app = mock_app_with_state(models, models_dir.clone());

        let result = delete_model(app.state::<AppState>(), "removable".to_string());

        assert!(result.is_ok());
        assert!(!models_dir.join("removable.toml").exists());
        assert_model_keys(&app, &["kept"]);
    }

    #[test]
    fn delete_model_when_file_removal_fails_returns_error_without_mutating_models() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(models_dir.join("undeletable.toml")).unwrap();
        let mut models = HashMap::new();
        models.insert("undeletable".into(), make_model("undeletable", &["claude"]));
        models.insert("kept".into(), make_model("kept", &["codex"]));
        let app = mock_app_with_state(models, models_dir.clone());

        let result = delete_model(app.state::<AppState>(), "undeletable".to_string());

        assert!(result.is_err());
        assert!(models_dir.join("undeletable.toml").exists());
        assert_model_keys(&app, &["kept", "undeletable"]);
    }

    #[test]
    fn update_pool_with_empty_new_commands_returns_error_without_mutating_models() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let mut models = HashMap::new();
        models.insert("existing".into(), make_model("existing", &["claude"]));
        let app = mock_app_with_state(models, models_dir.clone());

        let result = update_pool(app.state::<AppState>(), vec!["claude".to_string()], vec![]);

        assert!(result.is_err());
        assert!(!models_dir.join("existing.toml").exists());
        assert_model_provider_names(&app, "existing", &["claude"]);
    }

    #[test]
    fn update_pool_rejects_unmatched_original_commands_without_mutating_models() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let mut models = HashMap::new();
        models.insert("existing".into(), make_model("existing", &["claude"]));
        let app = mock_app_with_state(models, models_dir.clone());

        let result = update_pool(
            app.state::<AppState>(),
            vec!["codex".to_string()],
            vec!["gemini".to_string()],
        );

        assert!(result.is_err());
        assert!(!models_dir.join("existing.toml").exists());
        assert_model_provider_names(&app, "existing", &["claude"]);
    }

    #[test]
    fn update_pool_applies_deduped_command_set_to_all_matching_models_and_persists_them() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let mut models = HashMap::new();
        models.insert("alpha".into(), make_model("alpha", &["claude", "codex"]));
        models.insert("beta".into(), make_model("beta", &["codex", "claude"]));
        models.insert("gamma".into(), make_model("gamma", &["gemini"]));
        let app = mock_app_with_state(models, models_dir.clone());

        let result = update_pool(
            app.state::<AppState>(),
            vec![
                "codex".to_string(),
                "claude".to_string(),
                "claude".to_string(),
            ],
            vec![
                "gemini".to_string(),
                "claude".to_string(),
                "gemini".to_string(),
            ],
        );

        assert!(result.is_ok());
        assert_model_provider_names(&app, "alpha", &["claude", "gemini"]);
        assert_model_provider_names(&app, "beta", &["claude", "gemini"]);
        assert_model_provider_names(&app, "gamma", &["gemini"]);

        assert_eq!(
            provider_names(&read_model_from_disk(&models_dir, "alpha")),
            vec!["claude".to_string(), "gemini".to_string()]
        );
        assert_eq!(
            provider_names(&read_model_from_disk(&models_dir, "beta")),
            vec!["claude".to_string(), "gemini".to_string()]
        );
        assert!(!models_dir.join("gamma.toml").exists());
    }

    #[test]
    fn update_pool_matches_prefixed_runtime_provider_by_provider_name() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let mut model = make_model("prefixed", &[]);
        model.providers = vec![ProviderConfig::new(
            "env",
            vec![
                "-u".to_string(),
                "CLAUDECODE".to_string(),
                "claude".to_string(),
            ],
        )];
        let mut models = HashMap::new();
        models.insert("prefixed".into(), model);
        let app = mock_app_with_state(models, models_dir.clone());

        let result = update_pool(
            app.state::<AppState>(),
            vec!["claude".to_string()],
            vec!["codex".to_string()],
        );

        assert!(result.is_ok());
        assert_model_provider_names(&app, "prefixed", &["codex"]);
        assert_eq!(
            provider_names(&read_model_from_disk(&models_dir, "prefixed")),
            vec!["codex".to_string()]
        );
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
}
