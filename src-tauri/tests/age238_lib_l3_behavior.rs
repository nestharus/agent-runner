use agent_runner_lib::{AppState, commands};
use oulipoly_runtime::discovery;
use oulipoly_setup as setup_core;
use oulipoly_state::repositories::SetupRepository;
use oulipoly_state::{
    AccountRecord, AuthMethod, AuthStatus, CliMapping, CliProviderRecord, DiscoveredModel,
    ModelParameter, ParamType, StateDb,
};
use std::collections::HashMap;
use std::sync::Mutex;

fn test_state(models_dir: std::path::PathBuf) -> AppState {
    AppState::test_default(models_dir, HashMap::new())
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
        param_type: ParamType::String,
        description: format!("{name} parameter"),
        cli_mapping: CliMapping {
            flag: format!("--{name}"),
            value_template: "{value}".to_string(),
        },
    }
}

#[derive(Default)]
struct RecordingSetupRepository {
    calls: Mutex<Vec<String>>,
}

impl RecordingSetupRepository {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl SetupRepository for RecordingSetupRepository {
    fn upsert_cli_provider(&self, provider: &CliProviderRecord) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("upsert_cli_provider:{}", provider.cli_name));
        Ok(())
    }

    fn list_cli_providers(&self) -> Result<Vec<CliProviderRecord>, String> {
        self.calls
            .lock()
            .unwrap()
            .push("list_cli_providers".to_string());
        Ok(vec![cli_provider("codex", "Stub OpenAI")])
    }

    fn get_cli_provider(&self, cli_name: &str) -> Result<Option<CliProviderRecord>, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("get_cli_provider:{cli_name}"));
        Ok(Some(cli_provider(cli_name, "Stub OpenAI")))
    }

    fn insert_account(&self, account: &AccountRecord) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!(
            "insert_account:{}:{}",
            account.provider, account.id
        ));
        Ok(())
    }

    fn list_accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("list_accounts:{provider:?}"));
        Ok(vec![])
    }

    fn delete_account(&self, id: &str, provider: &str) -> Result<bool, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("delete_account:{provider}:{id}"));
        Ok(true)
    }

    fn upsert_discovered_model(&self, model: &DiscoveredModel) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!(
            "upsert_discovered_model:{}:{}",
            model.provider, model.canonical_name
        ));
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
        Ok(vec![discovered_model("codex", "stub-gpt", "stub-version")])
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
        Ok(vec![model_parameter("stub-param")])
    }
}

#[test]
fn provider_account_commands_validate_and_persist_through_state_db() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path().join("models"));
    let db = StateDb::open(&state.db_path()).unwrap();
    db.upsert_cli_provider(&cli_provider("codex", "OpenAI"))
        .unwrap();
    drop(db);

    assert_eq!(
        commands::providers_accounts::orchestration::add_account_inner(
            &state,
            commands::providers_accounts::AddAccountInput {
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
        commands::providers_accounts::orchestration::add_account_inner(
            &state,
            commands::providers_accounts::AddAccountInput {
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
        commands::providers_accounts::orchestration::add_account_inner(
            &state,
            commands::providers_accounts::AddAccountInput {
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
        commands::providers_accounts::orchestration::add_account_inner(
            &state,
            commands::providers_accounts::AddAccountInput {
                id: "acct-1".to_string(),
                provider: "missing".to_string(),
                profile_name: "default".to_string(),
                auth_method: AuthMethod::OAuth,
            },
        )
        .unwrap_err(),
        "Provider 'missing' not found"
    );

    let added = commands::providers_accounts::orchestration::add_account_inner(
        &state,
        commands::providers_accounts::AddAccountInput {
            id: "acct-1".to_string(),
            provider: "codex".to_string(),
            profile_name: "default".to_string(),
            auth_method: AuthMethod::OAuth,
        },
    )
    .unwrap();

    assert_eq!(added.auth_status, AuthStatus::Unknown);
    chrono::DateTime::parse_from_rfc3339(&added.created_at).unwrap();
    let accounts = commands::providers_accounts::orchestration::list_accounts_inner(
        &state,
        Some("codex".to_string()),
    )
    .unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, "acct-1");
    assert_eq!(
        commands::providers_accounts::orchestration::list_cli_providers_inner(&state)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        commands::providers_accounts::orchestration::get_cli_provider_inner(
            &state,
            "codex".to_string()
        )
        .unwrap()
        .display_name,
        "OpenAI"
    );
    assert!(
        commands::providers_accounts::orchestration::remove_account_inner(
            &state,
            "acct-1".to_string(),
            "codex".to_string()
        )
        .unwrap()
    );
    assert!(
        !commands::providers_accounts::orchestration::remove_account_inner(
            &state,
            "acct-1".to_string(),
            "codex".to_string()
        )
        .unwrap()
    );
}

#[test]
fn sync_provider_maps_display_name_and_persists_with_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path().join("models"));
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

    let record = commands::providers_accounts::orchestration::sync_provider_record_from_cli_info(
        "codex", cli_info,
    );
    commands::providers_accounts::orchestration::sync_provider_persist_record(&state, &record)
        .unwrap();

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

    assert_eq!(
        commands::providers_accounts::orchestration::sync_provider_display_name("claude"),
        "Anthropic"
    );
    assert_eq!(
        commands::providers_accounts::orchestration::sync_provider_display_name("gemini"),
        "Google"
    );
    assert_eq!(
        commands::providers_accounts::orchestration::sync_provider_display_name("opencode"),
        "OpenCode"
    );
    assert_eq!(
        commands::providers_accounts::orchestration::sync_provider_display_name("custom"),
        "custom"
    );
}

#[test]
fn discovery_persistence_source_routes_through_setup_repository_in_order() {
    let source = include_str!("../src/commands/discovery/accessor.rs");
    let persist_body = {
        let start = source
            .find("pub fn persist_discovery_result(")
            .expect("persist_discovery_result helper exists");
        let end = source[start..]
            .find("\npub fn delete_stale_models")
            .map(|idx| start + idx)
            .expect("delete_stale_models follows persist_discovery_result");
        &source[start..end]
    };
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
fn discovery_persistence_routes_through_setup_repository_with_stub_calls() {
    let empty_setup = RecordingSetupRepository::default();
    let empty_result = discovery::DiscoveryResult {
        cli_name: "codex".to_string(),
        cli_version: "1.2.3".to_string(),
        models: vec![],
        parameters: vec![],
    };

    let returned = commands::discovery::orchestration::persist_discovery_result(
        &empty_setup,
        "codex",
        empty_result,
    )
    .expect("empty discovery result should persist");

    assert!(returned.is_empty());
    assert!(empty_setup.calls().is_empty());

    let setup = RecordingSetupRepository::default();
    let model = discovered_model("codex", "gpt-5", "1.2.3");
    let parameter = model_parameter("temperature");
    let result = discovery::DiscoveryResult {
        cli_name: "codex".to_string(),
        cli_version: "1.2.3".to_string(),
        models: vec![model.clone()],
        parameters: vec![("gpt-5".to_string(), parameter)],
    };

    let returned =
        commands::discovery::orchestration::persist_discovery_result(&setup, "codex", result)
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
    let returned =
        commands::discovery::orchestration::persist_discovery_result(&db, "codex", empty_result)
            .unwrap();
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
    let returned =
        commands::discovery::orchestration::persist_discovery_result(&db, "codex", result).unwrap();
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
    let state = test_state(dir.path().join("models"));
    let db = StateDb::open(&state.db_path()).unwrap();
    db.upsert_discovered_model(&discovered_model("codex", "gpt-5", "codex-1"))
        .unwrap();
    db.upsert_discovered_model(&discovered_model("claude", "sonnet", "claude-1"))
        .unwrap();
    drop(db);

    let models = commands::discovery::orchestration::list_discovered_models_inner(
        &state,
        Some("codex".to_string()),
    )
    .unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].provider, "codex");
    assert_eq!(models[0].canonical_name, "gpt-5");
}

#[test]
fn get_model_parameters_filters_by_provider_and_model() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path().join("models"));
    let db = StateDb::open(&state.db_path()).unwrap();
    db.upsert_model_parameter("gpt-5", "codex", &model_parameter("model"))
        .unwrap();
    db.upsert_model_parameter("gpt-5", "claude", &model_parameter("max_tokens"))
        .unwrap();
    db.upsert_model_parameter("gpt-4", "codex", &model_parameter("temperature"))
        .unwrap();
    drop(db);

    let params = commands::discovery::orchestration::get_model_parameters_inner(
        &state,
        "gpt-5".to_string(),
        "codex".to_string(),
    )
    .unwrap();

    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "model");
    assert_eq!(params[0].cli_mapping.flag, "--model");
}
