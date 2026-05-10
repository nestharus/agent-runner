use oulipoly_config::{
    InputType, ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionCapture,
    SessionCaptureKind,
};
use oulipoly_state::CompositeInvocationId;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Child;
use std::process::{Command, ExitStatus, Stdio};
#[cfg(unix)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(unix)]
use std::thread;

#[cfg(unix)]
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::{Handle as SignalsHandle, Signals};

use super::{
    CapturedChildInvocation, ExecutionResult, ResumeAcceptanceResult, ResumeAcceptanceStatus,
    ReturnedArtifactRef, SessionCaptureMethod, SessionCaptureResult,
};

const LARGE_PROMPT_THRESHOLD: usize = 100 * 1024; // 100KB

pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    parent_invocation_env: Option<&str>,
) -> Result<ExecutionResult, String> {
    let provider = model.providers.get(provider_index).ok_or_else(|| {
        format!(
            "Provider index {} out of range for model {}",
            provider_index, model.name
        )
    })?;

    // Resolve inputs to flag args
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
    for path in temp_files {
        let _ = std::fs::remove_file(path);
    }

    Ok(ExecutionResult {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        provider_index,
        session_capture: result.session_capture,
        resume_acceptance: None,
        terminal_reason: result.terminal_reason,
        captured_child_invocations: result.captured_child_invocations,
        returned_artifacts: result.returned_artifacts,
    })
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
    let input_args = resolve_input_flags(request.model, request.extra_inputs)?;
    let (result, temp_files) = execute_provider(
        request.provider,
        request.prompt_mode,
        request.prompt,
        request.working_dir,
        &input_args,
        request.parent_invocation_env,
        start_known_provider_session_id,
    )?;
    for path in temp_files {
        let _ = std::fs::remove_file(path);
    }

    Ok(ExecutionResult {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        provider_index: request.provider_index,
        session_capture: result.session_capture,
        resume_acceptance: None,
        terminal_reason: result.terminal_reason,
        captured_child_invocations: result.captured_child_invocations,
        returned_artifacts: result.returned_artifacts,
    })
}

/// Map user-provided inputs to CLI flag arguments based on the model's input schema.
fn resolve_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    let mut args = Vec::new();

    for (key, values) in extra_inputs {
        // Find the matching input definition
        let input_def = model.inputs.iter().find(|i| i.name == *key);

        let flag = match input_def {
            Some(def) => {
                // Validate the value(s) against the schema
                validate_input_values(values, def)?;
                // Use declared flag, or fall back to --name
                def.flag.clone().unwrap_or_else(|| format!("--{}", key))
            }
            None => {
                // Unknown input — pass through as --name (user knows what they're doing)
                format!("--{}", key)
            }
        };

        // For array types or repeated values, emit the flag once per value
        for val in values {
            args.push(flag.clone());
            args.push(val.clone());
        }
    }

    // Apply defaults for inputs not provided by the user
    for input_def in &model.inputs {
        if input_def.default_input {
            continue; // Default input is the prompt, handled separately
        }
        if extra_inputs.contains_key(&input_def.name) {
            continue; // Already provided
        }
        if let Some(ref default) = input_def.default {
            if let Some(ref flag) = input_def.flag {
                let val = toml_value_to_string(default);
                args.push(flag.clone());
                args.push(val);
            }
        } else if input_def.required {
            return Err(format!("Required input '{}' not provided", input_def.name));
        }
    }

    Ok(args)
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
    captured_child_invocations: Vec<CapturedChildInvocation>,
    returned_artifacts: Vec<ReturnedArtifactRef>,
}

struct ReturnChannel {
    path: PathBuf,
    dir: PathBuf,
}

impl ReturnChannel {
    fn cleanup(&self) {
        if let Err(err) = std::fs::remove_file(&self.path) {
            eprintln!(
                "Warning: failed to delete return channel {}: {err}",
                self.path.display()
            );
        }
        if let Err(err) = std::fs::remove_dir(&self.dir)
            && err.kind() != std::io::ErrorKind::NotFound
            && err.kind() != std::io::ErrorKind::DirectoryNotEmpty
        {
            eprintln!(
                "Warning: failed to delete return channel directory {}: {err}",
                self.dir.display()
            );
        }
    }
}

fn prepare_return_channel(
    parent_invocation_env: Option<&str>,
) -> Result<Option<ReturnChannel>, String> {
    let Some(parent_invocation_env) = parent_invocation_env else {
        return Ok(None);
    };
    let invocation = CompositeInvocationId::parse_env_value(parent_invocation_env)
        .map_err(|err| format!("Failed to parse parent invocation for return channel: {err}"))?;
    let dir = std::env::temp_dir()
        .join("oulipoly-return-channels")
        .join(&invocation.id);
    std::fs::create_dir_all(&dir).map_err(|err| {
        format!(
            "Failed to create return channel directory {}: {err}",
            dir.display()
        )
    })?;
    let path = dir.join(format!("returns-{}.jsonl", uuid::Uuid::new_v4()));
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|err| format!("Failed to create return channel {}: {err}", path.display()))?;
    Ok(Some(ReturnChannel { path, dir }))
}

fn read_return_channel(channel: &ReturnChannel) -> Vec<ReturnedArtifactRef> {
    let body = match std::fs::read_to_string(&channel.path) {
        Ok(body) => body,
        Err(err) => {
            eprintln!(
                "Warning: failed to read return channel {}: {err}",
                channel.path.display()
            );
            return Vec::new();
        }
    };
    body.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            match serde_json::from_str::<ReturnedArtifactRef>(trimmed) {
                Ok(reference) => Some(reference),
                Err(err) => {
                    eprintln!(
                        "Warning: failed to parse return channel line {} in {}: {err}",
                        index + 1,
                        channel.path.display()
                    );
                    None
                }
            }
        })
        .collect()
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
    match strategy.kind {
        ResumeKind::Flag => {
            let flag = strategy
                .flag
                .as_ref()
                .ok_or_else(|| "resume.flag is required".to_string())?;
            provider_args.push(flag.clone());
            provider_args.push(session_id.to_string());
        }
        ResumeKind::Subcommand => {
            let subcommand = strategy
                .subcommand
                .as_ref()
                .ok_or_else(|| "resume.subcommand is required".to_string())?;
            if subcommand.is_empty() {
                return Err("resume.subcommand is required".to_string());
            }
            provider_args.extend(subcommand.iter().cloned());
            provider_args.push(session_id.to_string());
        }
    }
    Ok(())
}

fn build_command(
    provider: &ProviderConfig,
    provider_args: &[String],
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    return_channel: Option<&Path>,
) -> Result<Command, String> {
    let parts = shell_split(&provider.command);
    if parts.is_empty() {
        return Err("Empty command".to_string());
    }

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

    Ok(cmd)
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
    let return_channel = prepare_return_channel(parent_invocation_env)?;
    let mut cmd = build_command(
        provider,
        provider_args,
        working_dir,
        parent_invocation_env,
        return_channel
            .as_ref()
            .map(|channel| channel.path.as_path()),
    )?;

    // Append input flags
    for arg in input_args {
        cmd.arg(arg);
    }

    let (capture_plan, capture_args, capture_files) = build_capture_plan(
        provider.session_capture.as_ref(),
        start_known_provider_session_id,
    )?;
    for arg in capture_args {
        cmd.arg(arg);
    }

    let mut temp_files = capture_files;

    match prompt_mode {
        PromptMode::Arg => {
            if prompt.len() > LARGE_PROMPT_THRESHOLD {
                let dir = working_dir.unwrap_or(Path::new("."));
                let filename = format!("_agent_prompt_{}.md", uuid::Uuid::new_v4());
                let path = dir.join(&filename);
                std::fs::write(&path, prompt)
                    .map_err(|e| format!("Failed to write temp prompt file: {e}"))?;
                cmd.arg(format!("Follow the instructions in {filename}"));
                temp_files.push(path);
            } else {
                cmd.arg(prompt);
            }
            cmd.stdin(Stdio::null());
        }
        PromptMode::Stdin => {
            cmd.stdin(Stdio::piped());
        }
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {e}", provider.command))?;

    if prompt_mode == PromptMode::Stdin
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("Failed to write to stdin: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for process: {e}"))?;

    let returned_artifacts = return_channel
        .as_ref()
        .map(read_return_channel)
        .unwrap_or_default();
    if let Some(channel) = &return_channel {
        channel.cleanup();
    }

    let capture_outcome = finalize_capture(&capture_plan, &output.stdout);
    let stdout = maybe_restore_plain_stdout(&capture_plan, &capture_outcome, &output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let result = RawResult {
        stdout,
        telemetry_stdout: output.stdout.clone(),
        captured_child_invocations: captured_child_invocations_from_stderr(&stderr),
        stderr,
        exit_code: exit_code_from_status(&output.status),
        terminal_reason: classify_terminal_reason(&output.status),
        session_capture: capture_outcome,
        returned_artifacts,
    };

    Ok((result, temp_files))
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
    let session_id = resume.session_id.to_string();
    let provider_args = compose_resume_provider_args(provider.args.clone(), resume)?;
    let mut provider_without_capture = provider.clone();
    provider_without_capture.session_capture = None;
    let (result, temp_files) = execute_provider_with_args(
        &provider_without_capture,
        &provider_args,
        prompt_mode,
        prompt,
        working_dir,
        &[],
        parent_invocation_env,
        None,
    )?;
    for path in temp_files {
        let _ = std::fs::remove_file(path);
    }
    let resume_acceptance = classify_resume_acceptance(
        provider.resume_acceptance.as_ref(),
        result.exit_code,
        &result.telemetry_stdout,
        result.stderr.as_bytes(),
        &session_id,
    );
    Ok(ExecutionResult {
        stdout: result.stdout,
        stderr: result.stderr,
        exit_code: result.exit_code,
        provider_index,
        session_capture: SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
        resume_acceptance: Some(resume_acceptance),
        terminal_reason: result.terminal_reason,
        captured_child_invocations: result.captured_child_invocations,
        returned_artifacts: result.returned_artifacts,
    })
}

fn classify_resume_acceptance(
    rules: Option<&oulipoly_config::ResumeAcceptanceRules>,
    exit_code: i32,
    stdout: &[u8],
    stderr: &[u8],
    session_id: &str,
) -> ResumeAcceptanceResult {
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    if let Some(rules) = rules {
        if let Some(pattern) = first_matching_pattern(
            rules.rejected_output_patterns.as_deref(),
            &output,
            session_id,
        ) {
            return ResumeAcceptanceResult {
                status: ResumeAcceptanceStatus::Rejected,
                evidence: Some(format!("matched reject pattern: {pattern}")),
            };
        }
        if exit_code == 0
            && let Some(pattern) = first_matching_pattern(
                rules.accepted_output_patterns.as_deref(),
                &output,
                session_id,
            )
        {
            return ResumeAcceptanceResult {
                status: ResumeAcceptanceStatus::Accepted,
                evidence: Some(format!("matched accept pattern: {pattern}")),
            };
        }
    }

    let lower = output.to_lowercase();
    if lower.contains("no conversation found") || lower.contains("no session found") {
        return ResumeAcceptanceResult {
            status: ResumeAcceptanceStatus::Rejected,
            evidence: Some(
                "resume_session_mismatch: provider reported missing session".to_string(),
            ),
        };
    }

    if exit_code == 0 {
        let evidence = if rules.is_none() {
            "child exited 0 but provider has no resume_acceptance rules configured"
        } else {
            "child exited 0 but no accepted resume pattern matched"
        };
        ResumeAcceptanceResult {
            status: ResumeAcceptanceStatus::Unconfirmed,
            evidence: Some(evidence.to_string()),
        }
    } else {
        ResumeAcceptanceResult {
            status: ResumeAcceptanceStatus::Rejected,
            evidence: Some(format!(
                "child exited {exit_code}; no rejection patterns matched"
            )),
        }
    }
}

fn first_matching_pattern(
    patterns: Option<&[String]>,
    output: &str,
    session_id: &str,
) -> Option<String> {
    patterns?.iter().find_map(|pattern| {
        let expanded = pattern.replace("{session_id}", session_id);
        output.contains(&expanded).then(|| pattern.clone())
    })
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveExecutionResult {
    /// Numeric child-process exit code per `exit_code_from_status` (interactive/REPL path).
    pub exit_code: i32,
    pub terminal_reason: Option<String>,
}

pub fn execute_interactive_with_result(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
) -> Result<InteractiveExecutionResult, String> {
    let interactive_args = provider.interactive_args.as_ref().ok_or_else(|| {
        format!(
            "provider {} has no interactive_args; cannot launch interactively",
            provider.name
        )
    })?;

    let mut provider_args = interactive_args.clone();
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

    let terminal_reason = classify_terminal_reason(&status);
    Ok(InteractiveExecutionResult {
        exit_code: exit_code_from_status(&status),
        terminal_reason,
    })
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

pub fn start_known_provider_session_id(
    provider: &ProviderConfig,
) -> Result<Option<String>, String> {
    let Some(capture) = provider.session_capture.as_ref() else {
        return Ok(None);
    };
    match capture.kind {
        SessionCaptureKind::ForcedFlagVerified => {
            if capture.flag.is_none() {
                return Err("session_capture.flag is required".to_string());
            }
            Ok(Some(uuid::Uuid::new_v4().to_string()))
        }
        SessionCaptureKind::None | SessionCaptureKind::StdoutJsonEvent => Ok(None),
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
        SessionCaptureKind::None => Ok((CapturePlan::None, Vec::new(), Vec::new())),
        SessionCaptureKind::ForcedFlagVerified => {
            let flag = capture
                .flag
                .clone()
                .ok_or_else(|| "session_capture.flag is required".to_string())?;
            let requested_session_id = start_known_provider_session_id
                .map(str::to_string)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let mut args = vec![flag, requested_session_id.clone()];
            if let Some(readback_args) = &capture.readback_args {
                args.extend(readback_args.clone());
            }
            Ok((
                CapturePlan::ForcedFlagVerified {
                    requested_session_id,
                },
                args,
                Vec::new(),
            ))
        }
        SessionCaptureKind::StdoutJsonEvent => {
            let json_flag = capture
                .json_flag
                .clone()
                .ok_or_else(|| "session_capture.json_flag is required".to_string())?;
            let last_message_flag = capture
                .last_message_flag
                .clone()
                .ok_or_else(|| "session_capture.last_message_flag is required".to_string())?;
            let event_type = capture
                .event_type
                .clone()
                .ok_or_else(|| "session_capture.event_type is required".to_string())?;
            let event_id_path = capture
                .event_id_path
                .clone()
                .ok_or_else(|| "session_capture.event_id_path is required".to_string())?;
            let last_message_path = std::env::temp_dir()
                .join(format!("oulipoly-last-message-{}", uuid::Uuid::new_v4()));
            Ok((
                CapturePlan::StdoutJsonEvent {
                    event_type,
                    event_id_path,
                    last_message_path: last_message_path.clone(),
                },
                vec![
                    json_flag,
                    last_message_flag,
                    last_message_path.to_string_lossy().into_owned(),
                ],
                vec![last_message_path],
            ))
        }
    }
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
        } => match std::fs::read(last_message_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                if matches!(capture.method, SessionCaptureMethod::StdoutJsonEvent) {
                    eprintln!(
                        "Warning: failed to read reconstructed last-message output {}: {err}",
                        last_message_path.display()
                    );
                }
                stdout.to_vec()
            }
        },
        _ => stdout.to_vec(),
    }
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
        let Some(raw) = line.strip_prefix("OULIPOLY_INVOCATION=") else {
            continue;
        };
        let Ok(composite_id) = CompositeInvocationId::parse_env_value(raw) else {
            continue;
        };
        if seen.insert((composite_id.source.clone(), composite_id.id.clone())) {
            captured.push(CapturedChildInvocation {
                composite_id,
                raw_marker_line: line.to_string(),
            });
        }
    }

    captured
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
                if signal == SIGTERM && !thread_forwarded_sigterm.swap(true, Ordering::SeqCst) {
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
    use crate::executor::{SessionCaptureMethod, SessionCaptureResult};
    use oulipoly_config::{
        InputDef, InputType, ProviderConfig, ResumeKind, ResumeStrategy, SessionCapture,
        SessionCaptureKind,
    };
    use oulipoly_state::CompositeInvocationId;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

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
}
