//! ## Declared roles
//!
//! Roles: orchestration, validator, parser, mapper, accessor, predicate.
//!
//! - orchestration: `ProviderClient::{invoke_typed, invoke_json, launch}`
//!   sequence schema checks, artifact resolution, subprocess execution,
//!   launch-stream processing, diagnostics capture, and result delivery.
//! - validator: `validate_json_request`, `validate_launch_request`,
//!   `validate_json_success_envelope`, `validate_json_error_envelope`, and
//!   the stdout-presence/limit/object-shape guards enforce host/provider
//!   protocol invariants before mapping outcomes.
//! - parser: `parse_one_stdout_object`, `parse_stdout_utf8`, and
//!   `parse_stdout_json_value` parse one non-launch provider stdout envelope.
//! - mapper: `map_json_invocation_outcome`, `map_valid_json_success_outcome`,
//!   `map_valid_json_provider_error_outcome`, the `map_*_error` helpers,
//!   `process_limits_for`, `process_command_from_resolved`,
//!   `launch_diagnostics`, and `parse_launch_output` translate process,
//!   schema, and stream outcomes into provider-client results/errors.
//! - accessor: `ProviderClient::options`, `last_diagnostics`,
//!   `last_invocation_argv`, `ProviderEnv::into_env_vec`, and
//!   `response_envelope_ok` expose option, environment, and envelope fields.
//! - predicate: `success_envelope_has_nonzero_process`, `provider_nonzero`,
//!   and `trailing_stdout_error_kind` classify outcome and stdout conditions.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-provider/src/client.rs
//!     role: adapter
//!     Translates:
//!       - provider-client-options-contract
//!       - provider-cli-subprocess-contract
//!       - oulipoly-provider-generated-dto-contract
//!       - launch-jsonl-stream-contract
//!       - byte-limit-capture-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-provider/src/client.rs
//!     role: intrinsic-surface
//!     Domain: provider client transport orchestration
//!     Owns:
//!       - provider timeout and output-limit defaults
//!       - typed JSON invocation and launch entrypoints
//!       - request validation and response-envelope protocol mapping
//!       - stdout envelope parsing and launch stdout stream handoff
//!       - process diagnostics and last-invocation argv capture
//! ```

use crate::error::{
    HostErrorKind, ProviderCapabilityError, ProviderClientError, ProviderDiagnostics,
    check_contract_and_request, request_id_from,
};
use crate::generated::ProcessStatus;
use crate::process::{
    ByteLimit, ProcessCommand, ProcessLimits, ProcessOutcome, ProcessRunner, StdoutDrainOutput,
};
use crate::resolver::{
    ProviderArtifactRef, ProviderResolveOptions, ProviderResolver, ResolvedProviderCommand,
};
use crate::schemas::{SchemaRegistry, SchemaValidationError};
use crate::stream::{LaunchEventObserver, LaunchResult, LaunchStdoutDrain, LaunchStdoutProcessor};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::ffi::OsString;
use std::sync::Mutex;
use std::time::Duration;

pub use crate::process::{CancellationToken, ProcessSpawnObserver};

/// Handshake (`describe` + `policy.evaluate`) timeout.
///
/// The handshake spawns the provider artifact, which may embed a scripting
/// runtime that must cold-start before it can answer `describe` /
/// `policy.evaluate`. Under machine load (many concurrent dispatches racing for
/// CPU) that cold start has been observed to take tens of seconds. The previous
/// 30s budget fired `host_timeout` mid-handshake, and because a transport
/// timeout is terminal the whole dispatch failed instead of completing.
///
/// 90s gives roughly 3x headroom over the observed worst case, so a genuinely
/// hung handshake still fails in bounded time (and the host can then rotate to
/// the next pool account rather than terminal-failing the dispatch).
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(90);

/// Maximum gap between launch JSONL events before the host tears down the
/// provider process tree. The launch turn itself has no total deadline; real
/// agent turns can run for tens of minutes as long as the provider keeps
/// emitting events or heartbeat events within this 120s liveness window.
const DEFAULT_LAUNCH_HEARTBEAT_GAP: Duration = Duration::from_secs(120);

/// Grace period between SIGTERM and SIGKILL when tearing down a timed-out or
/// cancelled provider process tree.
const DEFAULT_KILL_AFTER_GRACE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTimeouts {
    /// Total timeout for non-launch handshake subcommands.
    pub default: Duration,
    /// Maximum gap between launch stdout JSONL event lines.
    pub launch: Duration,
    /// Grace period between SIGTERM and SIGKILL during teardown.
    pub kill_after_grace: Duration,
}

impl Default for ProviderTimeouts {
    fn default() -> Self {
        Self {
            default: DEFAULT_HANDSHAKE_TIMEOUT,
            launch: DEFAULT_LAUNCH_HEARTBEAT_GAP,
            kill_after_grace: DEFAULT_KILL_AFTER_GRACE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderOutputLimits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

impl Default for ProviderOutputLimits {
    fn default() -> Self {
        Self {
            stdout_bytes: 1024 * 1024,
            stderr_bytes: 128 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderClientOptions {
    pub timeouts: ProviderTimeouts,
    pub output_limits: ProviderOutputLimits,
    pub timeout: Duration,
    pub cancellation: Option<CancellationToken>,
    pub resolver: ProviderResolveOptions,
    pub spawn_observer: Option<ProcessSpawnObserver>,
    pub launch_event_observer: Option<LaunchEventObserver>,
}

pub trait ProviderEnv {
    fn into_env_vec(self) -> Vec<(OsString, OsString)>;
}

impl ProviderEnv for [(); 0] {
    fn into_env_vec(self) -> Vec<(OsString, OsString)> {
        Vec::new()
    }
}

impl ProviderEnv for Vec<(String, String)> {
    fn into_env_vec(self) -> Vec<(OsString, OsString)> {
        self.into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }
}

impl ProviderEnv for Vec<(String, OsString)> {
    fn into_env_vec(self) -> Vec<(OsString, OsString)> {
        self.into_iter()
            .map(|(key, value)| (OsString::from(key), value))
            .collect()
    }
}

impl Default for ProviderClientOptions {
    fn default() -> Self {
        let timeouts = ProviderTimeouts::default();
        Self {
            timeout: timeouts.default,
            timeouts,
            output_limits: ProviderOutputLimits::default(),
            cancellation: None,
            resolver: ProviderResolveOptions::default(),
            spawn_observer: None,
            launch_event_observer: None,
        }
    }
}

impl ProviderClientOptions {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self.timeouts.default = timeout;
        self.timeouts.launch = timeout;
        self
    }

    pub fn with_launch_heartbeat_gap(mut self, gap: Duration) -> Self {
        self.timeouts.launch = gap;
        self
    }

    pub fn with_kill_after_grace(mut self, duration: Duration) -> Self {
        self.timeouts.kill_after_grace = duration;
        self
    }

    pub fn with_cancellation(mut self, cancellation: Option<CancellationToken>) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn with_spawn_observer(mut self, observer: Option<ProcessSpawnObserver>) -> Self {
        self.spawn_observer = observer;
        self
    }

    pub fn with_launch_event_observer(mut self, observer: Option<LaunchEventObserver>) -> Self {
        self.launch_event_observer = observer;
        self
    }
}

#[derive(Debug)]
pub struct ProviderClient {
    artifact: ProviderArtifactRef,
    options: ProviderClientOptions,
    last_diagnostics: Mutex<ProviderDiagnostics>,
    last_argv: Mutex<Vec<OsString>>,
}

impl ProviderClient {
    pub fn new(artifact: ProviderArtifactRef, options: ProviderClientOptions) -> Self {
        Self {
            artifact,
            options,
            last_diagnostics: Mutex::new(ProviderDiagnostics::default()),
            last_argv: Mutex::new(Vec::new()),
        }
    }

    pub fn options(&self) -> &ProviderClientOptions {
        &self.options
    }

    pub fn last_diagnostics(&self) -> ProviderDiagnostics {
        self.last_diagnostics
            .lock()
            .expect("diagnostics mutex should not be poisoned")
            .clone()
    }

    pub fn last_invocation_argv(&self) -> Vec<OsString> {
        self.last_argv
            .lock()
            .expect("argv mutex should not be poisoned")
            .clone()
    }

    pub fn invoke_typed<T, I>(
        &self,
        subcommand: &str,
        request: Value,
        envs: I,
    ) -> Result<T, ProviderClientError>
    where
        T: DeserializeOwned,
        I: ProviderEnv,
    {
        let envelope = self.invoke_json(subcommand, request, envs)?;
        deserialize_typed_result(subcommand, envelope)
    }

    pub fn invoke_json<I>(
        &self,
        subcommand: &str,
        request: Value,
        envs: I,
    ) -> Result<Value, ProviderClientError>
    where
        I: ProviderEnv,
    {
        let request_id = request_id_from(&request);
        let registry = SchemaRegistry::new();
        validate_json_request(&registry, subcommand, &request, request_id.clone())?;
        let resolved = self.resolve(subcommand, request_id.clone())?;
        let outcome =
            self.run_resolved(subcommand, &resolved, request, envs, self.options.timeout)?;
        let diagnostics = outcome.diagnostics();
        self.record_process_state(&outcome, &diagnostics);
        ensure_invocation_stdout_within_limit(subcommand, &diagnostics, request_id.clone())?;
        ensure_invocation_stdout_present(
            subcommand,
            &outcome,
            diagnostics.clone(),
            request_id.clone(),
        )?;
        let envelope = parse_invocation_stdout_object(
            subcommand,
            &outcome.stdout.bytes,
            &diagnostics,
            request_id.clone(),
        )?;
        map_json_invocation_outcome(
            &registry,
            subcommand,
            envelope,
            outcome,
            diagnostics,
            request_id,
        )
    }

    pub fn launch<I>(&self, request: Value, envs: I) -> Result<LaunchResult, ProviderClientError>
    where
        I: ProviderEnv,
    {
        let request_id = request_id_from(&request);
        let registry = SchemaRegistry::new();
        validate_launch_request(&registry, &request, request_id.clone())?;
        let resolved = self.resolve("launch", request_id.clone())?;
        let outcome = self.run_launch_resolved(&resolved, request, envs)?;
        let diagnostics = launch_diagnostics(&outcome);
        self.record_process_state(&outcome, &diagnostics);
        parse_launch_output(outcome.stdout, diagnostics, outcome.status)
    }

    fn record_process_state<T: StdoutDrainOutput>(
        &self,
        outcome: &ProcessOutcome<T>,
        diagnostics: &ProviderDiagnostics,
    ) {
        *self
            .last_argv
            .lock()
            .expect("argv mutex should not be poisoned") = outcome.argv().to_vec();
        *self
            .last_diagnostics
            .lock()
            .expect("diagnostics mutex should not be poisoned") = diagnostics.clone();
    }

    fn resolve(
        &self,
        subcommand: &str,
        request_id: Option<String>,
    ) -> Result<ResolvedProviderCommand, ProviderClientError> {
        ProviderResolver::new(self.options.resolver.clone())
            .resolve(&self.artifact, None)
            .map_err(|error| {
                ProviderClientError::host_transport(
                    HostErrorKind::Other(error.kind().to_owned()),
                    subcommand,
                    request_id,
                    ProviderDiagnostics::default(),
                )
            })
    }

    fn run_resolved<I>(
        &self,
        subcommand: &str,
        resolved: &ResolvedProviderCommand,
        request: Value,
        envs: I,
        timeout: Duration,
    ) -> Result<crate::process::ProcessOutcome, ProviderClientError>
    where
        I: ProviderEnv,
    {
        let request_id = request_id_from(&request);
        let request_bytes = serialize_request_bytes(subcommand, &request, request_id.clone())?;
        let limits = process_limits_for(subcommand, timeout, &self.options);
        let command = process_command_from_resolved(resolved, subcommand);
        let runner = ProcessRunner::new(limits);
        let result = if subcommand == "launch" {
            runner.run_with_stdout_line_gap_timeout(command, request_bytes, envs.into_env_vec())
        } else {
            runner.run(command, request_bytes, envs.into_env_vec())
        };
        result.map_err(|error| error.with_request_id_if_missing(request_id))
    }

    fn run_launch_resolved<I>(
        &self,
        resolved: &ResolvedProviderCommand,
        request: Value,
        envs: I,
    ) -> Result<ProcessOutcome<LaunchStdoutDrain>, ProviderClientError>
    where
        I: ProviderEnv,
    {
        let request_id = request_id_from(&request);
        let request_bytes = serialize_request_bytes("launch", &request, request_id.clone())?;
        let limits = process_limits_for("launch", self.options.timeouts.launch, &self.options);
        let command = process_command_from_resolved(resolved, "launch");
        let stdout_processor =
            LaunchStdoutProcessor::new(request_id.clone().unwrap_or_default(), limits.stdout_limit)
                .with_event_observer(self.options.launch_event_observer.clone());
        let runner = ProcessRunner::new(limits);
        runner
            .run_with_stdout_line_gap_timeout_and_stdout_processor(
                command,
                request_bytes,
                envs.into_env_vec(),
                stdout_processor,
            )
            .map_err(|error| error.with_request_id_if_missing(request_id))
    }
}

fn deserialize_typed_result<T>(subcommand: &str, envelope: Value) -> Result<T, ProviderClientError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(envelope["result"].clone())
        .map_err(|error| map_typed_result_schema_error(error, subcommand, &envelope))
}

fn map_typed_result_schema_error(
    error: serde_json::Error,
    subcommand: &str,
    envelope: &Value,
) -> ProviderClientError {
    ProviderClientError::protocol(
        HostErrorKind::SchemaInvalidResponse,
        subcommand,
        request_id_from(envelope),
        ProviderDiagnostics::with_description(error.to_string()),
    )
}

fn validate_json_request(
    registry: &SchemaRegistry,
    subcommand: &str,
    request: &Value,
    request_id: Option<String>,
) -> Result<(), ProviderClientError> {
    registry
        .validate_request(subcommand, request)
        .map_err(|error| map_request_validation_error(error, subcommand, request_id))
}

fn validate_launch_request(
    registry: &SchemaRegistry,
    request: &Value,
    request_id: Option<String>,
) -> Result<(), ProviderClientError> {
    validate_json_request(registry, "launch", request, request_id)
}

fn ensure_invocation_stdout_within_limit(
    subcommand: &str,
    diagnostics: &ProviderDiagnostics,
    request_id: Option<String>,
) -> Result<(), ProviderClientError> {
    if diagnostics.stdout.truncated {
        return Err(ProviderClientError::protocol(
            HostErrorKind::StdoutLimitExceeded,
            subcommand,
            request_id,
            diagnostics.clone(),
        ));
    }
    Ok(())
}

fn ensure_invocation_stdout_present(
    subcommand: &str,
    outcome: &ProcessOutcome,
    diagnostics: ProviderDiagnostics,
    request_id: Option<String>,
) -> Result<(), ProviderClientError> {
    if outcome.stdout.bytes.is_empty() {
        return Err(map_empty_invocation_stdout_error(
            subcommand,
            outcome,
            diagnostics,
            request_id,
        ));
    }
    Ok(())
}

fn map_empty_invocation_stdout_error(
    subcommand: &str,
    outcome: &ProcessOutcome,
    diagnostics: ProviderDiagnostics,
    request_id: Option<String>,
) -> ProviderClientError {
    ProviderClientError::host_transport(
        empty_invocation_stdout_kind(outcome),
        subcommand,
        request_id,
        diagnostics,
    )
}

fn empty_invocation_stdout_kind(outcome: &ProcessOutcome) -> HostErrorKind {
    if provider_nonzero(&outcome.status) {
        HostErrorKind::ProviderProcessNonzero
    } else if outcome.stdin_closed_early && outcome.stderr.bytes.is_empty() {
        HostErrorKind::ProviderClosedStdinEarly
    } else {
        HostErrorKind::EmptyStdout
    }
}

fn parse_invocation_stdout_object(
    subcommand: &str,
    bytes: &[u8],
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: Option<String>,
) -> Result<Value, ProviderClientError> {
    parse_one_stdout_object(subcommand, bytes, diagnostics, fallback_request_id)
}

fn map_json_invocation_outcome(
    registry: &SchemaRegistry,
    subcommand: &str,
    envelope: Value,
    outcome: ProcessOutcome,
    diagnostics: ProviderDiagnostics,
    request_id: Option<String>,
) -> Result<Value, ProviderClientError> {
    match response_envelope_ok(&envelope) {
        Some(true) => map_json_success_outcome(
            registry,
            subcommand,
            envelope,
            &outcome,
            diagnostics,
            &request_id,
        ),
        Some(false) => map_json_provider_error_outcome(
            registry,
            subcommand,
            envelope,
            outcome,
            diagnostics,
            &request_id,
        ),
        None if outcome.stdin_closed_early => Err(ProviderClientError::host_transport(
            HostErrorKind::ProviderClosedStdinEarly,
            subcommand,
            request_id,
            diagnostics,
        )),
        None => Err(ProviderClientError::protocol(
            HostErrorKind::SchemaInvalidResponse,
            subcommand,
            request_id,
            diagnostics,
        )),
    }
}

fn map_json_success_outcome(
    registry: &SchemaRegistry,
    subcommand: &str,
    envelope: Value,
    outcome: &ProcessOutcome,
    diagnostics: ProviderDiagnostics,
    request_id: &Option<String>,
) -> Result<Value, ProviderClientError> {
    validate_json_success_envelope(registry, subcommand, &envelope, &diagnostics, request_id)?;
    map_valid_json_success_outcome(subcommand, envelope, outcome, diagnostics)
}

fn map_valid_json_success_outcome(
    subcommand: &str,
    envelope: Value,
    outcome: &ProcessOutcome,
    diagnostics: ProviderDiagnostics,
) -> Result<Value, ProviderClientError> {
    if success_envelope_has_nonzero_process(outcome) {
        return Err(ProviderClientError::host_transport(
            HostErrorKind::ProviderProcessNonzeroWithSuccess,
            subcommand,
            request_id_from(&envelope),
            diagnostics,
        ));
    }
    Ok(envelope)
}

fn map_json_provider_error_outcome(
    registry: &SchemaRegistry,
    subcommand: &str,
    envelope: Value,
    outcome: ProcessOutcome,
    diagnostics: ProviderDiagnostics,
    request_id: &Option<String>,
) -> Result<Value, ProviderClientError> {
    validate_json_error_envelope(registry, subcommand, &envelope, &diagnostics, request_id)?;
    let error = map_valid_json_provider_error_outcome(subcommand, envelope, outcome, diagnostics)?;
    Err(error)
}

fn map_valid_json_provider_error_outcome(
    subcommand: &str,
    envelope: Value,
    outcome: ProcessOutcome,
    diagnostics: ProviderDiagnostics,
) -> Result<ProviderClientError, ProviderClientError> {
    Ok(ProviderClientError::from_capability(
        valid_provider_capability_error(subcommand, envelope, outcome, diagnostics)?,
    ))
}

fn valid_provider_capability_error(
    subcommand: &str,
    envelope: Value,
    outcome: ProcessOutcome,
    diagnostics: ProviderDiagnostics,
) -> Result<ProviderCapabilityError, ProviderClientError> {
    ProviderCapabilityError::from_valid_envelope(
        subcommand,
        envelope,
        diagnostics,
        Some(outcome.status),
    )
}

fn validate_json_success_envelope(
    registry: &SchemaRegistry,
    subcommand: &str,
    envelope: &Value,
    diagnostics: &ProviderDiagnostics,
    request_id: &Option<String>,
) -> Result<(), ProviderClientError> {
    check_contract_and_request(
        envelope,
        request_id.as_deref().unwrap_or_default(),
        subcommand,
        diagnostics.clone(),
    )?;
    registry
        .validate_response(subcommand, envelope)
        .map_err(|_| map_response_validation_error(subcommand, envelope, diagnostics))
}

fn validate_json_error_envelope(
    registry: &SchemaRegistry,
    subcommand: &str,
    envelope: &Value,
    diagnostics: &ProviderDiagnostics,
    request_id: &Option<String>,
) -> Result<(), ProviderClientError> {
    check_contract_and_request(
        envelope,
        request_id.as_deref().unwrap_or_default(),
        subcommand,
        diagnostics.clone(),
    )?;
    registry
        .validate_error_response(subcommand, envelope)
        .map_err(|_| map_error_response_validation_error(subcommand, envelope, diagnostics))
}

fn map_response_validation_error(
    subcommand: &str,
    envelope: &Value,
    diagnostics: &ProviderDiagnostics,
) -> ProviderClientError {
    ProviderClientError::protocol(
        HostErrorKind::SchemaInvalidResponse,
        subcommand,
        request_id_from(envelope),
        diagnostics.clone(),
    )
}

fn map_error_response_validation_error(
    subcommand: &str,
    envelope: &Value,
    diagnostics: &ProviderDiagnostics,
) -> ProviderClientError {
    ProviderClientError::protocol(
        HostErrorKind::SchemaInvalidErrorResponse,
        subcommand,
        request_id_from(envelope),
        diagnostics.clone(),
    )
}

fn response_envelope_ok(envelope: &Value) -> Option<bool> {
    envelope.get("ok").and_then(Value::as_bool)
}

fn success_envelope_has_nonzero_process(outcome: &ProcessOutcome) -> bool {
    !outcome.status.exited_successfully() && !outcome.stdin_closed_early
}

fn launch_diagnostics<T: StdoutDrainOutput>(outcome: &ProcessOutcome<T>) -> ProviderDiagnostics {
    let mut diagnostics = outcome.diagnostics();
    diagnostics.provider_exit_code = provider_exit_code(&outcome.status);
    diagnostics.provider_process_nonzero = provider_nonzero(&outcome.status);
    diagnostics
}

fn parse_launch_output(
    stdout: LaunchStdoutDrain,
    diagnostics: ProviderDiagnostics,
    status: ProcessStatus,
) -> Result<LaunchResult, ProviderClientError> {
    match stdout.result {
        Ok(mut result) => {
            result.diagnostics = diagnostics.clone();
            Ok(result)
        }
        Err(error) => Err(error.with_process_context(diagnostics, status)),
    }
}

fn serialize_request_bytes(
    subcommand: &str,
    request: &Value,
    request_id: Option<String>,
) -> Result<Vec<u8>, ProviderClientError> {
    serde_json::to_vec(request)
        .map_err(|error| map_request_serialization_error(error, subcommand, request_id))
}

fn map_request_serialization_error(
    error: serde_json::Error,
    subcommand: &str,
    request_id: Option<String>,
) -> ProviderClientError {
    ProviderClientError::protocol(
        HostErrorKind::SchemaInvalidRequest,
        subcommand,
        request_id,
        ProviderDiagnostics::with_description(error.to_string()),
    )
}

fn process_limits_for(
    subcommand: &str,
    timeout: Duration,
    options: &ProviderClientOptions,
) -> ProcessLimits {
    ProcessLimits {
        timeout,
        kill_after_grace: kill_after_grace_for(subcommand, options),
        stdout_limit: ByteLimit::new(options.output_limits.stdout_bytes),
        stderr_limit: ByteLimit::new(options.output_limits.stderr_bytes),
        cancellation: options.cancellation.clone(),
        spawn_observer: launch_spawn_observer(subcommand, options),
    }
}

fn kill_after_grace_for(subcommand: &str, options: &ProviderClientOptions) -> Duration {
    if subcommand == "launch" && options.cancellation.is_some() {
        options
            .timeouts
            .kill_after_grace
            .max(Duration::from_millis(250))
    } else {
        options.timeouts.kill_after_grace
    }
}

fn process_command_from_resolved(
    resolved: &ResolvedProviderCommand,
    subcommand: &str,
) -> ProcessCommand {
    let mut argv = resolved.argv_for_subcommand(subcommand);
    let program = argv.remove(0);
    argv.into_iter()
        .fold(ProcessCommand::new(program), |command, arg| {
            command.arg(arg)
        })
}

fn launch_spawn_observer(
    subcommand: &str,
    options: &ProviderClientOptions,
) -> Option<ProcessSpawnObserver> {
    (subcommand == "launch")
        .then(|| options.spawn_observer.clone())
        .flatten()
}

fn map_request_validation_error(
    error: SchemaValidationError,
    subcommand: &str,
    request_id: Option<String>,
) -> ProviderClientError {
    let kind = match error {
        SchemaValidationError::UnknownSubcommand(_) => HostErrorKind::UnknownSubcommand,
        _ => HostErrorKind::SchemaInvalidRequest,
    };
    ProviderClientError::protocol(kind, subcommand, request_id, ProviderDiagnostics::default())
}

fn parse_one_stdout_object(
    subcommand: &str,
    bytes: &[u8],
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: Option<String>,
) -> Result<Value, ProviderClientError> {
    validate_stdout_not_empty(subcommand, bytes, diagnostics, &fallback_request_id)?;
    let text = map_stdout_utf8_result(parse_stdout_utf8(bytes), subcommand, &fallback_request_id)?;
    let trimmed = validate_stdout_prefix(subcommand, text, diagnostics, &fallback_request_id)?;
    let parsed = parse_stdout_json_value(subcommand, trimmed, diagnostics, &fallback_request_id)?;
    validate_stdout_object_shape(
        subcommand,
        trimmed,
        parsed,
        diagnostics,
        &fallback_request_id,
    )
}

struct ParsedStdoutJson {
    value: Value,
    byte_offset: usize,
}

type StdoutJsonStream<'a> = serde_json::StreamDeserializer<'a, serde_json::de::StrRead<'a>, Value>;

fn validate_stdout_not_empty(
    subcommand: &str,
    bytes: &[u8],
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: &Option<String>,
) -> Result<(), ProviderClientError> {
    if bytes.is_empty() {
        return Err(stdout_protocol_error(
            HostErrorKind::EmptyStdout,
            subcommand,
            fallback_request_id.clone(),
            diagnostics,
        ));
    }
    Ok(())
}

fn parse_stdout_utf8(bytes: &[u8]) -> Result<&str, std::str::Utf8Error> {
    std::str::from_utf8(bytes)
}

fn map_stdout_utf8_result<'a>(
    result: Result<&'a str, std::str::Utf8Error>,
    subcommand: &str,
    fallback_request_id: &Option<String>,
) -> Result<&'a str, ProviderClientError> {
    result.map_err(|error| {
        stdout_protocol_error_with_description(
            HostErrorKind::InvalidUtf8,
            subcommand,
            fallback_request_id.clone(),
            error.to_string(),
        )
    })
}

fn validate_stdout_prefix<'a>(
    subcommand: &str,
    text: &'a str,
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: &Option<String>,
) -> Result<&'a str, ProviderClientError> {
    let trimmed = text.trim_start();
    if !trimmed.starts_with('{') && trimmed.contains('{') {
        return Err(stdout_protocol_error(
            HostErrorKind::LeadingStdoutText,
            subcommand,
            fallback_request_id.clone(),
            diagnostics,
        ));
    }
    Ok(trimmed)
}

fn parse_stdout_json_value(
    subcommand: &str,
    trimmed: &str,
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: &Option<String>,
) -> Result<ParsedStdoutJson, ProviderClientError> {
    let mut stream = parse_stdout_json_stream(trimmed);
    let first = validate_stdout_json_present(
        next_stdout_json_value(&mut stream),
        subcommand,
        diagnostics,
        fallback_request_id,
    )?;
    let value = map_stdout_json_parse_result(first, subcommand, diagnostics, fallback_request_id)?;
    Ok(parsed_stdout_json(value, stream.byte_offset()))
}

fn parse_stdout_json_stream(trimmed: &str) -> StdoutJsonStream<'_> {
    serde_json::Deserializer::from_str(trimmed).into_iter::<Value>()
}

fn next_stdout_json_value(
    stream: &mut StdoutJsonStream<'_>,
) -> Option<Result<Value, serde_json::Error>> {
    stream.next()
}

fn validate_stdout_json_present(
    first: Option<Result<Value, serde_json::Error>>,
    subcommand: &str,
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: &Option<String>,
) -> Result<Result<Value, serde_json::Error>, ProviderClientError> {
    first.ok_or_else(|| {
        stdout_protocol_error(
            HostErrorKind::EmptyStdout,
            subcommand,
            fallback_request_id.clone(),
            diagnostics,
        )
    })
}

fn map_stdout_json_parse_result(
    first: Result<Value, serde_json::Error>,
    subcommand: &str,
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: &Option<String>,
) -> Result<Value, ProviderClientError> {
    first.map_err(|_| {
        stdout_protocol_error(
            HostErrorKind::InvalidJson,
            subcommand,
            fallback_request_id.clone(),
            diagnostics,
        )
    })
}

fn parsed_stdout_json(value: Value, byte_offset: usize) -> ParsedStdoutJson {
    ParsedStdoutJson { value, byte_offset }
}

fn validate_stdout_object_shape(
    subcommand: &str,
    trimmed: &str,
    parsed: ParsedStdoutJson,
    diagnostics: &ProviderDiagnostics,
    fallback_request_id: &Option<String>,
) -> Result<Value, ProviderClientError> {
    let value = parsed.value;
    if !value.is_object() {
        return Err(stdout_protocol_error(
            HostErrorKind::NonObjectJson,
            subcommand,
            request_id_from(&value).or_else(|| fallback_request_id.clone()),
            diagnostics,
        ));
    }
    let rest = trimmed[parsed.byte_offset..].trim();
    if !rest.is_empty() {
        return Err(stdout_protocol_error(
            trailing_stdout_error_kind(rest),
            subcommand,
            request_id_from(&value).or_else(|| fallback_request_id.clone()),
            diagnostics,
        ));
    }
    Ok(value)
}

fn trailing_stdout_error_kind(rest: &str) -> HostErrorKind {
    if rest.starts_with('{') {
        HostErrorKind::MultipleJsonObjects
    } else {
        HostErrorKind::TrailingNonWhitespace
    }
}

fn stdout_protocol_error(
    kind: HostErrorKind,
    subcommand: &str,
    request_id: Option<String>,
    diagnostics: &ProviderDiagnostics,
) -> ProviderClientError {
    ProviderClientError::protocol(kind, subcommand, request_id, diagnostics.clone())
}

fn stdout_protocol_error_with_description(
    kind: HostErrorKind,
    subcommand: &str,
    request_id: Option<String>,
    description: String,
) -> ProviderClientError {
    ProviderClientError::protocol(
        kind,
        subcommand,
        request_id,
        ProviderDiagnostics::with_description(description),
    )
}

fn provider_exit_code(status: &ProcessStatus) -> Option<i32> {
    match status {
        ProcessStatus::Exited { code } => Some(*code),
        _ => None,
    }
}

fn provider_nonzero(status: &ProcessStatus) -> bool {
    matches!(status, ProcessStatus::Exited { code } if *code != 0)
        || matches!(status, ProcessStatus::SignalTerminated { .. })
}
