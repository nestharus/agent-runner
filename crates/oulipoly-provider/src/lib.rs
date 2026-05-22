use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub trait ProviderLaunch {
    fn prepare_launch(&self, request: LaunchRequest<'_>) -> Result<LaunchPlan, CapabilityError>;
}

pub trait ProviderPolicy {
    fn evaluate_policy(
        &self,
        request: PolicyRequest<'_>,
    ) -> Result<PolicyTransform, CapabilityError>;
}

pub trait TerminalSignalRecognizer {
    fn recognize(&self, evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal;
}

pub trait ProviderQuota {
    fn has_quota_source(&self, context: ProviderContext<'_>) -> bool;
    fn probe_quota(&self, request: QuotaRequest<'_>) -> Result<QuotaSnapshot, CapabilityError>;
    fn refresh_auth(
        &self,
        request: AuthRefreshRequest<'_>,
    ) -> Result<AuthRefreshStatus, CapabilityError>;
}

pub trait ProviderSession {
    fn read_session_turns(
        &self,
        request: SessionTurnRequest<'_>,
    ) -> Result<SessionTurnBatch, CapabilityError>;

    fn capture_session(
        &self,
        request: SessionCaptureRequest<'_>,
    ) -> Result<SessionCapture, CapabilityError>;
}

pub trait ProviderRotation {
    fn assess_rotation(
        &self,
        request: RotationRequest<'_>,
    ) -> Result<RotationAssessment, CapabilityError>;

    fn materialize_rotation(
        &self,
        request: RotationMaterializationRequest<'_>,
    ) -> Result<RotationMaterialization, CapabilityError>;
}

pub trait ProviderDiscovery {
    fn discover(&self, request: DiscoveryRequest<'_>) -> Result<DiscoveryReport, CapabilityError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    Unsupported,
    Invalid { reason: String },
    Unavailable { reason: String },
    Failed { reason: String },
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("capability is unsupported"),
            Self::Invalid { reason } => write!(formatter, "invalid request: {reason}"),
            Self::Unavailable { reason } => {
                write!(formatter, "capability is unavailable: {reason}")
            }
            Self::Failed { reason } => write!(formatter, "capability failed: {reason}"),
        }
    }
}

impl std::error::Error for CapabilityError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchRequest<'a> {
    pub input: Option<&'a str>,
    pub working_directory: Option<&'a Path>,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchPlan {
    pub program: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: Option<PathBuf>,
    pub stdin: Vec<u8>,
    pub capture_stdout: bool,
    pub capture_stderr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolicyRequest<'a> {
    pub descriptors: BTreeMap<String, String>,
    pub requested_arguments: Vec<String>,
    pub input: Option<&'a str>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolicyTransform {
    pub accepted: bool,
    pub arguments_to_add: Vec<String>,
    pub environment_to_set: BTreeMap<String, String>,
    pub rejection: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalSignalEvidence<'a> {
    pub stdout: &'a [u8],
    pub stderr: &'a [u8],
    pub status_code: Option<i32>,
    pub elapsed: Option<Duration>,
    pub completed_at: Option<SystemTime>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TerminalSignal {
    #[default]
    Unknown,
    Success,
    Failure,
    Throttled,
    AuthenticationNeeded,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderContext<'a> {
    pub reference: Option<&'a str>,
    pub environment: BTreeMap<String, String>,
    pub observed_at: Option<SystemTime>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuotaRequest<'a> {
    pub reference: Option<&'a str>,
    pub observed_at: Option<SystemTime>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuotaSnapshot {
    pub available: bool,
    pub remaining_units: Option<u64>,
    pub reset_after: Option<Duration>,
    pub checked_at: Option<SystemTime>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthRefreshRequest<'a> {
    pub reference: Option<&'a str>,
    pub force: bool,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthRefreshStatus {
    pub refreshed: bool,
    pub available: bool,
    pub checked_at: Option<SystemTime>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionTurnRequest<'a> {
    pub session_id: Option<&'a str>,
    pub limit: Option<usize>,
    pub since: Option<SystemTime>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionTurnBatch {
    pub turns: Vec<BTreeMap<String, String>>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionCaptureRequest<'a> {
    pub session_id: Option<&'a str>,
    pub source: Option<&'a Path>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionCapture {
    pub session_id: Option<String>,
    pub payload: Vec<u8>,
    pub captured_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RotationRequest<'a> {
    pub session_id: Option<&'a str>,
    pub target: Option<&'a str>,
    pub reason: Option<&'a str>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RotationAssessment {
    pub allowed: bool,
    pub score: Option<u32>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RotationMaterializationRequest<'a> {
    pub source: Option<&'a Path>,
    pub target: Option<&'a Path>,
    pub dry_run: bool,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RotationMaterialization {
    pub changed: bool,
    pub artifacts: Vec<PathBuf>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscoveryRequest<'a> {
    pub roots: Vec<&'a Path>,
    pub hint: Option<&'a str>,
    marker: PhantomData<&'a ()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscoveryReport {
    pub items: BTreeMap<String, String>,
    pub defaults: BTreeMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCapabilities<
    Launch = (),
    Policy = (),
    Terminal = (),
    Quota = (),
    Session = (),
    Locator = (),
    Rotation = (),
    Discovery = (),
> {
    pub launch: Option<Launch>,
    pub policy: Option<Policy>,
    pub terminal: Option<Terminal>,
    pub quota: Option<Quota>,
    pub session: Option<Session>,
    pub transcript_locator: Option<Locator>,
    pub rotation: Option<Rotation>,
    pub discovery: Option<Discovery>,
}

impl<Launch, Policy, Terminal, Quota, Session, Locator, Rotation, Discovery> Default
    for ProviderCapabilities<Launch, Policy, Terminal, Quota, Session, Locator, Rotation, Discovery>
{
    fn default() -> Self {
        Self {
            launch: None,
            policy: None,
            terminal: None,
            quota: None,
            session: None,
            transcript_locator: None,
            rotation: None,
            discovery: None,
        }
    }
}
