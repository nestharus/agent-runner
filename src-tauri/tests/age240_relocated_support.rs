#![allow(dead_code)]

use agent_runner_lib::commands::accessor;
use agent_runner_lib::provider_settings;
use agent_runner_lib::test_model_command::{
    TestModelServices, effective_provider_for_model_provider, test_model_for_test,
    test_model_with_db_path,
};
use agent_runner_lib::{
    AppState, AppStateTestServices, load_providers_for_models_dir,
    load_providers_for_models_dir_with,
};
use oulipoly_config as config;
use oulipoly_state as state;
use oulipoly_state::{AccountRecord, CliProviderRecord, DiscoveredModel, ModelParameter, StateDb};
use std::path::Path;
use std::sync::{Arc, Mutex};

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
use oulipoly_runtime::quota;
use oulipoly_runtime::services::{
    DiagnosticsServiceOutput, DiagnosticsServicePort, DiagnosticsServiceRequest,
    ExecutorServiceOutput, ExecutorServicePort, ExecutorServiceRequest, QuotaServiceOutput,
    QuotaServicePort, QuotaServiceRequest, RoutingServiceOutput, RoutingServicePort,
    RoutingServiceRequest, ServiceError,
};
use oulipoly_state::repositories::{SetupRepository, StateDbOpener};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[cfg(unix)]
pub fn write_executable(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

pub fn make_model(name: &str, commands: &[&str]) -> ModelConfig {
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

pub fn model_with_provider_args(name: &str, provider_name: &str, args: &[&str]) -> ModelConfig {
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

pub fn model_with_provider_artifact(name: &str, provider_name: &str, path: &Path) -> ModelConfig {
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

pub fn test_state(models_dir: PathBuf, models: HashMap<String, ModelConfig>) -> AppState {
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
                provider, ..
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

pub fn services(
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

pub fn default_services(root: &Path) -> AppStateTestServices {
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

pub fn age38_load_providers_for_models_dir_with_routes_through_stub_and_defaults_errors() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    let repo =
        StubProvidersConfigRepository::returning(Err("sentinel provider failure".to_string()));

    let providers = load_providers_for_models_dir_with(&models_dir, &repo);

    assert_eq!(repo.calls(), vec![dir.path().join("providers.toml")]);
    assert!(providers.entries.is_empty());
}

pub fn age38_open_state_db_routes_through_injected_state_db_opener() {
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

    let db = accessor::open_state_db(&state).unwrap();
    db.upsert_cli_provider(&cli_provider("codex", "OpenAI"))
        .unwrap();
    drop(db);

    assert_eq!(opener.calls(), vec![dir.path().join("state.db")]);
    assert!(opened_db_path.exists());
    assert!(!dir.path().join("state.db").exists());
}

pub fn age38_open_state_db_returns_injected_opener_error() {
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

    let err = match accessor::open_state_db(&state) {
        Ok(_) => panic!("open_state_db should return the injected opener error"),
        Err(err) => err,
    };

    assert_eq!(err, "sentinel opener failure");
    assert_eq!(opener.calls(), vec![dir.path().join("state.db")]);
}

pub fn age38_test_model_success_routes_effective_request_through_stub_ports() {
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

    let result = test_model_with_db_path(
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

pub fn tauri_test_model_injects_policy() {
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

    let result = test_model_with_db_path(
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

pub fn age38_test_model_nonzero_not_exhausted_classifies_without_marking_quota() {
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

    let result = test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
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

pub fn age38_test_model_nonzero_exhausted_classifies_and_marks_quota() {
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

    let result = test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
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

pub fn age156_test_model_typed_rate_limited_signal_does_not_mark_exhausted_even_when_legacy_classifier_would()
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

    let result = test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
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

pub fn test_model_maybe_signal_is_non_durable() {
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

    let result = test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
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

pub fn age156_test_model_typed_quota_exhausted_inband_signal_marks_exhausted_even_when_legacy_classifier_would_not()
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

    let result = test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
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

pub fn test_model_nonzero_stdout_exhausted_classifies_and_marks_quota() {
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

    let result = test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello")
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

pub fn write_codex_providers(root: &Path) {
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

pub fn cli_provider(cli_name: &str, display_name: &str) -> CliProviderRecord {
    CliProviderRecord {
        cli_name: cli_name.to_string(),
        display_name: display_name.to_string(),
        installed: true,
        version: Some("1.2.3".to_string()),
        config_dir: Some("/tmp/config".to_string()),
        last_synced: Some("2026-05-08T12:00:00Z".to_string()),
    }
}

pub fn discovered_model(provider: &str, name: &str, cli_version: &str) -> DiscoveredModel {
    DiscoveredModel {
        canonical_name: name.to_string(),
        provider: provider.to_string(),
        discovered_at: "2026-05-08T12:00:00Z".to_string(),
        cli_version: cli_version.to_string(),
    }
}

pub fn model_parameter(name: &str) -> ModelParameter {
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

pub fn open_state_db_opens_models_parent_state_db_and_returns_state_db() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    let state = test_state(models_dir, HashMap::new());

    let db = accessor::open_state_db(&state).unwrap();
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

pub fn effective_provider_for_model_provider_rejects_out_of_range_index() {
    let model = make_model("gpt-high", &["codex"]);

    let err =
        effective_provider_for_model_provider(&model, 1, &ProvidersConfig::default()).unwrap_err();

    assert_eq!(err, "provider_index out of range");
}

pub fn effective_provider_for_model_provider_rejects_unresolved_empty_command() {
    let model = ModelConfig {
        name: "gpt-high".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::model_provider("missing-provider", vec![])],
        inputs: vec![],
        provider: None,
    };

    let err =
        effective_provider_for_model_provider(&model, 0, &ProvidersConfig::default()).unwrap_err();

    assert_eq!(
        err,
        "provider missing-provider is missing from providers.toml"
    );
}

#[cfg(unix)]
pub fn test_model_marks_provider_exhausted_on_quota_stderr() {
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
        test_model_with_db_path(services, model, models_dir, db_path.clone(), "hello").unwrap();

    assert!(!result.success);
    assert_eq!(result.exit_code, 7);
    assert!(result.stderr.contains("typed quota signal"));
    let db = StateDb::open(&db_path).unwrap();
    let quota = db.get_quota("quota-provider").unwrap().unwrap();
    assert!(quota.exhausted_at.is_some());
}

#[cfg(unix)]
pub fn test_model_migrated_provider_uses_providers_toml_effective_provider() {
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
pub fn test_model_raw_sigterm_returns_unified_signal_exit_code() {
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
pub fn provider_settings_command_args_deserialize_camel_case_ipc_payloads() {
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
pub fn provider_settings_command_preserves_structured_conflict_and_transport_errors() {
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
pub fn provider_settings_command_preserves_migration_diagnostics_from_real_host() {
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
pub fn provider_settings_targets_skip_central_config_only_models() {
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
pub fn provider_settings_diagnostic_provider_script() -> &'static str {
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
pub fn provider_settings_migration_packages_central_config_blocks_read_only() {
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
    let provider_path_toml = serde_json::to_string(&provider_path.display().to_string()).unwrap();
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
    let providers = load_providers_for_models_dir(&models_dir);
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
pub fn provider_settings_migration_recording_provider_script(record_path: &Path) -> String {
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
pub fn read_provider_settings_migration_record(record_path: &Path) -> serde_json::Value {
    std::fs::read_to_string(record_path)
        .expect("settings.migrate record should exist")
        .lines()
        .map(|line| serde_json::from_str(line).expect("recorded request should parse"))
        .next()
        .expect("settings.migrate request should be recorded")
}
