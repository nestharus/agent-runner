use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::collections::BTreeMap;
use std::marker::PhantomData;

pub const CONTRACT_VERSION: &str = "oulipoly.provider/v1";

pub type JsonObject = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostContext {
    pub app: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_root: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope<Params = Value> {
    pub contract: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_instance_id: Option<String>,
    pub host: HostContext,
    pub params: Params,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuccessResponseEnvelope<Result = Value> {
    pub contract: String,
    pub request_id: String,
    pub ok: TrueBool,
    pub result: Result,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrueBool;

impl Serialize for TrueBool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for TrueBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_bool(TrueBoolVisitor)
    }
}

struct TrueBoolVisitor;

impl<'de> Visitor<'de> for TrueBoolVisitor {
    type Value = TrueBool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ok=true")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        true_bool_from_value(value)
    }
}

fn true_bool_from_value<E>(value: bool) -> Result<TrueBool, E>
where
    E: de::Error,
{
    if value {
        Ok(TrueBool)
    } else {
        Err(E::custom(true_bool_error()))
    }
}

fn true_bool_error() -> &'static str {
    "expected ok=true"
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FalseBool;

impl Serialize for FalseBool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for FalseBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_bool(FalseBoolVisitor)
    }
}

struct FalseBoolVisitor;

impl<'de> Visitor<'de> for FalseBoolVisitor {
    type Value = FalseBool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ok=false")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        false_bool_from_value(value)
    }
}

fn false_bool_from_value<E>(value: bool) -> Result<FalseBool, E>
where
    E: de::Error,
{
    if value {
        Err(E::custom(false_bool_error()))
    } else {
        Ok(FalseBool)
    }
}

fn false_bool_error() -> &'static str {
    "expected ok=false"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Unsupported,
    InvalidRequest,
    InvalidSettings,
    Unavailable,
    Timeout,
    Conflict,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: String,
    pub category: ErrorCategory,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorResponseEnvelope {
    pub contract: String,
    pub request_id: String,
    pub ok: FalseBool,
    pub error: ErrorObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_status: Option<ProcessStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProcessStatus {
    Exited {
        code: i32,
    },
    #[serde(rename = "signal_terminated")]
    SignalTerminated {
        signal: i32,
    },
    SpawnError {
        reason: String,
    },
    ProlongedSilence {
        reason: String,
    },
    Cancelled,
    Unknown,
}

impl ProcessStatus {
    pub fn exited_successfully(&self) -> bool {
        matches!(self, Self::Exited { code: 0 })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSignalKind {
    CleanExit,
    NonzeroExit,
    SignalExit,
    SpawnError,
    QuotaExhaustedInband,
    MaybeQuotaExhausted,
    RateLimited,
    ProlongedSilence,
    Cancelled,
    Unknown,
}

impl TerminalSignalKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CleanExit => "clean_exit",
            Self::NonzeroExit => "nonzero_exit",
            Self::SignalExit => "signal_exit",
            Self::SpawnError => "spawn_error",
            Self::QuotaExhaustedInband => "quota_exhausted_inband",
            Self::MaybeQuotaExhausted => "maybe_quota_exhausted",
            Self::RateLimited => "rate_limited",
            Self::ProlongedSilence => "prolonged_silence",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalSignal {
    pub kind: TerminalSignalKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    pub observed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BytePayload {
    pub encoding: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderModelRequest {
    pub name: String,
    pub provider_args: Vec<String>,
    pub inputs: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptHint {
    pub format_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<JsonObject>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmptyParams {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescribeCapabilities {
    pub launch: bool,
    pub policy: bool,
    pub quota: bool,
    pub session: bool,
    pub terminal: bool,
    pub rotation: bool,
    pub discovery: bool,
    pub settings: bool,
    pub setup_brain: bool,
    pub setup: bool,
    pub migration: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConcurrency {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_for_parallel_invocation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_locking: Option<String>,
    #[serde(flatten)]
    pub metadata: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescribeResult {
    pub provider_id: String,
    pub display_name: String,
    pub contract_versions: Vec<String>,
    pub preferred_contract: String,
    pub capabilities: DescribeCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_schema_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<ProviderConcurrency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaParams {
    pub schema_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaResult {
    pub schema_id: String,
    pub schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<Value>,
}

pub type SettingsId = String;
pub type SettingsVersion = String;
pub type SettingsValues = JsonObject;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsRecordSummary {
    pub id: SettingsId,
    pub display_name: String,
    pub version: SettingsVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsRecord {
    pub id: SettingsId,
    pub display_name: String,
    pub version: SettingsVersion,
    pub values: SettingsValues,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsListResult {
    pub records: Vec<SettingsRecordSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsGetParams {
    pub id: SettingsId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsGetResult {
    pub record: SettingsRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsCreateParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub values: SettingsValues,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsWriteResult {
    pub record: SettingsRecord,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsUpdateParams {
    pub id: SettingsId,
    pub version: SettingsVersion,
    pub values: SettingsValues,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDeleteParams {
    pub id: SettingsId,
    pub version: SettingsVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDeleteResult {
    pub deleted: bool,
    pub id: SettingsId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsValidateParams {
    pub values: SettingsValues,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsValidateResult {
    pub valid: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsMigrateParams {
    pub dry_run: bool,
    pub legacy: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsMigrateResult {
    pub actions: Vec<Value>,
    pub warnings: Vec<String>,
    pub requires_user_input: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyEvaluateParams {
    pub settings_id: String,
    pub mode: String,
    pub model: ProviderModelRequest,
    pub launch: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyEvaluateResult {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    pub stdin: Option<String>,
    pub prompt: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    pub markers: Vec<Marker>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchParams {
    pub settings_id: String,
    pub mode: String,
    pub model: ProviderModelRequest,
    pub argv: Vec<String>,
    pub working_directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<BytePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalClassifyParams {
    pub stdout_base64: String,
    pub stderr_base64: String,
    pub status: ProcessStatus,
    pub observed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalClassifyResult {
    pub terminal_signal: TerminalSignal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaBaseParams {
    pub settings_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaSourceResult {
    pub has_source: bool,
    pub freshness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaProbeResult {
    pub available: bool,
    pub checked_at_unix_ms: u64,
    pub windows: Vec<QuotaProbeWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaProbeWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub remaining_ratio: f64,
    pub resets_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaRefreshAuthParams {
    pub settings_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaRefreshAuthResult {
    pub refreshed: bool,
    pub available: bool,
    pub checked_at_unix_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionBaseParams {
    pub settings_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub extra: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionLocateTranscriptResult {
    pub located: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_existing_observed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReadTurnsResult {
    pub turns: Vec<Value>,
    pub turn_count: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCaptureResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<Value>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionExportResult {
    pub canonical_format: String,
    pub data_base64: String,
    pub turn_count: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReplaceCanonicalTranscript {
    pub kind: String,
    pub data_base64: String,
    pub sha256: String,
    pub turn_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReplaceParams {
    pub settings_id: String,
    pub session_id: String,
    pub model_name: String,
    pub provider_name: String,
    pub replace_protocol: String,
    pub operation_id: String,
    pub canonical_format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_transcript: Option<SessionReplaceCanonicalTranscript>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preimage_sha256_expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_apply_capability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_id: Option<String>,
    #[serde(flatten)]
    pub extra: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReplaceCanonicalPostimage {
    pub format_id: String,
    pub sha256: String,
    pub turn_count: u64,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<SessionReplaceArtifactEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReplaceArtifactEvidence {
    pub role: String,
    pub format_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_len: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReplaceResult {
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preimage_sha256_observed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postimage_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_postimage: Option<SessionReplaceCanonicalPostimage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_preimage_artifact: Option<SessionReplaceArtifactEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_postimage_artifact: Option<SessionReplaceArtifactEvidence>,
    pub artifacts: Vec<Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_state_plan: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RotationObject {
    #[serde(flatten)]
    pub fields: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RotationAssessResult {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub requirements: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RotationMaterializeResult {
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_provider_session_id: Option<String>,
    pub artifacts: Vec<Artifact>,
    pub host_state_plan: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryObject {
    #[serde(flatten)]
    pub fields: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryModelsResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_version: Option<String>,
    pub models: Vec<Value>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryAccountsResult {
    pub accounts: Vec<Value>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupObject {
    #[serde(flatten)]
    pub fields: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupDetectResult {
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<Value>>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupInstallPlanResult {
    pub steps: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupSyncPlanResult {
    pub operations: Vec<Value>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupBrainTurnResult {
    pub conversation_id: String,
    pub message: Value,
    pub markers: Vec<Marker>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationObject {
    #[serde(flatten)]
    pub fields: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationPlanResult {
    pub actions: Vec<Value>,
    pub warnings: Vec<String>,
    pub requires_backup: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationApplyResult {
    pub applied_actions: Vec<Value>,
    pub artifacts: Vec<Artifact>,
    pub warnings: Vec<String>,
    pub outcome: Value,
}

macro_rules! fixed_str_type {
    ($name:ident, $value:literal) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str($value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserialize_fixed_str_type(deserializer, $value, stringify!($name))
            }
        }
    };
}

struct FixedStrTypeVisitor<T> {
    expected: &'static str,
    type_name: &'static str,
    marker: PhantomData<T>,
}

impl<T> FixedStrTypeVisitor<T> {
    fn new(expected: &'static str, type_name: &'static str) -> Self {
        Self {
            expected,
            type_name,
            marker: PhantomData,
        }
    }
}

impl<'de, T> Visitor<'de> for FixedStrTypeVisitor<T>
where
    T: Default,
{
    type Value = T;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}={:?}", self.type_name, self.expected)
    }

    fn visit_str<E>(self, actual: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        validate_fixed_str_type(actual, self.expected, self.type_name)?;
        Ok(T::default())
    }
}

fn deserialize_fixed_str_type<'de, D, T>(
    deserializer: D,
    expected: &'static str,
    type_name: &'static str,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default,
{
    deserializer.deserialize_str(FixedStrTypeVisitor::new(expected, type_name))
}

fn validate_fixed_str_type<E>(actual: &str, expected: &str, type_name: &str) -> Result<(), E>
where
    E: de::Error,
{
    if actual == expected {
        Ok(())
    } else {
        Err(E::custom(fixed_str_type_error(type_name, expected)))
    }
}

fn fixed_str_type_error(type_name: &str, expected: &str) -> String {
    format!("expected {type_name}={expected:?}")
}

fixed_str_type!(LaunchStdoutKind, "stdout");
fixed_str_type!(LaunchStderrKind, "stderr");
fixed_str_type!(LaunchMarkerKind, "marker");
fixed_str_type!(LaunchHeartbeatKind, "heartbeat");
fixed_str_type!(LaunchExitKind, "exit");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchStdoutEvent {
    pub contract: String,
    pub request_id: String,
    pub seq: u64,
    pub time_unix_ms: u64,
    pub kind: LaunchStdoutKind,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchStderrEvent {
    pub contract: String,
    pub request_id: String,
    pub seq: u64,
    pub time_unix_ms: u64,
    pub kind: LaunchStderrKind,
    pub data_base64: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchMarkerEvent {
    pub contract: String,
    pub request_id: String,
    pub seq: u64,
    pub time_unix_ms: u64,
    pub kind: LaunchMarkerKind,
    pub name: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchHeartbeatEvent {
    pub contract: String,
    pub request_id: String,
    pub seq: u64,
    pub time_unix_ms: u64,
    pub kind: LaunchHeartbeatKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchExitEvent {
    pub contract: String,
    pub request_id: String,
    pub seq: u64,
    pub time_unix_ms: u64,
    pub kind: LaunchExitKind,
    pub status: ProcessStatus,
    pub terminal_signal: TerminalSignal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum LaunchEvent {
    #[serde(rename = "stdout")]
    Stdout {
        contract: String,
        request_id: String,
        seq: u64,
        time_unix_ms: u64,
        data_base64: String,
    },
    #[serde(rename = "stderr")]
    Stderr {
        contract: String,
        request_id: String,
        seq: u64,
        time_unix_ms: u64,
        data_base64: String,
    },
    #[serde(rename = "marker")]
    Marker {
        contract: String,
        request_id: String,
        seq: u64,
        time_unix_ms: u64,
        name: String,
        value: Value,
    },
    #[serde(rename = "heartbeat")]
    Heartbeat {
        contract: String,
        request_id: String,
        seq: u64,
        time_unix_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    #[serde(rename = "exit")]
    Exit {
        contract: String,
        request_id: String,
        seq: u64,
        time_unix_ms: u64,
        status: ProcessStatus,
        terminal_signal: TerminalSignal,
        #[serde(skip_serializing_if = "Option::is_none")]
        session: Option<Value>,
    },
}

pub type DescribeRequest = RequestEnvelope<EmptyParams>;
pub type DescribeResponse = SuccessResponseEnvelope<DescribeResult>;
pub type DescribeErrorResponse = ErrorResponseEnvelope;
pub type SchemaRequest = RequestEnvelope<SchemaParams>;
pub type SchemaResponse = SuccessResponseEnvelope<SchemaResult>;
pub type SchemaErrorResponse = ErrorResponseEnvelope;
pub type SettingsListRequest = RequestEnvelope<EmptyParams>;
pub type SettingsListResponse = SuccessResponseEnvelope<SettingsListResult>;
pub type SettingsListErrorResponse = ErrorResponseEnvelope;
pub type SettingsGetRequest = RequestEnvelope<SettingsGetParams>;
pub type SettingsGetResponse = SuccessResponseEnvelope<SettingsGetResult>;
pub type SettingsGetErrorResponse = ErrorResponseEnvelope;
pub type SettingsCreateRequest = RequestEnvelope<SettingsCreateParams>;
pub type SettingsCreateResult = SettingsWriteResult;
pub type SettingsCreateResponse = SuccessResponseEnvelope<SettingsCreateResult>;
pub type SettingsCreateErrorResponse = ErrorResponseEnvelope;
pub type SettingsUpdateRequest = RequestEnvelope<SettingsUpdateParams>;
pub type SettingsUpdateResult = SettingsWriteResult;
pub type SettingsUpdateResponse = SuccessResponseEnvelope<SettingsUpdateResult>;
pub type SettingsUpdateErrorResponse = ErrorResponseEnvelope;
pub type SettingsDeleteRequest = RequestEnvelope<SettingsDeleteParams>;
pub type SettingsDeleteResponse = SuccessResponseEnvelope<SettingsDeleteResult>;
pub type SettingsDeleteErrorResponse = ErrorResponseEnvelope;
pub type SettingsValidateRequest = RequestEnvelope<SettingsValidateParams>;
pub type SettingsValidateResponse = SuccessResponseEnvelope<SettingsValidateResult>;
pub type SettingsValidateErrorResponse = ErrorResponseEnvelope;
pub type SettingsMigrateRequest = RequestEnvelope<SettingsMigrateParams>;
pub type SettingsMigrateResponse = SuccessResponseEnvelope<SettingsMigrateResult>;
pub type SettingsMigrateErrorResponse = ErrorResponseEnvelope;
pub type PolicyEvaluateRequest = RequestEnvelope<PolicyEvaluateParams>;
pub type PolicyEvaluateResponse = SuccessResponseEnvelope<PolicyEvaluateResult>;
pub type PolicyEvaluateErrorResponse = ErrorResponseEnvelope;
pub type LaunchRequest = RequestEnvelope<LaunchParams>;
pub type TerminalClassifyRequest = RequestEnvelope<TerminalClassifyParams>;
pub type TerminalClassifyResponse = SuccessResponseEnvelope<TerminalClassifyResult>;
pub type TerminalClassifyErrorResponse = ErrorResponseEnvelope;
pub type QuotaSourceRequest = RequestEnvelope<QuotaBaseParams>;
pub type QuotaSourceResponse = SuccessResponseEnvelope<QuotaSourceResult>;
pub type QuotaSourceErrorResponse = ErrorResponseEnvelope;
pub type QuotaProbeRequest = RequestEnvelope<QuotaBaseParams>;
pub type QuotaProbeResponse = SuccessResponseEnvelope<QuotaProbeResult>;
pub type QuotaProbeErrorResponse = ErrorResponseEnvelope;
pub type QuotaRefreshAuthRequest = RequestEnvelope<QuotaRefreshAuthParams>;
pub type QuotaRefreshAuthResponse = SuccessResponseEnvelope<QuotaRefreshAuthResult>;
pub type QuotaRefreshAuthErrorResponse = ErrorResponseEnvelope;
pub type SessionLocateTranscriptRequest = RequestEnvelope<SessionBaseParams>;
pub type SessionLocateTranscriptResponse = SuccessResponseEnvelope<SessionLocateTranscriptResult>;
pub type SessionLocateTranscriptErrorResponse = ErrorResponseEnvelope;
pub type SessionReadTurnsRequest = RequestEnvelope<SessionBaseParams>;
pub type SessionReadTurnsResponse = SuccessResponseEnvelope<SessionReadTurnsResult>;
pub type SessionReadTurnsErrorResponse = ErrorResponseEnvelope;
pub type SessionCaptureRequest = RequestEnvelope<SessionBaseParams>;
pub type SessionCaptureResponse = SuccessResponseEnvelope<SessionCaptureResult>;
pub type SessionCaptureErrorResponse = ErrorResponseEnvelope;
pub type SessionExportRequest = RequestEnvelope<SessionBaseParams>;
pub type SessionExportResponse = SuccessResponseEnvelope<SessionExportResult>;
pub type SessionExportErrorResponse = ErrorResponseEnvelope;
pub type SessionReplaceRequest = RequestEnvelope<SessionReplaceParams>;
pub type SessionReplaceResponse = SuccessResponseEnvelope<SessionReplaceResult>;
pub type SessionReplaceErrorResponse = ErrorResponseEnvelope;
pub type RotationAssessRequest = RequestEnvelope<RotationObject>;
pub type RotationAssessResponse = SuccessResponseEnvelope<RotationAssessResult>;
pub type RotationAssessErrorResponse = ErrorResponseEnvelope;
pub type RotationMaterializeRequest = RequestEnvelope<RotationObject>;
pub type RotationMaterializeResponse = SuccessResponseEnvelope<RotationMaterializeResult>;
pub type RotationMaterializeErrorResponse = ErrorResponseEnvelope;
pub type DiscoveryModelsRequest = RequestEnvelope<DiscoveryObject>;
pub type DiscoveryModelsResponse = SuccessResponseEnvelope<DiscoveryModelsResult>;
pub type DiscoveryModelsErrorResponse = ErrorResponseEnvelope;
pub type DiscoveryAccountsRequest = RequestEnvelope<DiscoveryObject>;
pub type DiscoveryAccountsResponse = SuccessResponseEnvelope<DiscoveryAccountsResult>;
pub type DiscoveryAccountsErrorResponse = ErrorResponseEnvelope;
pub type SetupDetectRequest = RequestEnvelope<SetupObject>;
pub type SetupDetectResponse = SuccessResponseEnvelope<SetupDetectResult>;
pub type SetupDetectErrorResponse = ErrorResponseEnvelope;
pub type SetupInstallPlanRequest = RequestEnvelope<SetupObject>;
pub type SetupInstallPlanResponse = SuccessResponseEnvelope<SetupInstallPlanResult>;
pub type SetupInstallPlanErrorResponse = ErrorResponseEnvelope;
pub type SetupSyncPlanRequest = RequestEnvelope<SetupObject>;
pub type SetupSyncPlanResponse = SuccessResponseEnvelope<SetupSyncPlanResult>;
pub type SetupSyncPlanErrorResponse = ErrorResponseEnvelope;
pub type SetupBrainTurnRequest = RequestEnvelope<SetupObject>;
pub type SetupBrainTurnResponse = SuccessResponseEnvelope<SetupBrainTurnResult>;
pub type SetupBrainTurnErrorResponse = ErrorResponseEnvelope;
pub type MigrationPlanRequest = RequestEnvelope<MigrationObject>;
pub type MigrationPlanResponse = SuccessResponseEnvelope<MigrationPlanResult>;
pub type MigrationPlanErrorResponse = ErrorResponseEnvelope;
pub type MigrationApplyRequest = RequestEnvelope<MigrationObject>;
pub type MigrationApplyResponse = SuccessResponseEnvelope<MigrationApplyResult>;
pub type MigrationApplyErrorResponse = ErrorResponseEnvelope;
