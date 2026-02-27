pub mod balancer;
pub mod config;
pub mod diagnostics;
pub mod discovery;
pub mod executor;
pub mod setup;
pub mod state;

use config::{ModelConfig, PromptMode};
use setup::actions::{SetupEvent, UserResponse};
#[allow(unused_imports)]
use state::StateDb;
use state::{AccountRecord, AuthMethod, AuthStatus, CliProviderRecord};
use state::{DiscoveredModel, ModelParameter};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use serde::{Deserialize, Serialize};
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
    pub prompt_mode: PromptMode,
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
        // Group by extracted provider names (last token, quotes stripped)
        // so `env -u CLAUDECODE claude` groups as "claude".
        let mut cmds: Vec<String> = model.providers.iter()
            .map(|p| executor::provider_name(&p.command))
            .collect();
        cmds.sort();
        cmds.dedup();
        groups.entry(cmds).or_default().push(model.name.clone());
    }

    let mut pools: Vec<PoolSummary> = groups.into_iter().map(|(commands, mut model_names)| {
        model_names.sort();
        PoolSummary {
            model_count: model_names.len(),
            commands,
            model_names,
        }
    }).collect();

    pools.sort_by(|a, b| a.commands.cmp(&b.commands));
    pools
}

pub struct AppState {
    pub models: Mutex<HashMap<String, config::ModelConfig>>,
    pub models_dir: PathBuf,
    pub setup_input_tx: Mutex<Option<mpsc::Sender<UserResponse>>>,
}

#[tauri::command]
fn check_setup_needed(state: tauri::State<AppState>) -> Result<bool, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    if models.is_empty() {
        return Ok(true);
    }
    // Check if claude CLI is available
    let output = std::process::Command::new("which")
        .arg("claude")
        .output();
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
    let db_path = state.models_dir.parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");

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

        let flow = setup::flow::SetupFlow::new(
            on_event,
            rx,
            memory,
            sid,
        );
        flow.run().await;
    });

    Ok(session_id)
}

#[tauri::command]
fn setup_respond(
    state: tauri::State<AppState>,
    response: UserResponse,
) -> Result<(), String> {
    let guard = state.setup_input_tx.lock().map_err(|e| e.to_string())?;
    if let Some(ref tx) = *guard {
        tx.blocking_send(response).map_err(|e| format!("Failed to send response: {e}"))
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
    let db_path = state.models_dir.parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");
    let cli = cli_name.clone();

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

        let flow = setup::flow::SetupFlow::new(
            on_event,
            rx,
            memory,
            sid,
        );
        flow.run_for_cli(&cli).await;
    });

    Ok(session_id)
}

#[tauri::command]
fn reload_models(state: tauri::State<AppState>) -> Result<(), String> {
    let fresh = config::load_models(&state.models_dir).unwrap_or_default();
    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    *models = fresh;
    Ok(())
}

#[tauri::command]
fn detect_clis() -> Result<setup::detection::DetectionReport, String> {
    Ok(setup::detection::detect_all())
}

#[tauri::command]
fn get_memory_graph(state: tauri::State<AppState>) -> Result<setup::memory::MemorySnapshot, String> {
    let db_path = state.models_dir.parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");
    let graph = setup::memory::MemoryGraph::open(&db_path)?;
    graph.snapshot()
}

#[tauri::command]
fn list_models(state: tauri::State<AppState>) -> Result<Vec<ModelSummary>, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    let mut summaries: Vec<ModelSummary> = models.values().map(|m| ModelSummary {
        name: m.name.clone(),
        prompt_mode: m.prompt_mode,
        provider_count: m.providers.len(),
    }).collect();
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summaries)
}

#[tauri::command]
fn get_model(state: tauri::State<AppState>, name: String) -> Result<ModelConfig, String> {
    let models = state.models.lock().map_err(|e| e.to_string())?;
    models.get(&name).cloned().ok_or_else(|| format!("Model '{}' not found", name))
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
        if p.command.is_empty() {
            return Err(format!("Provider {} has empty command", i + 1));
        }
    }

    let toml_content = model.to_toml();
    let path = state.models_dir.join(format!("{}.toml", model.name));

    std::fs::create_dir_all(&state.models_dir)
        .map_err(|e| format!("Failed to create models directory: {e}"))?;
    std::fs::write(&path, &toml_content)
        .map_err(|e| format!("Failed to write model file: {e}"))?;

    let mut models = state.models.lock().map_err(|e| e.to_string())?;
    models.insert(model.name.clone(), model);
    Ok(())
}

#[tauri::command]
fn delete_model(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    let path = state.models_dir.join(format!("{}.toml", name));
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete model file: {e}"))?;
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

    let mut models = state.models.lock().map_err(|e| e.to_string())?;

    // Find models matching the original command set (using provider names)
    let matching_names: Vec<String> = models.values().filter(|m| {
        let mut cmds: Vec<String> = m.providers.iter()
            .map(|p| executor::provider_name(&p.command))
            .collect();
        cmds.sort();
        cmds.dedup();
        cmds == orig_sorted
    }).map(|m| m.name.clone()).collect();

    if matching_names.is_empty() {
        return Err("No models found with the specified command set".to_string());
    }

    // Compute added and removed provider names
    let removed: Vec<&String> = orig_sorted.iter().filter(|c| !new_sorted.contains(c)).collect();
    let added: Vec<&String> = new_sorted.iter().filter(|c| !orig_sorted.contains(c)).collect();

    for name in &matching_names {
        let model = models.get_mut(name).unwrap();

        // Remove providers whose extracted provider name is in the removed set
        model.providers.retain(|p| !removed.contains(&&executor::provider_name(&p.command)));

        // Add providers with empty args for new commands
        for cmd in &added {
            model.providers.push(config::ProviderConfig {
                command: (*cmd).clone(),
                args: vec![],
            });
        }

        if model.providers.is_empty() {
            return Err(format!("Model '{}' would end up with zero providers", name));
        }

        // Write updated toml
        let toml_content = model.to_toml();
        let path = state.models_dir.join(format!("{}.toml", name));
        std::fs::write(&path, &toml_content)
            .map_err(|e| format!("Failed to write model file for '{}': {e}", name))?;
    }

    Ok(())
}

#[tauri::command]
async fn test_model(state: tauri::State<'_, AppState>, name: String) -> Result<TestModelResult, String> {
    let model = {
        let models = state.models.lock().map_err(|e| e.to_string())?;
        models.get(&name).cloned()
            .ok_or_else(|| format!("Model '{}' not found", name))?
    };

    let db_path = state.models_dir.parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");

    let result = tauri::async_runtime::spawn_blocking(move || {
        let db = state::StateDb::open(&db_path).map_err(|e| e.to_string())?;
        let provider_index = balancer::select_provider(&model, &db);
        executor::execute(&model, provider_index, "Say hello in one sentence.", None)
    }).await.map_err(|e| e.to_string())??;

    Ok(TestModelResult {
        success: result.exit_code == 0,
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
    })
}

// --- Provider & Account commands ---

/// Helper to open the state DB from AppState.
fn open_state_db(state: &AppState) -> Result<StateDb, String> {
    let db_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");
    StateDb::open(&db_path)
}

#[tauri::command]
fn list_cli_providers(state: tauri::State<AppState>) -> Result<Vec<CliProviderRecord>, String> {
    let db = open_state_db(&state)?;
    db.list_cli_providers()
}

#[tauri::command]
fn get_cli_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    let db = open_state_db(&state)?;
    db.get_cli_provider(&cli_name)?
        .ok_or_else(|| format!("Provider '{}' not found", cli_name))
}

#[tauri::command]
fn list_accounts(
    state: tauri::State<AppState>,
    provider: Option<String>,
) -> Result<Vec<AccountRecord>, String> {
    let db = open_state_db(&state)?;
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
    let db = open_state_db(&state)?;
    db.delete_account(&id, &provider)
}

#[tauri::command]
fn sync_provider(
    state: tauri::State<AppState>,
    cli_name: String,
) -> Result<CliProviderRecord, String> {
    // Detect the current state of this CLI using the existing detection module
    let cli_info = setup::detection::detect_single_cli(&cli_name);

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
    db.upsert_cli_provider(&record)?;
    Ok(record)
}

// --- Discovery commands ---

#[tauri::command]
async fn discover_models_cmd(
    state: tauri::State<'_, AppState>,
    cli_name: String,
) -> Result<Vec<DiscoveredModel>, String> {
    let db_path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("state.db");

    tauri::async_runtime::spawn_blocking(move || {
        let result = discovery::discover_models(&cli_name)?;

        let db = StateDb::open(&db_path)?;

        // Clean out models from older CLI versions
        if !result.models.is_empty() {
            db.delete_stale_models(&cli_name, &result.cli_version)?;
        }

        // Store discovered models
        for model in &result.models {
            db.upsert_discovered_model(model)?;
        }

        // Store discovered parameters
        for (model_name, param) in &result.parameters {
            db.upsert_model_parameter(model_name, &cli_name, param)?;
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
    db.list_discovered_models(provider.as_deref())
}

#[tauri::command]
fn get_model_parameters(
    state: tauri::State<AppState>,
    model_name: String,
    provider: String,
) -> Result<Vec<ModelParameter>, String> {
    let db = open_state_db(&state)?;
    db.list_model_parameters(&model_name, &provider)
}

pub fn run_tauri() {
    let models_dir = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"));

    let models = config::load_models(&models_dir).unwrap_or_default();

    tauri::Builder::default()
        .manage(AppState {
            models: Mutex::new(models),
            models_dir: models_dir.clone(),
            setup_input_tx: Mutex::new(None),
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
    use config::{ModelConfig, ProviderConfig, PromptMode};

    fn make_model(name: &str, commands: &[&str]) -> ModelConfig {
        ModelConfig {
            name: name.to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: commands.iter().map(|c| ProviderConfig {
                command: c.to_string(),
                args: vec![],
            }).collect(),
        }
    }

    #[test]
    fn derive_pools_groups_by_command_set() {
        let mut models = HashMap::new();
        models.insert("a".into(), make_model("a", &["claude", "codex"]));
        models.insert("b".into(), make_model("b", &["claude", "codex"]));
        models.insert("c".into(), make_model("c", &["gemini"]));

        let pools = derive_pools(&models);
        assert_eq!(pools.len(), 2);

        let pool_claude = pools.iter().find(|p| p.commands.contains(&"claude".to_string())).unwrap();
        assert_eq!(pool_claude.model_count, 2);
        assert!(pool_claude.model_names.contains(&"a".to_string()));
        assert!(pool_claude.model_names.contains(&"b".to_string()));

        let pool_gemini = pools.iter().find(|p| p.commands.contains(&"gemini".to_string())).unwrap();
        assert_eq!(pool_gemini.model_count, 1);
        assert_eq!(pool_gemini.model_names, vec!["c".to_string()]);
    }

    #[test]
    fn derive_pools_deduplicates_commands() {
        let mut models = HashMap::new();
        // Model with duplicate commands should deduplicate
        models.insert("x".into(), ModelConfig {
            name: "x".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![
                ProviderConfig { command: "claude".to_string(), args: vec![] },
                ProviderConfig { command: "claude".to_string(), args: vec!["-p".to_string()] },
            ],
        });
        models.insert("y".into(), make_model("y", &["claude"]));

        let pools = derive_pools(&models);
        // Both should be in the same pool since deduped command set is ["claude"]
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].model_count, 2);
    }

    #[test]
    fn derive_pools_extracts_provider_from_prefixed_command() {
        let mut models = HashMap::new();
        // Command with env prefix should group by the last token ("claude")
        models.insert("a".into(), ModelConfig {
            name: "a".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig {
                command: "env -u CLAUDECODE claude".to_string(),
                args: vec![],
            }],
        });
        // Plain command should also group as "claude"
        models.insert("b".into(), make_model("b", &["claude"]));

        let pools = derive_pools(&models);
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].commands, vec!["claude".to_string()]);
        assert_eq!(pools[0].model_count, 2);
    }
}
