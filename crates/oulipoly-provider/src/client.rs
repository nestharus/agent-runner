use crate::error::{
    HostErrorKind, ProviderCapabilityError, ProviderClientError, ProviderDiagnostics,
    check_contract_and_request, request_id_from,
};
use crate::generated::ProcessStatus;
use crate::process::{ByteLimit, ProcessCommand, ProcessLimits, ProcessRunner};
use crate::resolver::{
    ProviderArtifactRef, ProviderResolveOptions, ProviderResolver, ResolvedProviderCommand,
};
use crate::schemas::{SchemaRegistry, SchemaValidationError};
use crate::stream::{LaunchJsonlReader, LaunchResult};
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
/// 90s gives roughly 3x headroom over the observed worst case while staying far
/// below [`DEFAULT_LAUNCH_TIMEOUT`], so a genuinely hung handshake still fails
/// in bounded time (and the host can then rotate to the next pool account
/// rather than terminal-failing the dispatch).
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(90);

/// Launch timeout — the model turn itself can legitimately run for minutes, so
/// this stays generous and well above the handshake budget.
const DEFAULT_LAUNCH_TIMEOUT: Duration = Duration::from_secs(300);

/// Grace period between SIGTERM and SIGKILL when tearing down a timed-out or
/// cancelled provider process tree.
const DEFAULT_KILL_AFTER_GRACE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTimeouts {
    pub default: Duration,
    pub launch: Duration,
    pub kill_after_grace: Duration,
}

impl Default for ProviderTimeouts {
    fn default() -> Self {
        Self {
            default: DEFAULT_HANDSHAKE_TIMEOUT,
            launch: DEFAULT_LAUNCH_TIMEOUT,
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
        serde_json::from_value(envelope["result"].clone()).map_err(|error| {
            ProviderClientError::protocol(
                HostErrorKind::SchemaInvalidResponse,
                subcommand,
                request_id_from(&envelope),
                ProviderDiagnostics::with_description(error.to_string()),
            )
        })
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
        registry
            .validate_request(subcommand, &request)
            .map_err(|error| map_request_validation_error(error, subcommand, request_id.clone()))?;
        let resolved = self.resolve(subcommand, request_id.clone())?;
        let outcome =
            self.run_resolved(subcommand, &resolved, request, envs, self.options.timeout)?;
        *self
            .last_argv
            .lock()
            .expect("argv mutex should not be poisoned") = outcome.argv().to_vec();
        let diagnostics = outcome.diagnostics();
        *self
            .last_diagnostics
            .lock()
            .expect("diagnostics mutex should not be poisoned") = diagnostics.clone();
        if diagnostics.stdout.truncated {
            return Err(ProviderClientError::protocol(
                HostErrorKind::StdoutLimitExceeded,
                subcommand,
                request_id,
                diagnostics,
            ));
        }
        if outcome.stdout.bytes.is_empty() {
            let kind = if provider_nonzero(&outcome.status) {
                HostErrorKind::ProviderProcessNonzero
            } else if outcome.stdin_closed_early && outcome.stderr.bytes.is_empty() {
                HostErrorKind::ProviderClosedStdinEarly
            } else {
                HostErrorKind::EmptyStdout
            };
            return Err(ProviderClientError::host_transport(
                kind,
                subcommand,
                request_id,
                diagnostics,
            ));
        }
        let envelope = parse_one_stdout_object(
            subcommand,
            &outcome.stdout.bytes,
            &diagnostics,
            request_id.clone(),
        )?;
        let ok = envelope.get("ok").and_then(Value::as_bool);
        match ok {
            Some(true) => {
                check_contract_and_request(
                    &envelope,
                    request_id.as_deref().unwrap_or_default(),
                    subcommand,
                    diagnostics.clone(),
                )?;
                registry
                    .validate_response(subcommand, &envelope)
                    .map_err(|_| {
                        ProviderClientError::protocol(
                            HostErrorKind::SchemaInvalidResponse,
                            subcommand,
                            request_id_from(&envelope),
                            diagnostics.clone(),
                        )
                    })?;
                if !outcome.status.exited_successfully() && !outcome.stdin_closed_early {
                    return Err(ProviderClientError::host_transport(
                        HostErrorKind::ProviderProcessNonzeroWithSuccess,
                        subcommand,
                        request_id_from(&envelope),
                        diagnostics,
                    ));
                }
                Ok(envelope)
            }
            Some(false) => {
                check_contract_and_request(
                    &envelope,
                    request_id.as_deref().unwrap_or_default(),
                    subcommand,
                    diagnostics.clone(),
                )?;
                registry
                    .validate_error_response(subcommand, &envelope)
                    .map_err(|_| {
                        ProviderClientError::protocol(
                            HostErrorKind::SchemaInvalidErrorResponse,
                            subcommand,
                            request_id_from(&envelope),
                            diagnostics.clone(),
                        )
                    })?;
                Err(ProviderClientError::from_capability(
                    ProviderCapabilityError::from_valid_envelope(
                        subcommand,
                        envelope,
                        diagnostics,
                        Some(outcome.status),
                    )?,
                ))
            }
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

    pub fn launch<I>(&self, request: Value, envs: I) -> Result<LaunchResult, ProviderClientError>
    where
        I: ProviderEnv,
    {
        let request_id = request_id_from(&request);
        let registry = SchemaRegistry::new();
        registry
            .validate_request("launch", &request)
            .map_err(|error| map_request_validation_error(error, "launch", request_id.clone()))?;
        let resolved = self.resolve("launch", request_id.clone())?;
        let outcome = self.run_resolved(
            "launch",
            &resolved,
            request,
            envs,
            self.options.timeouts.launch,
        )?;
        *self
            .last_argv
            .lock()
            .expect("argv mutex should not be poisoned") = outcome.argv().to_vec();
        let mut diagnostics = outcome.diagnostics();
        diagnostics.provider_exit_code = provider_exit_code(&outcome.status);
        diagnostics.provider_process_nonzero = provider_nonzero(&outcome.status);
        *self
            .last_diagnostics
            .lock()
            .expect("diagnostics mutex should not be poisoned") = diagnostics.clone();
        if diagnostics.stdout.truncated {
            return Err(ProviderClientError::host_transport(
                HostErrorKind::StdoutLimitExceeded,
                "launch",
                request_id,
                diagnostics.clone(),
            )
            .with_process_context(diagnostics, outcome.status));
        }
        let reader = LaunchJsonlReader::new(request_id.clone().unwrap_or_default());
        let parsed = reader.read(&outcome.stdout.bytes[..]);
        match parsed {
            Ok(mut result) => {
                result.diagnostics = diagnostics.clone();
                Ok(result)
            }
            Err(error) => Err(error.with_process_context(diagnostics, outcome.status)),
        }
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
        let request_bytes = serde_json::to_vec(&request).map_err(|error| {
            ProviderClientError::protocol(
                HostErrorKind::SchemaInvalidRequest,
                subcommand,
                request_id.clone(),
                ProviderDiagnostics::with_description(error.to_string()),
            )
        })?;
        let kill_after_grace = if subcommand == "launch" && self.options.cancellation.is_some() {
            self.options
                .timeouts
                .kill_after_grace
                .max(Duration::from_millis(250))
        } else {
            self.options.timeouts.kill_after_grace
        };
        let limits = ProcessLimits {
            timeout,
            kill_after_grace,
            stdout_limit: ByteLimit::new(self.options.output_limits.stdout_bytes),
            stderr_limit: ByteLimit::new(self.options.output_limits.stderr_bytes),
            cancellation: self.options.cancellation.clone(),
            spawn_observer: launch_spawn_observer(subcommand, &self.options),
        };
        let mut argv = resolved.argv_for_subcommand(subcommand);
        let program = argv.remove(0);
        let mut command = ProcessCommand::new(program);
        for arg in argv {
            command = command.arg(arg);
        }
        ProcessRunner::new(limits)
            .run(command, request_bytes, envs.into_env_vec())
            .map_err(|error| error.with_request_id_if_missing(request_id))
    }
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
    if bytes.is_empty() {
        return Err(ProviderClientError::protocol(
            HostErrorKind::EmptyStdout,
            subcommand,
            fallback_request_id,
            diagnostics.clone(),
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|error| {
        ProviderClientError::protocol(
            HostErrorKind::InvalidUtf8,
            subcommand,
            fallback_request_id.clone(),
            ProviderDiagnostics::with_description(error.to_string()),
        )
    })?;
    let trimmed = text.trim_start();
    if !trimmed.starts_with('{') && trimmed.contains('{') {
        return Err(ProviderClientError::protocol(
            HostErrorKind::LeadingStdoutText,
            subcommand,
            fallback_request_id.clone(),
            diagnostics.clone(),
        ));
    }
    let mut stream = serde_json::Deserializer::from_str(trimmed).into_iter::<Value>();
    let Some(first) = stream.next() else {
        return Err(ProviderClientError::protocol(
            HostErrorKind::EmptyStdout,
            subcommand,
            fallback_request_id.clone(),
            diagnostics.clone(),
        ));
    };
    let value = first.map_err(|_| {
        ProviderClientError::protocol(
            HostErrorKind::InvalidJson,
            subcommand,
            fallback_request_id.clone(),
            diagnostics.clone(),
        )
    })?;
    if !value.is_object() {
        return Err(ProviderClientError::protocol(
            HostErrorKind::NonObjectJson,
            subcommand,
            request_id_from(&value).or_else(|| fallback_request_id.clone()),
            diagnostics.clone(),
        ));
    }
    let offset = stream.byte_offset();
    let rest = trimmed[offset..].trim();
    if !rest.is_empty() {
        let kind = if rest.starts_with('{') {
            HostErrorKind::MultipleJsonObjects
        } else {
            HostErrorKind::TrailingNonWhitespace
        };
        return Err(ProviderClientError::protocol(
            kind,
            subcommand,
            request_id_from(&value).or(fallback_request_id),
            diagnostics.clone(),
        ));
    }
    Ok(value)
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
