//! ## Declared roles
//! accessor
//!
//! Service DTO contract carrier. This module owns request and output shapes only;
//! production adapters and helper orchestration live in sibling modules.
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/services/dtos.rs
//!     role: intrinsic-surface
//!     Domain: service-boundary DTO carrier
//!     Owns:
//!       - service request DTOs
//!       - service output DTOs
//!       - byte-carrying terminal-classify request DTO
//!       - terminal-classify output DTO
//! ```

use crate::diagnostics::Diagnosis;
use crate::executor::ExecutionResult;
use crate::executor::terminal_signal::TerminalSignal;
use crate::quota::{InFlight, RefreshOutcome};
use crate::session_export::ExportError;
use crate::session_lock::{Lease, LockError, ReleaseReceipt};
use crate::session_replace::{ReplaceError, ReplaceReceipt, ReplaceSource};
use crate::trace::{TraceOptions, TraceReport};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig, SessionsConfig};
use oulipoly_provider::generated::ProcessStatus;
use oulipoly_state::{ResumeError, StateDb};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    pub external_provider: Option<SessionServiceExternalProviderIdentity>,
    pub invocation_row_id: i64,
    pub invocation_uuid: &'a str,
    pub effective_cwd: Option<&'a Path>,
    pub mode: SessionLifecycleIngestMode,
    pub stderr: &'a mut dyn Write,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionServiceExternalProviderIdentity {
    pub model_name: String,
    pub provider_name: String,
    pub provider_instance_id: Option<String>,
    pub settings_id: String,
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
    Migrated {
        segment: crate::migration::MigratedSegment,
    },
    /// AGE-163 WU-A.2: the auto-rotate-on-quota-threshold path tried one or
    /// more candidates that failed with `SourceMissing*`, advanced through
    /// the working set, and succeeded against `segment.target_provider`.
    AutoRotated {
        segment: crate::migration::MigratedSegment,
        candidates_tried: Vec<String>,
    },
    /// AGE-163 WU-A.2: the rotation request could not be honored.
    RotationFailed {
        reason: RotationFailedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationFailedReason {
    WorkingSetExhausted { candidates_tried: Vec<String> },
    ManualTargetNotInPool { target: String, pool: Vec<String> },
    ManualTargetNotMigratable { source: String, target: String },
    ManualTargetIsSingleProviderPool { provider: String },
    ManualTargetActiveNotInPool { active: String },
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
    pub external_provider: Option<SessionServiceExternalProviderIdentity>,
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
    pub external_provider: Option<SessionServiceExternalProviderIdentity>,
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
        models_dir: Option<PathBuf>,
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
        models_dir: Option<PathBuf>,
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
        models_dir: Option<PathBuf>,
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
    pub external_provider: Option<QuotaServiceExternalProviderIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaServiceExternalProviderIdentity {
    pub model_name: String,
    pub provider_instance_id: String,
    pub settings_id: String,
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
    ClassifyTerminal(TerminalClassifyServiceRequest),
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
    TerminalClassification(TerminalClassification),
    Diagnosis { diagnosis: Diagnosis },
}

#[derive(Debug, Clone)]
pub struct TerminalClassifyServiceRequest {
    pub model_name: String,
    pub provider_name: String,
    pub settings_id: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ProcessStatus,
    pub observed_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalClassification {
    pub exit_code: i32,
    pub terminal_reason: Option<String>,
    pub terminal_signal: TerminalSignal,
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
