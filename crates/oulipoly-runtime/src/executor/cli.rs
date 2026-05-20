//! ## Declared roles
//!
//! Roles: orchestration, mapper, parser, validator, formatter, filter,
//! accessor.
//!
//! - orchestration: top-level execute / execute_effective / execute_resume /
//!   execute_interactive entrypoints.
//! - mapper: translating ExecutionResult / InteractiveExecutionResult /
//!   TerminalSignal between internal carriers and external consumers;
//!   exit-status to terminal-reason mapping.
//! - parser: shell_split, provider_name classification, child-marker parsing
//!   from stderr, and in-band recognizer wiring.
//! - validator: InputType / ResumeKind / ResumeStrategy validation and spawn
//!   precondition checks.
//! - formatter: compose_resume_args and command construction for Command.
//! - filter: selecting matching output patterns and non-empty stream chunks.
//! - accessor: return-channel and nested JSON path retrieval helpers.
//!
//! ## Declared coupling carriers
//!
//! InputType, ModelConfig, PromptMode, ProviderConfig, ResumeKind,
//! ResumeStrategy, SessionCapture, SessionCaptureKind, ToolRestrictionKind,
//! oulipoly_config::InputDef, oulipoly_config::ResumeAcceptanceRules,
//! std::process::Child, std::process::Command, std::process::ExitStatus,
//! std::process::Stdio, std::io::Write, std::path::Path, std::path::PathBuf,
//! std::sync::Arc, std::sync::atomic::AtomicBool,
//! std::sync::atomic::Ordering, std::thread,
//! signal_hook::consts::signal::SIGHUP, signal_hook::consts::signal::SIGINT,
//! signal_hook::consts::signal::SIGTERM,
//! signal_hook::iterator::SignalsHandle, signal_hook::iterator::Signals,
//! libc.
//!
//! Subordinate carrier: std::os::unix::process::ExitStatusExt.

use oulipoly_config::{
    InputType, ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionCapture,
    SessionCaptureKind, ToolRestrictionKind,
};
use oulipoly_state::CompositeInvocationId;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
#[cfg(unix)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

#[cfg(unix)]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::{Handle as SignalsHandle, Signals};

use super::{
    CapturedChildInvocation, ExecutionResult, ResumeAcceptanceResult, ResumeAcceptanceStatus,
    ReturnedArtifactRef, SessionCaptureMethod, SessionCaptureResult, TerminalSignal,
};
use crate::executor::terminal_signal::{
    TerminalSignalEvidence, TerminalSignalKind, TerminalSignalRecognizer, TerminalStatusEvidence,
};

const LARGE_PROMPT_THRESHOLD: usize = 100 * 1024; // 100KB
const BOUNDED_SILENCE_DEFAULT: Duration = Duration::from_secs(90);
const BOUNDED_SILENCE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TERMINATE_GRACE_PERIOD: Duration = Duration::from_millis(250);

pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    parent_invocation_env: Option<&str>,
) -> Result<ExecutionResult, String> {
    let provider = provider_for_index(model, provider_index)?;
    let input_args = resolve_input_flags(model, extra_inputs)?;
    let (result, temp_files) = execute_provider(
        provider,
        model.prompt_mode,
        prompt,
        working_dir,
        &input_args,
        parent_invocation_env,
        None,
    )?;
    cleanup_temp_files(temp_files);

    Ok(execution_result_from_raw(
        result,
        provider_index,
        None,
        None,
    ))
}

fn provider_for_index(
    model: &ModelConfig,
    provider_index: usize,
) -> Result<&ProviderConfig, String> {
    model
        .providers
        .get(provider_index)
        .ok_or_else(|| provider_index_out_of_range_message(model, provider_index))
}

fn provider_index_out_of_range_message(model: &ModelConfig, provider_index: usize) -> String {
    format!(
        "Provider index {} out of range for model {}",
        provider_index, model.name
    )
}

pub struct EffectiveExecuteRequest<'a> {
    pub model: &'a ModelConfig,
    pub provider: &'a ProviderConfig,
    pub provider_index: usize,
    pub prompt_mode: PromptMode,
    pub prompt: &'a str,
    pub working_dir: Option<&'a Path>,
    pub extra_inputs: &'a HashMap<String, Vec<String>>,
    pub parent_invocation_env: Option<&'a str>,
}

pub fn execute_effective(request: EffectiveExecuteRequest<'_>) -> Result<ExecutionResult, String> {
    execute_effective_with_start_known_provider_session_id(request, None)
}

pub fn execute_effective_with_start_known_provider_session_id(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
) -> Result<ExecutionResult, String> {
    execute_effective_with_optional_bounded_silence_config(
        request,
        start_known_provider_session_id,
        None,
    )
}

#[cfg(test)]
fn execute_effective_with_bounded_silence_config(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
    bounded_silence_config: BoundedSilenceConfig,
) -> Result<ExecutionResult, String> {
    execute_effective_with_optional_bounded_silence_config(
        request,
        start_known_provider_session_id,
        Some(bounded_silence_config),
    )
}

fn execute_effective_with_optional_bounded_silence_config(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
    bounded_silence_config: Option<BoundedSilenceConfig>,
) -> Result<ExecutionResult, String> {
    let input_args = resolve_input_flags(request.model, request.extra_inputs)?;
    let (result, temp_files) = execute_provider_with_arg_parts_and_bounded_silence_config(
        request.provider,
        &request.provider.args,
        &[],
        request.prompt_mode,
        request.prompt,
        request.working_dir,
        &input_args,
        request.parent_invocation_env,
        start_known_provider_session_id,
        bounded_silence_config,
    )?;
    cleanup_temp_files(temp_files);

    Ok(execution_result_from_raw(
        result,
        request.provider_index,
        None,
        None,
    ))
}

fn cleanup_temp_files(temp_files: Vec<PathBuf>) {
    for path in temp_files {
        let _ = std::fs::remove_file(path);
    }
}

fn execution_result_from_raw(
    result: RawResult,
    provider_index: usize,
    resume_acceptance: Option<ResumeAcceptanceResult>,
    session_capture_override: Option<SessionCaptureResult>,
) -> ExecutionResult {
    ExecutionResult {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        provider_index,
        session_capture: session_capture_override.unwrap_or(result.session_capture),
        resume_acceptance,
        terminal_reason: result.terminal_reason,
        terminal_signal: result.terminal_signal,
        captured_child_invocations: result.captured_child_invocations,
        returned_artifacts: result.returned_artifacts,
    }
}

/// Map user-provided inputs to CLI flag arguments based on the model's input schema.
fn resolve_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    validate_extra_inputs(model, extra_inputs)?;
    validate_required_inputs(model, extra_inputs)?;
    Ok(format_input_flags(model, extra_inputs))
}

fn validate_extra_inputs(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<(), String> {
    for (key, values) in extra_inputs {
        if let Some(input_def) = input_def_by_name(model, key) {
            validate_input_values(values, input_def)?;
        }
    }
    Ok(())
}

fn validate_required_inputs(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<(), String> {
    for input_def in &model.inputs {
        if input_requires_user_value(input_def, extra_inputs) {
            return Err(required_input_message(&input_def.name));
        }
    }
    Ok(())
}

fn input_requires_user_value(
    input_def: &oulipoly_config::InputDef,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> bool {
    !input_def.default_input
        && !extra_inputs.contains_key(&input_def.name)
        && input_def.default.is_none()
        && input_def.required
}

fn required_input_message(name: &str) -> String {
    format!("Required input '{name}' not provided")
}

fn format_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut args = Vec::new();
    append_extra_input_flags(&mut args, model, extra_inputs);
    append_default_input_flags(&mut args, model, extra_inputs);
    args
}

fn append_extra_input_flags(
    args: &mut Vec<String>,
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) {
    for (key, values) in extra_inputs {
        let flag = input_def_by_name(model, key)
            .and_then(|def| def.flag.clone())
            .unwrap_or_else(|| fallback_input_flag(key));
        append_repeated_flag_values(args, &flag, values);
    }
}

fn append_default_input_flags(
    args: &mut Vec<String>,
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) {
    for input_def in &model.inputs {
        if input_def.default_input || extra_inputs.contains_key(&input_def.name) {
            continue;
        }
        if let (Some(default), Some(flag)) = (&input_def.default, &input_def.flag) {
            args.push(flag.clone());
            args.push(toml_value_to_string(default));
        }
    }
}

fn input_def_by_name<'a>(
    model: &'a ModelConfig,
    name: &str,
) -> Option<&'a oulipoly_config::InputDef> {
    model.inputs.iter().find(|input| input.name == name)
}

fn fallback_input_flag(name: &str) -> String {
    format!("--{name}")
}

fn append_repeated_flag_values(args: &mut Vec<String>, flag: &str, values: &[String]) {
    for val in values {
        args.push(flag.to_string());
        args.push(val.clone());
    }
}

fn validate_input_values(
    values: &[String],
    input_def: &oulipoly_config::InputDef,
) -> Result<(), String> {
    match &input_def.input_type {
        InputType::Enum { options } => {
            for val in values {
                if !options.contains(val) {
                    return Err(format!(
                        "Input '{}': '{}' is not a valid option. Valid: {:?}",
                        input_def.name, val, options
                    ));
                }
            }
        }
        InputType::Integer { min, max } => {
            for val in values {
                let n: i64 = val.parse().map_err(|_| {
                    format!(
                        "Input '{}': '{}' is not a valid integer",
                        input_def.name, val
                    )
                })?;
                if let Some(min_val) = min
                    && n < *min_val
                {
                    return Err(format!(
                        "Input '{}': {} is below minimum {}",
                        input_def.name, n, min_val
                    ));
                }
                if let Some(max_val) = max
                    && n > *max_val
                {
                    return Err(format!(
                        "Input '{}': {} exceeds maximum {}",
                        input_def.name, n, max_val
                    ));
                }
            }
        }
        InputType::Number { min, max } => {
            for val in values {
                let n: f64 = val.parse().map_err(|_| {
                    format!(
                        "Input '{}': '{}' is not a valid number",
                        input_def.name, val
                    )
                })?;
                if let Some(min_val) = min
                    && n < *min_val
                {
                    return Err(format!(
                        "Input '{}': {} is below minimum {}",
                        input_def.name, n, min_val
                    ));
                }
                if let Some(max_val) = max
                    && n > *max_val
                {
                    return Err(format!(
                        "Input '{}': {} exceeds maximum {}",
                        input_def.name, n, max_val
                    ));
                }
            }
        }
        InputType::Array {
            min_items,
            max_items,
            ..
        } => {
            if let Some(min) = min_items
                && values.len() < *min
            {
                return Err(format!(
                    "Input '{}': need at least {} items, got {}",
                    input_def.name,
                    min,
                    values.len()
                ));
            }
            if let Some(max) = max_items
                && values.len() > *max
            {
                return Err(format!(
                    "Input '{}': maximum {} items, got {}",
                    input_def.name,
                    max,
                    values.len()
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn toml_value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}

struct RawResult {
    stdout: Vec<u8>,
    telemetry_stdout: Vec<u8>,
    stderr: String,
    /// Numeric child-process exit code per `exit_code_from_status`.
    exit_code: i32,
    session_capture: SessionCaptureResult,
    terminal_reason: Option<String>,
    terminal_signal: Option<TerminalSignal>,
    captured_child_invocations: Vec<CapturedChildInvocation>,
    returned_artifacts: Vec<ReturnedArtifactRef>,
}

struct ReturnChannel {
    path: PathBuf,
    dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderRecognizer {
    Claude,
    Codex,
    OpenAiCompat,
}

impl ProviderRecognizer {
    fn for_provider(provider: &ProviderConfig) -> Self {
        let command_provider = provider_executable_name(provider)
            .unwrap_or_else(|| command_provider_basename(provider_name(&provider.command)));
        if provider.name.starts_with("claude") || command_provider.starts_with("claude") {
            Self::Claude
        } else if provider.name.starts_with("codex") || command_provider.starts_with("codex") {
            Self::Codex
        } else {
            Self::OpenAiCompat
        }
    }

    fn recognize(&self, evidence: &TerminalSignalEvidence<'_>) -> TerminalSignal {
        match self {
            Self::Claude => crate::executor::providers::claude::Recognizer.recognize(evidence),
            Self::Codex => crate::executor::providers::codex::Recognizer.recognize(evidence),
            Self::OpenAiCompat => {
                crate::executor::providers::openai_compat::Recognizer.recognize(evidence)
            }
        }
    }
}

#[derive(Clone, Debug)]
struct BoundedSilenceConfig {
    silence_ceiling: Duration,
    prompt_mode: PromptMode,
    prompt_payload: Option<Vec<u8>>,
    recognizer: ProviderRecognizer,
}

impl BoundedSilenceConfig {
    fn production(
        provider: &ProviderConfig,
        prompt_mode: PromptMode,
        prompt_payload: Vec<u8>,
    ) -> Self {
        Self {
            silence_ceiling: BOUNDED_SILENCE_DEFAULT,
            prompt_mode,
            prompt_payload: (prompt_mode == PromptMode::Stdin).then_some(prompt_payload),
            recognizer: ProviderRecognizer::for_provider(provider),
        }
    }

    fn with_prompt_contract(
        mut self,
        prompt_mode: PromptMode,
        prompt_payload: Option<Vec<u8>>,
    ) -> Self {
        self.prompt_mode = prompt_mode;
        self.prompt_payload = prompt_payload;
        self
    }
}

impl ReturnChannel {
    fn cleanup(&self) {
        cleanup_return_channel_file(&self.path);
        cleanup_return_channel_dir(&self.dir);
    }
}

fn cleanup_return_channel_file(path: &Path) {
    if let Err(err) = std::fs::remove_file(path) {
        eprintln!("{}", delete_return_channel_warning(path, &err));
    }
}

fn cleanup_return_channel_dir(dir: &Path) {
    if let Err(err) = std::fs::remove_dir(dir)
        && return_channel_dir_cleanup_should_warn(&err)
    {
        eprintln!("{}", delete_return_channel_dir_warning(dir, &err));
    }
}

fn return_channel_dir_cleanup_should_warn(err: &std::io::Error) -> bool {
    err.kind() != std::io::ErrorKind::NotFound
        && err.kind() != std::io::ErrorKind::DirectoryNotEmpty
}

fn delete_return_channel_warning(path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to delete return channel {}: {err}",
        path.display()
    )
}

fn delete_return_channel_dir_warning(dir: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to delete return channel directory {}: {err}",
        dir.display()
    )
}

fn prepare_return_channel(
    parent_invocation_env: Option<&str>,
) -> Result<Option<ReturnChannel>, String> {
    let Some(parent_invocation_env) = parent_invocation_env else {
        return Ok(None);
    };
    let invocation = parse_return_channel_parent_invocation(parent_invocation_env)?;
    let dir = return_channel_dir(&invocation);
    create_return_channel_dir(&dir)?;
    let path = return_channel_path(&dir);
    create_return_channel_file(&path)?;
    Ok(Some(return_channel(path, dir)))
}

fn parse_return_channel_parent_invocation(
    parent_invocation_env: &str,
) -> Result<CompositeInvocationId, String> {
    CompositeInvocationId::parse_env_value(parent_invocation_env)
        .map_err(|err| format!("Failed to parse parent invocation for return channel: {err}"))
}

fn return_channel_dir(invocation: &CompositeInvocationId) -> PathBuf {
    std::env::temp_dir()
        .join("oulipoly-return-channels")
        .join(&invocation.id)
}

fn create_return_channel_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| create_return_channel_dir_error(dir, &err))
}

fn create_return_channel_dir_error(dir: &Path, err: &std::io::Error) -> String {
    format!(
        "Failed to create return channel directory {}: {err}",
        dir.display()
    )
}

fn return_channel_path(dir: &Path) -> PathBuf {
    dir.join(format!("returns-{}.jsonl", uuid::Uuid::new_v4()))
}

fn create_return_channel_file(path: &Path) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map(|_| ())
        .map_err(|err| create_return_channel_file_error(path, &err))
}

fn create_return_channel_file_error(path: &Path, err: &std::io::Error) -> String {
    format!("Failed to create return channel {}: {err}", path.display())
}

fn return_channel(path: PathBuf, dir: PathBuf) -> ReturnChannel {
    ReturnChannel { path, dir }
}

fn read_return_channel(channel: &ReturnChannel) -> Vec<ReturnedArtifactRef> {
    let body = match read_return_channel_body(&channel.path) {
        Some(body) => body,
        None => return Vec::new(),
    };
    parse_return_channel_body(&body, &channel.path)
}

fn read_return_channel_body(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(body) => Some(body),
        Err(err) => {
            eprintln!("{}", read_return_channel_warning(path, &err));
            None
        }
    }
}

fn read_return_channel_warning(path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to read return channel {}: {err}",
        path.display()
    )
}

fn parse_return_channel_body(body: &str, path: &Path) -> Vec<ReturnedArtifactRef> {
    body.lines()
        .enumerate()
        .filter_map(|(index, line)| parse_return_channel_line(index, line, path))
        .collect()
}

fn parse_return_channel_line(index: usize, line: &str, path: &Path) -> Option<ReturnedArtifactRef> {
    let trimmed = line.trim();
    if return_channel_line_is_empty(trimmed) {
        return None;
    }
    match serde_json::from_str::<ReturnedArtifactRef>(trimmed) {
        Ok(reference) => Some(reference),
        Err(err) => {
            eprintln!(
                "{}",
                parse_return_channel_line_warning(index + 1, path, &err)
            );
            None
        }
    }
}

fn return_channel_line_is_empty(line: &str) -> bool {
    line.is_empty()
}

fn parse_return_channel_line_warning(
    line_number: usize,
    path: &Path,
    err: &serde_json::Error,
) -> String {
    format!(
        "Warning: failed to parse return channel line {} in {}: {err}",
        line_number,
        path.display()
    )
}

pub struct ResumePayload<'a> {
    pub session_id: &'a str,
    pub strategy: &'a ResumeStrategy,
}

pub fn compose_resume_args(
    strategy: &ResumeStrategy,
    session_id: &str,
) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    append_resume_args(&mut args, strategy, session_id)?;
    Ok(args)
}

fn compose_resume_provider_args(
    mut provider_args: Vec<String>,
    resume: ResumePayload<'_>,
) -> Result<Vec<String>, String> {
    append_resume_args(&mut provider_args, resume.strategy, resume.session_id)?;
    Ok(provider_args)
}

fn append_resume_args(
    provider_args: &mut Vec<String>,
    strategy: &ResumeStrategy,
    session_id: &str,
) -> Result<(), String> {
    let args = validate_resume_strategy(strategy)?;
    append_validated_resume_args(provider_args, args, session_id);
    Ok(())
}

enum ValidatedResumeArgs<'a> {
    Flag(&'a str),
    Subcommand(&'a [String]),
}

fn validate_resume_strategy(strategy: &ResumeStrategy) -> Result<ValidatedResumeArgs<'_>, String> {
    match strategy.kind {
        ResumeKind::Flag => {
            let flag = strategy
                .flag
                .as_ref()
                .ok_or_else(|| "resume.flag is required".to_string())?;
            Ok(ValidatedResumeArgs::Flag(flag))
        }
        ResumeKind::Subcommand => {
            let subcommand = strategy
                .subcommand
                .as_ref()
                .ok_or_else(|| "resume.subcommand is required".to_string())?;
            if subcommand.is_empty() {
                return Err("resume.subcommand is required".to_string());
            }
            Ok(ValidatedResumeArgs::Subcommand(subcommand))
        }
    }
}

fn append_validated_resume_args(
    provider_args: &mut Vec<String>,
    args: ValidatedResumeArgs<'_>,
    session_id: &str,
) {
    match args {
        ValidatedResumeArgs::Flag(flag) => {
            provider_args.push(flag.to_string());
            provider_args.push(session_id.to_string());
        }
        ValidatedResumeArgs::Subcommand(subcommand) => {
            provider_args.extend(subcommand.iter().cloned());
            provider_args.push(session_id.to_string());
        }
    }
}

fn build_command(
    provider: &ProviderConfig,
    provider_args: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    return_channel: Option<&Path>,
) -> Result<Command, String> {
    let parts = parse_command_parts(&provider.command);
    validate_command_parts(&parts)?;
    Ok(command_from_parts(
        &parts,
        provider_args,
        working_dir,
        parent_invocation_env,
        return_channel,
    ))
}

fn parse_command_parts(command: &str) -> Vec<String> {
    shell_split(command)
}

fn validate_command_parts(parts: &[String]) -> Result<(), String> {
    if parts.is_empty() {
        return Err("Empty command".to_string());
    }
    Ok(())
}

fn command_from_parts(
    parts: &[String],
    provider_args: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    return_channel: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(&parts[0]);
    for part in &parts[1..] {
        cmd.arg(part);
    }
    for arg in provider_args {
        cmd.arg(arg);
    }

    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    if let Some(parent_invocation_env) = parent_invocation_env {
        cmd.env("OULIPOLY_PARENT_INVOCATION", parent_invocation_env);
    }
    if let Some(return_channel) = return_channel {
        cmd.env("OULIPOLY_RETURN_CHANNEL", return_channel);
    } else {
        cmd.env_remove("OULIPOLY_RETURN_CHANNEL");
    }

    cmd
}

fn execute_provider(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    execute_provider_with_args(
        provider,
        &provider.args,
        prompt_mode,
        prompt,
        working_dir,
        input_args,
        parent_invocation_env,
        start_known_provider_session_id,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "executor call shape keeps provider args, prompt mode, cwd, input flags, parent marker, and optional start-bound session explicit"
)]
fn execute_provider_with_args(
    provider: &ProviderConfig,
    provider_args: &[String],
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    execute_provider_with_arg_parts(
        provider,
        provider_args,
        &[],
        prompt_mode,
        prompt,
        working_dir,
        input_args,
        parent_invocation_env,
        start_known_provider_session_id,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "executor call shape keeps base args, lifecycle tail args, prompt mode, cwd, input flags, parent marker, and optional start-bound session explicit"
)]
fn execute_provider_with_arg_parts(
    provider: &ProviderConfig,
    provider_args: &[String],
    tail_args: &[String],
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    execute_provider_with_arg_parts_and_bounded_silence_config(
        provider,
        provider_args,
        tail_args,
        prompt_mode,
        prompt,
        working_dir,
        input_args,
        parent_invocation_env,
        start_known_provider_session_id,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "executor call shape keeps base args, lifecycle tail args, prompt mode, cwd, input flags, parent marker, optional start-bound session, and optional bounded-silence test injection explicit"
)]
fn execute_provider_with_arg_parts_and_bounded_silence_config(
    provider: &ProviderConfig,
    provider_args: &[String],
    tail_args: &[String],
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
    bounded_silence_config: Option<BoundedSilenceConfig>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    let launch = assemble_provider_launch(
        ProviderLaunchRequest {
            provider,
            provider_args,
            tail_args,
            prompt_mode,
            prompt,
            working_dir,
            input_args,
            parent_invocation_env,
            start_known_provider_session_id,
        },
        bounded_silence_config,
    )?;
    let output = run_provider_supervisor(launch.cmd, provider, launch.supervisor_config)?;
    let returned_artifacts = read_and_cleanup_return_channel(&launch.return_channel);
    let result =
        raw_result_from_supervised_output(&launch.capture_plan, output, returned_artifacts);

    Ok((result, launch.temp_files))
}

struct ProviderLaunchRequest<'a> {
    provider: &'a ProviderConfig,
    provider_args: &'a [String],
    tail_args: &'a [String],
    prompt_mode: PromptMode,
    prompt: &'a str,
    working_dir: Option<&'a Path>,
    input_args: &'a [String],
    parent_invocation_env: Option<&'a str>,
    start_known_provider_session_id: Option<&'a str>,
}

struct ProviderLaunch {
    cmd: Command,
    supervisor_config: BoundedSilenceConfig,
    capture_plan: CapturePlan,
    return_channel: Option<ReturnChannel>,
    temp_files: Vec<PathBuf>,
}

fn assemble_provider_launch(
    request: ProviderLaunchRequest<'_>,
    bounded_silence_config: Option<BoundedSilenceConfig>,
) -> Result<ProviderLaunch, String> {
    let return_channel = prepare_return_channel(request.parent_invocation_env)?;
    let (base_args, rendered_prompt) =
        provider_policy_launch_parts(request.provider, request.provider_args, request.prompt)?;
    let mut cmd = build_command(
        request.provider,
        &base_args,
        request.working_dir,
        request.parent_invocation_env,
        return_channel
            .as_ref()
            .map(|channel| channel.path.as_path()),
    )?;
    append_command_args(&mut cmd, request.input_args);
    let (capture_plan, capture_args, mut temp_files) = build_capture_plan(
        request.provider.session_capture.as_ref(),
        request.start_known_provider_session_id,
    )?;
    append_command_args(&mut cmd, &capture_args);
    append_command_args(&mut cmd, request.tail_args);
    render_prompt_for_command(
        &mut cmd,
        request.prompt_mode,
        &rendered_prompt,
        request.working_dir,
        &mut temp_files,
    )?;
    let supervisor_config = supervisor_config_for_launch(
        request.provider,
        request.prompt_mode,
        rendered_prompt,
        bounded_silence_config,
    );

    Ok(ProviderLaunch {
        cmd,
        supervisor_config,
        capture_plan,
        return_channel,
        temp_files,
    })
}

fn provider_policy_launch_parts(
    provider: &ProviderConfig,
    provider_args: &[String],
    prompt: &str,
) -> Result<(Vec<String>, String), String> {
    let mut base_args = provider_args.to_vec();
    let mut policy_prompt = Some(prompt.to_string());
    apply_provider_policy(provider, &mut base_args, &mut policy_prompt)?;
    Ok((
        base_args,
        policy_prompt.unwrap_or_else(|| prompt.to_string()),
    ))
}

fn append_command_args(cmd: &mut Command, args: &[String]) {
    for arg in args {
        cmd.arg(arg);
    }
}

fn render_prompt_for_command(
    cmd: &mut Command,
    prompt_mode: PromptMode,
    rendered_prompt: &str,
    working_dir: Option<&Path>,
    temp_files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    match prompt_mode {
        PromptMode::Arg => render_arg_prompt(cmd, rendered_prompt, working_dir, temp_files),
        PromptMode::Stdin => {
            cmd.stdin(Stdio::piped());
            Ok(())
        }
    }
}

fn render_arg_prompt(
    cmd: &mut Command,
    rendered_prompt: &str,
    working_dir: Option<&Path>,
    temp_files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if rendered_prompt.len() > LARGE_PROMPT_THRESHOLD {
        let (path, instruction) = write_large_prompt_file(rendered_prompt, working_dir)?;
        cmd.arg(instruction);
        temp_files.push(path);
    } else {
        cmd.arg(rendered_prompt);
    }
    cmd.stdin(Stdio::null());
    Ok(())
}

fn write_large_prompt_file(
    rendered_prompt: &str,
    working_dir: Option<&Path>,
) -> Result<(PathBuf, String), String> {
    let dir = working_dir.unwrap_or(Path::new("."));
    let filename = format!("_agent_prompt_{}.md", uuid::Uuid::new_v4());
    let path = dir.join(&filename);
    std::fs::write(&path, rendered_prompt)
        .map_err(|e| format!("Failed to write temp prompt file: {e}"))?;
    Ok((path, format!("Follow the instructions in {filename}")))
}

fn supervisor_config_for_launch(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    rendered_prompt: String,
    bounded_silence_config: Option<BoundedSilenceConfig>,
) -> BoundedSilenceConfig {
    let prompt_payload = (prompt_mode == PromptMode::Stdin).then(|| rendered_prompt.into_bytes());
    bounded_silence_config
        .unwrap_or_else(|| {
            BoundedSilenceConfig::production(
                provider,
                prompt_mode,
                prompt_payload.clone().unwrap_or_default(),
            )
        })
        .with_prompt_contract(prompt_mode, prompt_payload)
}

fn run_provider_supervisor(
    cmd: Command,
    provider: &ProviderConfig,
    supervisor_config: BoundedSilenceConfig,
) -> Result<SupervisedOutput, String> {
    execute_with_bounded_silence(cmd, &provider.name, supervisor_config).map_err(|err| {
        if err.starts_with("Failed to spawn") {
            err
        } else {
            format!("Failed to wait for process: {err}")
        }
    })
}

fn read_and_cleanup_return_channel(
    return_channel: &Option<ReturnChannel>,
) -> Vec<ReturnedArtifactRef> {
    let returned_artifacts = return_channel
        .as_ref()
        .map(read_return_channel)
        .unwrap_or_default();
    if let Some(channel) = return_channel {
        channel.cleanup();
    }
    returned_artifacts
}

fn raw_result_from_supervised_output(
    capture_plan: &CapturePlan,
    output: SupervisedOutput,
    returned_artifacts: Vec<ReturnedArtifactRef>,
) -> RawResult {
    let capture_outcome = finalize_capture(capture_plan, &output.stdout);
    let stdout = maybe_restore_plain_stdout(capture_plan, &capture_outcome, &output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    RawResult {
        stdout,
        telemetry_stdout: output.stdout.clone(),
        captured_child_invocations: captured_child_invocations_from_stderr(&stderr),
        stderr,
        exit_code: output.exit_code,
        terminal_reason: output.terminal_reason,
        terminal_signal: Some(output.terminal_signal),
        session_capture: capture_outcome,
        returned_artifacts,
    }
}

struct SupervisedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
    terminal_reason: Option<String>,
    terminal_signal: TerminalSignal,
}

#[derive(Clone, Copy)]
enum DrainStream {
    Stdout,
    Stderr,
}

fn execute_with_bounded_silence(
    mut cmd: Command,
    provider_name: &str,
    config: BoundedSilenceConfig,
) -> Result<SupervisedOutput, String> {
    configure_supervised_command(&mut cmd, config.prompt_mode);
    configure_supervised_process_group(&mut cmd);
    let mut child = spawn_supervised_child(cmd, provider_name)?;
    write_prompt_to_child(&mut child, &config)?;
    let drains = start_child_drains(&mut child)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut last_output_seen = Instant::now();

    let (terminal_status, terminal_signal, real_status) = loop {
        drain_output_events(&drains.rx, &mut stdout, &mut stderr, &mut last_output_seen);

        if let Some(status) = poll_child_status(&mut child)? {
            break terminal_outcome_from_status(status);
        }

        let live_signal =
            recognize_live_terminal_signal(provider_name, config.recognizer, &stdout, &stderr);
        if live_signal.kind == TerminalSignalKind::QuotaExhaustedInband {
            break terminate_for_live_quota(
                &mut child,
                provider_name,
                config.recognizer,
                &stdout,
                &stderr,
                live_signal,
            )?;
        }

        if last_output_seen.elapsed() >= config.silence_ceiling {
            let terminal_status = prolonged_silence_status();
            let terminal_signal = recognize_terminal_signal(
                provider_name,
                config.recognizer,
                &stdout,
                &stderr,
                terminal_status.clone(),
            );
            break (
                terminal_status,
                Some(terminal_signal),
                terminate_child(&mut child)?,
            );
        }

        match drains.rx.recv_timeout(BOUNDED_SILENCE_POLL_INTERVAL) {
            Ok((stream, chunk)) => append_output_chunk(
                stream,
                chunk,
                &mut stdout,
                &mut stderr,
                &mut last_output_seen,
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
    };

    finish_child_drains(drains, &mut stdout, &mut stderr, &mut last_output_seen);
    Ok(supervised_output_from_terminal(
        provider_name,
        config.recognizer,
        stdout,
        stderr,
        terminal_status,
        terminal_signal,
        real_status,
    ))
}

struct ChildDrains {
    rx: mpsc::Receiver<(DrainStream, Vec<u8>)>,
    stdout_handle: thread::JoinHandle<()>,
    stderr_handle: thread::JoinHandle<()>,
}

fn configure_supervised_command(cmd: &mut Command, prompt_mode: PromptMode) {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    match prompt_mode {
        PromptMode::Arg => {
            cmd.stdin(Stdio::null());
        }
        PromptMode::Stdin => {
            cmd.stdin(Stdio::piped());
        }
    }
}

#[cfg(unix)]
fn configure_supervised_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;

    cmd.process_group(0);
}

#[cfg(not(unix))]
fn configure_supervised_process_group(_cmd: &mut Command) {}

fn spawn_supervised_child(mut cmd: Command, provider_name: &str) -> Result<Child, String> {
    cmd.spawn()
        .map_err(|err| format!("Failed to spawn '{provider_name}': {err}"))
}

fn write_prompt_to_child(child: &mut Child, config: &BoundedSilenceConfig) -> Result<(), String> {
    if supervised_stdin_write_needed(config)
        && let Some(mut stdin) = child.stdin.take()
        && let Some(payload) = &config.prompt_payload
        && let Err(err) = stdin.write_all(payload)
    {
        let _ = terminate_child(child);
        return Err(write_stdin_error(&err));
    }
    Ok(())
}

fn supervised_stdin_write_needed(config: &BoundedSilenceConfig) -> bool {
    config.prompt_mode == PromptMode::Stdin
}

fn write_stdin_error(err: &std::io::Error) -> String {
    format!("Failed to write to stdin: {err}")
}

fn start_child_drains(child: &mut Child) -> Result<ChildDrains, String> {
    let stdout = take_child_stdout(child)?;
    let stderr = take_child_stderr(child)?;
    let (tx, rx) = mpsc::channel();
    let stdout_handle = spawn_drain_thread(stdout, DrainStream::Stdout, tx.clone());
    let stderr_handle = spawn_drain_thread(stderr, DrainStream::Stderr, tx.clone());
    drop(tx);
    Ok(ChildDrains {
        rx,
        stdout_handle,
        stderr_handle,
    })
}

fn take_child_stdout(child: &mut Child) -> Result<impl Read + Send + 'static, String> {
    child
        .stdout
        .take()
        .ok_or_else(|| "Child stdout was not piped".to_string())
}

fn take_child_stderr(child: &mut Child) -> Result<impl Read + Send + 'static, String> {
    child
        .stderr
        .take()
        .ok_or_else(|| "Child stderr was not piped".to_string())
}

fn poll_child_status(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| format!("try_wait failed: {err}"))
}

fn terminal_outcome_from_status(
    status: ExitStatus,
) -> (
    TerminalStatusEvidence,
    Option<TerminalSignal>,
    Option<ExitStatus>,
) {
    (
        terminal_status_from_exit_status(&status),
        None,
        Some(status),
    )
}

fn recognize_live_terminal_signal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
) -> TerminalSignal {
    recognize_terminal_signal(
        provider_name,
        recognizer,
        stdout,
        stderr,
        TerminalStatusEvidence::Unknown,
    )
}

fn terminate_for_live_quota(
    child: &mut Child,
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
    live_signal: TerminalSignal,
) -> Result<
    (
        TerminalStatusEvidence,
        Option<TerminalSignal>,
        Option<ExitStatus>,
    ),
    String,
> {
    if let Some(status) = child
        .try_wait()
        .map_err(|err| format!("try_wait before quota terminate failed: {err}"))?
    {
        let terminal_status = terminal_status_from_exit_status(&status);
        let terminal_signal = recognize_terminal_signal(
            provider_name,
            recognizer,
            stdout,
            stderr,
            terminal_status.clone(),
        );
        Ok((terminal_status, Some(terminal_signal), Some(status)))
    } else {
        Ok((
            TerminalStatusEvidence::Unknown,
            Some(live_signal),
            terminate_child(child)?,
        ))
    }
}

fn prolonged_silence_status() -> TerminalStatusEvidence {
    TerminalStatusEvidence::ProlongedSilence {
        reason: "bounded_silence".to_string(),
    }
}

fn finish_child_drains(
    drains: ChildDrains,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    let _ = drains.stdout_handle.join();
    let _ = drains.stderr_handle.join();
    drain_output_events(&drains.rx, stdout, stderr, last_output_seen);
}

fn supervised_output_from_terminal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    terminal_status: TerminalStatusEvidence,
    terminal_signal: Option<TerminalSignal>,
    real_status: Option<ExitStatus>,
) -> SupervisedOutput {
    let terminal_signal = terminal_signal.unwrap_or_else(|| {
        recognize_terminal_signal(
            provider_name,
            recognizer,
            &stdout,
            &stderr,
            terminal_status.clone(),
        )
    });
    let exit_code = real_status
        .as_ref()
        .map(exit_code_from_status)
        .unwrap_or_else(|| synthetic_exit_code(&terminal_signal));
    let terminal_reason = terminal_reason_from_signal(&terminal_signal, real_status.as_ref());

    SupervisedOutput {
        stdout,
        stderr,
        exit_code,
        terminal_reason,
        terminal_signal,
    }
}

fn spawn_drain_thread<R>(
    mut reader: R,
    stream: DrainStream,
    sender: mpsc::Sender<(DrainStream, Vec<u8>)>,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        while let Some(chunk) = read_drain_chunk(&mut reader, &mut buffer) {
            if send_drain_chunk(&sender, stream, chunk).is_err() {
                break;
            }
        }
    })
}

fn read_drain_chunk<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Option<Vec<u8>> {
    match reader.read(buffer) {
        Ok(0) | Err(_) => None,
        Ok(n) => Some(buffer[..n].to_vec()),
    }
}

fn send_drain_chunk(
    sender: &mpsc::Sender<(DrainStream, Vec<u8>)>,
    stream: DrainStream,
    chunk: Vec<u8>,
) -> Result<(), mpsc::SendError<(DrainStream, Vec<u8>)>> {
    sender.send((stream, chunk))
}

fn drain_output_events(
    rx: &mpsc::Receiver<(DrainStream, Vec<u8>)>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    while let Ok((stream, chunk)) = rx.try_recv() {
        append_output_chunk(stream, chunk, stdout, stderr, last_output_seen);
    }
}

fn append_output_chunk(
    stream: DrainStream,
    chunk: Vec<u8>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    last_output_seen: &mut Instant,
) {
    if output_chunk_is_empty(&chunk) {
        return;
    }
    append_non_empty_output_chunk(stream, &chunk, stdout, stderr);
    *last_output_seen = Instant::now();
}

fn output_chunk_is_empty(chunk: &[u8]) -> bool {
    chunk.is_empty()
}

fn append_non_empty_output_chunk(
    stream: DrainStream,
    chunk: &[u8],
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    match stream {
        DrainStream::Stdout => stdout.extend_from_slice(chunk),
        DrainStream::Stderr => stderr.extend_from_slice(chunk),
    }
}

fn terminate_child(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    if let Some(status) = try_wait_before_terminate(child)? {
        return Ok(mapped_child_status(status));
    }

    send_child_sigterm(child);
    if let Some(status) = wait_for_child_after_sigterm(child)? {
        return Ok(mapped_child_status(status));
    }

    send_child_sigkill(child)?;
    reap_child_after_kill(child)
}

fn try_wait_before_terminate(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|err| format!("try_wait before terminate failed: {err}"))
}

fn wait_for_child_after_sigterm(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    let started = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("try_wait after terminate failed: {err}"))?
        {
            return Ok(Some(status));
        }
        if terminate_grace_period_elapsed(started) {
            return Ok(None);
        }
        thread::sleep(BOUNDED_SILENCE_POLL_INTERVAL);
    }
}

fn terminate_grace_period_elapsed(started: Instant) -> bool {
    started.elapsed() >= TERMINATE_GRACE_PERIOD
}

fn mapped_child_status(status: ExitStatus) -> Option<ExitStatus> {
    Some(status)
}

#[cfg(unix)]
fn send_child_sigkill(child: &mut Child) -> Result<(), String> {
    let pid = child_process_group_id(child);
    let rc = unsafe { libc::killpg(pid, libc::SIGKILL) };
    if rc == -1 {
        child
            .kill()
            .map_err(|err| format!("Failed to kill child process: {err}"))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn send_child_sigkill(child: &mut Child) -> Result<(), String> {
    child
        .kill()
        .map_err(|err| format!("Failed to kill child process: {err}"))
}

fn reap_child_after_kill(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .wait()
        .map(Some)
        .map_err(|err| format!("Failed to reap child process: {err}"))
}

#[cfg(unix)]
fn send_child_sigterm(child: &Child) {
    let _ = unsafe { libc::killpg(child_process_group_id(child), SIGTERM) };
}

#[cfg(unix)]
fn child_process_group_id(child: &Child) -> libc::pid_t {
    child.id() as libc::pid_t
}

#[cfg(not(unix))]
fn send_child_sigterm(child: &mut Child) {
    let _ = child.kill();
}

fn recognize_terminal_signal(
    provider_name: &str,
    recognizer: ProviderRecognizer,
    stdout: &[u8],
    stderr: &[u8],
    terminal_status: TerminalStatusEvidence,
) -> TerminalSignal {
    let evidence = terminal_signal_evidence(provider_name, stdout, stderr, terminal_status);
    recognizer.recognize(&evidence)
}

fn terminal_signal_evidence<'a>(
    provider_name: &'a str,
    stdout: &'a [u8],
    stderr: &'a [u8],
    terminal_status: TerminalStatusEvidence,
) -> TerminalSignalEvidence<'a> {
    TerminalSignalEvidence {
        provider_name,
        stdout,
        stderr,
        terminal_status,
        observed_at: SystemTime::now(),
    }
}

#[cfg(test)]
fn terminal_signal_for_spawn_error(provider_name: &str, reason: &str) -> TerminalSignal {
    recognize_terminal_signal(
        provider_name,
        ProviderRecognizer::OpenAiCompat,
        &[],
        &[],
        TerminalStatusEvidence::SpawnError {
            reason: reason.to_string(),
        },
    )
}

fn terminal_status_from_exit_status(status: &ExitStatus) -> TerminalStatusEvidence {
    if let Some(code) = status.code() {
        return TerminalStatusEvidence::Exited { code };
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return TerminalStatusEvidence::SignalTerminated { signal };
        }
    }

    TerminalStatusEvidence::Unknown
}

fn terminal_reason_from_signal(
    signal: &TerminalSignal,
    status: Option<&ExitStatus>,
) -> Option<String> {
    match signal.kind {
        TerminalSignalKind::ProlongedSilence => Some("bounded_silence".to_string()),
        TerminalSignalKind::QuotaExhaustedInband => Some("quota_exhausted_inband".to_string()),
        TerminalSignalKind::RateLimited => Some("rate_limited".to_string()),
        TerminalSignalKind::CleanExit
        | TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit => status.and_then(classify_terminal_reason),
        TerminalSignalKind::SpawnError => Some("spawn_error".to_string()),
        TerminalSignalKind::Unknown => Some("unknown_exit".to_string()),
    }
}

fn synthetic_exit_code(signal: &TerminalSignal) -> i32 {
    match signal.kind {
        TerminalSignalKind::CleanExit => 0,
        TerminalSignalKind::ProlongedSilence
        | TerminalSignalKind::QuotaExhaustedInband
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::Unknown => -1,
    }
}

fn apply_provider_policy(
    provider: &ProviderConfig,
    base_args: &mut Vec<String>,
    prompt: &mut Option<String>,
) -> Result<(), String> {
    if provider.system_prompt_override.is_none() && provider.tool_restrictions.is_none() {
        return Ok(());
    }

    let kind = provider_policy_kind(provider);
    let restrictions = provider.tool_restrictions.as_ref();
    validate_provider_tool_restrictions(provider, restrictions)?;

    match kind {
        Some(ToolRestrictionKind::Claude) => {
            append_claude_provider_policy(provider, restrictions, base_args)
        }
        Some(ToolRestrictionKind::Codex) => {
            append_codex_provider_policy(provider, restrictions, base_args, prompt)?
        }
        None => return Err(provider_policy_kind_error(provider)),
    }

    Ok(())
}

fn validate_provider_tool_restrictions(
    provider: &ProviderConfig,
    restrictions: Option<&oulipoly_config::ToolRestrictions>,
) -> Result<(), String> {
    if let Some(restrictions) = restrictions {
        validate_provider_restriction_shape(provider, restrictions)?;
        validate_provider_restriction_kind(provider, restrictions)?;
    }
    Ok(())
}

fn validate_provider_restriction_shape(
    provider: &ProviderConfig,
    restrictions: &oulipoly_config::ToolRestrictions,
) -> Result<(), String> {
    if !restrictions.claude.is_empty() && !restrictions.codex.is_empty() {
        return Err(format!(
            "provider {} has both Claude and Codex tool restrictions populated",
            provider.name
        ));
    }
    Ok(())
}

fn validate_provider_restriction_kind(
    provider: &ProviderConfig,
    restrictions: &oulipoly_config::ToolRestrictions,
) -> Result<(), String> {
    let claude_non_empty = !restrictions.claude.is_empty();
    let codex_non_empty = !restrictions.codex.is_empty();
    match restrictions.kind {
        ToolRestrictionKind::Claude if codex_non_empty => Err(format!(
            "provider {} declares Claude restrictions but populated Codex settings",
            provider.name
        )),
        ToolRestrictionKind::Codex if claude_non_empty => Err(format!(
            "provider {} declares Codex restrictions but populated Claude settings",
            provider.name
        )),
        _ => Ok(()),
    }
}

fn append_claude_provider_policy(
    provider: &ProviderConfig,
    restrictions: Option<&oulipoly_config::ToolRestrictions>,
    base_args: &mut Vec<String>,
) {
    if let Some(override_text) = &provider.system_prompt_override {
        base_args.push("--append-system-prompt".to_string());
        base_args.push(override_text.clone());
    }
    if let Some(restrictions) = restrictions {
        if !restrictions.claude.disallowed_tools.is_empty() {
            base_args.push("--disallowed-tools".to_string());
            base_args.push(restrictions.claude.disallowed_tools.join(","));
        }
        if !restrictions.claude.allowed_tools.is_empty() {
            base_args.push("--allowed-tools".to_string());
            base_args.push(restrictions.claude.allowed_tools.join(","));
        }
        if restrictions.claude.disable_slash_commands {
            base_args.push("--disable-slash-commands".to_string());
        }
    }
}

fn append_codex_provider_policy(
    provider: &ProviderConfig,
    restrictions: Option<&oulipoly_config::ToolRestrictions>,
    base_args: &mut Vec<String>,
    prompt: &mut Option<String>,
) -> Result<(), String> {
    if let Some(override_text) = &provider.system_prompt_override {
        let Some(existing_prompt) = prompt.take() else {
            return Err(codex_policy_missing_prompt_error(provider));
        };
        *prompt = Some(codex_policy_prompt(override_text, &existing_prompt));
    }
    if let Some(restrictions) = restrictions {
        append_codex_config_pairs(base_args, &restrictions.codex.config_pairs);
        append_codex_disabled_features(base_args, &restrictions.codex.disabled_features);
    }
    Ok(())
}

fn codex_policy_missing_prompt_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} has Codex system_prompt_override but this execution path has no prompt to prepend it to",
        provider.name
    )
}

fn codex_policy_prompt(override_text: &str, existing_prompt: &str) -> String {
    format!("<<<NESTHARUS-POLICY>>>\n{override_text}\n<<<END-POLICY>>>\n\n{existing_prompt}")
}

fn append_codex_config_pairs(base_args: &mut Vec<String>, config_pairs: &[String]) {
    for pair in config_pairs {
        base_args.push("-c".to_string());
        base_args.push(pair.clone());
    }
}

fn append_codex_disabled_features(base_args: &mut Vec<String>, disabled_features: &[String]) {
    for feature in disabled_features {
        base_args.push("--disable".to_string());
        base_args.push(feature.clone());
    }
}

fn provider_policy_kind_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} defines policy but its ecosystem could not be inferred",
        provider.name
    )
}

fn provider_policy_kind(provider: &ProviderConfig) -> Option<ToolRestrictionKind> {
    if let Some(restrictions) = &provider.tool_restrictions {
        return Some(restrictions.kind);
    }
    let command_provider = provider_executable_name(provider)
        .unwrap_or_else(|| command_provider_basename(provider_name(&provider.command)));
    if provider.name.starts_with("claude") || command_provider.starts_with("claude") {
        Some(ToolRestrictionKind::Claude)
    } else if provider.name.starts_with("codex") || command_provider.starts_with("codex") {
        Some(ToolRestrictionKind::Codex)
    } else {
        None
    }
}

fn command_provider_basename(provider: String) -> String {
    std::path::Path::new(&provider)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(provider.as_str())
        .to_string()
}

fn provider_executable_name(provider: &ProviderConfig) -> Option<String> {
    let mut tokens = shell_split(&provider.command);
    tokens.extend(provider.args.iter().cloned());

    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        if token == "env" || token.ends_with("/env") {
            i += 1;
            continue;
        }
        if let Some(rest) = token.strip_prefix("--") {
            if rest.is_empty() {
                i += 1;
                continue;
            }
            i += 1;
            continue;
        }
        if let Some(rest) = token.strip_prefix('-') {
            if matches!(rest, "u" | "e" | "S") {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if token.contains('=') && token.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            i += 1;
            continue;
        }

        return Some(command_provider_basename(token.to_string()));
    }

    None
}

pub fn execute_resume(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
) -> Result<ExecutionResult, String> {
    execute_resume_with_optional_bounded_silence_config(
        provider,
        provider_index,
        prompt_mode,
        prompt,
        working_dir,
        parent_invocation_env,
        resume,
        None,
    )
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "test injection mirrors the public resume execution contract plus bounded-silence config"
)]
fn execute_resume_with_bounded_silence_config(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
    bounded_silence_config: BoundedSilenceConfig,
) -> Result<ExecutionResult, String> {
    execute_resume_with_optional_bounded_silence_config(
        provider,
        provider_index,
        prompt_mode,
        prompt,
        working_dir,
        parent_invocation_env,
        resume,
        Some(bounded_silence_config),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "resume execution keeps provider, prompt, cwd, parent marker, resume payload, and optional bounded-silence test injection explicit"
)]
fn execute_resume_with_optional_bounded_silence_config(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
    bounded_silence_config: Option<BoundedSilenceConfig>,
) -> Result<ExecutionResult, String> {
    let session_id = resume.session_id.to_string();
    let resume_args = compose_resume_args(resume.strategy, resume.session_id)?;
    let mut provider_without_capture = provider.clone();
    provider_without_capture.session_capture = None;
    let (result, temp_files) = execute_provider_with_arg_parts_and_bounded_silence_config(
        &provider_without_capture,
        &provider.args,
        &resume_args,
        prompt_mode,
        prompt,
        working_dir,
        &[],
        parent_invocation_env,
        None,
        bounded_silence_config,
    )?;
    cleanup_temp_files(temp_files);
    let resume_acceptance = classify_resume_acceptance(
        provider.resume_acceptance.as_ref(),
        result.exit_code,
        &result.telemetry_stdout,
        result.stderr.as_bytes(),
        &session_id,
    );
    Ok(execution_result_from_raw(
        result,
        provider_index,
        Some(resume_acceptance),
        Some(SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        }),
    ))
}

fn classify_resume_acceptance(
    rules: Option<&oulipoly_config::ResumeAcceptanceRules>,
    exit_code: i32,
    stdout: &[u8],
    stderr: &[u8],
    session_id: &str,
) -> ResumeAcceptanceResult {
    let output = resume_acceptance_output(stdout, stderr);
    if let Some(rules) = rules {
        if let Some(pattern) = rejected_resume_pattern(rules, &output, session_id) {
            return rejected_resume_pattern_result(&pattern);
        }
        if exit_code == 0
            && let Some(pattern) = accepted_resume_pattern(rules, &output, session_id)
        {
            return accepted_resume_pattern_result(&pattern);
        }
    }

    if output_reports_missing_session(&output) {
        return missing_session_resume_result();
    }

    if resume_child_exited_successfully(exit_code) {
        unconfirmed_resume_result(rules)
    } else {
        rejected_resume_exit_result(exit_code)
    }
}

fn resume_acceptance_output(stdout: &[u8], stderr: &[u8]) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
}

fn rejected_resume_pattern(
    rules: &oulipoly_config::ResumeAcceptanceRules,
    output: &str,
    session_id: &str,
) -> Option<String> {
    first_matching_pattern(
        rules.rejected_output_patterns.as_deref(),
        output,
        session_id,
    )
}

fn accepted_resume_pattern(
    rules: &oulipoly_config::ResumeAcceptanceRules,
    output: &str,
    session_id: &str,
) -> Option<String> {
    first_matching_pattern(
        rules.accepted_output_patterns.as_deref(),
        output,
        session_id,
    )
}

fn rejected_resume_pattern_result(pattern: &str) -> ResumeAcceptanceResult {
    ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Rejected,
        evidence: Some(format!("matched reject pattern: {pattern}")),
    }
}

fn accepted_resume_pattern_result(pattern: &str) -> ResumeAcceptanceResult {
    ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Accepted,
        evidence: Some(format!("matched accept pattern: {pattern}")),
    }
}

fn output_reports_missing_session(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("no conversation found") || lower.contains("no session found")
}

fn missing_session_resume_result() -> ResumeAcceptanceResult {
    ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Rejected,
        evidence: Some("resume_session_mismatch: provider reported missing session".to_string()),
    }
}

fn resume_child_exited_successfully(exit_code: i32) -> bool {
    exit_code == 0
}

fn unconfirmed_resume_result(
    rules: Option<&oulipoly_config::ResumeAcceptanceRules>,
) -> ResumeAcceptanceResult {
    let evidence = if rules.is_none() {
        "child exited 0 but provider has no resume_acceptance rules configured"
    } else {
        "child exited 0 but no accepted resume pattern matched"
    };
    ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Unconfirmed,
        evidence: Some(evidence.to_string()),
    }
}

fn rejected_resume_exit_result(exit_code: i32) -> ResumeAcceptanceResult {
    ResumeAcceptanceResult {
        status: ResumeAcceptanceStatus::Rejected,
        evidence: Some(format!(
            "child exited {exit_code}; no rejection patterns matched"
        )),
    }
}

fn first_matching_pattern(
    patterns: Option<&[String]>,
    output: &str,
    session_id: &str,
) -> Option<String> {
    patterns?
        .iter()
        .find(|pattern| pattern_matches_output(pattern, output, session_id))
        .cloned()
}

fn pattern_matches_output(pattern: &str, output: &str, session_id: &str) -> bool {
    output.contains(&expand_resume_pattern(pattern, session_id))
}

fn expand_resume_pattern(pattern: &str, session_id: &str) -> String {
    pattern.replace("{session_id}", session_id)
}

pub fn execute_interactive(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
) -> Result<i32, String> {
    execute_interactive_with_result(provider, working_dir, parent_invocation_env, resume)
        .map(|result| result.exit_code)
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractiveExecutionResult {
    /// Numeric child-process exit code per `exit_code_from_status` (interactive/REPL path).
    pub exit_code: i32,
    pub terminal_reason: Option<String>,
    pub terminal_signal: Option<TerminalSignal>,
}

pub fn execute_interactive_with_result(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
) -> Result<InteractiveExecutionResult, String> {
    // Keep provider.interactive_args as the interactive argv source.
    let mut provider_args = validated_interactive_args(provider)?;
    let mut no_prompt = None;
    apply_provider_policy(provider, &mut provider_args, &mut no_prompt)?;
    if let Some(resume) = resume {
        provider_args = compose_resume_provider_args(provider_args, resume)?;
    }

    let mut cmd = build_command(
        provider,
        &provider_args,
        working_dir,
        parent_invocation_env,
        None,
    )?;
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {e}", provider.command))?;

    #[cfg(unix)]
    let signal_guard = InteractiveSignalGuard::install(&mut child)?;

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for process: {e}"))?;

    #[cfg(unix)]
    drop(signal_guard);

    Ok(interactive_result_from_status(provider, &status))
}

enum CapturePlan {
    None,
    ForcedFlagVerified {
        requested_session_id: String,
    },
    StdoutJsonEvent {
        event_type: String,
        event_id_path: String,
        last_message_path: PathBuf,
    },
}

fn validated_interactive_args(provider: &ProviderConfig) -> Result<Vec<String>, String> {
    provider
        .interactive_args
        .clone()
        .ok_or_else(|| interactive_args_missing_error(provider))
}

fn interactive_args_missing_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} has no interactive_args; cannot launch interactively",
        provider.name
    )
}

fn interactive_result_from_status(
    provider: &ProviderConfig,
    status: &ExitStatus,
) -> InteractiveExecutionResult {
    let terminal_reason = classify_terminal_reason(status);
    let terminal_signal = recognize_terminal_signal(
        &provider.name,
        ProviderRecognizer::for_provider(provider),
        &[],
        &[],
        terminal_status_from_exit_status(status),
    );
    InteractiveExecutionResult {
        exit_code: exit_code_from_status(status),
        terminal_reason,
        terminal_signal: Some(terminal_signal),
    }
}

pub fn start_known_provider_session_id(
    provider: &ProviderConfig,
) -> Result<Option<String>, String> {
    let Some(capture) = provider.session_capture.as_ref() else {
        return Ok(None);
    };
    validate_start_known_capture(capture)?;
    Ok(start_known_session_id_for_capture(capture))
}

fn validate_start_known_capture(capture: &SessionCapture) -> Result<(), String> {
    match capture.kind {
        SessionCaptureKind::ForcedFlagVerified => {
            if capture.flag.is_none() {
                return Err("session_capture.flag is required".to_string());
            }
            Ok(())
        }
        SessionCaptureKind::None | SessionCaptureKind::StdoutJsonEvent => Ok(()),
    }
}

fn start_known_session_id_for_capture(capture: &SessionCapture) -> Option<String> {
    match capture.kind {
        SessionCaptureKind::ForcedFlagVerified => Some(uuid::Uuid::new_v4().to_string()),
        SessionCaptureKind::None | SessionCaptureKind::StdoutJsonEvent => None,
    }
}

fn build_capture_plan(
    capture: Option<&SessionCapture>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let Some(capture) = capture else {
        return Ok((CapturePlan::None, Vec::new(), Vec::new()));
    };

    match capture.kind {
        SessionCaptureKind::None => Ok(empty_capture_plan()),
        SessionCaptureKind::ForcedFlagVerified => {
            build_forced_flag_capture_plan(capture, start_known_provider_session_id)
        }
        SessionCaptureKind::StdoutJsonEvent => build_stdout_json_event_capture_plan(capture),
    }
}

fn empty_capture_plan() -> (CapturePlan, Vec<String>, Vec<PathBuf>) {
    (CapturePlan::None, Vec::new(), Vec::new())
}

fn build_forced_flag_capture_plan(
    capture: &SessionCapture,
    start_known_provider_session_id: Option<&str>,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let flag = required_capture_field(&capture.flag, "session_capture.flag")?;
    let requested_session_id = capture_requested_session_id(start_known_provider_session_id);
    let args = forced_flag_capture_args(flag, &requested_session_id, capture);
    Ok((
        CapturePlan::ForcedFlagVerified {
            requested_session_id,
        },
        args,
        Vec::new(),
    ))
}

fn build_stdout_json_event_capture_plan(
    capture: &SessionCapture,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let json_flag = required_capture_field(&capture.json_flag, "session_capture.json_flag")?;
    let last_message_flag = required_capture_field(
        &capture.last_message_flag,
        "session_capture.last_message_flag",
    )?;
    let event_type = required_capture_field(&capture.event_type, "session_capture.event_type")?;
    let event_id_path =
        required_capture_field(&capture.event_id_path, "session_capture.event_id_path")?;
    let last_message_path = last_message_capture_path();
    Ok((
        CapturePlan::StdoutJsonEvent {
            event_type,
            event_id_path,
            last_message_path: last_message_path.clone(),
        },
        stdout_json_event_capture_args(json_flag, last_message_flag, &last_message_path),
        vec![last_message_path],
    ))
}

fn required_capture_field(field: &Option<String>, name: &str) -> Result<String, String> {
    field.clone().ok_or_else(|| format!("{name} is required"))
}

fn capture_requested_session_id(start_known_provider_session_id: Option<&str>) -> String {
    start_known_provider_session_id
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

fn forced_flag_capture_args(
    flag: String,
    requested_session_id: &str,
    capture: &SessionCapture,
) -> Vec<String> {
    let mut args = vec![flag, requested_session_id.to_string()];
    if let Some(readback_args) = &capture.readback_args {
        args.extend(readback_args.clone());
    }
    args
}

fn last_message_capture_path() -> PathBuf {
    std::env::temp_dir().join(format!("oulipoly-last-message-{}", uuid::Uuid::new_v4()))
}

fn stdout_json_event_capture_args(
    json_flag: String,
    last_message_flag: String,
    last_message_path: &Path,
) -> Vec<String> {
    vec![
        json_flag,
        last_message_flag,
        last_message_path.to_string_lossy().into_owned(),
    ]
}

fn finalize_capture(plan: &CapturePlan, stdout: &[u8]) -> SessionCaptureResult {
    match plan {
        CapturePlan::None => SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
        CapturePlan::ForcedFlagVerified {
            requested_session_id,
        } => match parse_forced_flag_verified_session_id(stdout) {
            Ok(observed) if observed == *requested_session_id => SessionCaptureResult {
                session_id: Some(requested_session_id.clone()),
                method: SessionCaptureMethod::ForcedFlagVerified,
            },
            Ok(observed) => SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::Failed(format!(
                    "readback mismatch: requested {requested_session_id}, observed {observed}"
                )),
            },
            Err(err) => SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::Failed(err),
            },
        },
        CapturePlan::StdoutJsonEvent {
            event_type,
            event_id_path,
            ..
        } => match parse_stdout_json_event_session_id(stdout, event_type, event_id_path) {
            Ok(session_id) => SessionCaptureResult {
                session_id: Some(session_id),
                method: SessionCaptureMethod::StdoutJsonEvent,
            },
            Err(err) => SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::Failed(err),
            },
        },
    }
}

fn maybe_restore_plain_stdout(
    plan: &CapturePlan,
    capture: &SessionCaptureResult,
    stdout: &[u8],
) -> Vec<u8> {
    match plan {
        CapturePlan::StdoutJsonEvent {
            last_message_path, ..
        } => restore_plain_stdout_from_file(last_message_path, capture, stdout),
        _ => fallback_plain_stdout(stdout),
    }
}

fn restore_plain_stdout_from_file(
    last_message_path: &Path,
    capture: &SessionCaptureResult,
    stdout: &[u8],
) -> Vec<u8> {
    match std::fs::read(last_message_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn_restore_plain_stdout(last_message_path, capture, &err);
            fallback_plain_stdout(stdout)
        }
    }
}

fn warn_restore_plain_stdout(
    last_message_path: &Path,
    capture: &SessionCaptureResult,
    err: &std::io::Error,
) {
    if matches!(capture.method, SessionCaptureMethod::StdoutJsonEvent) {
        eprintln!("{}", restore_plain_stdout_warning(last_message_path, err));
    }
}

fn restore_plain_stdout_warning(last_message_path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to read reconstructed last-message output {}: {err}",
        last_message_path.display()
    )
}

fn fallback_plain_stdout(stdout: &[u8]) -> Vec<u8> {
    stdout.to_vec()
}

fn parse_forced_flag_verified_session_id(stdout: &[u8]) -> Result<String, String> {
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("result")
            && let Some(session_id) = value.get("session_id").and_then(Value::as_str)
        {
            return Ok(session_id.to_string());
        }
        if value.get("type").and_then(Value::as_str) != Some("system") {
            continue;
        }
        if value.get("subtype").and_then(Value::as_str) != Some("init") {
            continue;
        }
        return value
            .get("session_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| "system.init event missing session_id".to_string());
    }
    Err("stdout did not contain a result or system.init session_id event".to_string())
}

fn parse_stdout_json_event_session_id(
    stdout: &[u8],
    event_type: &str,
    event_id_path: &str,
) -> Result<String, String> {
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some(event_type) {
            continue;
        }
        return lookup_json_path(&value, event_id_path)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("event '{event_type}' missing id path '{event_id_path}'"));
    }
    Err(format!("stdout did not contain event '{event_type}'"))
}

fn lookup_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

pub fn shell_split(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

pub fn provider_name(command: &str) -> String {
    shell_split(command)
        .last()
        .cloned()
        .unwrap_or_else(|| command.to_string())
}

/// `exit_code_from_status` is the canonical executor-boundary conversion from
/// a child `ExitStatus` to the runtime's `i32` exit-code representation:
///
/// - `status.code()` is preserved when `Some(n)` (covers normal exits including
///   non-zero).
/// - On Unix, when `status.code()` is `None` and `ExitStatusExt::signal()`
///   returns `sig`, the helper returns `128 + sig` (POSIX shell convention;
///   e.g., SIGTERM -> 143).
/// - When neither a normal code nor a signal is available, the helper returns
///   `-1` (preserved fallback).
///
/// All executor paths that own a real child `ExitStatus` (one-shot provider,
/// headless resume via `execute_provider_with_args`, interactive REPL via
/// `execute_interactive_with_result`) must route their conversion through this
/// helper. Synthetic lifecycle sentinels (spawn error, `FinalizerGuard` drop,
/// captured-child supervisor fallback) do not have a child `ExitStatus` and do
/// not call this helper.
fn exit_code_from_status(status: &ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(code) = status.code() {
            return code;
        }
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    status.code().unwrap_or(-1)
}

pub fn classify_terminal_reason(status: &std::process::ExitStatus) -> Option<String> {
    if let Some(code) = status.code() {
        return (code != 0).then(|| "exit_nonzero".to_string());
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return Some(format!("signal:{}", signal_name(signal)));
        }
    }

    Some("unknown_exit".to_string())
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        libc::SIGQUIT => "SIGQUIT".to_string(),
        libc::SIGILL => "SIGILL".to_string(),
        libc::SIGTRAP => "SIGTRAP".to_string(),
        libc::SIGABRT => "SIGABRT".to_string(),
        libc::SIGBUS => "SIGBUS".to_string(),
        libc::SIGFPE => "SIGFPE".to_string(),
        libc::SIGKILL => "SIGKILL".to_string(),
        libc::SIGUSR1 => "SIGUSR1".to_string(),
        libc::SIGSEGV => "SIGSEGV".to_string(),
        libc::SIGUSR2 => "SIGUSR2".to_string(),
        libc::SIGPIPE => "SIGPIPE".to_string(),
        libc::SIGALRM => "SIGALRM".to_string(),
        libc::SIGTERM => "SIGTERM".to_string(),
        libc::SIGCHLD => "SIGCHLD".to_string(),
        libc::SIGCONT => "SIGCONT".to_string(),
        libc::SIGSTOP => "SIGSTOP".to_string(),
        libc::SIGTSTP => "SIGTSTP".to_string(),
        libc::SIGTTIN => "SIGTTIN".to_string(),
        libc::SIGTTOU => "SIGTTOU".to_string(),
        _ => signal.to_string(),
    }
}

fn captured_child_invocations_from_stderr(stderr: &str) -> Vec<CapturedChildInvocation> {
    let mut captured = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in stderr.lines() {
        if let Some(invocation) = captured_child_invocation_from_line(line)
            && mark_captured_child_seen(&mut seen, &invocation.composite_id)
        {
            captured.push(invocation);
        }
    }

    captured
}

fn captured_child_invocation_from_line(line: &str) -> Option<CapturedChildInvocation> {
    let composite_id = captured_child_composite_id(line)?;
    Some(CapturedChildInvocation {
        composite_id,
        raw_marker_line: line.to_string(),
    })
}

fn captured_child_composite_id(line: &str) -> Option<CompositeInvocationId> {
    let raw = line.strip_prefix("OULIPOLY_INVOCATION=")?;
    CompositeInvocationId::parse_env_value(raw).ok()
}

fn mark_captured_child_seen(
    seen: &mut std::collections::HashSet<(String, String)>,
    composite_id: &CompositeInvocationId,
) -> bool {
    seen.insert((composite_id.source.clone(), composite_id.id.clone()))
}

#[cfg(unix)]
struct InteractiveSignalGuard {
    handle: SignalsHandle,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl InteractiveSignalGuard {
    fn install(child: &mut Child) -> Result<Self, String> {
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])
            .map_err(|e| format!("Failed to install signal handlers: {e}"))?;
        let handle = signals.handle();
        let child_pid = child.id() as i32;
        let forwarded_sigterm = Arc::new(AtomicBool::new(false));
        let thread_forwarded_sigterm = Arc::clone(&forwarded_sigterm);

        let thread = thread::spawn(move || {
            for signal in signals.forever() {
                if should_forward_interactive_sigterm(signal, &thread_forwarded_sigterm) {
                    send_sigterm(child_pid);
                }
            }
        });

        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

#[cfg(unix)]
fn should_forward_interactive_sigterm(signal: i32, forwarded_sigterm: &AtomicBool) -> bool {
    signal == SIGTERM && !forwarded_sigterm.swap(true, Ordering::SeqCst)
}

#[cfg(unix)]
impl Drop for InteractiveSignalGuard {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
fn send_sigterm(pid: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    // Best-effort forward so the parent survives long enough to reap/finalize.
    let _ = unsafe { kill(pid, SIGTERM) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::terminal_signal::{
        TerminalSignal, TerminalSignalEvidence, TerminalSignalKind, TerminalSignalRecognizer,
        TerminalStatusEvidence,
    };
    use crate::executor::{SessionCaptureMethod, SessionCaptureResult};
    use oulipoly_agent_messenger::{ReturnedArtifactRef, ReturnedArtifactSource, StoreAddress};
    use oulipoly_config::{
        ClaudeRestrictions, CodexRestrictions, InputDef, InputType, InvocationMode, ProviderConfig,
        ResumeKind, ResumeStrategy, SessionCapture, SessionCaptureKind, ToolRestrictionKind,
        ToolRestrictions,
    };
    use oulipoly_state::CompositeInvocationId;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant, SystemTime};
    use uuid::Uuid;

    struct FixtureScript {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    fn fixture_script(body: &str) -> FixtureScript {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture-provider.sh");
        std::fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        FixtureScript { _dir: dir, path }
    }

    fn read_argv_dump(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let start_idx = source
            .find(start)
            .unwrap_or_else(|| panic!("missing {start}"));
        let after_start = &source[start_idx..];
        let end_idx = after_start
            .find(end)
            .unwrap_or_else(|| panic!("missing {end} after {start}"));
        &after_start[..end_idx]
    }

    fn age141_source() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/executor/cli.rs");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
    }

    fn age141_bounded_silence_config(silence_ceiling: Duration) -> BoundedSilenceConfig {
        BoundedSilenceConfig {
            silence_ceiling,
            prompt_mode: PromptMode::Arg,
            prompt_payload: None,
            recognizer: ProviderRecognizer::Claude,
        }
    }

    fn age141_model_for_provider(provider: ProviderConfig, prompt_mode: PromptMode) -> ModelConfig {
        ModelConfig {
            name: "age-141-fixture-model".to_string(),
            prompt_mode,
            providers: vec![provider],
            inputs: Vec::new(),
        }
    }

    fn age141_provider(script: &FixtureScript) -> ProviderConfig {
        ProviderConfig {
            name: "claude".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: Vec::new(),
            interactive_args: Some(Vec::new()),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }
    }

    #[cfg(unix)]
    fn age141_execute_script_with_config(
        script: &FixtureScript,
        config: BoundedSilenceConfig,
    ) -> ExecutionResult {
        let provider = age141_provider(script);
        let model = age141_model_for_provider(provider.clone(), PromptMode::Arg);
        let extra_inputs = HashMap::new();

        execute_effective_with_bounded_silence_config(
            EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "age-141 prompt",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: None,
            },
            None,
            config,
        )
        .unwrap()
    }

    fn age141_signal(
        signal: &Option<TerminalSignal>,
        expected_kind: TerminalSignalKind,
    ) -> &TerminalSignal {
        let signal = signal
            .as_ref()
            .expect("AGE-141 paths must carry TerminalSignal evidence");
        assert_eq!(signal.kind, expected_kind);
        signal
    }

    fn age141_receipt_json(invocation_uuid: Uuid, name: &str, version: u64) -> String {
        let reference = ReturnedArtifactRef {
            version_id: format!("store://return/{invocation_uuid}/{name}/{version}"),
            name: name.to_string(),
            store_address: StoreAddress {
                workflow_run_id: format!("return:{invocation_uuid}"),
                artifact_name: name.to_string(),
                version,
            },
            sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            content_len: 3,
            format_hint: Some("text/plain".to_string()),
            verdict_line: Some("APPROVED: age-141 fixture".to_string()),
            source: ReturnedArtifactSource::InlineBytes,
            producer_invocation_uuid: invocation_uuid,
            returned_at: chrono::Utc::now(),
        };
        serde_json::to_string(&reference).expect("receipt json")
    }

    fn age141_parent_env(invocation_uuid: Uuid) -> String {
        format!(r#"{{"source":"age-141-test","id":"{invocation_uuid}"}}"#)
    }

    #[cfg(unix)]
    #[test]
    fn t01_bounded_silence_headless_kill_reap() {
        let script = fixture_script("sleep 9999");
        let started = Instant::now();

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(120)),
        );

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "test ceiling must be injectable; elapsed={:?}",
            started.elapsed()
        );
        assert_ne!(result.exit_code, 0);
        assert_eq!(result.terminal_reason.as_deref(), Some("bounded_silence"));
        let signal = age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::ProlongedSilence,
        );
        assert!(
            signal.evidence.contains("silence") || signal.evidence.contains("bounded"),
            "{signal:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn t02_bounded_silence_stdout_heartbeat_resets_clock() {
        let script = fixture_script(
            r#"printf 'ping-1\n'
sleep 0.08
printf 'ping-2\n'
sleep 9999"#,
        );
        let started = Instant::now();

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(120)),
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed >= Duration::from_millis(170),
            "silence fired from spawn time instead of after stdout heartbeat; elapsed={elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "bounded-silence fixture should not wait for production ceiling; elapsed={elapsed:?}"
        );
        assert_eq!(result.stdout, b"ping-1\nping-2\n".to_vec());
        age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::ProlongedSilence,
        );
    }

    #[cfg(unix)]
    #[test]
    fn t03_bounded_silence_stderr_heartbeat_resets_clock() {
        let script = fixture_script(
            r#"printf 'warn-1\n' >&2
sleep 0.08
printf 'warn-2\n' >&2
sleep 9999"#,
        );
        let started = Instant::now();

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(120)),
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed >= Duration::from_millis(170),
            "silence fired from spawn time instead of after stderr heartbeat; elapsed={elapsed:?}"
        );
        assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
        assert_eq!(result.stderr, "warn-1\nwarn-2\n");
        age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::ProlongedSilence,
        );
    }

    #[cfg(unix)]
    #[test]
    fn t04_bounded_silence_resume_inherits_headless_supervisor() {
        let script = fixture_script("sleep 9999");
        let provider = age141_provider(&script);
        let strategy = ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        };

        let result = execute_resume_with_bounded_silence_config(
            &provider,
            0,
            PromptMode::Arg,
            "resume prompt",
            None,
            None,
            ResumePayload {
                session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
                strategy: &strategy,
            },
            age141_bounded_silence_config(Duration::from_millis(120)),
        )
        .unwrap();

        assert_ne!(result.exit_code, 0);
        assert_eq!(result.terminal_reason.as_deref(), Some("bounded_silence"));
        age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::ProlongedSilence,
        );
        let acceptance = result
            .resume_acceptance
            .expect("resume path must still classify acceptance after silence");
        assert!(matches!(
            acceptance.status,
            ResumeAcceptanceStatus::Rejected
        ));
        assert!(
            acceptance
                .evidence
                .as_deref()
                .unwrap_or_default()
                .contains("child exited"),
            "{acceptance:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn t05_interactive_silent_child_does_not_use_headless_helper() {
        let source = age141_source();
        let interactive_start = source
            .find("pub fn execute_interactive_with_result")
            .expect("interactive function exists");
        let after_start = &source[interactive_start..];
        let interactive_end = after_start
            .find("\npub fn start_known_provider_session_id")
            .expect("next public function after interactive exists");
        let interactive_body = &after_start[..interactive_end];
        assert!(interactive_body.contains("Stdio::inherit()"));
        assert!(interactive_body.contains(".wait()"));
        assert!(!interactive_body.contains("execute_with_bounded_silence"));
        assert!(!interactive_body.contains("BoundedSilenceConfig"));
        assert!(!interactive_body.contains("try_wait"));

        let script = fixture_script("exit 0");
        let provider = age141_provider(&script);
        let result = execute_interactive_with_result(&provider, None, None, None).unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.terminal_reason, None);
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }

    #[cfg(unix)]
    #[test]
    fn t06_repl_interactive_posture_has_no_idle_timeout() {
        let script = fixture_script("sleep 0.2\nexit 0");
        let provider = age141_provider(&script);
        let started = Instant::now();

        let exit_code = execute_interactive(&provider, None, None, None).unwrap();

        assert_eq!(exit_code, 0);
        assert!(
            started.elapsed() >= Duration::from_millis(180),
            "interactive path must preserve child wait posture rather than applying headless idle timeout"
        );
    }

    #[cfg(unix)]
    #[test]
    fn t07_terminal_signal_clean_exit() {
        let script = fixture_script("exit 0");

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(120)),
        );

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.terminal_reason, None);
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }

    #[cfg(unix)]
    #[test]
    fn t08_terminal_signal_nonzero_exit() {
        let script = fixture_script("exit 42");

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(120)),
        );

        assert_eq!(result.exit_code, 42);
        assert_eq!(result.terminal_reason.as_deref(), Some("exit_nonzero"));
        age141_signal(&result.terminal_signal, TerminalSignalKind::NonzeroExit);
    }

    #[cfg(unix)]
    #[test]
    fn t09_terminal_signal_unix_signal_exit() {
        let script = fixture_script("kill -TERM $$");

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(500)),
        );

        assert_eq!(result.exit_code, 143);
        assert_eq!(result.terminal_reason.as_deref(), Some("signal:SIGTERM"));
        age141_signal(&result.terminal_signal, TerminalSignalKind::SignalExit);
    }

    #[cfg(unix)]
    #[test]
    fn t10_terminal_signal_spawn_error_preserves_public_error() {
        let provider = ProviderConfig {
            name: "claude".to_string(),
            command: "/definitely/not/a/real/age-141-command".to_string(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let model = age141_model_for_provider(provider.clone(), PromptMode::Arg);
        let extra_inputs = HashMap::new();

        let err = execute_effective_with_bounded_silence_config(
            EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "prompt",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: None,
            },
            None,
            age141_bounded_silence_config(Duration::from_millis(120)),
        )
        .unwrap_err();

        assert!(err.contains("Failed to spawn"), "{err}");
        let signal = terminal_signal_for_spawn_error("claude", &err);
        assert_eq!(signal.kind, TerminalSignalKind::SpawnError);
        assert_eq!(signal.provider_name, "claude");
    }

    #[cfg(unix)]
    #[test]
    fn t11_inband_quota_recognized_live_before_silence() {
        let script = fixture_script(
            r#"printf 'Claude usage limit reached; resets at 2026-05-18T10:00:00Z\n' >&2
sleep 9999"#,
        );
        let ceiling = Duration::from_millis(900);
        let started = Instant::now();

        let result =
            age141_execute_script_with_config(&script, age141_bounded_silence_config(ceiling));

        assert!(
            started.elapsed() < ceiling,
            "live quota recognition must terminate before silence ceiling; elapsed={:?}",
            started.elapsed()
        );
        assert_eq!(
            result.terminal_reason.as_deref(),
            Some("quota_exhausted_inband")
        );
        age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::QuotaExhaustedInband,
        );
        let source = age141_source();
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(&source);
        assert!(
            !production_source.contains("Claude usage limit reached"),
            "cli.rs must not add provider quota string vocabulary; AGE-139 recognizers own it"
        );
    }

    #[cfg(unix)]
    #[test]
    fn t12_quota_precedence_over_clean_and_nonzero_exit() {
        for (exit_code, body) in [
            (
                0,
                "printf 'Claude usage limit reached; resets at 2026-05-18T10:00:00Z\\n'\nexit 0",
            ),
            (
                1,
                "printf 'Claude usage limit reached; resets at 2026-05-18T10:00:00Z\\n' >&2\nexit 1",
            ),
        ] {
            let script = fixture_script(body);

            let result = age141_execute_script_with_config(
                &script,
                age141_bounded_silence_config(Duration::from_millis(500)),
            );

            assert_eq!(
                result.exit_code, exit_code,
                "fixture should still expose the natural exit code"
            );
            assert_eq!(
                result.terminal_reason.as_deref(),
                Some("quota_exhausted_inband")
            );
            age141_signal(
                &result.terminal_signal,
                TerminalSignalKind::QuotaExhaustedInband,
            );
        }
    }

    #[test]
    fn t13_silence_precedence_over_quota_looking_output() {
        let evidence = TerminalSignalEvidence {
            provider_name: "claude",
            stdout: b"",
            stderr: b"Claude usage limit reached; resets at 2026-05-18T10:00:00Z",
            terminal_status: TerminalStatusEvidence::ProlongedSilence {
                reason: "bounded_silence".to_string(),
            },
            observed_at: SystemTime::now(),
        };

        let signal = crate::executor::providers::claude::Recognizer.recognize(&evidence);

        assert_eq!(signal.kind, TerminalSignalKind::ProlongedSilence);
    }

    #[cfg(unix)]
    #[test]
    fn t14_binary_stdout_preserved_under_bounded_supervisor() {
        let script = fixture_script("printf 'raw\\000\\377Z'");

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(120)),
        );

        assert_eq!(result.stdout, b"raw\0\xffZ".to_vec());
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }

    #[cfg(unix)]
    #[test]
    fn t15_return_channel_cleanup_after_silence() {
        let invocation = Uuid::new_v4();
        let receipt = age141_receipt_json(invocation, "silence.md", 1).replace('\'', r#"'\''"#);
        let dir = tempfile::tempdir().unwrap();
        let observed_channel = dir.path().join("observed-channel.txt");
        let script = fixture_script(&format!(
            r#"test -n "${{OULIPOLY_RETURN_CHANNEL:-}}"
printf '%s' "$OULIPOLY_RETURN_CHANNEL" > "{observed_channel}"
printf '%s\n' '{receipt}' >> "$OULIPOLY_RETURN_CHANNEL"
sleep 9999"#,
            observed_channel = observed_channel.display()
        ));
        let provider = age141_provider(&script);
        let model = age141_model_for_provider(provider.clone(), PromptMode::Arg);
        let extra_inputs = HashMap::new();

        let result = execute_effective_with_bounded_silence_config(
            EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "prompt",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: Some(&age141_parent_env(invocation)),
            },
            None,
            age141_bounded_silence_config(Duration::from_millis(120)),
        )
        .unwrap();

        age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::ProlongedSilence,
        );
        assert_eq!(result.returned_artifacts.len(), 1);
        assert_eq!(result.returned_artifacts[0].name, "silence.md");
        let channel_path = PathBuf::from(std::fs::read_to_string(observed_channel).unwrap());
        assert!(
            !channel_path.exists(),
            "return-channel sidecar must be cleaned after silence kill+reap"
        );
    }

    #[cfg(unix)]
    #[test]
    fn t16_child_marker_parsing_after_silence() {
        let marker = CompositeInvocationId {
            source: "age-141-child".to_string(),
            id: "11111111-1111-4111-8111-111111111111".to_string(),
        };
        let marker_line = marker.stderr_line();
        let script = fixture_script(&format!(
            r#"printf '%s\n' '{marker_line}' >&2
sleep 9999"#
        ));

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(Duration::from_millis(120)),
        );

        age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::ProlongedSilence,
        );
        assert_eq!(result.captured_child_invocations.len(), 1);
        assert_eq!(result.captured_child_invocations[0].composite_id, marker);
        assert_eq!(
            result.captured_child_invocations[0].raw_marker_line,
            marker_line
        );
    }

    #[cfg(unix)]
    #[test]
    fn t17_session_capture_normal_drain_carries_clean_signal() {
        let script = fixture_script(
            r#"requested=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id)
      requested="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$requested""#,
        );
        let mut provider = age141_provider(&script);
        provider.args = vec!["-p".to_string()];
        provider.session_capture = Some(SessionCapture {
            kind: SessionCaptureKind::ForcedFlagVerified,
            flag: Some("--session-id".to_string()),
            readback_args: Some(vec!["--verbose".to_string()]),
            event_type: None,
            event_id_path: None,
            json_flag: None,
            last_message_flag: None,
        });
        let model = age141_model_for_provider(provider.clone(), PromptMode::Arg);
        let extra_inputs = HashMap::new();

        let result = execute_effective_with_bounded_silence_config(
            EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "prompt",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: None,
            },
            None,
            age141_bounded_silence_config(Duration::from_millis(500)),
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(matches!(
            result.session_capture.method,
            SessionCaptureMethod::ForcedFlagVerified
        ));
        assert!(result.session_capture.session_id.is_some());
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }

    #[test]
    fn t18_declared_roles_header_source_guard() {
        let source = age141_source();
        let header = source
            .find("## Declared roles")
            .map(|idx| &source[..idx + 512])
            .expect("cli.rs must declare a top-of-file ## Declared roles module doc header");

        for role in [
            "orchestration",
            "mapper",
            "parser",
            "validator",
            "formatter",
        ] {
            assert!(
                header.contains(role),
                "declared roles header must contain role token {role:?}; header was:\n{header}"
            );
        }
        assert!(
            source[..source.find("use ").unwrap_or(source.len())].contains("## Declared roles"),
            "declared roles must be in the module doc-comment header before imports"
        );
    }

    #[test]
    fn t19_declared_coupling_carriers_source_guard() {
        let source = age141_source();
        let start = source
            .find("## Declared coupling carriers")
            .expect("cli.rs must declare a top-of-file ## Declared coupling carriers block");
        let header_end = source.find("use ").unwrap_or(source.len());
        assert!(
            start < header_end,
            "declared coupling carriers must live in the module doc-comment header before imports"
        );
        let block = &source[start..header_end];
        for carrier in [
            "InputType",
            "ModelConfig",
            "PromptMode",
            "ProviderConfig",
            "ResumeKind",
            "ResumeStrategy",
            "SessionCapture",
            "SessionCaptureKind",
            "ToolRestrictionKind",
            "oulipoly_config::InputDef",
            "oulipoly_config::ResumeAcceptanceRules",
            "std::process::Child",
            "std::process::Command",
            "std::process::ExitStatus",
            "std::process::Stdio",
            "std::io::Write",
            "std::path::Path",
            "std::path::PathBuf",
            "std::sync::Arc",
            "std::sync::atomic::AtomicBool",
            "std::sync::atomic::Ordering",
            "std::thread",
            "signal_hook::consts::signal::SIGHUP",
            "signal_hook::consts::signal::SIGINT",
            "signal_hook::consts::signal::SIGTERM",
            "signal_hook::iterator::SignalsHandle",
            "signal_hook::iterator::Signals",
            "libc",
            "std::os::unix::process::ExitStatusExt",
        ] {
            assert!(
                block.contains(carrier),
                "declared coupling-carrier block must contain literal token {carrier:?}; block was:\n{block}"
            );
        }
    }

    #[test]
    fn t20_existing_inline_regression_suite_contract_sentinel() {
        assert_eq!(
            shell_split(r#"env -u FOO "my provider""#),
            vec!["env", "-u", "FOO", "my provider"]
        );
        assert_eq!(provider_name("env -u CLAUDECODE claude"), "claude");

        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;

            let success = std::process::ExitStatus::from_raw(0);
            let nonzero = std::process::ExitStatus::from_raw(42 << 8);
            let sigterm = std::process::ExitStatus::from_raw(15);

            assert_eq!(classify_terminal_reason(&success), None);
            assert_eq!(
                classify_terminal_reason(&nonzero).as_deref(),
                Some("exit_nonzero")
            );
            assert_eq!(
                classify_terminal_reason(&sigterm).as_deref(),
                Some("signal:SIGTERM")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn t21_bounded_silence_killpg_handles_stubborn_grandchild() {
        let script = fixture_script(
            r#"trap '' SIGTERM
(
  trap '' SIGTERM
  printf 't21-grandchild-stdout\n'
  printf 't21-grandchild-stderr\n' >&2
  sleep 9999
) &
sleep 9999"#,
        );
        let silence_ceiling = Duration::from_millis(120);
        let started = Instant::now();

        let result = age141_execute_script_with_config(
            &script,
            age141_bounded_silence_config(silence_ceiling),
        );
        let elapsed = started.elapsed();

        assert!(
            elapsed < silence_ceiling + TERMINATE_GRACE_PERIOD + Duration::from_millis(700),
            "stubborn descendant kept inherited pipes open after hard-kill fallback; elapsed={elapsed:?}"
        );
        assert_ne!(result.exit_code, 0);
        assert_eq!(result.terminal_reason.as_deref(), Some("bounded_silence"));
        age141_signal(
            &result.terminal_signal,
            TerminalSignalKind::ProlongedSilence,
        );
        assert!(
            result
                .stdout
                .windows(b"t21-grandchild-stdout\n".len())
                .any(|chunk| chunk == b"t21-grandchild-stdout\n"),
            "stdout drain did not preserve the stubborn descendant output: {:?}",
            result.stdout
        );
        assert!(
            result.stderr.contains("t21-grandchild-stderr\n"),
            "stderr drain did not preserve the stubborn descendant output: {:?}",
            result.stderr
        );
    }

    fn age28_claude_provider(script_path: &Path, args: Vec<String>) -> ProviderConfig {
        ProviderConfig {
            name: "claude".to_string(),
            command: script_path.to_string_lossy().into_owned(),
            args,
            interactive_args: Some(vec![
                "--interactive-root".to_string(),
                "--model".to_string(),
                "opus".to_string(),
            ]),
            resume: None,
            session_capture: Some(SessionCapture {
                kind: SessionCaptureKind::ForcedFlagVerified,
                flag: Some("--session-id".to_string()),
                readback_args: None,
                event_type: None,
                event_id_path: None,
                json_flag: None,
                last_message_flag: None,
            }),
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: Some("AGE-28 Claude override".to_string()),
            tool_restrictions: Some(ToolRestrictions {
                kind: ToolRestrictionKind::Claude,
                claude: ClaudeRestrictions {
                    disallowed_tools: vec!["Task".to_string(), "Task tool".to_string()],
                    allowed_tools: Vec::new(),
                    disable_slash_commands: false,
                },
                codex: CodexRestrictions::default(),
            }),
            invocation_mode: Default::default(),
        }
    }

    fn age28_codex_provider(script_path: &Path, args: Vec<String>) -> ProviderConfig {
        ProviderConfig {
            name: "codex".to_string(),
            command: script_path.to_string_lossy().into_owned(),
            args,
            interactive_args: Some(vec![
                "exec".to_string(),
                "-m".to_string(),
                "gpt-5.5".to_string(),
            ]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: Some("AGE-28 Codex override".to_string()),
            tool_restrictions: Some(ToolRestrictions {
                kind: ToolRestrictionKind::Codex,
                claude: ClaudeRestrictions::default(),
                codex: CodexRestrictions {
                    config_pairs: vec!["sandbox=workspace-write".to_string()],
                    disabled_features: vec!["web_search".to_string()],
                },
            }),
            invocation_mode: Default::default(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn execute_effective_uses_explicit_provider_and_prompt_mode() {
        let dir = tempfile::tempdir().unwrap();
        let argv_dump = dir.path().join("argv.txt");
        let stdin_dump = dir.path().join("stdin.txt");
        let script_path = dir.path().join("effective-provider.sh");
        std::fs::write(
            &script_path,
            format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "{argv_dump}"
cat > "{stdin_dump}"
printf 'effective stdout\n'
"#,
                argv_dump = argv_dump.display(),
                stdin_dump = stdin_dump.display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        let model = ModelConfig {
            name: "image-model".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider(
                "effective-provider",
                vec!["--raw-model-arg".to_string()],
            )],
            inputs: vec![
                InputDef {
                    name: "size".to_string(),
                    input_type: InputType::Enum {
                        options: vec!["small".to_string(), "large".to_string()],
                    },
                    required: false,
                    default_input: false,
                    default: None,
                    description: None,
                    flag: Some("--size".to_string()),
                },
                InputDef {
                    name: "quality".to_string(),
                    input_type: InputType::String,
                    required: false,
                    default_input: false,
                    default: Some(toml::Value::String("standard".to_string())),
                    description: None,
                    flag: Some("--quality".to_string()),
                },
            ],
        };
        assert_eq!(model.providers[0].command, "");
        let provider = ProviderConfig {
            name: "effective-provider".to_string(),
            command: script_path.to_string_lossy().into_owned(),
            args: vec!["--provider".to_string()],
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let mut extra_inputs = HashMap::new();
        extra_inputs.insert("size".to_string(), vec!["large".to_string()]);

        let result = execute_effective(EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "prompt routed as argv",
            working_dir: Some(dir.path()),
            extra_inputs: &extra_inputs,
            parent_invocation_env: None,
        })
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "effective stdout\n"
        );
        assert_eq!(result.provider_index, 0);
        assert_eq!(std::fs::read_to_string(&stdin_dump).unwrap(), "");
        let argv = std::fs::read_to_string(&argv_dump).unwrap();
        assert!(argv.contains("--provider\n"), "{argv}");
        assert!(
            !argv.contains("--raw-model-arg"),
            "raw model-provider args must not be used without effective merge: {argv}"
        );
        assert!(argv.contains("--size\nlarge\n"), "{argv}");
        assert!(argv.contains("--quality\nstandard\n"), "{argv}");
        assert!(argv.ends_with("prompt routed as argv\n"), "{argv}");
    }

    #[cfg(unix)]
    #[test]
    fn claude_oneshot_renders_append_system_prompt_once_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let argv_dump = dir.path().join("argv.txt");
        let stdin_dump = dir.path().join("stdin.txt");
        let script = fixture_script(&format!(
            r#"requested=""
printf '%s\n' "$@" > "{argv_dump}"
cat > "{stdin_dump}"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id)
      requested="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$requested""#,
            argv_dump = argv_dump.display(),
            stdin_dump = stdin_dump.display()
        ));
        let provider = age28_claude_provider(
            &script.path,
            vec!["-p".to_string(), "--model".to_string(), "opus".to_string()],
        );
        let model = ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider("claude", Vec::new())],
            inputs: vec![InputDef {
                name: "tone".to_string(),
                input_type: InputType::String,
                required: false,
                default_input: false,
                default: None,
                description: None,
                flag: Some("--tone".to_string()),
            }],
        };
        let mut extra_inputs = HashMap::new();
        extra_inputs.insert("tone".to_string(), vec!["terse".to_string()]);
        let request = EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Stdin,
            prompt: "user prompt",
            working_dir: Some(dir.path()),
            extra_inputs: &extra_inputs,
            parent_invocation_env: None,
        };

        execute_effective_with_start_known_provider_session_id(
            request,
            Some("11111111-2222-4333-8444-555555555555"),
        )
        .unwrap();

        let argv = read_argv_dump(&argv_dump);
        assert_eq!(
            argv,
            vec![
                "-p",
                "--model",
                "opus",
                "--append-system-prompt",
                "AGE-28 Claude override",
                "--disallowed-tools",
                "Task,Task tool",
                "--tone",
                "terse",
                "--session-id",
                "11111111-2222-4333-8444-555555555555",
            ]
        );
        assert_eq!(
            argv.iter()
                .filter(|arg| arg.as_str() == "--append-system-prompt")
                .count(),
            1
        );
        assert_eq!(std::fs::read_to_string(stdin_dump).unwrap(), "user prompt");
    }

    #[cfg(unix)]
    #[test]
    fn claude_oneshot_renders_disallowed_tools_comma_joined() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let mut provider = age28_claude_provider(&script.path, vec!["-p".to_string()]);
        provider.session_capture = None;
        let model = ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider("claude", Vec::new())],
            inputs: vec![],
        };

        execute_effective(EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "hello",
            working_dir: None,
            extra_inputs: &HashMap::new(),
            parent_invocation_env: None,
        })
        .unwrap();

        assert_eq!(
            read_argv_dump(&argv_dump_path),
            vec![
                "-p",
                "--append-system-prompt",
                "AGE-28 Claude override",
                "--disallowed-tools",
                "Task,Task tool",
                "hello",
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_resume_renders_policy_before_resume_tail() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = age28_claude_provider(
            &script.path,
            vec!["-p".to_string(), "--model".to_string(), "opus".to_string()],
        );
        let strategy = ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        };

        execute_resume(
            &provider,
            0,
            PromptMode::Arg,
            "continue work",
            None,
            None,
            ResumePayload {
                session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
                strategy: &strategy,
            },
        )
        .unwrap();

        assert_eq!(
            read_argv_dump(&argv_dump_path),
            vec![
                "-p",
                "--model",
                "opus",
                "--append-system-prompt",
                "AGE-28 Claude override",
                "--disallowed-tools",
                "Task,Task tool",
                "--resume",
                "5169694d-de0f-40d1-890c-6e28e55bab27",
                "continue work",
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_interactive_renders_policy_before_resume_tail() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = age28_claude_provider(&script.path, vec!["one-shot-only".to_string()]);
        let strategy = ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        };

        execute_interactive(
            &provider,
            None,
            None,
            Some(ResumePayload {
                session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
                strategy: &strategy,
            }),
        )
        .unwrap();

        assert_eq!(
            read_argv_dump(&argv_dump_path),
            vec![
                "--interactive-root",
                "--model",
                "opus",
                "--append-system-prompt",
                "AGE-28 Claude override",
                "--disallowed-tools",
                "Task,Task tool",
                "--resume",
                "5169694d-de0f-40d1-890c-6e28e55bab27",
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn codex_oneshot_prepends_policy_block_to_arg_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let argv_dump = dir.path().join("argv.txt");
        let prompt_dump = dir.path().join("prompt.txt");
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{argv_dump}"
last="${{@: -1}}"
printf '%s' "$last" > "{prompt_dump}""#,
            argv_dump = argv_dump.display(),
            prompt_dump = prompt_dump.display()
        ));
        let provider = age28_codex_provider(
            &script.path,
            vec!["exec".to_string(), "-m".to_string(), "gpt-5.5".to_string()],
        );
        let model = ModelConfig {
            name: "gpt-high".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider("codex", Vec::new())],
            inputs: vec![],
        };

        execute_effective(EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "user codex prompt",
            working_dir: Some(dir.path()),
            extra_inputs: &HashMap::new(),
            parent_invocation_env: None,
        })
        .unwrap();

        let argv = read_argv_dump(&argv_dump);
        assert!(!argv.iter().any(|arg| arg == "--append-system-prompt"));
        assert!(!argv.iter().any(|arg| arg == "--disallowed-tools"));
        let prompt = std::fs::read_to_string(prompt_dump).unwrap();
        let policy_index = prompt.find("AGE-28 Codex override").unwrap();
        let user_index = prompt.find("user codex prompt").unwrap();
        assert!(policy_index < user_index, "{prompt}");
        assert!(!prompt.starts_with("user codex prompt"), "{prompt}");
    }

    #[cfg(unix)]
    #[test]
    fn codex_promptless_interactive_with_override_fails_closed() {
        let script = fixture_script("exit 99");
        let provider = age28_codex_provider(
            &script.path,
            vec!["exec".to_string(), "-m".to_string(), "gpt-5.5".to_string()],
        );

        let err = execute_interactive(&provider, None, None, None).unwrap_err();

        assert!(
            err.contains("Codex system_prompt_override")
                && err.contains("no prompt to prepend it to"),
            "{err}"
        );
    }

    #[test]
    fn policy_with_command_path_basename_still_infers_provider_kind() {
        let provider = ProviderConfig {
            name: "primary".to_string(),
            command: "/opt/bin/claude".to_string(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: Some("AGE-28 override".to_string()),
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let mut args = Vec::new();
        let mut prompt = Some("user prompt".to_string());

        apply_provider_policy(&provider, &mut args, &mut prompt).unwrap();

        assert_eq!(
            args,
            vec![
                "--append-system-prompt".to_string(),
                "AGE-28 override".to_string()
            ]
        );
        assert_eq!(prompt.as_deref(), Some("user prompt"));
    }

    #[test]
    fn policy_with_split_env_command_args_still_infers_provider_kind() {
        let provider = ProviderConfig {
            name: "primary".to_string(),
            command: "env".to_string(),
            args: vec![
                "-u".to_string(),
                "CLAUDECODE".to_string(),
                "claude".to_string(),
                "-p".to_string(),
            ],
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: Some("AGE-28 override".to_string()),
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let mut args = provider.args.clone();
        let mut prompt = Some("user prompt".to_string());

        apply_provider_policy(&provider, &mut args, &mut prompt).unwrap();

        assert!(
            args.contains(&"--append-system-prompt".to_string()),
            "{args:?}"
        );
        assert!(args.contains(&"AGE-28 override".to_string()), "{args:?}");
    }

    #[test]
    fn policy_with_unknown_provider_kind_fails_closed() {
        let provider = ProviderConfig {
            name: "primary".to_string(),
            command: "custom-ai".to_string(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: Some("AGE-28 override".to_string()),
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let mut args = Vec::new();
        let mut prompt = Some("user prompt".to_string());

        let err = apply_provider_policy(&provider, &mut args, &mut prompt).unwrap_err();

        assert!(err.contains("ecosystem could not be inferred"), "{err}");
    }

    #[test]
    fn policy_with_kind_payload_mismatch_fails_closed() {
        let provider = ProviderConfig {
            name: "claude".to_string(),
            command: "claude".to_string(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: Some(ToolRestrictions {
                kind: ToolRestrictionKind::Claude,
                claude: ClaudeRestrictions::default(),
                codex: CodexRestrictions {
                    config_pairs: Vec::new(),
                    disabled_features: vec!["web_search".to_string()],
                },
            }),
            invocation_mode: Default::default(),
        };
        let mut args = Vec::new();
        let mut prompt = Some("user prompt".to_string());

        let err = apply_provider_policy(&provider, &mut args, &mut prompt).unwrap_err();

        assert!(err.contains("declares Claude restrictions"), "{err}");
        assert!(err.contains("populated Codex settings"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn codex_largeprompt_tempfile_prepends_policy_block() {
        let dir = tempfile::tempdir().unwrap();
        let argv_dump = dir.path().join("argv.txt");
        let prompt_dump = dir.path().join("prompt.txt");
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{argv_dump}"
last="${{@: -1}}"
file="${{last#Follow the instructions in }}"
cat "$file" > "{prompt_dump}""#,
            argv_dump = argv_dump.display(),
            prompt_dump = prompt_dump.display()
        ));
        let provider = age28_codex_provider(
            &script.path,
            vec!["exec".to_string(), "-m".to_string(), "gpt-5.5".to_string()],
        );
        let model = ModelConfig {
            name: "gpt-high".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider("codex", Vec::new())],
            inputs: vec![],
        };
        let prompt = format!(
            "user codex prompt\n{}",
            "x".repeat(LARGE_PROMPT_THRESHOLD + 1)
        );

        execute_effective(EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: &prompt,
            working_dir: Some(dir.path()),
            extra_inputs: &HashMap::new(),
            parent_invocation_env: None,
        })
        .unwrap();

        let argv = read_argv_dump(&argv_dump);
        assert!(
            argv.last()
                .unwrap()
                .starts_with("Follow the instructions in ")
        );
        let captured_prompt = std::fs::read_to_string(prompt_dump).unwrap();
        let policy_index = captured_prompt.find("AGE-28 Codex override").unwrap();
        let user_index = captured_prompt.find("user codex prompt").unwrap();
        assert!(policy_index < user_index, "{captured_prompt}");
    }

    #[cfg(unix)]
    #[test]
    fn codex_oneshot_renders_typed_config_pairs_only() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}"
cat > /dev/null"#,
            dump = argv_dump_path.display()
        ));
        let provider = age28_codex_provider(
            &script.path,
            vec!["exec".to_string(), "-m".to_string(), "gpt-5.5".to_string()],
        );
        let model = ModelConfig {
            name: "gpt-high".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider("codex", Vec::new())],
            inputs: vec![],
        };

        execute_effective(EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Stdin,
            prompt: "stdin prompt",
            working_dir: None,
            extra_inputs: &HashMap::new(),
            parent_invocation_env: None,
        })
        .unwrap();

        let argv = read_argv_dump(&argv_dump_path);
        assert_eq!(
            argv,
            vec![
                "exec",
                "-m",
                "gpt-5.5",
                "-c",
                "sandbox=workspace-write",
                "--disable",
                "web_search",
            ]
        );
        assert!(!argv.iter().any(|arg| arg == "--append-system-prompt"));
        assert!(!argv.iter().any(|arg| arg == "--allowed-tools"));
        assert!(!argv.iter().any(|arg| arg == "--disallowed-tools"));
    }

    #[test]
    fn shell_split_simple() {
        assert_eq!(shell_split("echo hello"), vec!["echo", "hello"]);
    }

    #[test]
    fn shell_split_single() {
        assert_eq!(shell_split("codex"), vec!["codex"]);
    }

    #[test]
    fn shell_split_quoted_token() {
        assert_eq!(
            shell_split(r#"env -u FOO "my cmd""#),
            vec!["env", "-u", "FOO", "my cmd"]
        );
    }

    #[test]
    fn provider_name_simple() {
        assert_eq!(provider_name("claude"), "claude");
    }

    #[test]
    fn provider_name_with_prefix() {
        assert_eq!(provider_name("env -u CLAUDECODE claude"), "claude");
    }

    #[test]
    fn provider_name_quoted() {
        assert_eq!(provider_name(r#"env -u FOO "claude""#), "claude");
    }

    #[test]
    fn provider_name_with_spaces() {
        assert_eq!(provider_name(r#"env -u FOO "my provider""#), "my provider");
    }

    // risk: exhaustive surfaces 1-7 and 11-12; level: unit; source: contract § 5.1, A1, A9, A10
    #[cfg(unix)]
    #[test]
    fn exit_code_from_status_uses_unified_child_process_contract() {
        use std::os::unix::process::ExitStatusExt;

        let success = std::process::ExitStatus::from_raw(0);
        let nonzero = std::process::ExitStatus::from_raw(7 << 8);
        let sigterm = std::process::ExitStatus::from_raw(15);
        let sigint = std::process::ExitStatus::from_raw(2);
        let unknown = std::process::ExitStatus::from_raw(0x7f);

        assert_eq!(exit_code_from_status(&success), 0);
        assert_eq!(exit_code_from_status(&nonzero), 7);
        assert_eq!(exit_code_from_status(&sigterm), 143);
        assert_eq!(exit_code_from_status(&sigint), 130);
        assert_eq!(exit_code_from_status(&unknown), -1);
    }

    // risk: exhaustive surfaces 8-9; level: unit; source: contract § 5.2, A5, A10
    #[cfg(unix)]
    #[test]
    fn t_terminal_reason_classifier_uses_stable_exit_and_signal_vocabulary() {
        use std::os::unix::process::ExitStatusExt;

        let success = std::process::ExitStatus::from_raw(0);
        let nonzero = std::process::ExitStatus::from_raw(7 << 8);
        let sigterm = std::process::ExitStatus::from_raw(15);

        assert_eq!(classify_terminal_reason(&success), None);
        assert_eq!(
            classify_terminal_reason(&nonzero).as_deref(),
            Some("exit_nonzero")
        );
        assert_eq!(
            classify_terminal_reason(&sigterm).as_deref(),
            Some("signal:SIGTERM")
        );
    }

    // RISK: captured child marker parsing could accept malformed or duplicate OULIPOLY_INVOCATION lines and corrupt unrelated rows (proposal §test-intent "supervised-child fence test", assumptions A1/A2/A4)
    // LEVEL: unit
    // SOURCE: contracts/nes-250-contract.md § Process-supervision fence (T-FENCE-SUPERVISOR-CLEAN-CHILD)
    #[test]
    fn t_captured_child_marker_parser_keeps_one_valid_marker_and_drops_noise() {
        let marker = CompositeInvocationId {
            source: "fixture-child".to_string(),
            id: "11111111-1111-1111-1111-111111111111".to_string(),
        };
        let marker_line = marker.stderr_line();
        let stderr = format!(
            "noise\n{}\nOULIPOLY_INVOCATION={{\"source\":\"fixture-child\",\"id\":\"not-a-uuid\"}}\n{}\n",
            marker_line, marker_line
        );

        let captured = captured_child_invocations_from_stderr(&stderr);

        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].composite_id, marker);
        assert_eq!(captured[0].raw_marker_line, marker_line);
    }

    #[test]
    fn execute_interactive_errors_when_provider_has_no_interactive_args() {
        let provider = ProviderConfig {
            name: "fixture-provider".to_string(),
            command: "echo".to_string(),
            args: vec!["one-shot".to_string()],
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };

        let err = execute_interactive(&provider, None, None, None).unwrap_err();
        assert!(
            err.contains("provider fixture-provider has no interactive_args"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_interactive_uses_interactive_args_for_child_argv() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "fixture-provider".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["one-shot-only".to_string()],
            interactive_args: Some(vec!["hello".to_string(), "world".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };

        let exit_code = execute_interactive(&provider, None, None, None).unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&argv_dump_path).unwrap(),
            "hello\nworld\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_interactive_with_result_preserves_invocation_mode() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "fixture-provider".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["one-shot-only".to_string()],
            interactive_args: Some(vec!["hello".to_string(), "world".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Proxy,
        };

        let result = execute_interactive_with_result(&provider, None, None, None).unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&argv_dump_path).unwrap(),
            "hello\nworld\n"
        );

        let source_path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/executor/cli.rs");
        let source = std::fs::read_to_string(source_path).expect("read cli source");
        let body = source_between(
            &source,
            "pub fn execute_interactive_with_result",
            "enum CapturePlan",
        );
        assert!(body.contains("provider.interactive_args"), "{body}");
        assert!(body.contains("apply_provider_policy(provider"), "{body}");
        assert!(
            body.contains("build_command(\n        provider,")
                || body.contains("build_command(provider,"),
            "{body}"
        );
        assert!(
            !body.contains("ProviderConfig {"),
            "interactive execution must not rebuild ProviderConfig in function body"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_interactive_propagates_working_directory() {
        let tempdir = tempfile::tempdir().unwrap();
        let cwd_dump = tempdir.path().join("cwd.txt");
        let script = fixture_script(&format!(r#"pwd > "{dump}""#, dump = cwd_dump.display()));
        let provider = ProviderConfig {
            name: "fixture-provider".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["one-shot-only".to_string()],
            interactive_args: Some(vec!["launch".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };

        let exit_code = execute_interactive(&provider, Some(tempdir.path()), None, None).unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&cwd_dump).unwrap().trim(),
            tempdir.path().display().to_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_interactive_propagates_parent_invocation_env() {
        let env_dump = tempfile::NamedTempFile::new().unwrap();
        let env_dump_path = env_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s' "${{OULIPOLY_PARENT_INVOCATION-}}" > "{dump}""#,
            dump = env_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "fixture-provider".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["one-shot-only".to_string()],
            interactive_args: Some(vec!["launch".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let parent_env =
            r#"{"source":"fixture-provider","id":"11111111-1111-1111-1111-111111111111"}"#;

        let exit_code = execute_interactive(&provider, None, Some(parent_env), None).unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(std::fs::read_to_string(&env_dump_path).unwrap(), parent_env);
    }

    #[cfg(unix)]
    // risk: Executor resume payload/argv without target JSONL path; level: unit; source: proposal §5 / A4.
    #[test]
    fn execute_interactive_appends_flag_resume_args_to_child_argv() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "claude2".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["one-shot-only".to_string()],
            interactive_args: Some(vec!["--model".to_string(), "opus".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let strategy = ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        };
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";

        let exit_code = execute_interactive(
            &provider,
            None,
            None,
            Some(ResumePayload {
                session_id,
                strategy: &strategy,
            }),
        )
        .unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&argv_dump_path).unwrap(),
            "--model\nopus\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\n"
        );
    }

    #[cfg(unix)]
    // risk: Executor resume payload/argv without target JSONL path; level: unit; source: proposal §5 / A4.
    #[test]
    fn execute_resume_appends_flag_resume_args_and_prompt_to_one_shot_args() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "claude2".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["-p".to_string(), "--model".to_string(), "opus".to_string()],
            interactive_args: Some(vec!["--model".to_string(), "opus".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let strategy = ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        };

        let result = execute_resume(
            &provider,
            0,
            PromptMode::Arg,
            "answer text",
            None,
            None,
            ResumePayload {
                session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
                strategy: &strategy,
            },
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&argv_dump_path).unwrap(),
            "-p\n--model\nopus\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\nanswer text\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_resume_suppresses_configured_session_capture() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}"
printf '{{"type":"system","subtype":"init","session_id":"8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22"}}\n'"#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "claude2".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["-p".to_string()],
            interactive_args: Some(vec!["launch".to_string()]),
            resume: None,
            session_capture: Some(SessionCapture {
                kind: SessionCaptureKind::ForcedFlagVerified,
                flag: Some("--session-id".to_string()),
                readback_args: Some(vec![
                    "--verbose".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                ]),
                event_type: None,
                event_id_path: None,
                json_flag: None,
                last_message_flag: None,
            }),
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let strategy = ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        };

        let result = execute_resume(
            &provider,
            0,
            PromptMode::Arg,
            "answer text",
            None,
            None,
            ResumePayload {
                session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
                strategy: &strategy,
            },
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.session_capture.session_id, None);
        assert!(matches!(
            result.session_capture.method,
            SessionCaptureMethod::None
        ));
        let argv = std::fs::read_to_string(&argv_dump_path).unwrap();
        assert_eq!(
            argv,
            "-p\n--resume\n5169694d-de0f-40d1-890c-6e28e55bab27\nanswer text\n"
        );
        assert!(!argv.contains("--session-id"));
        assert!(!argv.contains("--verbose"));
        assert!(String::from_utf8_lossy(&result.stdout).contains("8f0a6a1f"));
    }

    #[cfg(unix)]
    #[test]
    fn execute_resume_preserves_invocation_mode_while_clearing_session_capture() {
        let script = fixture_script(
            r#"printf '{"type":"system","subtype":"init","session_id":"8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22"}\n'"#,
        );
        let provider = ProviderConfig {
            name: "claude2".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["-p".to_string()],
            interactive_args: Some(vec!["launch".to_string()]),
            resume: None,
            session_capture: Some(SessionCapture {
                kind: SessionCaptureKind::ForcedFlagVerified,
                flag: Some("--session-id".to_string()),
                readback_args: Some(vec![
                    "--verbose".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                ]),
                event_type: None,
                event_id_path: None,
                json_flag: None,
                last_message_flag: None,
            }),
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Proxy,
        };
        let strategy = ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        };

        let result = execute_resume(
            &provider,
            0,
            PromptMode::Arg,
            "answer text",
            None,
            None,
            ResumePayload {
                session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
                strategy: &strategy,
            },
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.session_capture.session_id, None);
        assert!(matches!(
            result.session_capture.method,
            SessionCaptureMethod::None
        ));

        let source_path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/executor/cli.rs");
        let source = std::fs::read_to_string(source_path).expect("read cli source");
        let body = source_between(
            &source,
            "pub fn execute_resume",
            "fn classify_resume_acceptance",
        );
        assert!(body.contains("provider.clone()"), "{body}");
        assert!(body.contains(".session_capture = None"), "{body}");
    }

    #[cfg(unix)]
    // risk: Executor resume payload/argv without target JSONL path; level: unit; source: proposal §5 / A4.
    #[test]
    fn execute_resume_appends_subcommand_resume_args_and_prompt_to_one_shot_args() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "codex".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["exec".to_string(), "-m".to_string(), "gpt-5.4".to_string()],
            interactive_args: Some(vec!["-m".to_string(), "gpt-5.4".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let strategy = ResumeStrategy {
            kind: ResumeKind::Subcommand,
            flag: None,
            subcommand: Some(vec!["resume".to_string()]),
        };

        let result = execute_resume(
            &provider,
            0,
            PromptMode::Arg,
            "answer text",
            None,
            None,
            ResumePayload {
                session_id: "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22",
                strategy: &strategy,
            },
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&argv_dump_path).unwrap(),
            "exec\n-m\ngpt-5.4\nresume\n8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22\nanswer text\n"
        );
    }

    #[cfg(unix)]
    // risk: Executor resume payload/argv without target JSONL path; level: unit; source: proposal §5 / A4.
    #[test]
    fn execute_interactive_appends_subcommand_resume_args_to_child_argv() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "codex".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["exec".to_string()],
            interactive_args: Some(vec![
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                "-m".to_string(),
                "gpt-5.4".to_string(),
            ]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let strategy = ResumeStrategy {
            kind: ResumeKind::Subcommand,
            flag: None,
            subcommand: Some(vec!["resume".to_string()]),
        };
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";

        let exit_code = execute_interactive(
            &provider,
            None,
            None,
            Some(ResumePayload {
                session_id,
                strategy: &strategy,
            }),
        )
        .unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&argv_dump_path).unwrap(),
            "--dangerously-bypass-approvals-and-sandbox\n-m\ngpt-5.4\nresume\n5169694d-de0f-40d1-890c-6e28e55bab27\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_interactive_without_resume_preserves_pr_e_argv_shape() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s\n' "$@" > "{dump}""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "fixture-provider".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["one-shot-only".to_string()],
            interactive_args: Some(vec!["launch".to_string(), "interactive".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };

        let exit_code = execute_interactive(&provider, None, None, None).unwrap();

        assert_eq!(exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&argv_dump_path).unwrap(),
            "launch\ninteractive\n"
        );
    }

    #[test]
    fn resolve_flags_with_schema() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![
                InputDef {
                    name: "prompt".to_string(),
                    input_type: InputType::String,
                    required: true,
                    default_input: true,
                    default: None,
                    description: None,
                    flag: None,
                },
                InputDef {
                    name: "size".to_string(),
                    input_type: InputType::Enum {
                        options: vec!["2048*2048".to_string(), "1024*1024".to_string()],
                    },
                    required: false,
                    default_input: false,
                    default: Some(toml::Value::String("2048*2048".to_string())),
                    description: None,
                    flag: Some("--size".to_string()),
                },
            ],
        };
        let mut inputs = HashMap::new();
        inputs.insert("size".to_string(), vec!["1024*1024".to_string()]);
        let flags = resolve_input_flags(&model, &inputs).unwrap();
        assert_eq!(flags, vec!["--size", "1024*1024"]);
    }

    #[test]
    fn resolve_flags_applies_defaults() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![InputDef {
                name: "format".to_string(),
                input_type: InputType::Enum {
                    options: vec!["jpeg".to_string(), "png".to_string()],
                },
                required: false,
                default_input: false,
                default: Some(toml::Value::String("jpeg".to_string())),
                description: None,
                flag: Some("--format".to_string()),
            }],
        };
        let flags = resolve_input_flags(&model, &HashMap::new()).unwrap();
        assert_eq!(flags, vec!["--format", "jpeg"]);
    }

    #[test]
    fn resolve_flags_rejects_invalid_enum() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![InputDef {
                name: "size".to_string(),
                input_type: InputType::Enum {
                    options: vec!["small".to_string(), "large".to_string()],
                },
                required: false,
                default_input: false,
                default: None,
                description: None,
                flag: Some("--size".to_string()),
            }],
        };
        let mut inputs = HashMap::new();
        inputs.insert("size".to_string(), vec!["huge".to_string()]);
        let result = resolve_input_flags(&model, &inputs);
        assert!(result.unwrap_err().contains("not a valid option"));
    }

    #[test]
    fn resolve_flags_rejects_missing_required() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![InputDef {
                name: "image".to_string(),
                input_type: InputType::String,
                required: true,
                default_input: false,
                default: None,
                description: None,
                flag: Some("--image".to_string()),
            }],
        };
        let result = resolve_input_flags(&model, &HashMap::new());
        assert!(result.unwrap_err().contains("Required input 'image'"));
    }

    #[test]
    fn resolve_flags_unknown_passthrough() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("test", vec![])],
            inputs: vec![],
        };
        let mut inputs = HashMap::new();
        inputs.insert("custom".to_string(), vec!["value".to_string()]);
        let flags = resolve_input_flags(&model, &inputs).unwrap();
        assert_eq!(flags, vec!["--custom", "value"]);
    }

    #[cfg(unix)]
    #[test]
    fn execute_echo_arg_mode() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new("echo", vec![])],
            inputs: vec![],
        };
        let result = execute(&model, 0, "hello world", None, &HashMap::new(), None).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&result.stdout).trim(),
            "hello world"
        );
    }

    /// Per contract: when a provider has no `session_capture` config,
    /// execution proceeds normally and the SessionCaptureResult is
    /// `(None, SessionCaptureMethod::None)` — confirming the dispatch
    /// doesn't accidentally engage capture logic.
    #[cfg(unix)]
    #[test]
    fn execute_without_session_capture_returns_none_capture_result() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new("echo", vec![])],
            inputs: vec![],
        };
        let result = execute(&model, 0, "no capture", None, &HashMap::new(), None).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.session_capture.session_id, None);
        assert!(matches!(
            result.session_capture.method,
            SessionCaptureMethod::None
        ));
    }

    #[test]
    fn classify_resume_acceptance_without_rules_uses_no_rules_evidence() {
        let result =
            classify_resume_acceptance(None, 0, b"ok", b"", "5169694d-de0f-40d1-890c-6e28e55bab27");

        assert_eq!(result.status, ResumeAcceptanceStatus::Unconfirmed);
        assert_eq!(
            result.evidence.as_deref(),
            Some("child exited 0 but provider has no resume_acceptance rules configured")
        );
    }

    #[test]
    fn resume_provider_missing_session_classified() {
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let result = classify_resume_acceptance(
            None,
            1,
            b"",
            format!("No conversation found for session {session_id}").as_bytes(),
            session_id,
        );

        assert_eq!(result.status, ResumeAcceptanceStatus::Rejected);
        assert!(
            result
                .evidence
                .as_deref()
                .unwrap_or_default()
                .contains("resume_session_mismatch"),
            "{result:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_cat_stdin_mode() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("cat", vec![])],
            inputs: vec![],
        };
        let result = execute(&model, 0, "piped input", None, &HashMap::new(), None).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, b"piped input");
    }

    #[cfg(unix)]
    #[test]
    fn execute_with_input_flags() {
        let model = ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new("echo", vec![])],
            inputs: vec![InputDef {
                name: "greeting".to_string(),
                input_type: InputType::String,
                required: false,
                default_input: false,
                default: None,
                description: None,
                flag: Some("--greet".to_string()),
            }],
        };
        let mut inputs = HashMap::new();
        inputs.insert("greeting".to_string(), vec!["hello".to_string()]);
        // echo ignores stdin, prints its args
        let result = execute(&model, 0, "", None, &inputs, None).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&result.stdout).trim(),
            "--greet hello"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_claude_forced_flag_verified_happy_path_captures_requested_session_id() {
        // Fixture script also writes its observed argv to a sidecar
        // file so the test can prove the runner injected `--session-id`
        // and the configured `readback_args` into the spawned command.
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"requested=""
# Dump every arg seen by the script so the test can assert injection.
printf '%s\n' "$@" > {dump}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id)
      requested="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$requested""#,
            dump = argv_dump_path.display()
        ));
        let model = ModelConfig {
            name: "claude-test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig {
                name: "claude-fixture".to_string(),
                command: script.path.to_string_lossy().into_owned(),
                args: vec!["-p".to_string()],
                interactive_args: None,
                resume: None,
                session_capture: Some(SessionCapture {
                    kind: SessionCaptureKind::ForcedFlagVerified,
                    flag: Some("--session-id".to_string()),
                    readback_args: Some(vec![
                        "--verbose".to_string(),
                        "--output-format".to_string(),
                        "stream-json".to_string(),
                    ]),
                    event_type: None,
                    event_id_path: None,
                    json_flag: None,
                    last_message_flag: None,
                }),
                resume_acceptance: None,
                session_storage: None,
                system_prompt_override: None,
                tool_restrictions: None,
                invocation_mode: Default::default(),
            }],
            inputs: vec![],
        };

        let result = execute(&model, 0, "hello", None, &HashMap::new(), None).unwrap();
        let capture: &SessionCaptureResult = &result.session_capture;
        let stdout = String::from_utf8_lossy(&result.stdout);

        assert_eq!(result.exit_code, 0);
        assert!(stdout.contains("\"type\":\"system\""));
        assert!(matches!(
            capture.method,
            SessionCaptureMethod::ForcedFlagVerified
        ));

        // Capture must have produced a session id...
        let captured_id = capture
            .session_id
            .as_deref()
            .expect("FFV happy path must populate session_id");

        // ...and that captured id must equal the runner-supplied UUID
        // that the script saw on argv (proves readback verification
        // matched, not just "any session_id surfaced from the CLI").
        let argv_seen = std::fs::read_to_string(&argv_dump_path).unwrap();
        let argv_lines: Vec<&str> = argv_seen.lines().collect();
        let session_id_idx = argv_lines
            .iter()
            .position(|a| *a == "--session-id")
            .expect("--session-id must have been injected onto argv");
        let injected_id = argv_lines
            .get(session_id_idx + 1)
            .expect("--session-id needs a value");
        assert_eq!(
            captured_id, *injected_id,
            "captured id must equal the injected --session-id value"
        );

        // Per contract, readback_args must have been appended onto argv
        // alongside the --session-id flag.
        for required_readback in &["--verbose", "--output-format", "stream-json"] {
            assert!(
                argv_lines.iter().any(|a| a == required_readback),
                "expected readback arg {required_readback:?} on argv, got: {argv_lines:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn forced_flag_start_id_reused() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"requested=""
printf '%s\n' "$@" > {dump}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id)
      requested="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$requested""#,
            dump = argv_dump_path.display()
        ));
        let model = ModelConfig {
            name: "claude-test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig {
                name: "claude-fixture".to_string(),
                command: script.path.to_string_lossy().into_owned(),
                args: vec!["-p".to_string()],
                interactive_args: None,
                resume: None,
                session_capture: Some(SessionCapture {
                    kind: SessionCaptureKind::ForcedFlagVerified,
                    flag: Some("--session-id".to_string()),
                    readback_args: Some(vec!["--verbose".to_string()]),
                    event_type: None,
                    event_id_path: None,
                    json_flag: None,
                    last_message_flag: None,
                }),
                resume_acceptance: None,
                session_storage: None,
                system_prompt_override: None,
                tool_restrictions: None,
                invocation_mode: Default::default(),
            }],
            inputs: vec![],
        };

        let result = execute(&model, 0, "hello", None, &HashMap::new(), None).unwrap();
        let captured_id = result.session_capture.session_id.as_deref().unwrap();
        let argv_seen = std::fs::read_to_string(&argv_dump_path).unwrap();
        let argv_lines: Vec<&str> = argv_seen.lines().collect();
        let injected_id = argv_lines
            .iter()
            .position(|arg| *arg == "--session-id")
            .and_then(|idx| argv_lines.get(idx + 1))
            .expect("--session-id value must be present");

        assert_eq!(captured_id, *injected_id);
    }

    #[cfg(unix)]
    #[test]
    fn execute_effective_with_start_known_provider_session_id_threads_caller_supplied_id() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let argv_dump_path = argv_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"requested=""
printf '%s\n' "$@" > {dump}
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id)
      requested="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$requested""#,
            dump = argv_dump_path.display()
        ));
        let provider = ProviderConfig {
            name: "claude-fixture".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["-p".to_string()],
            interactive_args: None,
            resume: None,
            session_capture: Some(SessionCapture {
                kind: SessionCaptureKind::ForcedFlagVerified,
                flag: Some("--session-id".to_string()),
                readback_args: Some(vec!["--verbose".to_string()]),
                event_type: None,
                event_id_path: None,
                json_flag: None,
                last_message_flag: None,
            }),
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let model = ModelConfig {
            name: "claude-test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![provider.clone()],
            inputs: vec![],
        };
        let extra_inputs = HashMap::new();
        let pinned_id = "11111111-2222-4333-8444-555555555555";

        let request = EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "hello",
            working_dir: None,
            extra_inputs: &extra_inputs,
            parent_invocation_env: None,
        };
        let result =
            execute_effective_with_start_known_provider_session_id(request, Some(pinned_id))
                .unwrap();

        assert_eq!(
            result.session_capture.session_id.as_deref(),
            Some(pinned_id)
        );
        let argv_seen = std::fs::read_to_string(&argv_dump_path).unwrap();
        let argv_lines: Vec<&str> = argv_seen.lines().collect();
        let injected_id = argv_lines
            .iter()
            .position(|arg| *arg == "--session-id")
            .and_then(|idx| argv_lines.get(idx + 1))
            .expect("--session-id value must be present on argv");
        assert_eq!(*injected_id, pinned_id);
    }

    #[cfg(unix)]
    #[test]
    fn execute_claude_forced_flag_verified_mismatch_fails_closed() {
        let script = fixture_script(
            r#"while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id)
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{"type":"system","subtype":"init","session_id":"11111111-1111-1111-1111-111111111111"}\n'"#,
        );
        let model = ModelConfig {
            name: "claude-test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig {
                name: "claude-fixture".to_string(),
                command: script.path.to_string_lossy().into_owned(),
                args: vec!["-p".to_string()],
                interactive_args: None,
                resume: None,
                session_capture: Some(SessionCapture {
                    kind: SessionCaptureKind::ForcedFlagVerified,
                    flag: Some("--session-id".to_string()),
                    readback_args: Some(vec![
                        "--verbose".to_string(),
                        "--output-format".to_string(),
                        "stream-json".to_string(),
                    ]),
                    event_type: None,
                    event_id_path: None,
                    json_flag: None,
                    last_message_flag: None,
                }),
                resume_acceptance: None,
                session_storage: None,
                system_prompt_override: None,
                tool_restrictions: None,
                invocation_mode: Default::default(),
            }],
            inputs: vec![],
        };

        let result = execute(&model, 0, "hello", None, &HashMap::new(), None).unwrap();

        assert!(result.session_capture.session_id.is_none());
        // Per contract: failure reason must be the canonical
        // "readback mismatch: ..." text so trace can surface a
        // consistent diagnostic to the user (V10).
        match &result.session_capture.method {
            SessionCaptureMethod::Failed(reason) => {
                assert!(
                    reason.contains("readback mismatch"),
                    "expected 'readback mismatch' in failure reason, got: {reason}"
                );
            }
            other => panic!("expected Failed(_), got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn execute_codex_stdout_json_event_replays_last_message_file_to_stdout() {
        let script = fixture_script(
            r#"output_file=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      output_file="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{"type":"thread.started","thread_id":"thread-123"}\n'
printf 'assistant text from tmpfile\n' > "$output_file""#,
        );
        let model = ModelConfig {
            name: "codex-test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig {
                name: "codex-fixture".to_string(),
                command: script.path.to_string_lossy().into_owned(),
                args: vec!["exec".to_string()],
                interactive_args: None,
                resume: None,
                session_capture: Some(SessionCapture {
                    kind: SessionCaptureKind::StdoutJsonEvent,
                    flag: None,
                    readback_args: None,
                    event_type: Some("thread.started".to_string()),
                    event_id_path: Some("thread_id".to_string()),
                    json_flag: Some("--json".to_string()),
                    last_message_flag: Some("-o".to_string()),
                }),
                resume_acceptance: None,
                session_storage: None,
                system_prompt_override: None,
                tool_restrictions: None,
                invocation_mode: Default::default(),
            }],
            inputs: vec![],
        };

        let result = execute(&model, 0, "hello", None, &HashMap::new(), None).unwrap();

        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "assistant text from tmpfile\n"
        );
        assert_eq!(
            result.session_capture.session_id.as_deref(),
            Some("thread-123")
        );
        assert!(matches!(
            result.session_capture.method,
            SessionCaptureMethod::StdoutJsonEvent
        ));
    }

    #[cfg(unix)]
    #[test]
    fn execute_codex_stdout_json_event_without_thread_started_marks_capture_failed() {
        let script = fixture_script(
            r#"output_file=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      output_file="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{"type":"response.completed"}\n'
printf 'assistant text from tmpfile\n' > "$output_file""#,
        );
        let model = ModelConfig {
            name: "codex-test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig {
                name: "codex-fixture".to_string(),
                command: script.path.to_string_lossy().into_owned(),
                args: vec!["exec".to_string()],
                interactive_args: None,
                resume: None,
                session_capture: Some(SessionCapture {
                    kind: SessionCaptureKind::StdoutJsonEvent,
                    flag: None,
                    readback_args: None,
                    event_type: Some("thread.started".to_string()),
                    event_id_path: Some("thread_id".to_string()),
                    json_flag: Some("--json".to_string()),
                    last_message_flag: Some("-o".to_string()),
                }),
                resume_acceptance: None,
                session_storage: None,
                system_prompt_override: None,
                tool_restrictions: None,
                invocation_mode: Default::default(),
            }],
            inputs: vec![],
        };

        let result = execute(&model, 0, "hello", None, &HashMap::new(), None).unwrap();

        assert!(result.session_capture.session_id.is_none());
        // Per contract: when the configured event_type doesn't appear
        // in the JSON event stream, the failure reason must mention
        // either the missing event type or "no event" so trace can
        // surface a useful diagnostic (V10).
        match &result.session_capture.method {
            SessionCaptureMethod::Failed(reason) => {
                assert!(
                    reason.contains("thread.started")
                        || reason.contains("event_type")
                        || reason.to_lowercase().contains("no event"),
                    "expected thread.started/event_type/'no event' in reason, got: {reason}"
                );
            }
            other => panic!("expected Failed(_), got {other:?}"),
        }
    }

    #[test]
    fn execute_effective_headless_mode_preserves_current_argv_shape() {
        let argv_dump = tempfile::NamedTempFile::new().unwrap();
        let script = fixture_script(&format!(
            r#"for arg in "$@"; do
  printf '%s\n' "$arg" >> "{}"
done"#,
            argv_dump.path().display()
        ));
        let model = ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(
                "claude",
                vec!["--model".to_string(), "opus".to_string()],
            )],
            inputs: vec![],
        };
        let provider = ProviderConfig {
            name: "claude".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: vec!["-p".to_string(), "--model".to_string(), "opus".to_string()],
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: InvocationMode::Headless,
        };

        let result = execute_effective(EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 0,
            prompt_mode: PromptMode::Arg,
            prompt: "hello",
            working_dir: None,
            extra_inputs: &HashMap::new(),
            parent_invocation_env: None,
        })
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            read_argv_dump(argv_dump.path()),
            vec!["-p", "--model", "opus", "hello"]
        );
    }
}
