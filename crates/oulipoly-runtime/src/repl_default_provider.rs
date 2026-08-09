use crate::services::{
    InvocationLifecycleFinalizeRequest, InvocationLifecycleServicePort,
    InvocationLifecycleStartRequest, ProductionInvocationLifecycleService,
    ProductionRoutingService, ProductionSessionLifecycleService, RoutingServicePort,
    RoutingServiceRequest, SessionLifecycleIngestMode, SessionLifecycleRequest,
    SessionLifecycleServicePort,
};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_state::repositories::{ProductionStateDbOpener, StateDbOpener};
use oulipoly_state::{CompositeInvocationId, InvocationStart};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

use crate::executor::cli::InteractiveLiveSessionBinding;
use crate::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use crate::services::{
    LauncherServiceOutput, LauncherServicePort, LauncherServiceRequest, ServiceError,
};
use crate::session_provider::SessionProviderIdentity;

const UNKNOWN_DEFAULT_PROVIDER_MODEL: &str = "<unknown>";
const DEFAULT_PROVIDER_REPL_CAPTURE_METHOD: &str = "turn_script";
const LIVE_SESSION_IDENTITY_UNAVAILABLE: &str = "live_session_identity_unavailable";

pub struct RuntimeServices<O: StateDbOpener = ProductionStateDbOpener> {
    pub config_root: PathBuf,
    pub state_db_path: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub state_db_opener: O,
    pub routing_service: Arc<dyn RoutingServicePort>,
}

impl RuntimeServices<ProductionStateDbOpener> {
    pub fn production(working_dir: Option<PathBuf>) -> Result<Self, String> {
        let config_root = dirs::config_dir()
            .map(|path| path.join("oulipoly-agent-runner"))
            .unwrap_or_else(|| PathBuf::from("oulipoly-agent-runner"));

        Ok(Self {
            config_root,
            state_db_path: None,
            working_dir,
            state_db_opener: ProductionStateDbOpener,
            routing_service: Arc::new(ProductionRoutingService),
        })
    }
}

#[allow(dead_code)]
pub(crate) trait InteractiveLauncher {
    fn launch(
        &self,
        provider: &ProviderConfig,
        working_dir: Option<&Path>,
        parent_invocation_env: Option<&str>,
        state_db_path: Option<&Path>,
        live_session_binding: Option<InteractiveLiveSessionBinding>,
    ) -> Result<crate::executor::cli::InteractiveExecutionResult, String>;
}

pub struct RuntimeLauncherService;

impl LauncherServicePort for RuntimeLauncherService {
    fn launch(
        &self,
        request: LauncherServiceRequest,
    ) -> Result<LauncherServiceOutput, ServiceError> {
        let exit_code = crate::executor::cli::execute_interactive(
            &request.provider,
            request.working_dir.as_deref(),
            None,
            None,
        )
        .map_err(|message| ServiceError::Dependency { message })?;

        Ok(LauncherServiceOutput { exit_code })
    }
}

impl InteractiveLauncher for RuntimeLauncherService {
    fn launch(
        &self,
        provider: &ProviderConfig,
        working_dir: Option<&Path>,
        parent_invocation_env: Option<&str>,
        state_db_path: Option<&Path>,
        live_session_binding: Option<InteractiveLiveSessionBinding>,
    ) -> Result<crate::executor::cli::InteractiveExecutionResult, String> {
        crate::executor::cli::execute_interactive_with_result_and_state_db_path(
            provider,
            working_dir,
            parent_invocation_env,
            None,
            state_db_path,
            live_session_binding,
        )
    }
}

pub fn run_repl_with_default_provider<O: StateDbOpener>(
    services: RuntimeServices<O>,
) -> Result<i32, String> {
    let launcher = RuntimeLauncherService;
    run_repl_with_default_provider_with_launcher(services, &launcher)
}

#[allow(dead_code)]
pub(crate) fn run_repl_with_default_provider_with_launcher<O: StateDbOpener>(
    services: RuntimeServices<O>,
    launcher: &dyn InteractiveLauncher,
) -> Result<i32, String> {
    let app_config_path = services.config_root.join("config.toml");
    let app = oulipoly_config::app::AppConfig::load(&app_config_path)?;
    let family = app.default_provider.ok_or_else(|| {
        format!(
            "'default_provider' must be set in {} for '--new'",
            app_config_path.display()
        )
    })?;

    let providers_path = services.config_root.join("providers.toml");
    let providers = ProvidersConfig::load(&providers_path)?;
    let members = resolve_family_keys(&providers, &family);
    if members.is_empty() {
        return Err(format!(
            "default_provider '{family}' resolved to an empty provider pool in {}",
            providers_path.display()
        ));
    }

    let carrier_model = ModelConfig {
        name: format!("<provider-family:{family}>"),
        prompt_mode: PromptMode::Stdin,
        providers: members
            .iter()
            .map(|member| ProviderConfig::model_provider(*member, Vec::new()))
            .collect(),
        inputs: Vec::new(),
        provider: None,
    };
    let state_db_path = default_provider_state_db_path(&services)?;
    let registry_data_root = state_db_path
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(services.config_root.as_path());
    let provider_registry = Arc::new(
        ProviderRegistry::from_model_configs_with_provider_config(
            std::slice::from_ref(&carrier_model),
            &providers,
            ProviderRegistryOptions::default()
                .with_path_entries_from_process_path()
                .with_config_root(&services.config_root)
                .with_data_root(registry_data_root),
        )
        .map_err(|err| err.to_string())?,
    );

    let state = match services.state_db_path.as_ref() {
        Some(path) => services.state_db_opener.open_at(path),
        None => services.state_db_opener.open_default(),
    }?;

    let provider_index = services
        .routing_service
        .select_route(RoutingServiceRequest {
            model: &carrier_model,
            state: &state,
            ctx: None,
        })
        .map_err(|error| error.to_string())?
        .provider_index;
    if provider_index >= carrier_model.providers.len() {
        return Err(format!(
            "selected provider index {provider_index} is out of bounds"
        ));
    }
    let member_name = members
        .get(provider_index)
        .ok_or_else(|| format!("selected provider index {provider_index} is out of bounds"))?;
    let (provider, _prompt_mode) = providers.runtime_provider(member_name)?;
    let selected_provider_name = provider.name.clone();
    let launch_provider = ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: carrier_model.name.clone(),
        ..provider
    };

    run_registered_default_provider_repl(RegisteredDefaultProviderReplInput {
        services: &services,
        state: &state,
        providers: &providers,
        provider_name: &selected_provider_name,
        provider_index,
        carrier_model: &carrier_model,
        provider_registry,
        state_db_path: state_db_path.as_deref(),
        launch_provider: &launch_provider,
        launcher,
    })
}

struct RegisteredDefaultProviderReplInput<'a, O: StateDbOpener> {
    services: &'a RuntimeServices<O>,
    state: &'a oulipoly_state::StateDb,
    providers: &'a ProvidersConfig,
    provider_name: &'a str,
    provider_index: usize,
    carrier_model: &'a ModelConfig,
    provider_registry: Arc<ProviderRegistry>,
    state_db_path: Option<&'a Path>,
    launch_provider: &'a ProviderConfig,
    launcher: &'a dyn InteractiveLauncher,
}

fn run_registered_default_provider_repl<O: StateDbOpener>(
    input: RegisteredDefaultProviderReplInput<'_, O>,
) -> Result<i32, String> {
    let lifecycle = ProductionInvocationLifecycleService::new();
    let invocation = default_provider_invocation(input.provider_name);
    let invocation_start =
        default_provider_invocation_start(&invocation, input.provider_name, input.provider_index);
    let invocation_row_id = lifecycle
        .start_invocation(InvocationLifecycleStartRequest {
            state: input.state,
            start: &invocation_start,
        })
        .map_err(|err| err.to_string())?
        .invocation_row_id;
    eprintln!("{}", invocation.stderr_line());
    let parent_invocation_env = serde_json::to_string(&invocation)
        .map_err(|err| format!("Failed to serialize invocation id: {err}"))?;

    let result = match input.launcher.launch(
        input.launch_provider,
        input.services.working_dir.as_deref(),
        Some(&parent_invocation_env),
        input.state_db_path,
        default_provider_live_session_binding(&input, invocation_row_id, &invocation.id),
    ) {
        Ok(result) => result,
        Err(err) => {
            finalize_default_provider_spawn_error(&lifecycle, input.state, invocation_row_id)?;
            return Err(err);
        }
    };

    let bound_session_id = default_provider_invocation_session_id(input.state, &invocation.id)?;
    if result.exit_code == 0
        && result.live_session_capture_required
        && (result.live_session_id.is_none()
            || result.live_session_id.as_deref() != bound_session_id.as_deref())
    {
        finalize_default_provider_live_session_error(&lifecycle, input.state, invocation_row_id)?;
        return Err(format!(
            "{LIVE_SESSION_IDENTITY_UNAVAILABLE}: provider {} exited successfully without reporting and binding its exact live session; nested asynchronous completion is unavailable",
            input.provider_name
        ));
    }

    finalize_default_provider_repl_result(&lifecycle, input.state, invocation_row_id, &result)?;
    if result.exit_code == 0 && bound_session_id.is_none() {
        ingest_default_provider_session(DefaultProviderSessionIngestInput {
            services: input.services,
            state: input.state,
            providers: input.providers,
            provider_name: input.provider_name,
            invocation_row_id,
            invocation_uuid: &invocation.id,
        });
    }
    Ok(result.exit_code)
}

fn default_provider_invocation_session_id(
    state: &oulipoly_state::StateDb,
    invocation_uuid: &str,
) -> Result<Option<String>, String> {
    state.get_invocation_by_uuid(invocation_uuid).map(|record| {
        record.and_then(|record| {
            if record.provider_session_capture_method.as_deref()
                == Some(crate::executor::cli::PENDING_LIVE_SESSION_CAPTURE_METHOD)
            {
                return None;
            }
            record
                .provider_session_id
                .or(record.session_id)
                .filter(|session_id| !session_id.trim().is_empty())
        })
    })
}

fn default_provider_state_db_path<O: StateDbOpener>(
    services: &RuntimeServices<O>,
) -> Result<Option<PathBuf>, String> {
    match services.state_db_path.as_deref() {
        Some(path) => Ok(Some(path.to_path_buf())),
        None => services.state_db_opener.default_path(),
    }
}

fn default_provider_live_session_binding<O: StateDbOpener>(
    input: &RegisteredDefaultProviderReplInput<'_, O>,
    invocation_row_id: i64,
    invocation_uuid: &str,
) -> Option<InteractiveLiveSessionBinding> {
    let state_db_path = input.state_db_path?;
    Some(InteractiveLiveSessionBinding {
        registry: Arc::clone(&input.provider_registry),
        identity: SessionProviderIdentity {
            model_name: input.carrier_model.name.clone(),
            provider_name: input.provider_name.to_string(),
            provider_instance_id: None,
            settings_id: input.provider_name.to_string(),
        },
        state_db_path: state_db_path.to_path_buf(),
        invocation_row_id,
        invocation_uuid: invocation_uuid.to_string(),
        effective_cwd: default_provider_effective_cwd(input.services.working_dir.as_deref()),
    })
}

fn default_provider_invocation(provider_name: &str) -> CompositeInvocationId {
    CompositeInvocationId {
        source: provider_name.to_string(),
        id: Uuid::new_v4().to_string(),
    }
}

fn default_provider_invocation_start(
    invocation: &CompositeInvocationId,
    provider_name: &str,
    provider_index: usize,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: UNKNOWN_DEFAULT_PROVIDER_MODEL.to_string(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id: None,
    }
}

fn finalize_default_provider_repl_result(
    lifecycle: &ProductionInvocationLifecycleService,
    state: &oulipoly_state::StateDb,
    invocation_row_id: i64,
    result: &crate::executor::cli::InteractiveExecutionResult,
) -> Result<(), String> {
    lifecycle
        .finalize_invocation(InvocationLifecycleFinalizeRequest {
            state,
            invocation_row_id,
            success: result.exit_code == 0,
            exit_code: result.exit_code,
            error_category: None,
            terminal_reason: result.terminal_reason.as_deref(),
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn finalize_default_provider_spawn_error(
    lifecycle: &ProductionInvocationLifecycleService,
    state: &oulipoly_state::StateDb,
    invocation_row_id: i64,
) -> Result<(), String> {
    lifecycle
        .finalize_invocation(InvocationLifecycleFinalizeRequest {
            state,
            invocation_row_id,
            success: false,
            exit_code: 1,
            error_category: Some("spawn_error"),
            terminal_reason: Some("spawn_error"),
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn finalize_default_provider_live_session_error(
    lifecycle: &ProductionInvocationLifecycleService,
    state: &oulipoly_state::StateDb,
    invocation_row_id: i64,
) -> Result<(), String> {
    lifecycle
        .finalize_invocation(InvocationLifecycleFinalizeRequest {
            state,
            invocation_row_id,
            success: false,
            exit_code: 1,
            error_category: Some(LIVE_SESSION_IDENTITY_UNAVAILABLE),
            terminal_reason: Some(LIVE_SESSION_IDENTITY_UNAVAILABLE),
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

struct DefaultProviderSessionIngestInput<'a, O: StateDbOpener> {
    services: &'a RuntimeServices<O>,
    state: &'a oulipoly_state::StateDb,
    providers: &'a ProvidersConfig,
    provider_name: &'a str,
    invocation_row_id: i64,
    invocation_uuid: &'a str,
}

fn ingest_default_provider_session<O: StateDbOpener>(
    input: DefaultProviderSessionIngestInput<'_, O>,
) {
    let sessions_cfg =
        oulipoly_config::SessionsConfig::load(&input.services.config_root.join("sessions.toml"))
            .unwrap_or_default();
    let effective_cwd = default_provider_effective_cwd(input.services.working_dir.as_deref());
    let mut stderr = std::io::stderr();
    if let Err(err) =
        ProductionSessionLifecycleService::new().ingest_session(SessionLifecycleRequest {
            state: input.state,
            sessions_cfg: &sessions_cfg,
            providers_cfg: Some(input.providers),
            provider_name: input.provider_name,
            external_provider: None,
            invocation_row_id: input.invocation_row_id,
            invocation_uuid: input.invocation_uuid,
            effective_cwd: effective_cwd.as_deref(),
            mode: SessionLifecycleIngestMode::Unpinned {
                capture_method: DEFAULT_PROVIDER_REPL_CAPTURE_METHOD.to_string(),
            },
            stderr: &mut stderr,
        })
    {
        log_default_provider_session_ingest_error(input.provider_name, &err);
    }
}

fn log_default_provider_session_ingest_error(provider_name: &str, err: &ServiceError) {
    let Some(warning) = default_provider_session_ingest_user_warning(provider_name, err) else {
        tracing::debug!(
            provider_name,
            error = %err,
            error_code = err.code(),
            "default-provider session ingest skipped benign failure"
        );
        return;
    };

    tracing::warn!(
        provider_name,
        warning = %warning,
        error = %err,
        error_code = err.code(),
        "default-provider session ingest failed"
    );
    eprintln!("{warning}");
}

fn default_provider_session_ingest_user_warning(
    provider_name: &str,
    err: &ServiceError,
) -> Option<String> {
    if is_benign_default_provider_session_ingest_error(err) {
        None
    } else {
        Some(format!(
            "Warning: Session ingest failed for {provider_name}: {err}"
        ))
    }
}

fn is_benign_default_provider_session_ingest_error(err: &ServiceError) -> bool {
    err.code()
        .is_some_and(is_benign_default_provider_session_ingest_token)
        || default_provider_session_ingest_message_has_benign_token(&err.to_string())
}

fn default_provider_session_ingest_message_has_benign_token(message: &str) -> bool {
    message
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .any(is_benign_default_provider_session_ingest_token)
}

fn is_benign_default_provider_session_ingest_token(token: &str) -> bool {
    matches!(
        token,
        "ambiguous_session_transcript"
            | "session_transcript_not_found"
            | "partial_native_transcript"
            | "provider_capability"
            | "session_locate_missing"
            | "session_locate_require_existing_unobserved"
    )
}

fn default_provider_effective_cwd(working_dir: Option<&Path>) -> Option<PathBuf> {
    let cwd = match working_dir {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => std::env::current_dir().ok()?.join(path),
        None => std::env::current_dir().ok()?,
    };
    cwd.canonicalize().ok().or(Some(cwd))
}

#[allow(dead_code)]
pub(crate) fn resolve_family_keys<'a>(
    providers: &'a ProvidersConfig,
    family: &str,
) -> Vec<&'a str> {
    let mut exact = Vec::new();
    let mut suffixed = Vec::new();

    for key in providers.entries.keys() {
        let key = key.as_str();
        if key == family {
            exact.push(key);
            continue;
        }

        let Some(suffix) = key.strip_prefix(family) else {
            continue;
        };
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }

        suffixed.push((suffix, key));
    }

    exact.sort_unstable();
    suffixed.sort_by(|(left_suffix, left_key), (right_suffix, right_key)| {
        compare_digit_suffix(left_suffix, right_suffix).then_with(|| left_key.cmp(right_key))
    });

    exact
        .into_iter()
        .chain(suffixed.into_iter().map(|(_, key)| key))
        .filter(|key| match providers.runtime_provider(key) {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(
                    provider_name = *key,
                    error = error.as_str(),
                    "dropping invalid provider-family member"
                );
                false
            }
        })
        .collect()
}

fn compare_digit_suffix(left: &str, right: &str) -> std::cmp::Ordering {
    let left_trimmed = left.trim_start_matches('0');
    let right_trimmed = right.trim_start_matches('0');
    let left_numeric = if left_trimmed.is_empty() {
        "0"
    } else {
        left_trimmed
    };
    let right_numeric = if right_trimmed.is_empty() {
        "0"
    } else {
        right_trimmed
    };

    left_numeric
        .len()
        .cmp(&right_numeric.len())
        .then_with(|| left_numeric.cmp(right_numeric))
        .then_with(|| left.len().cmp(&right.len()))
        .then_with(|| left.cmp(right))
}

#[cfg(test)]
pub(crate) use resolve_family_keys as resolve_family_keys_for_test;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{LauncherServicePort, LauncherServiceRequest};
    use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};
    use rusqlite::Connection;
    use std::cell::RefCell;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn runtime_services(config_root: PathBuf) -> RuntimeServices {
        RuntimeServices {
            config_root,
            state_db_path: None,
            working_dir: None,
            state_db_opener: ProductionStateDbOpener,
            routing_service: Arc::new(ProductionRoutingService),
        }
    }

    fn runtime_services_with_state(
        config_root: PathBuf,
        state_db_path: PathBuf,
    ) -> RuntimeServices {
        RuntimeServices {
            config_root,
            state_db_path: Some(state_db_path),
            working_dir: None,
            state_db_opener: ProductionStateDbOpener,
            routing_service: Arc::new(ProductionRoutingService),
        }
    }

    fn write_config(root: &Path, contents: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("config.toml"), contents).unwrap();
    }

    fn write_providers(root: &Path, contents: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("providers.toml"), contents).unwrap();
    }

    fn write_sessions(root: &Path, contents: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("sessions.toml"), contents).unwrap();
    }

    fn provider_fixture(name: &str) -> String {
        format!(
            r#"[{name}]
command = "printf"
interactive_args = ["ok"]
"#
        )
    }

    fn load_providers(root: &Path, contents: &str) -> ProvidersConfig {
        write_providers(root, contents);
        ProvidersConfig::load(&root.join("providers.toml")).unwrap()
    }

    type LauncherCall = (String, Option<PathBuf>, Option<String>);

    #[derive(Default)]
    struct RecordingLauncher {
        calls: RefCell<Vec<LauncherCall>>,
    }

    impl InteractiveLauncher for RecordingLauncher {
        fn launch(
            &self,
            provider: &ProviderConfig,
            working_dir: Option<&Path>,
            parent_invocation_env: Option<&str>,
            _state_db_path: Option<&Path>,
            _live_session_binding: Option<InteractiveLiveSessionBinding>,
        ) -> Result<crate::executor::cli::InteractiveExecutionResult, String> {
            self.calls.borrow_mut().push((
                provider.name.clone(),
                working_dir.map(Path::to_path_buf),
                parent_invocation_env.map(str::to_string),
            ));
            Ok(successful_interactive_result())
        }
    }

    fn successful_interactive_result() -> crate::executor::cli::InteractiveExecutionResult {
        crate::executor::cli::InteractiveExecutionResult {
            exit_code: 0,
            terminal_reason: None,
            terminal_signal: None,
            live_session_id: None,
            live_session_capture_required: false,
        }
    }

    #[cfg(unix)]
    struct TurnWritingLauncher {
        turns_path: PathBuf,
        session_id: String,
    }

    struct CapturingTerminalLauncher {
        session_id: String,
        exit_code: i32,
        terminal_reason: String,
    }

    impl InteractiveLauncher for CapturingTerminalLauncher {
        fn launch(
            &self,
            _provider: &ProviderConfig,
            _working_dir: Option<&Path>,
            _parent_invocation_env: Option<&str>,
            _state_db_path: Option<&Path>,
            live_session_binding: Option<InteractiveLiveSessionBinding>,
        ) -> Result<crate::executor::cli::InteractiveExecutionResult, String> {
            let binding = live_session_binding.expect("live-session binding context");
            StateDb::open(&binding.state_db_path)?.bind_invocation_provider_session_start(
                binding.invocation_row_id,
                &ProviderSessionBinding {
                    provider_session_id: self.session_id.clone(),
                    capture_method: "provider_live_report",
                    resume_input_id: None,
                    provider_session_resolved_account: Some(binding.identity.settings_id.clone()),
                },
            )?;
            Ok(crate::executor::cli::InteractiveExecutionResult {
                exit_code: self.exit_code,
                terminal_reason: Some(self.terminal_reason.clone()),
                terminal_signal: None,
                live_session_id: Some(self.session_id.clone()),
                live_session_capture_required: true,
            })
        }
    }

    #[cfg(unix)]
    impl InteractiveLauncher for TurnWritingLauncher {
        fn launch(
            &self,
            _provider: &ProviderConfig,
            _working_dir: Option<&Path>,
            _parent_invocation_env: Option<&str>,
            _state_db_path: Option<&Path>,
            _live_session_binding: Option<InteractiveLiveSessionBinding>,
        ) -> Result<crate::executor::cli::InteractiveExecutionResult, String> {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let timestamp = chrono::Utc::now().to_rfc3339();
            std::fs::write(
                &self.turns_path,
                format!(
                    r#"{{"session_id":"{}","turn_id":"turn-1","timestamp":"{}","role":"assistant"}}
"#,
                    self.session_id, timestamp
                ),
            )
            .unwrap();
            Ok(successful_interactive_result())
        }
    }

    fn table_count(db_path: &Path, table: &str) -> i64 {
        let conn = Connection::open(db_path).unwrap();
        let sql = format!("SELECT COUNT(*) FROM {table}");
        conn.query_row(&sql, [], |row| row.get(0)).unwrap()
    }

    fn invocation_row(db_path: &Path) -> (String, String, String, Option<String>, Option<String>) {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(
            "SELECT model_name, provider_name, status, provider_session_id, provider_session_capture_method
             FROM invocations
             LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap()
    }

    fn single_string_column(db_path: &Path, sql: &str) -> String {
        let conn = Connection::open(db_path).unwrap();
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[cfg(unix)]
    fn toml_path(path: &Path) -> String {
        path.to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    }

    #[cfg(unix)]
    fn launcher_fixture_script(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher.sh");
        std::fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        (dir, path)
    }

    #[cfg(unix)]
    fn runtime_launcher_test_provider(path: &Path) -> ProviderConfig {
        ProviderConfig {
            environment: Default::default(),
            unset_environment: Default::default(),
            name: "runtime-launcher-test".to_string(),
            command: path.to_string_lossy().into_owned(),
            args: vec!["one-shot-only".to_string()],
            interactive_args: Some(vec!["interactive".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }
    }

    /// Risk: R-A4 / proposal T11 - the private InteractiveLauncher seam on
    /// RuntimeLauncherService must stay observably equivalent to the public
    /// LauncherServicePort path on the same concrete service.
    /// Level: unit.
    /// Source: AGE-34 contract risk annotation R-A4.
    #[cfg(unix)]
    #[test]
    fn runtime_launcher_service_private_seam_matches_launcher_service_port() {
        let working_dir = tempfile::tempdir().unwrap();
        let marker = tempfile::NamedTempFile::new().unwrap();
        let (_script_dir, script_path) = launcher_fixture_script(&format!(
            r#"printf 'ran\n' >> "{marker}"
exit 17"#,
            marker = marker.path().display()
        ));
        let provider = runtime_launcher_test_provider(&script_path);
        let service = RuntimeLauncherService;
        let service_port: &dyn LauncherServicePort = &service;
        let private_launcher: &dyn InteractiveLauncher = &service;

        let service_output = service_port
            .launch(LauncherServiceRequest {
                provider: provider.clone(),
                working_dir: Some(working_dir.path().to_path_buf()),
            })
            .expect("service launch");
        let private_result = private_launcher
            .launch(&provider, Some(working_dir.path()), None, None, None)
            .expect("private seam launch");

        assert_eq!(service_output.exit_code, private_result.exit_code);
        assert_eq!(service_output.exit_code, 17);
        assert_eq!(
            std::fs::read_to_string(marker.path()).unwrap(),
            "ran\nran\n"
        );
    }

    #[test]
    fn production_services_match_known_baseline_for_none_and_some_project() {
        let without_project = RuntimeServices::production(None).unwrap();

        assert!(
            without_project
                .config_root
                .ends_with("oulipoly-agent-runner")
        );
        assert_eq!(without_project.state_db_path, None);
        assert_eq!(without_project.working_dir, None);

        let project = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("age31-production-services-project");
        let with_project = RuntimeServices::production(Some(project.clone())).unwrap();

        assert_eq!(with_project.config_root, without_project.config_root);
        assert_eq!(with_project.state_db_path, None);
        assert_eq!(with_project.working_dir.as_ref(), Some(&project));
    }

    #[test]
    fn missing_default_provider_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        write_config(temp.path(), r#"diagnostics_model = "codex~high""#);

        let error = run_repl_with_default_provider(runtime_services(temp.path().to_path_buf()))
            .expect_err("missing default_provider should be rejected");

        assert_eq!(
            error,
            format!(
                "'default_provider' must be set in {} for '--new'",
                temp.path().join("config.toml").display()
            )
        );
    }

    #[test]
    fn empty_family_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("codex"));

        let error = run_repl_with_default_provider(runtime_services(temp.path().to_path_buf()))
            .expect_err("empty provider family should be rejected");

        assert_eq!(
            error,
            format!(
                "default_provider 'claude' resolved to an empty provider pool in {}",
                temp.path().join("providers.toml").display()
            )
        );
    }

    #[test]
    fn family_resolver_includes_exact_and_digit_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let providers = load_providers(
            temp.path(),
            &[
                provider_fixture("claude3"),
                provider_fixture("claude-work"),
                provider_fixture("claude"),
                provider_fixture("myclaude"),
                provider_fixture("claude2"),
            ]
            .join("\n"),
        );

        assert_eq!(
            resolve_family_keys_for_test(&providers, "claude"),
            vec!["claude", "claude2", "claude3"]
        );
    }

    #[test]
    fn family_resolver_excludes_dashed_and_prefixed_keys() {
        let temp = tempfile::tempdir().unwrap();
        let providers = load_providers(
            temp.path(),
            &[
                provider_fixture("claude-work"),
                provider_fixture("myclaude"),
                provider_fixture("claude2"),
                provider_fixture("codex"),
            ]
            .join("\n"),
        );

        assert_eq!(
            resolve_family_keys_for_test(&providers, "claude"),
            vec!["claude2"]
        );
    }

    #[test]
    fn family_resolver_drops_invalid_runtime_provider() {
        let temp = tempfile::tempdir().unwrap();
        let providers = load_providers(
            temp.path(),
            r#"[claude]
command = "printf"
interactive_args = ["ok"]

[claude2]
interactive_args = ["ok"]

[claude3]
command = "printf"
interactive_args = ["ok"]
"#,
        );

        assert_eq!(
            resolve_family_keys_for_test(&providers, "claude"),
            vec!["claude", "claude3"]
        );
    }

    #[test]
    fn benign_default_provider_session_ingest_error_has_no_user_warning() {
        let provider_name = provider_name_for_test(&["cla", "ude"]);
        let error = ServiceError::Unavailable {
            message: "session.read_turns: ambiguous_session_transcript: provider client error"
                .to_string(),
            code: Some("ambiguous_session_transcript".to_string()),
        };

        let mut stderr = Vec::new();
        if let Some(warning) = default_provider_session_ingest_user_warning(&provider_name, &error)
        {
            stderr.extend_from_slice(warning.as_bytes());
        }

        assert!(stderr.is_empty());
    }

    #[test]
    fn benign_dependency_session_ingest_error_message_has_no_user_warning() {
        let provider_name = provider_name_for_test(&["cla", "ude"]);

        for message in [
            "session.read_turns: ambiguous_session_transcript: provider client error",
            "session.read_turns: provider client error: provider_capability",
        ] {
            let error = ServiceError::Dependency {
                message: message.to_string(),
            };

            assert_eq!(
                default_provider_session_ingest_user_warning(&provider_name, &error),
                None
            );
        }
    }

    #[test]
    fn benign_dependency_session_ingest_error_message_requires_token_boundary() {
        let provider_name = provider_name_for_test(&["cla", "ude"]);
        let error = ServiceError::Dependency {
            message: "session.read_turns: ambiguous_session_transcript_suffix".to_string(),
        };
        let expected = format!(
            "Warning: Session ingest failed for {provider_name}: session.read_turns: ambiguous_session_transcript_suffix"
        );

        assert_eq!(
            default_provider_session_ingest_user_warning(&provider_name, &error).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn unexpected_default_provider_session_ingest_error_keeps_warning() {
        let provider_name = provider_name_for_test(&["cla", "ude"]);
        let error = ServiceError::Dependency {
            message: "database is locked".to_string(),
        };
        let expected =
            format!("Warning: Session ingest failed for {provider_name}: database is locked");

        assert_eq!(
            default_provider_session_ingest_user_warning(&provider_name, &error).as_deref(),
            Some(expected.as_str())
        );
    }

    fn provider_name_for_test(parts: &[&str]) -> String {
        parts.concat()
    }

    #[test]
    fn synthetic_carrier_name_format() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path),
            &launcher,
        )
        .expect("synthetic carrier launch should succeed");

        assert_eq!(code, 0);
        assert_eq!(launcher.calls.borrow().len(), 1);
        assert_eq!(launcher.calls.borrow()[0].0, "<provider-family:claude>");
    }

    #[test]
    fn does_not_load_model_toml() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        std::fs::write(
            temp.path().join("claude.toml"),
            "this sentinel model TOML must not be read = [",
        )
        .unwrap();

        let launcher = RecordingLauncher::default();
        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path),
            &launcher,
        )
        .expect("model TOML sentinels must not affect default-provider REPL launch");

        assert_eq!(code, 0);
        assert_eq!(launcher.calls.borrow().len(), 1);
    }

    #[test]
    fn creates_unknown_model_invocation_row() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "generic""#);
        write_providers(temp.path(), &provider_fixture("generic"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
            &launcher,
        )
        .expect("stubbed launcher should allow the default-provider path to complete");

        assert_eq!(code, 0);
        assert_eq!(table_count(&state_path, "invocations"), 1);
        let (model_name, provider_name, status, provider_session_id, capture_method) =
            invocation_row(&state_path);
        assert_eq!(model_name, "<unknown>");
        assert_eq!(provider_name, "generic");
        assert_eq!(status, "succeeded");
        assert_eq!(provider_session_id, None);
        assert_eq!(capture_method, None);
    }

    #[test]
    fn passes_parent_invocation_env_to_launcher() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "generic""#);
        write_providers(temp.path(), &provider_fixture("generic"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
            &launcher,
        )
        .expect("stubbed launcher should allow the default-provider path to complete");

        assert_eq!(code, 0);
        let parent_env = launcher.calls.borrow()[0]
            .2
            .clone()
            .expect("parent invocation env");
        let parsed = CompositeInvocationId::parse_env_value(&parent_env).unwrap();
        let (_model_name, provider_name, _status, _provider_session_id, _capture_method) =
            invocation_row(&state_path);
        assert_eq!(parsed.source, provider_name);
    }

    #[cfg(unix)]
    #[test]
    fn registers_ingested_session_chain() {
        const SESSION_ID: &str = "session-from-new-repl";

        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        let turns_path = temp.path().join("turns.jsonl");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "generic""#);
        write_providers(temp.path(), &provider_fixture("generic"));
        let (_turn_script_dir, turn_script_path) =
            launcher_fixture_script(&format!(r#"cat "{turns}""#, turns = turns_path.display()));
        write_sessions(
            temp.path(),
            &format!(
                r#"[generic]
turn_script = "{}"
"#,
                toml_path(&turn_script_path)
            ),
        );
        let launcher = TurnWritingLauncher {
            turns_path,
            session_id: SESSION_ID.to_string(),
        };

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
            &launcher,
        )
        .expect("stubbed launcher should allow the default-provider path to complete");

        assert_eq!(code, 0);
        assert_eq!(table_count(&state_path, "invocations"), 1);
        assert_eq!(table_count(&state_path, "session_chains"), 1);
        assert_eq!(table_count(&state_path, "session_chain_segments"), 1);
        let (model_name, provider_name, status, provider_session_id, capture_method) =
            invocation_row(&state_path);
        assert_eq!(model_name, "<unknown>");
        assert_eq!(provider_name, "generic");
        assert_eq!(status, "succeeded");
        assert_eq!(provider_session_id.as_deref(), Some(SESSION_ID));
        assert_eq!(capture_method.as_deref(), Some("turn_script"));
        assert_eq!(
            single_string_column(&state_path, "SELECT model_name FROM session_chains"),
            "<unknown>"
        );
        assert_eq!(
            single_string_column(
                &state_path,
                "SELECT provider_name FROM session_chain_segments"
            ),
            "generic"
        );
        assert_eq!(
            single_string_column(&state_path, "SELECT session_id FROM session_chain_segments"),
            SESSION_ID
        );
    }

    #[test]
    fn captured_live_session_survives_cancellation_and_terminal_failure() {
        for (exit_code, terminal_reason) in [(130, "cancelled"), (17, "terminal_failure")] {
            let temp = tempfile::tempdir().unwrap();
            let state_path = temp.path().join("state.db");
            StateDb::open(&state_path).unwrap();
            write_config(temp.path(), r#"default_provider = "generic""#);
            write_providers(temp.path(), &provider_fixture("generic"));
            let session_id = format!("session-terminal-{exit_code}");
            let launcher = CapturingTerminalLauncher {
                session_id: session_id.clone(),
                exit_code,
                terminal_reason: terminal_reason.to_string(),
            };

            let code = run_repl_with_default_provider_with_launcher(
                runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
                &launcher,
            )
            .expect("captured terminal result should finalize normally");

            assert_eq!(code, exit_code);
            let (_model, _provider, status, provider_session_id, capture_method) =
                invocation_row(&state_path);
            assert_eq!(status, "failed");
            assert_eq!(provider_session_id.as_deref(), Some(session_id.as_str()));
            assert_eq!(capture_method.as_deref(), Some("provider_live_report"));
        }
    }

    #[test]
    fn pending_live_session_binding_is_not_ready_until_marker_promotion() {
        const INVOCATION_UUID: &str = "pending-live-session-invocation";
        const SESSION_ID: &str = "pending-live-session";

        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();
        let invocation_row_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .bind_invocation_provider_session_start(
                invocation_row_id,
                &ProviderSessionBinding {
                    provider_session_id: SESSION_ID.to_string(),
                    capture_method: crate::executor::cli::PENDING_LIVE_SESSION_CAPTURE_METHOD,
                    resume_input_id: None,
                    provider_session_resolved_account: Some("fixture-provider".to_string()),
                },
            )
            .unwrap();

        assert_eq!(
            default_provider_invocation_session_id(&state, INVOCATION_UUID).unwrap(),
            None
        );

        state
            .transition_invocation_provider_session_capture_method(
                invocation_row_id,
                SESSION_ID,
                crate::executor::cli::PENDING_LIVE_SESSION_CAPTURE_METHOD,
                "provider_live_report",
            )
            .unwrap();
        assert_eq!(
            default_provider_invocation_session_id(&state, INVOCATION_UUID).unwrap(),
            Some(SESSION_ID.to_string())
        );
    }

    #[test]
    fn does_not_increment_quota_tick() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
            &launcher,
        )
        .expect("stubbed launcher should allow the default-provider path to complete");

        assert_eq!(code, 0);
        assert_eq!(table_count(&state_path, "provider_quotas"), 0);
        assert_eq!(table_count(&state_path, "provider_quota_windows"), 0);
    }

    #[test]
    fn live_session_working_directory_resolves_relative_project_without_git() {
        let relative = PathBuf::from("target/age284-relative-project");
        let absolute = std::env::current_dir().unwrap().join(&relative);
        std::fs::create_dir_all(&absolute).unwrap();

        let resolved = default_provider_effective_cwd(Some(&relative)).unwrap();

        assert_eq!(resolved, absolute.canonicalize().unwrap());
        assert!(!absolute.join(".git").exists());
    }

    #[test]
    fn does_not_call_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(
            temp.path(),
            r#"
diagnostics_model = "malformed-diagnostics-sentinel"
default_provider = "claude"
"#,
        );
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path),
            &launcher,
        )
        .expect("diagnostics_model must not be consulted by the default-provider REPL");

        assert_eq!(code, 0);
        assert_eq!(launcher.calls.borrow().len(), 1);
    }
}
