use crate::RuntimeServices;
use agent_runner_balancer::{self as balancer, BalanceEffects};
use agent_runner_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use agent_runner_executor as executor;
use agent_runner_quota::{RefreshOutcome, is_stale, refresh_provider};
use agent_runner_state::{
    CompositeInvocationId, InvocationStart, ResolvedResume, ResumeError, StateDb,
};
use serde_json::json;
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct ReplOptions {
    pub model: Option<String>,
    pub resume: Option<String>,
    pub migrate: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub models_dir_override: Option<PathBuf>,
}

pub fn run_repl_with_services(opts: ReplOptions, services: RuntimeServices) -> Result<i32, String> {
    let mut stderr = std::io::stderr();
    run_repl_with_services_and_stderr(opts, services, &mut stderr)
}

pub fn run_repl_with_services_and_stderr(
    opts: ReplOptions,
    services: RuntimeServices,
    stderr: &mut dyn Write,
) -> Result<i32, String> {
    let state = services.state_opener.open_default()?;
    let models = services.model_repo.load_models()?;
    let providers_cfg = services
        .provider_source
        .load_providers()
        .unwrap_or_default();
    let sessions_cfg = services.sessions_source.load_sessions().unwrap_or_default();
    let mut resolved_resume = if let Some(session_id) = opts.resume.as_deref() {
        Some(
            match state.resolve_resume(&models, session_id, opts.model.as_deref()) {
                Ok(resolved) => resolved,
                Err(ResumeError::ProviderModelMismatch {
                    active_provider, ..
                }) => {
                    return Err(resume_model_pool_mismatch_message(
                        &models,
                        opts.model.as_deref().unwrap_or("<unknown>"),
                        session_id,
                        &active_provider,
                    ));
                }
                Err(err) => return Err(format_resume_error(err)),
            },
        )
    } else {
        None
    };
    let mut fallback_target = match resolved_resume.as_ref() {
        Some(resolved) => {
            Some(resume_execution_target(resolved, &providers_cfg).map_err(format_resume_error)?)
        }
        None => None,
    };
    let direct_model = if fallback_target.is_none() {
        let model_name = opts
            .model
            .as_deref()
            .ok_or_else(|| "model is required unless --resume is present".to_string())?;
        Some(
            models
                .get(model_name)
                .cloned()
                .ok_or_else(|| format!("Unknown model: {model_name}"))?,
        )
    } else {
        None
    };
    let model = fallback_target
        .as_ref()
        .and_then(|target| target.model.clone())
        .or(direct_model)
        .unwrap_or_else(|| ModelConfig {
            name: "<provider-default>".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: Vec::new(),
            inputs: Vec::new(),
        });

    let ctx = BalanceContext {
        services: &services,
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        state: &state,
    };

    let parent_invocation_id = resolve_parent_invocation_id(&state);
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let (provider_index, provider, resume_session_id) = if let Some(resolved) =
        resolved_resume.as_mut()
    {
        let selected_provider = &resolved.active_provider;
        if should_emit_resume_short_line(stderr_is_terminal) {
            let _ = writeln!(stderr, "[resume] -> {selected_provider}");
        }
        let migration_model = resume_migration_pool(resolved, &providers_cfg);
        if let Ok(balancer::MigrationDecision::Migrate {
            target_provider_index,
            reason,
        }) =
            balancer::decide_migration(&state, &migration_model, resolved, opts.migrate.as_deref())
        {
            match agent_runner_balancer::migration::migrate_chain_segment(
                &state,
                &sessions_cfg,
                &migration_model,
                resolved,
                target_provider_index,
                reason,
                stderr,
            ) {
                Ok(migrated) => {
                    resolved.active_provider = migrated.target_provider.clone();
                    resolved.active_session_id = migrated.target_session_id.clone();
                    fallback_target = Some(
                        resume_execution_target(resolved, &providers_cfg)
                            .map_err(format_resume_error)?,
                    );
                }
                Err(err) => {
                    let _ = writeln!(stderr, "migration failed: {err:?}");
                    return Ok(1);
                }
            }
        }

        let target = fallback_target
            .as_ref()
            .expect("resume target must be resolved before spawn");
        let provider_index = target.provider_index;
        let provider = target.provider.clone();
        if provider.resume.is_none() {
            let _ = writeln!(
                stderr,
                "provider {} has no [providers.resume] block; cannot resume",
                provider.name
            );
            return Ok(1);
        }

        (
            provider_index,
            provider,
            Some(resolved.active_session_id.clone()),
        )
    } else {
        let provider_index = balancer::select_provider(&model, &state, Some(&ctx));
        let (provider, _) = effective_model_for_execution(&model, provider_index, &providers_cfg)?;
        (provider_index, provider, None)
    };
    if provider.interactive_args.is_none() {
        return Err(format!(
            "Provider {} has no interactive_args; cannot launch interactively",
            provider.name
        ));
    }

    let invocation = CompositeInvocationId {
        source: provider.name.clone(),
        id: Uuid::new_v4().to_string(),
    };
    let invocation_model_name = resolved_resume
        .as_ref()
        .and_then(|resolved| resolved.model_name.clone())
        .unwrap_or_else(|| {
            if opts.resume.is_some() {
                "<unknown>".to_string()
            } else {
                model.name.clone()
            }
        });
    let invocation_row_id = state.start_invocation(&InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name: invocation_model_name,
        provider_name: provider.name.clone(),
        provider_index,
        parent_invocation_id,
    })?;
    let mut guard = FinalizerGuard::new(&state, invocation_row_id);
    let invocation_env = serde_json::to_string(&invocation)
        .map_err(|e| format!("Failed to serialize invocation id: {e}"))?;

    if let Some(session_id) = opts.resume.as_deref() {
        state.update_session_capture(invocation_row_id, Some(session_id), "resumed")?;
    }

    if should_emit_invocation_line(stderr_is_terminal) {
        let _ = writeln!(stderr, "{}", invocation.stderr_line());
    }

    let resume_payload = resume_session_id.as_deref().map(|session_id| {
        let strategy = provider
            .resume
            .as_ref()
            .expect("resumable provider must have a resume strategy");
        executor::cli::ResumePayload {
            session_id,
            strategy,
            target_jsonl_path: None,
        }
    });

    match executor::cli::execute_interactive_with_runner(
        services.process_runner.as_ref(),
        &provider,
        opts.working_dir.as_deref(),
        Some(&invocation_env),
        resume_payload,
    ) {
        Ok(exit_code) => {
            if opts.resume.is_none() {
                state.update_session_capture(invocation_row_id, None, "none")?;
            }
            state.finalize_invocation(invocation_row_id, exit_code == 0, exit_code, None, None)?;
            guard.mark_finalized();
            if exit_code == 0 {
                let emitted = ingest_and_emit_session_id(
                    &services,
                    &state,
                    &sessions_cfg,
                    &provider.name,
                    invocation_row_id,
                    &invocation.id,
                    if opts.resume.is_some() {
                        "resumed"
                    } else {
                        "turn_script"
                    },
                    stderr,
                );
                if !emitted && let Some(session_id) = opts.resume.as_deref() {
                    emit_known_session_id(
                        &state,
                        invocation_row_id,
                        &invocation.id,
                        session_id,
                        "resumed",
                        stderr,
                    );
                }
            }
            Ok(exit_code)
        }
        Err(spawn_err) => {
            if opts.resume.is_none() {
                state.update_session_capture(invocation_row_id, None, "none")?;
            }
            state.finalize_invocation(
                invocation_row_id,
                false,
                1,
                Some("spawn_error"),
                Some(&spawn_err),
            )?;
            guard.mark_finalized();
            Ok(1)
        }
    }
}

struct BalanceContext<'a> {
    services: &'a RuntimeServices,
    providers_cfg: &'a ProvidersConfig,
    sessions_cfg: &'a agent_runner_config::SessionsConfig,
    state: &'a StateDb,
}

impl BalanceEffects for BalanceContext<'_> {
    fn refresh_quota_if_stale(&self, provider_name: &str) {
        if is_stale(self.state, provider_name) {
            let _: RefreshOutcome = refresh_provider(
                provider_name,
                self.providers_cfg,
                &self.services.quota_in_flight,
                self.state,
                self.services.process_runner.as_ref(),
            );
        }
    }

    fn scan_provider_sessions(&self, provider_name: &str) {
        let _ = self
            .services
            .scan_provider_sessions(provider_name, self.sessions_cfg, self.state);
    }
}

struct FinalizerGuard<'a> {
    db: &'a StateDb,
    invocation_id: i64,
    finalized: bool,
}

impl<'a> FinalizerGuard<'a> {
    fn new(db: &'a StateDb, invocation_id: i64) -> Self {
        Self {
            db,
            invocation_id,
            finalized: false,
        }
    }

    fn mark_finalized(&mut self) {
        self.finalized = true;
    }
}

impl Drop for FinalizerGuard<'_> {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }

        if let Err(err) =
            self.db
                .finalize_invocation(self.invocation_id, false, -1, Some("guard_drop"), None)
        {
            eprintln!("Warning: Failed to finalize invocation in guard: {err}");
        }
    }
}

fn should_emit_invocation_line(is_terminal: bool) -> bool {
    !is_terminal
}

fn should_emit_resume_short_line(_is_terminal: bool) -> bool {
    true
}

#[allow(clippy::too_many_arguments)]
fn ingest_and_emit_session_id(
    services: &RuntimeServices,
    state: &StateDb,
    sessions_cfg: &agent_runner_config::SessionsConfig,
    provider_name: &str,
    invocation_row_id: i64,
    invocation_uuid: &str,
    capture_method: &str,
    stderr: &mut dyn Write,
) -> bool {
    let invocation = match state.get_invocation_by_uuid(invocation_uuid) {
        Ok(Some(row)) => row,
        Ok(None) => {
            let _ = writeln!(
                stderr,
                "Warning: Could not resolve invocation {invocation_uuid} for session ingest"
            );
            return false;
        }
        Err(err) => {
            let _ = writeln!(
                stderr,
                "Warning: Failed to load invocation {invocation_uuid} for session ingest: {err}"
            );
            return false;
        }
    };
    let Some(finished_at) = invocation.finished_at else {
        let _ = writeln!(
            stderr,
            "Warning: Invocation {invocation_uuid} was not finalized before session ingest"
        );
        return false;
    };

    for err in services.scan_provider_sessions(provider_name, sessions_cfg, state) {
        let _ = writeln!(
            stderr,
            "Warning: Session ingest failed for {provider_name}: {err}"
        );
    }

    let session_id = match state.find_session_for_invocation_window(
        provider_name,
        &invocation.created_at,
        &finished_at,
    ) {
        Ok(Some(session_id)) => session_id,
        Ok(None) => return false,
        Err(err) => {
            let _ = writeln!(
                stderr,
                "Warning: Failed to resolve session for invocation {invocation_uuid}: {err}"
            );
            return false;
        }
    };

    emit_known_session_id(
        state,
        invocation_row_id,
        invocation_uuid,
        session_id.as_str(),
        capture_method,
        stderr,
    )
}

fn emit_known_session_id(
    state: &StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
    stderr: &mut dyn Write,
) -> bool {
    if let Err(err) =
        state.update_session_capture(invocation_row_id, Some(session_id), capture_method)
    {
        let _ = writeln!(
            stderr,
            "Warning: Failed to update invocation session_id: {err}"
        );
        return false;
    }
    if let Err(err) = state.mint_chain_for_invocation_session(invocation_row_id) {
        let _ = writeln!(stderr, "Warning: Failed to mint session chain: {err}");
    }
    let payload = json!({
        "id": invocation_uuid,
        "session_id": session_id,
    });
    let _ = writeln!(stderr, "OULIPOLY_SESSION={payload}");
    true
}

fn resume_model_pool_mismatch_message(
    models: &HashMap<String, ModelConfig>,
    model_name: &str,
    session_id: &str,
    provider_name: &str,
) -> String {
    let mut suggestions: Vec<String> = models
        .values()
        .filter(|model| {
            model
                .providers
                .iter()
                .any(|provider| provider.name == provider_name)
        })
        .map(|model| model.name.clone())
        .collect();
    suggestions.sort();
    suggestions.dedup();

    if suggestions.is_empty() {
        format!(
            "session {session_id} belongs to provider {provider_name}, which is not in model {model_name}'s provider pool.\nTry a model that includes {provider_name}: (no other model in the loaded config includes {provider_name})"
        )
    } else {
        format!(
            "session {session_id} belongs to provider {provider_name}, which is not in model {model_name}'s provider pool.\nTry a model that includes {provider_name}: {}",
            suggestions.join(", ")
        )
    }
}

fn format_resume_error(err: ResumeError) -> String {
    match err {
        ResumeError::InvalidUuid { input } => format!("invalid session UUID: {input}"),
        ResumeError::NoChainFound { input } => format!(
            "No session found matching {input}. Check that session ingestion is configured and that the provider still has resumable local state."
        ),
        ResumeError::Ambiguous { input, previews } => {
            let mut out = format!(
                "[resume] session {input} matches {} chains:\n",
                previews.len()
            );
            for preview in previews {
                out.push_str(&format!(
                    "  chain {} - last used {} - {} - {} turns\n",
                    preview.chain_id,
                    preview.last_used_at.to_rfc3339(),
                    preview.active_provider,
                    preview.turn_count
                ));
            }
            out.push_str("Re-run with: agents resume <chain_id>");
            out
        }
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            suggestions,
        } => {
            let suffix = if suggestions.is_empty() {
                format!("(no other model in the loaded config includes {active_provider})")
            } else {
                format!("Try one of: {}", suggestions.join(", "))
            };
            format!(
                "session belongs to provider {active_provider}, which is not in model {model_name}'s provider pool. Model {model_name} does not include active segment's owning provider {active_provider}. {suffix}"
            )
        }
        ResumeError::UnknownModel { model_name } => format!("Unknown model: {model_name}"),
        ResumeError::ActiveSegmentMissing { chain_id } => {
            format!("No active segment found for chain {chain_id}")
        }
        ResumeError::ProviderNotConfigured { provider } => {
            format!("provider {provider} is not configured in any loaded model")
        }
        ResumeError::ProviderMissingResume { provider_name } => {
            format!("provider {provider_name} has no [providers.resume] block; cannot resume")
        }
        ResumeError::Db { message } => message,
    }
}

#[derive(Clone)]
struct ResumeExecutionTarget {
    model: Option<ModelConfig>,
    provider_index: usize,
    provider: ProviderConfig,
}

fn resume_execution_target(
    resolved: &ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ResumeExecutionTarget, ResumeError> {
    if let Some(model) = resolved.model.as_ref() {
        let provider_index = model
            .providers
            .iter()
            .position(|provider| provider.name == resolved.active_provider)
            .ok_or_else(|| ResumeError::ProviderModelMismatch {
                model_name: model.name.clone(),
                active_provider: resolved.active_provider.clone(),
                suggestions: Vec::new(),
            })?;
        let (provider, _) = providers_cfg
            .effective_provider(&model.providers[provider_index])
            .map_err(|message| ResumeError::Db { message })?;
        Ok(ResumeExecutionTarget {
            model: Some(model.clone()),
            provider_index,
            provider,
        })
    } else {
        let (provider, _) = providers_cfg
            .runtime_provider(&resolved.active_provider)
            .map_err(|message| ResumeError::Db { message })?;
        let provider_index =
            provider_index_in_providers_cfg(providers_cfg, &resolved.active_provider);
        Ok(ResumeExecutionTarget {
            model: None,
            provider_index,
            provider,
        })
    }
}

fn provider_index_in_providers_cfg(providers_cfg: &ProvidersConfig, provider_name: &str) -> usize {
    let mut names = providers_cfg.entries.keys().collect::<Vec<_>>();
    names.sort();
    names
        .into_iter()
        .position(|name| name == provider_name)
        .unwrap_or(0)
}

fn effective_model_for_execution(
    model: &ModelConfig,
    provider_index: usize,
    providers_cfg: &ProvidersConfig,
) -> Result<(ProviderConfig, PromptMode), String> {
    providers_cfg.effective_provider(&model.providers[provider_index])
}

fn resume_migration_pool(
    resolved: &ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> ModelConfig {
    if let Some(model) = resolved.model.as_ref() {
        let mut effective = model.clone();
        effective.providers = model
            .providers
            .iter()
            .filter_map(|provider| providers_cfg.effective_provider(provider).ok().map(|p| p.0))
            .collect();
        return effective;
    }

    let mut names = providers_cfg.entries.keys().cloned().collect::<Vec<_>>();
    names.sort();
    let mut providers = Vec::new();
    for name in names {
        let is_candidate = name == resolved.active_provider
            || providers_cfg
                .get(&name)
                .is_some_and(|entry| entry.session_storage.is_some());
        if is_candidate && let Ok((provider, _)) = providers_cfg.runtime_provider(&name) {
            providers.push(provider);
        }
    }
    ModelConfig {
        name: "<provider-default>".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers,
        inputs: Vec::new(),
    }
}

fn resolve_parent_invocation_id(state: &StateDb) -> Option<i64> {
    let raw = std::env::var("OULIPOLY_PARENT_INVOCATION").ok()?;
    let composite = CompositeInvocationId::parse_env_value(&raw).ok()?;
    let record = state.get_invocation_by_uuid(&composite.id).ok()??;
    if record.provider_name.as_deref() == Some(composite.source.as_str()) {
        Some(record.id)
    } else {
        None
    }
}
