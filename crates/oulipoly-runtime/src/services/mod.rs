pub mod error;

use crate::balancer::MigrationDecision;
use crate::diagnostics::Diagnosis;
use crate::executor::ExecutionResult;
use crate::quota::{InFlight, RefreshOutcome};
use crate::session_export::ExportError;
use crate::session_lock::{Lease, LockError, ReleaseReceipt, SessionLock};
use crate::session_replace::{ReplaceError, ReplaceReceipt, ReplaceSource};
use crate::trace::{TraceOptions, TraceReport};
pub use error::ServiceError;
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ProvidersConfig, SessionsConfig, load_models,
};
use oulipoly_state::repositories::ResumeRepository;
use oulipoly_state::{ResumeError, StateDb};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

macro_rules! service_dto {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, Default, PartialEq, Eq)]
            pub struct $name;
        )+
    };
}

service_dto!(
    ConfigServiceRequest,
    ConfigServiceOutput,
    MigrationMaintenanceServiceRequest,
    MigrationMaintenanceServiceOutput,
);

pub struct ResumeServiceRequest<'a> {
    pub state: &'a oulipoly_state::StateDb,
    pub models: &'a oulipoly_state::ModelStore,
    pub input: &'a str,
    pub model_override: Option<&'a str>,
}

#[derive(Debug)]
pub enum ResumeServiceOutput {
    ResumeResolved {
        resolved: oulipoly_state::ResolvedResume,
    },
    ResumeRejected {
        error: oulipoly_state::ResumeError,
    },
}

pub struct ResumeAcceptanceRequest<'a> {
    pub state: &'a oulipoly_state::StateDb,
    pub invocation_row_id: i64,
    pub status: &'a str,
    pub evidence: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumeAcceptanceOutput;

pub struct SessionLifecycleRequest<'a> {
    pub state: &'a oulipoly_state::StateDb,
    pub sessions_cfg: &'a oulipoly_config::SessionsConfig,
    pub providers_cfg: Option<&'a oulipoly_config::ProvidersConfig>,
    pub provider_name: &'a str,
    pub invocation_row_id: i64,
    pub invocation_uuid: &'a str,
    pub effective_cwd: Option<&'a Path>,
    pub mode: SessionLifecycleIngestMode,
    pub stderr: &'a mut dyn Write,
}

#[derive(Debug, Clone)]
pub enum SessionLifecycleIngestMode {
    Pinned { resume_target: String },
    Unpinned { capture_method: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLifecycleOutput {
    pub emitted: bool,
    pub session_id: Option<String>,
}

pub struct MigrationServiceRequest<'a> {
    pub state: &'a oulipoly_state::StateDb,
    pub sessions_cfg: &'a oulipoly_config::SessionsConfig,
    pub resolved: &'a oulipoly_state::ResolvedResume,
    pub manual_target: Option<&'a str>,
    pub active_exhausted: bool,
    pub migration_model: &'a oulipoly_config::ModelConfig,
    pub effective_cwd: &'a Path,
    pub stderr: &'a mut dyn Write,
}

#[derive(Debug)]
pub enum MigrationServiceOutput {
    Stay,
    DecisionFailed {
        warning: String,
    },
    Migrated {
        segment: crate::migration::MigratedSegment,
    },
}

pub struct TraceServiceRequest<'a> {
    pub state: &'a StateDb,
    pub sessions_cfg: &'a SessionsConfig,
    pub invocation_uuid: &'a str,
    pub options: TraceOptions,
}

#[derive(Debug)]
pub struct TraceServiceOutput {
    pub result: Result<TraceReport, TraceServiceFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceServiceFailure {
    InvalidInvocationId { input: String, message: String },
    InvocationNotFound { input: String, message: String },
    Operational { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionExportServiceRequest {
    pub session_id: String,
}

#[derive(Debug)]
pub struct SessionExportServiceOutput {
    pub result: Result<Vec<u8>, ExportError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReplaceServiceRequest {
    pub session_id: String,
    pub source: ReplaceSource,
    pub preimage_sha256: Option<String>,
}

#[derive(Debug)]
pub struct SessionReplaceServiceOutput {
    pub result: Result<ReplaceReceipt, ReplaceError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLockServiceRequest {
    Acquire { session_id: String, ttl_ms: u64 },
    Release { session_id: String, token: String },
}

#[derive(Debug)]
pub struct SessionLockServiceOutput {
    pub result: Result<SessionLockSuccess, SessionLockFailure>,
}

#[derive(Debug, Clone)]
pub enum SessionLockSuccess {
    Acquired {
        session_id: String,
        chain_id: String,
        provider_name: String,
        lease: Lease,
    },
    Released {
        receipt: ReleaseReceipt,
    },
}

#[derive(Debug, Clone)]
pub enum SessionLockFailure {
    Resume(ResumeError),
    Lock(LockError),
}

#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "AGE-34 DTO contract pins unboxed ModelConfig and ProviderConfig fields"
)]
pub enum ExecutorServiceRequest {
    Facade {
        model: ModelConfig,
        provider_index: usize,
        prompt: String,
        working_dir: Option<PathBuf>,
        extra_inputs: HashMap<String, Vec<String>>,
        parent_invocation_env: Option<String>,
    },
    Effective {
        model: ModelConfig,
        provider: ProviderConfig,
        provider_index: usize,
        prompt_mode: PromptMode,
        prompt: String,
        working_dir: Option<PathBuf>,
        extra_inputs: HashMap<String, Vec<String>>,
        parent_invocation_env: Option<String>,
    },
    EffectiveWithStartKnownProviderSessionId {
        model: ModelConfig,
        provider: ProviderConfig,
        provider_index: usize,
        prompt_mode: PromptMode,
        prompt: String,
        working_dir: Option<PathBuf>,
        extra_inputs: HashMap<String, Vec<String>>,
        parent_invocation_env: Option<String>,
        start_known_provider_session_id: String,
    },
}

pub struct ExecutorServiceOutput {
    pub result: ExecutionResult,
}

#[derive(Debug)]
pub struct LauncherServiceRequest {
    pub provider: ProviderConfig,
    pub working_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub struct LauncherServiceOutput {
    pub exit_code: i32,
}

pub struct QuotaServiceRequest<'a> {
    pub provider_name: String,
    pub providers_cfg: &'a ProvidersConfig,
    pub in_flight: &'a InFlight,
    pub state: &'a StateDb,
}

pub struct QuotaServiceOutput {
    pub outcome: RefreshOutcome,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "AGE-34 DTO contract pins unboxed ModelConfig and ProviderConfig fields"
)]
pub enum DiagnosticsServiceRequest {
    ClassifyExhaustion {
        stderr: String,
    },
    DiagnoseError {
        diagnostics_model: ModelConfig,
        effective_provider: ProviderConfig,
        provider_index: usize,
        prompt_mode: PromptMode,
        exit_code: i32,
        stderr: String,
        working_dir: Option<PathBuf>,
    },
}

#[derive(Debug)]
pub enum DiagnosticsServiceOutput {
    ExhaustionClassification { is_exhausted: bool },
    Diagnosis { diagnosis: Diagnosis },
}

pub struct RoutingServiceRequest<'a> {
    pub model: &'a oulipoly_config::ModelConfig,
    pub state: &'a oulipoly_state::StateDb,
    pub ctx: Option<&'a crate::balancer::BalanceContext<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingServiceOutput {
    pub provider_index: usize,
}

pub struct InvocationLifecycleStartRequest<'a> {
    pub state: &'a oulipoly_state::StateDb,
    pub start: &'a oulipoly_state::InvocationStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationLifecycleStartOutput {
    pub invocation_row_id: i64,
}

pub struct InvocationLifecycleFinalizeRequest<'a> {
    pub state: &'a oulipoly_state::StateDb,
    pub invocation_row_id: i64,
    pub success: bool,
    pub exit_code: i32,
    pub error_category: Option<&'a str>,
    pub terminal_reason: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InvocationLifecycleFinalizeOutput;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionRoutingService;

impl ProductionRoutingService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionInvocationLifecycleService;

impl ProductionInvocationLifecycleService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionResumeService;

impl ProductionResumeService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionSessionLifecycleService;

impl ProductionSessionLifecycleService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionMigrationService;

impl ProductionMigrationService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductionTraceService {
    _private: (),
}

impl ProductionTraceService {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductionSessionExportService {
    _private: (),
}

impl ProductionSessionExportService {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductionSessionReplaceService {
    _private: (),
}

impl ProductionSessionReplaceService {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductionSessionLockService {
    _private: (),
}

impl ProductionSessionLockService {
    pub fn new() -> Self {
        Self::default()
    }
}

pub trait ConfigServicePort: Send + Sync {
    fn load_config(
        &self,
        request: ConfigServiceRequest,
    ) -> Result<ConfigServiceOutput, ServiceError>;
}

pub trait ExecutorServicePort: Send + Sync {
    fn execute(
        &self,
        request: ExecutorServiceRequest,
    ) -> Result<ExecutorServiceOutput, ServiceError>;
}

pub trait LauncherServicePort: Send + Sync {
    fn launch(
        &self,
        request: LauncherServiceRequest,
    ) -> Result<LauncherServiceOutput, ServiceError>;
}

pub trait QuotaServicePort: Send + Sync {
    fn refresh_quota(
        &self,
        request: QuotaServiceRequest<'_>,
    ) -> Result<QuotaServiceOutput, ServiceError>;
}

pub trait RoutingServicePort: Send + Sync {
    fn select_route(
        &self,
        request: RoutingServiceRequest<'_>,
    ) -> Result<RoutingServiceOutput, ServiceError>;
}

pub trait InvocationLifecycleServicePort: Send + Sync {
    fn start_invocation(
        &self,
        request: InvocationLifecycleStartRequest<'_>,
    ) -> Result<InvocationLifecycleStartOutput, ServiceError>;

    fn finalize_invocation(
        &self,
        request: InvocationLifecycleFinalizeRequest<'_>,
    ) -> Result<InvocationLifecycleFinalizeOutput, ServiceError>;
}

pub trait SessionLifecycleServicePort: Send + Sync {
    fn ingest_session(
        &self,
        request: SessionLifecycleRequest<'_>,
    ) -> Result<SessionLifecycleOutput, ServiceError>;
}

pub trait ResumeServicePort: Send + Sync {
    fn resolve_resume(
        &self,
        request: ResumeServiceRequest<'_>,
    ) -> Result<ResumeServiceOutput, ServiceError>;

    fn record_acceptance(
        &self,
        request: ResumeAcceptanceRequest<'_>,
    ) -> Result<ResumeAcceptanceOutput, ServiceError>;
}

pub trait DiagnosticsServicePort: Send + Sync {
    fn diagnose(
        &self,
        request: DiagnosticsServiceRequest,
    ) -> Result<DiagnosticsServiceOutput, ServiceError>;
}

pub trait MigrationServicePort: Send + Sync {
    fn migrate(
        &self,
        request: MigrationServiceRequest<'_>,
    ) -> Result<MigrationServiceOutput, ServiceError>;
}

pub trait TraceServicePort: Send + Sync {
    fn trace(&self, request: TraceServiceRequest) -> Result<TraceServiceOutput, ServiceError>;
}

pub trait SessionExportServicePort: Send + Sync {
    fn export_session(
        &self,
        request: SessionExportServiceRequest,
    ) -> Result<SessionExportServiceOutput, ServiceError>;
}

pub trait SessionReplaceServicePort: Send + Sync {
    fn replace_session(
        &self,
        request: SessionReplaceServiceRequest,
    ) -> Result<SessionReplaceServiceOutput, ServiceError>;
}

pub trait SessionLockServicePort: Send + Sync {
    fn lock_session(
        &self,
        request: SessionLockServiceRequest,
    ) -> Result<SessionLockServiceOutput, ServiceError>;
}

pub trait MigrationMaintenanceServicePort: Send + Sync {
    fn run_maintenance(
        &self,
        request: MigrationMaintenanceServiceRequest,
    ) -> Result<MigrationMaintenanceServiceOutput, ServiceError>;
}

impl RoutingServicePort for ProductionRoutingService {
    fn select_route(
        &self,
        request: RoutingServiceRequest<'_>,
    ) -> Result<RoutingServiceOutput, ServiceError> {
        Ok(RoutingServiceOutput {
            provider_index: crate::balancer::select_provider(
                request.model,
                request.state,
                request.ctx,
            ),
        })
    }
}

impl InvocationLifecycleServicePort for ProductionInvocationLifecycleService {
    fn start_invocation(
        &self,
        request: InvocationLifecycleStartRequest<'_>,
    ) -> Result<InvocationLifecycleStartOutput, ServiceError> {
        request
            .state
            .start_invocation(request.start)
            .map(|invocation_row_id| InvocationLifecycleStartOutput { invocation_row_id })
            .map_err(|message| ServiceError::Dependency { message })
    }

    fn finalize_invocation(
        &self,
        request: InvocationLifecycleFinalizeRequest<'_>,
    ) -> Result<InvocationLifecycleFinalizeOutput, ServiceError> {
        request
            .state
            .finalize_invocation(
                request.invocation_row_id,
                request.success,
                request.exit_code,
                request.error_category,
                request.terminal_reason,
            )
            .map(|_| InvocationLifecycleFinalizeOutput)
            .map_err(|message| ServiceError::Dependency { message })
    }
}

impl ResumeServicePort for ProductionResumeService {
    fn resolve_resume(
        &self,
        request: ResumeServiceRequest<'_>,
    ) -> Result<ResumeServiceOutput, ServiceError> {
        match <StateDb as ResumeRepository>::resolve_resume(
            request.state,
            request.models,
            request.input,
            request.model_override,
        ) {
            Ok(resolved) => Ok(ResumeServiceOutput::ResumeResolved { resolved }),
            Err(error) => Ok(ResumeServiceOutput::ResumeRejected { error }),
        }
    }

    fn record_acceptance(
        &self,
        request: ResumeAcceptanceRequest<'_>,
    ) -> Result<ResumeAcceptanceOutput, ServiceError> {
        request
            .state
            .update_resume_acceptance(request.invocation_row_id, request.status, request.evidence)
            .map(|_| ResumeAcceptanceOutput)
            .map_err(|message| ServiceError::Dependency { message })
    }
}

impl SessionLifecycleServicePort for ProductionSessionLifecycleService {
    fn ingest_session(
        &self,
        request: SessionLifecycleRequest<'_>,
    ) -> Result<SessionLifecycleOutput, ServiceError> {
        let SessionLifecycleRequest {
            state,
            sessions_cfg,
            providers_cfg,
            provider_name,
            invocation_row_id,
            invocation_uuid,
            effective_cwd,
            mode,
            stderr,
        } = request;

        let invocation = match state.get_invocation_by_uuid(invocation_uuid) {
            Ok(Some(row)) => row,
            Ok(None) => {
                writeln!(
                    stderr,
                    "Warning: Could not resolve invocation {invocation_uuid} for session ingest"
                )
                .map_err(|err| ServiceError::Dependency {
                    message: format!("Failed to write session ingest warning: {err}"),
                })?;
                return emit_pinned_session_id_for_service(
                    state,
                    stderr,
                    invocation_row_id,
                    invocation_uuid,
                    mode,
                );
            }
            Err(err) => {
                writeln!(
                    stderr,
                    "Warning: Failed to load invocation {invocation_uuid} for session ingest: {err}"
                )
                .map_err(|err| ServiceError::Dependency {
                    message: format!("Failed to write session ingest warning: {err}"),
                })?;
                return emit_pinned_session_id_for_service(
                    state,
                    stderr,
                    invocation_row_id,
                    invocation_uuid,
                    mode,
                );
            }
        };
        if invocation.id != invocation_row_id {
            return Err(ServiceError::InvalidRequest {
                message: format!(
                    "session ingest request mismatched invocation identifiers: row_id={invocation_row_id} uuid={invocation_uuid}"
                ),
            });
        }
        let Some(finished_at) = invocation.finished_at else {
            writeln!(
                stderr,
                "Warning: Invocation {invocation_uuid} was not finalized before session ingest"
            )
            .map_err(|err| ServiceError::Dependency {
                message: format!("Failed to write session ingest warning: {err}"),
            })?;
            return emit_pinned_session_id_for_service(
                state,
                stderr,
                invocation_row_id,
                invocation_uuid,
                mode,
            );
        };

        let report = crate::sessions::scan_provider(provider_name, sessions_cfg, state);
        for err in report.errors {
            writeln!(
                stderr,
                "Warning: Session ingest failed for {provider_name}: {err}"
            )
            .map_err(|err| ServiceError::Dependency {
                message: format!("Failed to write session ingest warning: {err}"),
            })?;
        }

        let matched_session_id = match find_session_for_invocation_window(
            state,
            providers_cfg,
            provider_name,
            &invocation.created_at,
            &finished_at,
            effective_cwd,
            stderr,
        ) {
            Ok(session_id) => session_id,
            Err(err) => {
                writeln!(
                    stderr,
                    "Warning: Failed to resolve session for invocation {invocation_uuid}: {err}"
                )
                .map_err(|err| ServiceError::Dependency {
                    message: format!("Failed to write session ingest warning: {err}"),
                })?;
                None
            }
        };

        match mode {
            SessionLifecycleIngestMode::Unpinned { capture_method } => {
                if let Some(session_id) = invocation.provider_session_id.as_deref() {
                    if matched_session_id
                        .as_deref()
                        .is_some_and(|matched| matched != session_id)
                    {
                        writeln!(
                            stderr,
                            "Warning: post-run session inference found {matched:?}, preserving start-bound provider_session_id {session_id}",
                            matched = matched_session_id.as_deref()
                        )
                        .map_err(|err| ServiceError::Dependency {
                            message: format!("Failed to write session ingest warning: {err}"),
                        })?;
                    }
                    emit_known_session_id_for_service(
                        state,
                        stderr,
                        invocation_row_id,
                        invocation_uuid,
                        session_id,
                        invocation
                            .provider_session_capture_method
                            .as_deref()
                            .unwrap_or(&capture_method),
                    )
                } else if let Some(session_id) = matched_session_id {
                    emit_known_session_id_for_service(
                        state,
                        stderr,
                        invocation_row_id,
                        invocation_uuid,
                        &session_id,
                        &capture_method,
                    )
                } else {
                    Ok(SessionLifecycleOutput {
                        emitted: false,
                        session_id: None,
                    })
                }
            }
            SessionLifecycleIngestMode::Pinned { resume_target } => {
                emit_known_session_id_for_service(
                    state,
                    stderr,
                    invocation_row_id,
                    invocation_uuid,
                    &resume_target,
                    "resumed",
                )
            }
        }
    }
}

impl MigrationServicePort for ProductionMigrationService {
    fn migrate(
        &self,
        request: MigrationServiceRequest<'_>,
    ) -> Result<MigrationServiceOutput, ServiceError> {
        match crate::balancer::decide_migration(
            request.state,
            request.migration_model,
            request.resolved,
            request.manual_target,
        ) {
            Ok(MigrationDecision::Stay) => Ok(MigrationServiceOutput::Stay),
            Err(err) => Ok(MigrationServiceOutput::DecisionFailed {
                warning: format!("{err:?}"),
            }),
            Ok(MigrationDecision::Migrate {
                target_provider_index,
                reason,
            }) => crate::migration::migrate_chain_segment(
                request.state,
                request.sessions_cfg,
                request.migration_model,
                request.resolved,
                request.effective_cwd,
                target_provider_index,
                reason,
                request.stderr,
            )
            .map(|segment| MigrationServiceOutput::Migrated { segment })
            .map_err(|err| ServiceError::Dependency {
                message: format!("{err:?}"),
            }),
        }
    }
}

impl TraceServicePort for ProductionTraceService {
    fn trace(&self, request: TraceServiceRequest<'_>) -> Result<TraceServiceOutput, ServiceError> {
        let result = crate::trace::trace_invocation_with_sessions(
            request.state,
            request.invocation_uuid,
            request.options,
            Some(request.sessions_cfg),
        )
        .map_err(|message| classify_trace_failure(request.invocation_uuid, message));
        Ok(TraceServiceOutput { result })
    }
}

impl SessionExportServicePort for ProductionSessionExportService {
    fn export_session(
        &self,
        request: SessionExportServiceRequest,
    ) -> Result<SessionExportServiceOutput, ServiceError> {
        let result = crate::session_export::resolve_export_session_metadata(&request.session_id)
            .and_then(|metadata| {
                crate::session_export::read_canonical_transcript(&metadata)
                    .and_then(|records| crate::session_export::canonical_jsonl_bytes(&records))
            });
        Ok(SessionExportServiceOutput { result })
    }
}

impl SessionReplaceServicePort for ProductionSessionReplaceService {
    fn replace_session(
        &self,
        request: SessionReplaceServiceRequest,
    ) -> Result<SessionReplaceServiceOutput, ServiceError> {
        let input_path = match &request.source {
            ReplaceSource::File(path) => Some(path.as_path()),
            ReplaceSource::Stdin => None,
        };
        let result = crate::session_replace::run_import_replace(
            &request.session_id,
            input_path,
            request.preimage_sha256.as_deref(),
        );
        Ok(SessionReplaceServiceOutput { result })
    }
}

impl SessionLockServicePort for ProductionSessionLockService {
    fn lock_session(
        &self,
        request: SessionLockServiceRequest,
    ) -> Result<SessionLockServiceOutput, ServiceError> {
        let result = match request {
            SessionLockServiceRequest::Acquire { session_id, ttl_ms } => {
                acquire_session_lock(&session_id, ttl_ms)
            }
            SessionLockServiceRequest::Release { session_id, token } => {
                release_session_lock(&session_id, &token)
            }
        };
        Ok(SessionLockServiceOutput { result })
    }
}

fn classify_trace_failure(input: &str, message: String) -> TraceServiceFailure {
    let lower = message.to_ascii_lowercase();
    if lower.contains("invalid invocation uuid") {
        TraceServiceFailure::InvalidInvocationId {
            input: input.to_string(),
            message,
        }
    } else if lower.contains("unknown invocation") || lower.contains("invocation not found") {
        TraceServiceFailure::InvocationNotFound {
            input: input.to_string(),
            message,
        }
    } else {
        TraceServiceFailure::Operational { message }
    }
}

fn acquire_session_lock(
    session_id: &str,
    ttl_ms: u64,
) -> Result<SessionLockSuccess, SessionLockFailure> {
    let state = StateDb::open_default()
        .map_err(|message| SessionLockFailure::Lock(LockError::Operational { message }))?;
    let providers_cfg =
        ProvidersConfig::load(&default_config_root().join("providers.toml")).unwrap_or_default();
    let models = load_models(&default_models_dir(), Some(&providers_cfg))
        .map_err(|message| SessionLockFailure::Lock(LockError::Operational { message }))?;
    reject_recent_ambiguous_resume(&state, session_id).map_err(SessionLockFailure::Resume)?;
    let resolved = <StateDb as ResumeRepository>::resolve_resume(&state, &models, session_id, None)
        .map_err(SessionLockFailure::Resume)?;
    let lock_dir = default_lock_dir().map_err(SessionLockFailure::Lock)?;
    let lock = SessionLock::new(&lock_dir).map_err(|err| {
        SessionLockFailure::Lock(LockError::Operational {
            message: format!("failed to open locks: {err}"),
        })
    })?;
    let lease = lock
        .acquire(
            &resolved.active_session_id,
            &resolved.active_provider,
            Duration::from_millis(ttl_ms),
        )
        .map_err(SessionLockFailure::Lock)?;
    Ok(SessionLockSuccess::Acquired {
        session_id: resolved.active_session_id,
        chain_id: resolved.chain_id,
        provider_name: resolved.active_provider,
        lease,
    })
}

fn reject_recent_ambiguous_resume(state: &StateDb, session_id: &str) -> Result<(), ResumeError> {
    let previews = state
        .resume_previews(session_id)
        .map_err(|message| ResumeError::Db { message })?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    let recent_count = previews
        .iter()
        .filter(|preview| preview.last_used_at >= cutoff)
        .count();
    if recent_count > 1 {
        return Err(ResumeError::Ambiguous {
            input: session_id.to_string(),
            previews,
        });
    }
    Ok(())
}

fn release_session_lock(
    session_id: &str,
    token: &str,
) -> Result<SessionLockSuccess, SessionLockFailure> {
    // Preserve resume-handshake's state-open gate; release itself does not resolve providers.
    StateDb::open_default()
        .map_err(|message| SessionLockFailure::Lock(LockError::Operational { message }))?;
    let lock_dir = default_lock_dir().map_err(SessionLockFailure::Lock)?;
    let lock = SessionLock::new(&lock_dir).map_err(|err| {
        SessionLockFailure::Lock(LockError::Operational {
            message: format!("failed to open locks: {err}"),
        })
    })?;
    let receipt = lock
        .release(session_id, token)
        .map_err(SessionLockFailure::Lock)?;
    Ok(SessionLockSuccess::Released { receipt })
}

fn default_config_root() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_models_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}

fn default_lock_dir() -> Result<PathBuf, LockError> {
    dirs::data_dir()
        .map(|dir| dir.join("oulipoly-agent-runner").join("locks"))
        .ok_or_else(|| LockError::Operational {
            message: "Could not determine data directory".to_string(),
        })
}

fn emit_pinned_session_id_for_service(
    state: &StateDb,
    stderr: &mut dyn Write,
    invocation_row_id: i64,
    invocation_uuid: &str,
    mode: SessionLifecycleIngestMode,
) -> Result<SessionLifecycleOutput, ServiceError> {
    match mode {
        SessionLifecycleIngestMode::Unpinned { .. } => Ok(SessionLifecycleOutput {
            emitted: false,
            session_id: None,
        }),
        SessionLifecycleIngestMode::Pinned { resume_target } => emit_known_session_id_for_service(
            state,
            stderr,
            invocation_row_id,
            invocation_uuid,
            &resume_target,
            "resumed",
        ),
    }
}

fn find_session_for_invocation_window(
    state: &StateDb,
    providers_cfg: Option<&ProvidersConfig>,
    provider_name: &str,
    started_at: &chrono::DateTime<chrono::Utc>,
    finished_at: &chrono::DateTime<chrono::Utc>,
    effective_cwd: Option<&Path>,
    stderr: &mut dyn Write,
) -> Result<Option<String>, String> {
    let candidates =
        state.find_sessions_for_invocation_window(provider_name, started_at, finished_at)?;
    if candidates.len() <= 1 {
        return Ok(candidates.into_iter().next());
    }

    let Some(effective_cwd) = effective_cwd else {
        return Ok(candidates.into_iter().next());
    };
    let Some(session_storage) = providers_cfg
        .and_then(|providers| providers.get(provider_name))
        .and_then(|entry| entry.session_storage.as_ref())
    else {
        return Ok(candidates.into_iter().next());
    };

    let expected_cwd = comparable_workspace_path(effective_cwd);
    let mut resolved_any = false;
    for session_id in &candidates {
        let Ok(workspace_root) =
            crate::session_metadata::resolve_workspace_root_for_provider_session(
                Some(session_storage),
                provider_name,
                session_id,
            )
        else {
            continue;
        };
        resolved_any = true;
        if comparable_workspace_path(&workspace_root) == expected_cwd {
            return Ok(Some(session_id.clone()));
        }
    }

    if resolved_any {
        writeln!(
            stderr,
            "Warning: no in-window session for provider {provider_name} matched cwd {}; not emitting inferred session id",
            effective_cwd.display()
        )
        .map_err(|err| format!("Failed to write session ingest warning: {err}"))?;
        Ok(None)
    } else {
        Ok(candidates.into_iter().next())
    }
}

fn comparable_workspace_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn emit_known_session_id_for_service(
    state: &StateDb,
    stderr: &mut dyn Write,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
) -> Result<SessionLifecycleOutput, ServiceError> {
    if let Err(err) =
        state.update_session_capture(invocation_row_id, Some(session_id), capture_method)
    {
        writeln!(
            stderr,
            "Warning: Failed to update invocation session_id: {err}"
        )
        .map_err(|err| ServiceError::Dependency {
            message: format!("Failed to write session ingest warning: {err}"),
        })?;
        return Ok(SessionLifecycleOutput {
            emitted: false,
            session_id: None,
        });
    }
    let record = state.get_invocation_by_uuid(invocation_uuid).ok().flatten();
    let should_mint_chain = record
        .as_ref()
        .is_none_or(|row| row.resume_input_id.as_deref() != row.provider_session_id.as_deref());
    if should_mint_chain
        && let Err(err) = state.mint_chain_for_invocation_session(invocation_row_id)
    {
        writeln!(stderr, "Warning: Failed to mint session chain: {err}").map_err(|err| {
            ServiceError::Dependency {
                message: format!("Failed to write session ingest warning: {err}"),
            }
        })?;
    }
    let provider_name = record.as_ref().and_then(|row| row.provider_name.clone());
    let provider_session_id = record
        .as_ref()
        .and_then(|row| row.provider_session_id.clone())
        .or_else(|| {
            if capture_method == "resumed" {
                None
            } else {
                Some(session_id.to_string())
            }
        });
    let agent_runner_chain_id = provider_name.as_deref().and_then(|provider_name| {
        provider_session_id
            .as_deref()
            .and_then(|provider_session_id| {
                state
                    .chain_id_for_segment(provider_name, provider_session_id)
                    .ok()
                    .flatten()
            })
    });
    let payload = oulipoly_state::SessionMarkerPayload {
        agent_runner_invocation_id: invocation_uuid.to_string(),
        provider_session_id: provider_session_id.clone(),
        provider_name,
        agent_runner_chain_id,
        resume_input_id: record.as_ref().and_then(|row| row.resume_input_id.clone()),
        legacy_id: invocation_uuid.to_string(),
        legacy_session_id: Some(session_id.to_string()),
    };
    write!(stderr, "{}", payload.stderr_line()).map_err(|err| ServiceError::Dependency {
        message: format!("Failed to write session marker: {err}"),
    })?;
    Ok(SessionLifecycleOutput {
        emitted: true,
        session_id: Some(session_id.to_string()),
    })
}
