//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/resume/execution.rs
//!     role: adapter
//!     Translates:
//!       - runtime-resume-and-executor-service-contract
//!       - StateDb-mailbox-and-resume-resolution-contract
//!       - runner-model-and-provider-configuration-contract
//!       - resume-prompt-and-spawn-working-directory-contract
//! ```

use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oulipoly_runtime::executor;
use oulipoly_runtime::provider_registry::ProviderRegistryOptions;
use oulipoly_runtime::services::{
    ExecutorServiceRequest, MailboxDeliveryCorrelation, ResumeServiceOutput,
};

use super::orchestration::ResumeAttemptInput;
use super::{formatter, mapper, migration, validator, wake};
use crate::cli::inputs::resolve_resume_answer;
use crate::migration_providers::{ResumeExecutionEnvironment, load_resume_execution_environment};
use crate::resume_cli::{
    ResumeExecutionTarget, format_resume_service_rejection, render_resume_model_pool_mismatch,
    renderable_resume_execution_target,
};
use crate::spawn_cwd::effective_resume_spawn_cwd;
use crate::wiring;

pub(in crate::run) struct PreparedHeadlessResumeExecution {
    pub(super) answer: Option<String>,
    pub(super) mailbox_session_id: String,
    pub(super) mailbox_delivery_seqs: Vec<i64>,
    pub(super) mailbox_delivery_nonce: Option<String>,
    pub(super) mailbox_delivery_requires_turn_confirmation: bool,
    pub(super) env: ResumeExecutionEnvironment,
    pub(super) resolved: oulipoly_state::ResolvedResume,
    pub(super) effective_spawn_cwd: PathBuf,
    pub(super) parent_invocation_id: Option<i64>,
    pub(super) max_attempts: usize,
    pub(super) provider_prompt_accepted: bool,
}

impl PreparedHeadlessResumeExecution {
    pub(in crate::run) fn provider_prompt_accepted(&self) -> bool {
        self.provider_prompt_accepted
    }
}

pub(super) fn reject_invalid_resume_input(session_id: &str) -> Option<i32> {
    match validator::validate_resume_input(session_id) {
        Ok(()) => None,
        Err(message) => {
            formatter::emit_stderr(&message);
            Some(1)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::run) fn prepare_headless_resume_execution(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    target_kind: oulipoly_state::InboxTargetKind,
    prompt: Option<&str>,
    file: Option<&Path>,
    submission_token: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
) -> Result<Result<PreparedHeadlessResumeExecution, i32>, String> {
    let answer = resolve_resume_answer(prompt, file)?;
    let answer = persist_tokenized_resume_input(answer, submission_token, target_kind, session_id)?;
    let env = load_resume_execution_environment(models_dir_override)?;
    refresh_resume_provider_registry(agent_runtime_services, &env)?;
    let resolved = match resolve_resume_for_headless_execution(
        agent_runtime_services,
        &env,
        session_id,
        model_name,
    ) {
        Ok(resolved) => resolved,
        Err(exit_code) => return Ok(Err(exit_code)),
    };
    if let Err(exit_code) = prepare_initial_headless_resume_target(
        &resolved,
        &env.providers_cfg,
        std::io::stderr().is_terminal(),
    ) {
        return Ok(Err(exit_code));
    }
    let effective_spawn_cwd = effective_resume_spawn_cwd(
        &env.state,
        &env.providers_cfg,
        &env.sessions_cfg,
        &resolved,
        session_id,
        working_dir,
    )?;
    let parent_invocation_id = crate::dispatch::resolve_parent_invocation_id(&env.state);
    let max_attempts = headless_resume_retry_budget(&resolved);
    if let Err(error) = wake::reconcile_pending_headless_delivery_observations(
        agent_runtime_services,
        &resolved,
        &effective_spawn_cwd,
    ) {
        formatter::emit_stderr(&format!(
            "Warning: Pending mailbox delivery observation recovery failed: {error}"
        ));
    }
    let mailbox_delivery =
        wake::prepare_headless_resume_delivery(&resolved, answer, Some(&env.models_dir))?;
    if crate::wake_coordinator::is_auto_wake_invocation()
        && mailbox_delivery.seqs.is_empty()
        && mailbox_delivery.answer.is_none()
    {
        wake::release_current_auto_wake_claim(session_id);
        return Ok(Err(0));
    }
    Ok(Ok(mapper::prepared_headless_resume_execution(
        mailbox_delivery,
        env,
        resolved,
        effective_spawn_cwd,
        parent_invocation_id,
        max_attempts,
    )))
}

fn persist_tokenized_resume_input(
    answer: Option<String>,
    submission_token: Option<&str>,
    target_kind: oulipoly_state::InboxTargetKind,
    target_id: &str,
) -> Result<Option<String>, String> {
    let Some(submission_token) = submission_token else {
        return Ok(answer);
    };
    let Some(answer) = answer else {
        return Ok(None);
    };
    let mut mailbox = oulipoly_state::mailbox::MailboxDb::open_default()?;
    match mailbox.enqueue_submitted_input(&oulipoly_state::SubmittedInputEnqueue {
        submission_token,
        target: oulipoly_state::InboxTarget {
            kind: target_kind,
            id: target_id,
        },
        input: answer.as_bytes(),
    })? {
        oulipoly_state::mailbox::EnqueueResult::Inserted(_)
        | oulipoly_state::mailbox::EnqueueResult::AlreadyEnqueued(_) => Ok(None),
        oulipoly_state::mailbox::EnqueueResult::Conflict { existing } => {
            Err(format_submission_token_conflict(existing.seq))
        }
    }
}

fn format_submission_token_conflict(existing_seq: i64) -> String {
    format!("submission token conflicts with existing inbox item {existing_seq}")
}

fn refresh_resume_provider_registry(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
) -> Result<(), String> {
    let models = mapper::resume_provider_models(&env.models);
    let registry = mapper::resume_provider_registry(
        &models,
        &env.providers_cfg,
        resume_provider_registry_options(env)?,
    )
    .map_err(formatter::resume_provider_registry_failure)?;
    agent_runtime_services
        .provider_registry_handle
        .replace(Arc::new(registry));
    Ok(())
}

fn resume_provider_registry_options(
    env: &ResumeExecutionEnvironment,
) -> Result<ProviderRegistryOptions, String> {
    Ok(ProviderRegistryOptions::default()
        .with_config_root(env.config_root.clone())
        .with_data_root(oulipoly_state::paths::data_dir()?))
}

fn headless_resume_retry_budget(resolved: &oulipoly_state::ResolvedResume) -> usize {
    resolved
        .model
        .as_ref()
        .map(|model| model.providers.len())
        .unwrap_or(1)
        .max(1)
        + 1
}

pub(super) fn prepare_resume_attempt_target(
    input: &mut ResumeAttemptInput<'_>,
) -> Result<Result<ResumeExecutionTarget, i32>, String> {
    let mut target =
        match renderable_resume_execution_target(input.resolved, &input.env.providers_cfg) {
            Ok(target) => target,
            Err(exit_code) => return Ok(Err(exit_code)),
        };
    if super::migration_allowed(input.reservation)
        && let Err(exit_code) = migration::migrate_resume_target(
            input.agent_runtime_services,
            input.env,
            input.resolved,
            &mut target,
            input.manual_migrate,
            input.attempts,
            input.effective_spawn_cwd,
        )
    {
        return Ok(Err(exit_code));
    }
    Ok(Ok(target))
}

pub(super) fn resume_attempt_strategy_for_target(
    provider: &oulipoly_config::ProviderConfig,
    account_endpoint_configured: bool,
) -> Result<Option<&oulipoly_config::ResumeStrategy>, i32> {
    if account_endpoint_configured {
        return Ok(None);
    }
    provider.resume.as_ref().map(Some).ok_or_else(|| {
        formatter::emit_missing_resume_block(&provider.name);
        1
    })
}

pub(super) fn execute_resume_attempt_command(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    provider_index: usize,
    prompt_mode: oulipoly_config::PromptMode,
    invocation_env: &str,
    strategy: Option<&oulipoly_config::ResumeStrategy>,
) -> Result<executor::ExecutionResult, String> {
    if input
        .agent_runtime_services
        .provider_registry_handle
        .current()
        .has_account_endpoint(&provider.name)
    {
        let fallback_model;
        let (model, provider_index) = if let Some(model) = input.resolved.model.as_ref() {
            (model, provider_index)
        } else {
            fallback_model = provider_only_resume_model(input, provider, prompt_mode);
            (&fallback_model, 0)
        };
        let request = provider_ref_resume_executor_request(
            input,
            model,
            provider,
            provider_index,
            prompt_mode,
            invocation_env,
        );
        return input
            .agent_runtime_services
            .executor_service
            .execute(request)
            .map(|output| output.result)
            .map_err(|err| err.to_string());
    }
    let resume_payload = mapper::legacy_resume_payload(
        &input.resolved.active_session_id,
        strategy.expect("legacy resume target must have a resume strategy"),
    );
    executor::cli::execute_resume_optional_prompt_with_model_identity(
        provider,
        provider_index,
        prompt_mode,
        input.answer,
        Some(input.effective_spawn_cwd),
        Some(invocation_env),
        resume_payload,
        input.resolved.model_name.as_deref().unwrap_or("<unknown>"),
        Some(&input.env.models_dir),
    )
}

fn provider_only_resume_model(
    input: &ResumeAttemptInput<'_>,
    provider: &oulipoly_config::ProviderConfig,
    prompt_mode: oulipoly_config::PromptMode,
) -> oulipoly_config::ModelConfig {
    oulipoly_config::ModelConfig {
        name: input
            .resolved
            .model_name
            .clone()
            .unwrap_or_else(|| "<unknown>".to_string()),
        prompt_mode,
        providers: vec![oulipoly_config::ProviderConfig::model_provider(
            &provider.name,
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: None,
    }
}

fn provider_ref_resume_executor_request(
    input: &ResumeAttemptInput<'_>,
    model: &oulipoly_config::ModelConfig,
    provider: &oulipoly_config::ProviderConfig,
    provider_index: usize,
    prompt_mode: oulipoly_config::PromptMode,
    invocation_env: &str,
) -> ExecutorServiceRequest {
    ExecutorServiceRequest::EffectiveWithStartKnownProviderSessionId {
        model: model.clone(),
        provider: provider.clone(),
        provider_index,
        prompt_mode,
        prompt: input.answer.unwrap_or_default().to_string(),
        working_dir: Some(input.effective_spawn_cwd.to_path_buf()),
        models_dir: Some(input.env.models_dir.clone()),
        extra_inputs: HashMap::new(),
        parent_invocation_env: Some(invocation_env.to_string()),
        start_known_provider_session_id: input.resolved.active_session_id.clone(),
        mailbox_delivery_correlation: input.mailbox_delivery_nonce.map(|delivery_nonce| {
            MailboxDeliveryCorrelation {
                delivery_nonce: delivery_nonce.to_string(),
            }
        }),
    }
}

pub(super) fn validate_reserved_resume_options(
    reservation: Option<&crate::run::reservation::ReservedRun>,
    manual_migrate: Option<&str>,
) -> Result<(), String> {
    if reservation.is_some() && manual_migrate.is_some() {
        return Err("Reserved resume execution cannot migrate providers".to_string());
    }
    Ok(())
}

fn resolve_resume_for_headless_execution(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    env: &ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) -> Result<oulipoly_state::ResolvedResume, i32> {
    let output = agent_runtime_services
        .resume_service
        .resolve_resume(mapper::resume_service_request(env, session_id, model_name))
        .map_err(|err| err.to_string());
    headless_resume_resolution_result(output, env, session_id, model_name)
}

fn headless_resume_resolution_result(
    output: Result<ResumeServiceOutput, String>,
    env: &ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) -> Result<oulipoly_state::ResolvedResume, i32> {
    match map_headless_resume_resolution(output) {
        HeadlessResumeResolution::Resolved(resolved) => Ok(resolved),
        resolution => {
            render_headless_resume_resolution_error(resolution, env, session_id, model_name);
            Err(1)
        }
    }
}

enum HeadlessResumeResolution {
    Resolved(oulipoly_state::ResolvedResume),
    ProviderModelMismatch { active_provider: String },
    Rejected(oulipoly_runtime::services::ResumeServiceRejection),
    ServiceFailure(String),
}

fn map_headless_resume_resolution(
    output: Result<ResumeServiceOutput, String>,
) -> HeadlessResumeResolution {
    match output {
        Ok(ResumeServiceOutput::ResumeResolved { resolved }) => {
            HeadlessResumeResolution::Resolved(resolved)
        }
        Ok(ResumeServiceOutput::ResumeRejected {
            error:
                oulipoly_runtime::services::ResumeServiceRejection::State(
                    oulipoly_state::ResumeError::ProviderModelMismatch {
                        active_provider, ..
                    },
                ),
        }) => HeadlessResumeResolution::ProviderModelMismatch { active_provider },
        Ok(ResumeServiceOutput::ResumeRejected { error }) => {
            HeadlessResumeResolution::Rejected(error)
        }
        Err(err) => HeadlessResumeResolution::ServiceFailure(err),
    }
}

fn render_headless_resume_resolution_error(
    resolution: HeadlessResumeResolution,
    env: &ResumeExecutionEnvironment,
    session_id: &str,
    model_name: Option<&str>,
) {
    match resolution {
        HeadlessResumeResolution::ProviderModelMismatch { active_provider } => {
            render_resume_model_pool_mismatch(
                &env.models,
                model_name.unwrap_or("<unknown>"),
                session_id,
                &active_provider,
            );
        }
        HeadlessResumeResolution::Rejected(error) => {
            formatter::emit_stderr(&format_resume_service_rejection(error));
        }
        HeadlessResumeResolution::ServiceFailure(err) => {
            formatter::emit_stderr(&format!("resume service failed: {err}"));
        }
        HeadlessResumeResolution::Resolved(_) => {}
    }
}

fn prepare_initial_headless_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &oulipoly_config::ProvidersConfig,
    stderr_is_terminal: bool,
) -> Result<(), i32> {
    let target = renderable_resume_execution_target(resolved, providers_cfg)?;
    if crate::dispatch::should_emit_resume_short_line(stderr_is_terminal) {
        formatter::emit_resume_short_line(&resolved.active_provider);
    }
    let selected_provider = &resolved.active_provider;
    let account_endpoint_configured = providers_cfg
        .entries
        .get(selected_provider)
        .and_then(|provider| provider.implementation.as_ref())
        .is_some();
    validate_headless_resume_target(
        resolved,
        &target,
        selected_provider,
        account_endpoint_configured,
    )
}

fn validate_headless_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    target: &ResumeExecutionTarget,
    selected_provider: &str,
    account_endpoint_configured: bool,
) -> Result<(), i32> {
    if account_endpoint_configured {
        return validate_selected_provider_resume_target(resolved, target, selected_provider)
            .map_err(|message| provider_ref_resume_block_exit_code(&message));
    }
    if resolved_uses_provider_ref(resolved) {
        return validate_provider_ref_headless_resume_target(resolved, target, selected_provider)
            .map_err(|message| provider_ref_resume_block_exit_code(&message));
    }
    if target.provider.resume.is_some() {
        Ok(())
    } else {
        formatter::emit_missing_resume_block(selected_provider);
        Err(1)
    }
}

pub(super) fn validate_provider_ref_headless_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    target: &ResumeExecutionTarget,
    selected_provider: &str,
) -> Result<(), String> {
    let model = resolved.model.as_ref().ok_or_else(|| {
        format!("provider-ref resume target {selected_provider} has no model config")
    })?;
    if model.provider.is_none() {
        return Err(format!(
            "provider-ref resume target {selected_provider} has no provider implementation"
        ));
    }
    validate_selected_provider_resume_target(resolved, target, selected_provider)
}

fn validate_selected_provider_resume_target(
    resolved: &oulipoly_state::ResolvedResume,
    target: &ResumeExecutionTarget,
    selected_provider: &str,
) -> Result<(), String> {
    if let Some(model) = resolved.model.as_ref() {
        let target_member_name = model
            .providers
            .get(target.provider_index)
            .map(|provider| provider.name.as_str())
            .ok_or_else(|| {
                format!(
                    "provider-ref resume target {selected_provider} has invalid provider index {}",
                    target.provider_index
                )
            })?;
        if target_member_name != selected_provider {
            return Err(format!(
                "provider-ref resume target {selected_provider} resolved provider {target_member_name}"
            ));
        }
    }
    if target.provider.name != selected_provider {
        return Err(format!(
            "provider-ref resume target {selected_provider} loaded provider {}",
            target.provider.name
        ));
    }
    Ok(())
}

pub(super) fn provider_ref_resume_block_exit_code(message: &str) -> i32 {
    formatter::emit_stderr(message);
    1
}

pub(super) fn resolved_uses_provider_ref(resolved: &oulipoly_state::ResolvedResume) -> bool {
    resolved
        .model
        .as_ref()
        .is_some_and(|model| model.provider.is_some())
}
