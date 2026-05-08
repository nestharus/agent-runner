pub mod error;

use crate::diagnostics::Diagnosis;
use crate::executor::ExecutionResult;
use crate::quota::{InFlight, RefreshOutcome};
pub use error::ServiceError;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_state::StateDb;
use std::collections::HashMap;
use std::path::PathBuf;

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
    RoutingServiceRequest,
    RoutingServiceOutput,
    InvocationLifecycleRequest,
    InvocationLifecycleOutput,
    SessionLifecycleRequest,
    SessionLifecycleOutput,
    ResumeServiceRequest,
    ResumeServiceOutput,
    MigrationServiceRequest,
    MigrationServiceOutput,
    TraceServiceRequest,
    TraceServiceOutput,
    SessionExportServiceRequest,
    SessionExportServiceOutput,
    SessionReplaceServiceRequest,
    SessionReplaceServiceOutput,
    SessionLockServiceRequest,
    SessionLockServiceOutput,
    MigrationMaintenanceServiceRequest,
    MigrationMaintenanceServiceOutput,
);

#[derive(Debug)]
#[expect(
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
        request: RoutingServiceRequest,
    ) -> Result<RoutingServiceOutput, ServiceError>;
}

pub trait InvocationLifecycleServicePort: Send + Sync {
    fn record_invocation(
        &self,
        request: InvocationLifecycleRequest,
    ) -> Result<InvocationLifecycleOutput, ServiceError>;
}

pub trait SessionLifecycleServicePort: Send + Sync {
    fn ingest_session(
        &self,
        request: SessionLifecycleRequest,
    ) -> Result<SessionLifecycleOutput, ServiceError>;
}

pub trait ResumeServicePort: Send + Sync {
    fn resolve_resume(
        &self,
        request: ResumeServiceRequest,
    ) -> Result<ResumeServiceOutput, ServiceError>;
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
        request: MigrationServiceRequest,
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
