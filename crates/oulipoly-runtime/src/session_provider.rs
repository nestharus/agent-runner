//! S7a external-provider transcript/session dispatch adapter.
//!
//! Providers return facts over the provider JSON contract. This module maps
//! those facts into runtime-owned transcript, turn, and capture shapes; callers
//! remain responsible for host-owned state transitions.
//!
//! ## Declared roles
//! orchestration, accessor, validator, mapper, formatter, parser, filter, predicate

use crate::provider_registry::{ProviderRegistry, ProviderRegistryError, ProviderRegistryHandle};
use crate::services::{
    ProductionSessionLifecycleService, SessionLifecycleIngestMode, SessionLifecycleOutput,
    SessionLifecycleRequest, SessionLifecycleServicePort,
};
use crate::session_metadata::{LocatedTranscript, SessionStorageType, TranscriptLookupMode};
use chrono::{DateTime, Utc};
use oulipoly_config::{ScriptSessionStorageType, SessionStorage, SessionsConfig};
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{
    Artifact, CONTRACT_VERSION, HostContext, JsonObject, RequestEnvelope, SessionBaseParams,
    SessionCaptureResult, SessionLocateTranscriptResult, SessionReadTurnsResult,
};
use oulipoly_state::{SessionTurnIngest, StateDb};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

pub const S7A_NEUTRAL_SETTINGS_ID: &str = "s7a-neutral-settings";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderIdentity {
    pub model_name: String,
    pub provider_name: String,
    pub provider_instance_id: Option<String>,
    pub settings_id: String,
}

#[derive(Debug, Clone)]
pub struct SessionProviderLocateRequest<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub session_id: &'a str,
    pub lookup_mode: TranscriptLookupMode,
    pub effective_cwd: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub struct SessionProviderReadTurnsRequest<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub session_id: &'a str,
    pub effective_cwd: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub struct SessionProviderCaptureRequest<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub invocation_uuid: &'a str,
    pub effective_cwd: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub struct SessionProviderLifecycleContext<'a> {
    pub registry: &'a ProviderRegistry,
    pub identity: SessionProviderIdentity,
    pub invocation_uuid: &'a str,
    pub invocation_row_id: i64,
    pub effective_cwd: Option<&'a Path>,
    pub pinned_target: Option<&'a str>,
    pub start_bound_provider_session_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionProviderReadTurnsResult {
    pub turns: Vec<SessionProviderTurn>,
    pub turn_count: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionProviderTurn {
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: DateTime<Utc>,
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: bool,
    pub is_compaction_boundary: bool,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionProviderCaptureResult {
    pub provider_session_id: Option<String>,
    pub state: Option<Value>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProviderError {
    token: String,
    message: String,
}

impl SessionProviderError {
    fn new(token: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for SessionProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.token, self.message)
    }
}

impl std::error::Error for SessionProviderError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoRefProofOutput {
    pub lifecycle_stderr: Vec<u8>,
    pub lifecycle: SessionLifecycleOutput,
}

pub struct NoRefProofRequest<'a> {
    pub state: &'a StateDb,
    pub registry: Option<ProviderRegistryHandle>,
    pub model_name: &'a str,
    pub provider_name: &'a str,
    pub session_id: &'a str,
    pub invocation_row_id: i64,
    pub invocation_uuid: &'a str,
}

pub fn locate_transcript(
    request: SessionProviderLocateRequest<'_>,
) -> Result<LocatedTranscript, SessionProviderError> {
    let client = session_client(request.registry, &request.identity)?;
    let provider_result = invoke_session::<SessionLocateTranscriptResult>(
        &client,
        "session.locate_transcript",
        base_request(
            &request.identity,
            Some(request.session_id),
            request.effective_cwd,
            locate_extra(request.lookup_mode),
            "locate",
        )?,
    )?;
    map_locate_result(provider_result, request.lookup_mode)
}

pub fn read_turns(
    request: SessionProviderReadTurnsRequest<'_>,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let client = provider_client(request.registry, &request.identity)?;
    read_turns_with_client(
        &client,
        &request.identity,
        Some(request.session_id),
        request.effective_cwd,
        JsonObject::new(),
    )
}

pub fn capture(
    request: SessionProviderCaptureRequest<'_>,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let client = provider_client(request.registry, &request.identity)?;
    capture_with_client(
        &client,
        &request.identity,
        request.effective_cwd,
        capture_extra(request.invocation_uuid),
    )
}

pub fn read_turns_for_lifecycle(
    context: &SessionProviderLifecycleContext<'_>,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let client = session_client(context.registry, &context.identity)?;
    read_turns_with_client(
        &client,
        &context.identity,
        read_session_id_for_lifecycle(context),
        context.effective_cwd,
        lifecycle_extra(context),
    )
}

pub fn capture_for_lifecycle(
    context: &SessionProviderLifecycleContext<'_>,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let client = session_client(context.registry, &context.identity)?;
    capture_with_client(
        &client,
        &context.identity,
        context.effective_cwd,
        lifecycle_extra(context),
    )
}

pub fn ingest_owned_turns(
    db: &StateDb,
    provider_name: &str,
    result: &SessionProviderReadTurnsResult,
) -> Result<u64, SessionProviderError> {
    let batch = provider_turns_to_ingest(&result.turns)?;
    let inserted = persist_owned_turn_batch(db, provider_name, &batch)?;
    mint_imported_chains(db, provider_name, &batch)?;
    Ok(inserted)
}

fn provider_turns_to_ingest(
    turns: &[SessionProviderTurn],
) -> Result<Vec<SessionTurnIngest>, SessionProviderError> {
    turns.iter().map(provider_turn_to_ingest).collect()
}

fn persist_owned_turn_batch(
    db: &StateDb,
    provider_name: &str,
    batch: &[SessionTurnIngest],
) -> Result<u64, SessionProviderError> {
    db.ingest_session_turns_batch(provider_name, batch)
        .map_err(provider_turn_ingest_failed)
}

fn provider_turn_ingest_failed(error: String) -> SessionProviderError {
    SessionProviderError::new("provider_turn_ingest_failed", error)
}

pub fn assert_turn_count_diagnostic(
    result: &SessionProviderReadTurnsResult,
) -> Result<(), SessionProviderError> {
    if result.turn_count == result.turns.len() as u64 {
        return Err(SessionProviderError::new(
            "provider_turn_count_matches",
            "turn_count matched accepted turn length",
        ));
    }
    Ok(())
}

pub fn dispatch_aware_no_ref_lifecycle_proof(
    request: NoRefProofRequest<'_>,
) -> Result<NoRefProofOutput, SessionProviderError> {
    let lifecycle = run_no_ref_lifecycle_proof(&request)?;
    Ok(no_ref_proof_output(lifecycle))
}

fn run_no_ref_lifecycle_proof(
    request: &NoRefProofRequest<'_>,
) -> Result<NoRefLifecycleProof, SessionProviderError> {
    let mut stderr = Vec::new();
    let sessions_cfg = SessionsConfig::default();
    let lifecycle = no_ref_lifecycle_service(request.registry.clone())
        .ingest_session(no_ref_lifecycle_request(
            request,
            &sessions_cfg,
            &mut stderr,
        ))
        .map_err(|error| {
            SessionProviderError::new("no_ref_lifecycle_proof_failed", error.to_string())
        })?;
    Ok(NoRefLifecycleProof { stderr, lifecycle })
}

struct NoRefLifecycleProof {
    stderr: Vec<u8>,
    lifecycle: SessionLifecycleOutput,
}

fn no_ref_lifecycle_request<'a>(
    request: &'a NoRefProofRequest<'a>,
    sessions_cfg: &'a SessionsConfig,
    stderr: &'a mut Vec<u8>,
) -> SessionLifecycleRequest<'a> {
    SessionLifecycleRequest {
        state: request.state,
        sessions_cfg,
        providers_cfg: None,
        provider_name: request.provider_name,
        external_provider: None,
        invocation_row_id: request.invocation_row_id,
        invocation_uuid: request.invocation_uuid,
        effective_cwd: None,
        mode: SessionLifecycleIngestMode::Pinned {
            resume_target: request.session_id.to_string(),
        },
        stderr,
    }
}

fn no_ref_proof_output(lifecycle: NoRefLifecycleProof) -> NoRefProofOutput {
    NoRefProofOutput {
        lifecycle_stderr: lifecycle.stderr,
        lifecycle: lifecycle.lifecycle,
    }
}

fn no_ref_lifecycle_service(
    registry: Option<ProviderRegistryHandle>,
) -> ProductionSessionLifecycleService {
    registry.map_or_else(
        ProductionSessionLifecycleService::new,
        ProductionSessionLifecycleService::with_registry_handle,
    )
}

fn session_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let describe = describe_session_provider(registry, &identity.model_name)?;
    require_session_capability(&describe)?;
    enabled_provider_client(registry, &identity.model_name)
}

fn describe_session_provider(
    registry: &ProviderRegistry,
    model_name: &str,
) -> Result<oulipoly_provider::generated::DescribeResult, SessionProviderError> {
    registry
        .describe_model_provider(model_name)
        .map_err(map_registry_error)
}

fn require_session_capability(
    describe: &oulipoly_provider::generated::DescribeResult,
) -> Result<(), SessionProviderError> {
    if describe.capabilities.session {
        Ok(())
    } else {
        Err(SessionProviderError::new(
            "session_capability_missing",
            "provider describe did not advertise session capability",
        ))
    }
}

fn enabled_provider_client(
    registry: &ProviderRegistry,
    model_name: &str,
) -> Result<ProviderClient, SessionProviderError> {
    let artifact = registry
        .enabled_artifact_for_model(model_name)
        .map_err(map_registry_error)?;
    Ok(registry.client_factory().client_for(artifact))
}

fn provider_client(
    registry: &ProviderRegistry,
    identity: &SessionProviderIdentity,
) -> Result<ProviderClient, SessionProviderError> {
    let artifact = registry
        .enabled_artifact_for_model(&identity.model_name)
        .map_err(map_registry_error)?;
    Ok(registry.client_factory().client_for(artifact))
}

fn read_turns_with_client(
    client: &ProviderClient,
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let result = invoke_session::<SessionReadTurnsResult>(
        client,
        "session.read_turns",
        base_request(identity, session_id, effective_cwd, extra, "read-turns")?,
    )?;
    map_read_turns_result(result)
}

fn capture_with_client(
    client: &ProviderClient,
    identity: &SessionProviderIdentity,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
) -> Result<SessionProviderCaptureResult, SessionProviderError> {
    let result = invoke_session::<SessionCaptureResult>(
        client,
        "session.capture",
        base_request(identity, None, effective_cwd, extra, "capture")?,
    )?;
    Ok(map_capture_result(result))
}

fn map_capture_result(result: SessionCaptureResult) -> SessionProviderCaptureResult {
    SessionProviderCaptureResult {
        provider_session_id: non_empty_optional(result.provider_session_id),
        state: result.state,
        artifacts: result.artifacts,
    }
}

fn invoke_session<T>(
    client: &ProviderClient,
    subcommand: &str,
    request: Value,
) -> Result<T, SessionProviderError>
where
    T: serde::de::DeserializeOwned,
{
    client
        .invoke_typed(subcommand, request, Vec::<(String, String)>::new())
        .map_err(map_client_error)
}

fn base_request(
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
    request_label: &str,
) -> Result<Value, SessionProviderError> {
    let request_id = session_request_id(request_label);
    let envelope = session_request_envelope(identity, session_id, effective_cwd, extra, request_id);
    serialize_session_request(envelope)
}

fn session_request_id(request_label: &str) -> String {
    format!("session-{request_label}-{}", uuid::Uuid::new_v4())
}

fn session_request_envelope(
    identity: &SessionProviderIdentity,
    session_id: Option<&str>,
    effective_cwd: Option<&Path>,
    extra: JsonObject,
    request_id: String,
) -> RequestEnvelope<SessionBaseParams> {
    let session_id = session_id_string(non_empty_session_id(session_id));
    RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id,
        provider_instance_id: Some(provider_instance_id(identity)),
        host: host_context(effective_cwd),
        params: session_base_params(identity, session_id, extra),
    }
}

fn serialize_session_request(
    envelope: RequestEnvelope<SessionBaseParams>,
) -> Result<Value, SessionProviderError> {
    serde_json::to_value(envelope).map_err(|err| {
        SessionProviderError::new("session_request_serialize_failed", err.to_string())
    })
}

fn session_base_params(
    identity: &SessionProviderIdentity,
    session_id: Option<String>,
    mut extra: JsonObject,
) -> SessionBaseParams {
    extra.insert(
        "model_name".to_string(),
        Value::String(identity.model_name.clone()),
    );
    extra.insert(
        "provider_name".to_string(),
        Value::String(identity.provider_name.clone()),
    );
    SessionBaseParams {
        settings_id: identity.settings_id.clone(),
        session_id,
        extra,
    }
}

fn non_empty_session_id(session_id: Option<&str>) -> Option<&str> {
    session_id.filter(|value| !value.is_empty())
}

fn session_id_string(session_id: Option<&str>) -> Option<String> {
    session_id.map(str::to_string)
}

fn host_context(effective_cwd: Option<&Path>) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: Some(std::env::consts::OS.to_string()),
        working_directory: effective_cwd.map(|path| path.display().to_string()),
        config_root: None,
        data_root: None,
        env: BTreeMap::new(),
        deadline_unix_ms: None,
    }
}

fn locate_extra(mode: TranscriptLookupMode) -> JsonObject {
    let mut extra = JsonObject::new();
    extra.insert(
        "lookup_mode".to_string(),
        Value::String(
            match mode {
                TranscriptLookupMode::RequireExisting => "require_existing",
                TranscriptLookupMode::AllowMissing => "allow_missing",
            }
            .to_string(),
        ),
    );
    extra
}

fn capture_extra(invocation_uuid: &str) -> JsonObject {
    let mut extra = JsonObject::new();
    extra.insert(
        "invocation_uuid".to_string(),
        Value::String(invocation_uuid.to_string()),
    );
    extra
}

fn lifecycle_extra(context: &SessionProviderLifecycleContext<'_>) -> JsonObject {
    let mut extra = capture_extra(context.invocation_uuid);
    extra.insert(
        "invocation_row_id".to_string(),
        Value::Number(context.invocation_row_id.into()),
    );
    insert_path(&mut extra, "effective_cwd", context.effective_cwd);
    insert_optional_str(&mut extra, "pinned_target", context.pinned_target);
    insert_optional_str(
        &mut extra,
        "start_bound_provider_session_id",
        context.start_bound_provider_session_id,
    );
    extra
}

fn insert_path(extra: &mut JsonObject, key: &str, value: Option<&Path>) {
    if let Some(path) = value {
        extra.insert(key.to_string(), Value::String(path.display().to_string()));
    }
}

fn insert_optional_str(extra: &mut JsonObject, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        extra.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn read_session_id_for_lifecycle<'a>(
    context: &'a SessionProviderLifecycleContext<'a>,
) -> Option<&'a str> {
    context
        .pinned_target
        .or(context.start_bound_provider_session_id)
}

fn provider_instance_id(identity: &SessionProviderIdentity) -> String {
    identity
        .provider_instance_id
        .clone()
        .unwrap_or_else(|| identity.provider_name.clone())
}

fn map_locate_result(
    result: SessionLocateTranscriptResult,
    mode: TranscriptLookupMode,
) -> Result<LocatedTranscript, SessionProviderError> {
    let facts = validate_locate_result(result, mode)?;
    Ok(located_transcript_from_facts(facts, mode))
}

struct ValidLocateFacts {
    path: PathBuf,
    format_id: Option<String>,
}

fn validate_locate_result(
    result: SessionLocateTranscriptResult,
    mode: TranscriptLookupMode,
) -> Result<ValidLocateFacts, SessionProviderError> {
    require_located(result.located)?;
    let path = validate_provider_path(require_located_path(result.path)?, mode)?;
    require_existing_observed(result.require_existing_observed, mode)?;
    Ok(ValidLocateFacts {
        path,
        format_id: result.format_id,
    })
}

fn require_located(located: bool) -> Result<(), SessionProviderError> {
    if located {
        Ok(())
    } else {
        Err(SessionProviderError::new(
            "session_locate_missing",
            "provider did not locate a transcript",
        ))
    }
}

fn located_transcript_from_facts(
    facts: ValidLocateFacts,
    mode: TranscriptLookupMode,
) -> LocatedTranscript {
    LocatedTranscript {
        path: facts.path,
        storage_classification: map_format_id(facts.format_id.as_deref()),
        require_existing_observed: matches!(mode, TranscriptLookupMode::RequireExisting),
    }
}

fn require_located_path(path: Option<String>) -> Result<PathBuf, SessionProviderError> {
    let Some(path) = path else {
        return Err(SessionProviderError::new(
            "session_locate_missing_path",
            "provider returned located=true without path",
        ));
    };
    if path.is_empty() {
        return Err(SessionProviderError::new(
            "session_locate_empty_path",
            "provider returned an empty transcript path",
        ));
    }
    Ok(PathBuf::from(path))
}

fn require_existing_observed(
    observed: Option<bool>,
    mode: TranscriptLookupMode,
) -> Result<(), SessionProviderError> {
    if matches!(mode, TranscriptLookupMode::RequireExisting) && observed != Some(true) {
        return Err(SessionProviderError::new(
            "session_locate_require_existing_unobserved",
            "provider did not report require_existing observation",
        ));
    }
    Ok(())
}

fn validate_provider_path(
    path: PathBuf,
    mode: TranscriptLookupMode,
) -> Result<PathBuf, SessionProviderError> {
    validate_absolute_provider_path(&path)?;
    if provider_path_exists(&path) {
        return canonicalize_provider_path(&path);
    }
    validate_missing_provider_path(&path, mode)?;
    Ok(path)
}

fn validate_absolute_provider_path(path: &Path) -> Result<(), SessionProviderError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(invalid_provider_relative_path(path))
    }
}

fn provider_path_exists(path: &Path) -> bool {
    path.exists()
}

fn canonicalize_provider_path(path: &Path) -> Result<PathBuf, SessionProviderError> {
    path.canonicalize()
        .map_err(|err| invalid_provider_canonicalize_path(path, err))
}

fn validate_missing_provider_path(
    path: &Path,
    mode: TranscriptLookupMode,
) -> Result<(), SessionProviderError> {
    if matches!(mode, TranscriptLookupMode::AllowMissing) {
        return Ok(());
    }
    Err(invalid_provider_missing_path(path))
}

fn invalid_provider_relative_path(path: &Path) -> SessionProviderError {
    SessionProviderError::new(
        "session_locate_invalid_path",
        format!("provider returned relative path {}", path.display()),
    )
}

fn invalid_provider_canonicalize_path(path: &Path, error: std::io::Error) -> SessionProviderError {
    SessionProviderError::new(
        "session_locate_invalid_path",
        format!("failed to canonicalize {}: {error}", path.display()),
    )
}

fn invalid_provider_missing_path(path: &Path) -> SessionProviderError {
    SessionProviderError::new(
        "session_locate_invalid_path",
        format!("provider returned missing path {}", path.display()),
    )
}

fn map_format_id(format_id: Option<&str>) -> SessionStorageType {
    map_parsed_format_id(parse_format_id(format_id))
}

fn parse_format_id(format_id: Option<&str>) -> Option<ScriptSessionStorageType> {
    let format_id = format_id?;
    serde_json::from_value::<ScriptSessionStorageType>(Value::String(format_id.to_string())).ok()
}

fn map_parsed_format_id(storage_type: Option<ScriptSessionStorageType>) -> SessionStorageType {
    match storage_type {
        Some(storage_type) => SessionStorageType::from(&Some(SessionStorage::Script {
            cwd_script: String::new(),
            transcript_script: None,
            storage_type: Some(storage_type),
        })),
        None => SessionStorageType::Other,
    }
}

fn map_read_turns_result(
    result: SessionReadTurnsResult,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let turns = map_provider_turns(result.turns)?;
    validate_unique_provider_turns(&turns)?;
    Ok(session_provider_read_turns_result(
        turns,
        result.turn_count,
        result.complete,
    ))
}

fn session_provider_read_turns_result(
    turns: Vec<SessionProviderTurn>,
    turn_count: u64,
    complete: bool,
) -> SessionProviderReadTurnsResult {
    SessionProviderReadTurnsResult {
        turns,
        turn_count,
        complete,
    }
}

fn map_provider_turns(
    values: Vec<Value>,
) -> Result<Vec<SessionProviderTurn>, SessionProviderError> {
    provider_turns_from_values(values)
}

fn provider_turns_from_values(
    values: Vec<Value>,
) -> Result<Vec<SessionProviderTurn>, SessionProviderError> {
    values.into_iter().map(provider_turn_from_value).collect()
}

fn validate_unique_provider_turns(
    turns: &[SessionProviderTurn],
) -> Result<(), SessionProviderError> {
    let mut seen = HashSet::new();
    for turn in turns {
        if !seen.insert((turn.session_id.clone(), turn.turn_id.clone())) {
            return Err(SessionProviderError::new(
                "provider_turn_duplicate",
                "provider returned duplicate turn id for a session",
            ));
        }
    }
    Ok(())
}

fn provider_turn_from_value(value: Value) -> Result<SessionProviderTurn, SessionProviderError> {
    let fields = validate_provider_turn_fields(parse_provider_turn_fields(value)?)?;
    Ok(session_provider_turn_from_fields(fields))
}

struct ParsedProviderTurnFields {
    session_id: String,
    turn_id: String,
    role: String,
    timestamp: String,
    parent_turn_id: Option<String>,
    is_sidechain: Option<bool>,
    is_compaction_boundary: Option<bool>,
    body: Option<Value>,
}

fn parse_provider_turn_fields(
    value: Value,
) -> Result<ParsedProviderTurnFields, SessionProviderError> {
    let object = provider_turn_object(&value)?;
    Ok(ParsedProviderTurnFields {
        session_id: required_string(object, "session_id", "provider_turn_missing_session_id")?,
        turn_id: required_string(object, "turn_id", "provider_turn_missing_turn_id")?,
        role: required_string(object, "role", "provider_turn_missing_role")?,
        timestamp: required_string(object, "timestamp", "provider_turn_missing_timestamp")?,
        parent_turn_id: optional_string(object, "parent_turn_id")?,
        is_sidechain: optional_bool(object, "is_sidechain")?,
        is_compaction_boundary: optional_bool(object, "is_compaction_boundary")?,
        body: optional_body(object)?,
    })
}

fn validate_provider_turn_fields(
    fields: ParsedProviderTurnFields,
) -> Result<ProviderTurnFields, SessionProviderError> {
    Ok(provider_turn_fields_from_validated_parts(
        fields.session_id,
        fields.turn_id,
        parse_turn_timestamp(fields.timestamp)?,
        fields.role,
        fields.parent_turn_id,
        fields.is_sidechain.unwrap_or(false),
        fields.is_compaction_boundary.unwrap_or(false),
        fields.body,
    ))
}

#[allow(clippy::too_many_arguments)]
fn provider_turn_fields_from_validated_parts(
    session_id: String,
    turn_id: String,
    timestamp: DateTime<Utc>,
    role: String,
    parent_turn_id: Option<String>,
    is_sidechain: bool,
    is_compaction_boundary: bool,
    body: Option<Value>,
) -> ProviderTurnFields {
    ProviderTurnFields {
        session_id,
        turn_id,
        timestamp,
        role,
        parent_turn_id,
        is_sidechain,
        is_compaction_boundary,
        body,
    }
}

fn provider_turn_object(value: &Value) -> Result<&Map<String, Value>, SessionProviderError> {
    value.as_object().ok_or_else(|| {
        SessionProviderError::new(
            "provider_turn_invalid_type",
            "provider turn was not an object",
        )
    })
}

struct ProviderTurnFields {
    session_id: String,
    turn_id: String,
    timestamp: DateTime<Utc>,
    role: String,
    parent_turn_id: Option<String>,
    is_sidechain: bool,
    is_compaction_boundary: bool,
    body: Option<Value>,
}

fn session_provider_turn_from_fields(fields: ProviderTurnFields) -> SessionProviderTurn {
    SessionProviderTurn {
        session_id: fields.session_id,
        turn_id: fields.turn_id,
        timestamp: fields.timestamp,
        role: fields.role,
        parent_turn_id: fields.parent_turn_id,
        is_sidechain: fields.is_sidechain,
        is_compaction_boundary: fields.is_compaction_boundary,
        body: fields.body,
    }
}

fn required_string(
    object: &Map<String, Value>,
    key: &str,
    missing_token: &str,
) -> Result<String, SessionProviderError> {
    let Some(value) = object.get(key) else {
        return Err(SessionProviderError::new(
            missing_token,
            format!("provider turn missing {key}"),
        ));
    };
    value.as_str().map(str::to_string).ok_or_else(|| {
        SessionProviderError::new(
            "provider_turn_invalid_type",
            format!("provider turn field {key} was not a string"),
        )
    })
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, SessionProviderError> {
    object
        .get(key)
        .map(|value| {
            value.as_str().map(str::to_string).ok_or_else(|| {
                SessionProviderError::new(
                    "provider_turn_invalid_type",
                    format!("provider turn field {key} was not a string"),
                )
            })
        })
        .transpose()
}

fn optional_bool(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, SessionProviderError> {
    object
        .get(key)
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                SessionProviderError::new(
                    "provider_turn_invalid_type",
                    format!("provider turn field {key} was not a boolean"),
                )
            })
        })
        .transpose()
}

fn optional_body(object: &Map<String, Value>) -> Result<Option<Value>, SessionProviderError> {
    optional_body_value(object)
        .map(validate_provider_turn_body)
        .transpose()
        .map(map_optional_body_value)
}

fn optional_body_value(object: &Map<String, Value>) -> Option<&Value> {
    object.get("body")
}

fn validate_provider_turn_body(body: &Value) -> Result<&Value, SessionProviderError> {
    if crate::sessions::is_canonical_body_shape(body) {
        Ok(body)
    } else {
        Err(provider_turn_noncanonical_body())
    }
}

fn provider_turn_noncanonical_body() -> SessionProviderError {
    SessionProviderError::new(
        "provider_turn_noncanonical_body",
        "provider turn body was not a canonical content chunk array",
    )
}

fn map_optional_body_value(body: Option<&Value>) -> Option<Value> {
    body.cloned()
}

fn parse_turn_timestamp(input: String) -> Result<DateTime<Utc>, SessionProviderError> {
    DateTime::parse_from_rfc3339(&input)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|err| {
            SessionProviderError::new(
                "provider_turn_invalid_timestamp",
                format!("invalid provider turn timestamp {input}: {err}"),
            )
        })
}

fn provider_turn_to_ingest(
    turn: &SessionProviderTurn,
) -> Result<SessionTurnIngest, SessionProviderError> {
    Ok(SessionTurnIngest {
        session_id: turn.session_id.clone(),
        turn_id: turn.turn_id.clone(),
        timestamp: turn.timestamp,
        role: turn.role.clone(),
        parent_turn_id: turn.parent_turn_id.clone(),
        is_sidechain: turn.is_sidechain,
        is_compaction_boundary: turn.is_compaction_boundary,
        body: serialize_optional_body(turn.body.as_ref())?,
    })
}

fn serialize_optional_body(body: Option<&Value>) -> Result<Option<String>, SessionProviderError> {
    body.map(serde_json::to_string).transpose().map_err(|err| {
        SessionProviderError::new("provider_turn_body_serialize_failed", err.to_string())
    })
}

fn mint_imported_chains(
    db: &StateDb,
    provider_name: &str,
    batch: &[SessionTurnIngest],
) -> Result<(), SessionProviderError> {
    for turn in batch {
        db.mint_imported_chain_if_absent(
            provider_name,
            &turn.session_id,
            &turn.timestamp,
            "<unknown>",
        )
        .map_err(provider_turn_chain_mint_failed)?;
    }
    Ok(())
}

fn provider_turn_chain_mint_failed(error: String) -> SessionProviderError {
    SessionProviderError::new("provider_turn_chain_mint_failed", error)
}

fn non_empty_optional(input: Option<String>) -> Option<String> {
    input.filter(|value| !value.is_empty())
}

fn map_registry_error(error: ProviderRegistryError) -> SessionProviderError {
    SessionProviderError::new("provider_registry_error", error.to_string())
}

fn map_client_error(error: ProviderClientError) -> SessionProviderError {
    match error.provider_error_code() {
        Some(code) => SessionProviderError::new(code.to_string(), error.to_string()),
        None => SessionProviderError::new(error.transport_kind().to_string(), error.to_string()),
    }
}
