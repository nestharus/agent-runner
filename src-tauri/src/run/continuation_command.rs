//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `formatter`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/run/continuation_command.rs
//!     role: adapter
//!     Translates:
//!       - fresh-continuation-request-and-handoff-contract
//!       - oulipoly-runtime-fresh-continuation-port-contract
//!       - oulipoly-state-continuation-and-invocation-contract
//!       - prepared-headless-resume-execution-contract
//!       - reserved-balancing-execution-contract
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oulipoly_config::{ModelConfig, ProvidersConfig, load_models};
use oulipoly_runtime::fresh_continuation::{
    ContinuationBlock, ContinuationBlockKind, DefaultContinuationEvidenceValidator,
    FreshContinuation, FreshContinuationCoordinator, FreshContinuationOutcome,
    FreshContinuationRequest, InvocationOutcome, StateDbContinuationStore, ValidatedContinuation,
};
use oulipoly_state::repositories::{ProductionStateDbOpener, StateDbOpener};
use oulipoly_state::{InboxTargetKind, StateDb};

use super::{
    balancing::run_reserved_with_balancing,
    continuation_artifact::FilesystemContinuationArtifactSource,
    continuation_fresh::ContinuationFreshRunner,
    continuation_handoff::FilesystemHandoffPublisher,
    continuation_request,
    continuation_resume::ContinuationResumeRunner,
    reservation::ReservedRun,
    resume::{prepare_resume, run_prepared_resume},
};
use crate::cli::paths::{default_config_root, default_models_dir};
use crate::wiring;

struct ResumeExecution<'a> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    model_name: Option<&'a str>,
    session_id: &'a str,
    target_kind: InboxTargetKind,
    prompt: Option<&'a str>,
    file: Option<&'a Path>,
    submission_token: Option<&'a str>,
    working_dir: &'a Path,
    models_dir_override: Option<&'a Path>,
}

#[allow(clippy::too_many_arguments)]
fn resume_execution<'a>(
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    model_name: Option<&'a str>,
    session_id: &'a str,
    target_kind: InboxTargetKind,
    prompt: Option<&'a str>,
    file: Option<&'a Path>,
    submission_token: Option<&'a str>,
    working_dir: &'a Path,
    models_dir_override: Option<&'a Path>,
) -> ResumeExecution<'a> {
    ResumeExecution {
        agent_runtime_services,
        model_name,
        session_id,
        target_kind,
        prompt,
        file,
        submission_token,
        working_dir,
        models_dir_override,
    }
}

impl ResumeExecution<'_> {
    fn execute(
        &self,
        reserved: &ReservedRun,
        _context: &ValidatedContinuation,
    ) -> Result<(), ContinuationBlock> {
        let mut prepared = self.prepare()?;
        run_prepared_resume(
            self.agent_runtime_services,
            &mut prepared,
            Some(reserved),
            None,
            self.session_id,
            Some(self.working_dir),
        )
        .map(|_| ())
        .map_err(invocation_failed)
    }

    fn prepare(&self) -> Result<super::resume::PreparedHeadlessResumeExecution, ContinuationBlock> {
        match prepare_resume(
            self.agent_runtime_services,
            self.model_name,
            self.session_id,
            self.target_kind,
            self.prompt,
            self.file,
            self.submission_token,
            Some(self.working_dir),
            self.models_dir_override,
        )
        .map_err(invocation_failed)?
        {
            Ok(prepared) => Ok(prepared),
            Err(exit_code) => Err(resume_preparation_failed(exit_code)),
        }
    }
}

struct FreshExecution<'a> {
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    models_dir_override: Option<&'a Path>,
}

fn fresh_execution<'a>(
    agent_runtime_services: &'a wiring::AgentRuntimeServices,
    models_dir_override: Option<&'a Path>,
) -> FreshExecution<'a> {
    FreshExecution {
        agent_runtime_services,
        models_dir_override,
    }
}

impl FreshExecution<'_> {
    fn execute(
        &self,
        reserved: &ReservedRun,
        context: &ValidatedContinuation,
        resume: &InvocationOutcome,
    ) -> Result<(), ContinuationBlock> {
        let (model, all_models, models_dir) =
            load_target_model(&context.request.target_model, self.models_dir_override)?;
        let fresh_prompt = continuation_request::fresh_prompt(context, resume);
        run_reserved_with_balancing(
            self.agent_runtime_services,
            &ProductionStateDbOpener,
            &model,
            &fresh_prompt,
            &all_models,
            &models_dir,
            Some(&context.request.worktree),
            &HashMap::new(),
            reserved,
        )
        .map(|_| ())
        .map_err(invocation_failed)
    }
}

pub(in crate::run) fn execute_with_callbacks(
    request: FreshContinuationRequest,
    store_state: StateDb,
    observation_state: &StateDb,
    resume_execution: impl FnMut(&ReservedRun, &ValidatedContinuation) -> Result<(), ContinuationBlock>,
    fresh_execution: impl FnMut(
        &ReservedRun,
        &ValidatedContinuation,
        &InvocationOutcome,
    ) -> Result<(), ContinuationBlock>,
) -> FreshContinuationOutcome {
    let validator = DefaultContinuationEvidenceValidator::new(
        FilesystemContinuationArtifactSource::new(&request.planning_root),
    );
    let store = StateDbContinuationStore::new(store_state);
    let resume = ContinuationResumeRunner::new(observation_state, resume_execution);
    let fresh = ContinuationFreshRunner::new(observation_state, fresh_execution);
    let publisher = FilesystemHandoffPublisher::new(request.planning_root.clone());

    FreshContinuationCoordinator::new(validator, store, resume, fresh, publisher).execute(request)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: Option<&str>,
    session_id: &str,
    target_kind: InboxTargetKind,
    prompt: Option<&str>,
    file: Option<&Path>,
    submission_token: Option<&str>,
    working_dir: Option<&Path>,
    models_dir_override: Option<&Path>,
    request_path: &Path,
) -> Result<i32, String> {
    let request = continuation_request::read(request_path).map_err(format_block)?;
    validate_request_target(&request, session_id, working_dir)?;
    let store_state = ProductionStateDbOpener.open_default()?;
    let observation_state = ProductionStateDbOpener.open_default()?;
    let effective_worktree = request.worktree.clone();
    let resume_execution = resume_execution(
        agent_runtime_services,
        model_name,
        session_id,
        target_kind,
        prompt,
        file,
        submission_token,
        &effective_worktree,
        models_dir_override,
    );
    let fresh_execution = fresh_execution(agent_runtime_services, models_dir_override);
    let outcome = execute_with_callbacks(
        request,
        store_state,
        &observation_state,
        |reserved, context| resume_execution.execute(reserved, context),
        |reserved, context, resume| fresh_execution.execute(reserved, context, resume),
    );
    Ok(report_outcome(&outcome))
}

fn validate_request_target(
    request: &FreshContinuationRequest,
    session_id: &str,
    working_dir: Option<&Path>,
) -> Result<(), String> {
    if request.origin_session_id != session_id {
        return Err("Fresh continuation origin session does not match --resume".to_string());
    }
    if working_dir.is_some_and(|path| path != request.worktree) {
        return Err("Fresh continuation worktree does not match --project".to_string());
    }
    Ok(())
}

fn load_target_model(
    target_model: &str,
    models_dir_override: Option<&Path>,
) -> Result<
    (
        ModelConfig,
        HashMap<String, ModelConfig>,
        std::path::PathBuf,
    ),
    ContinuationBlock,
> {
    let providers = load_provider_configuration();
    let models_dir = resolve_models_directory(models_dir_override);
    let models = load_model_configuration(&models_dir, &providers).map_err(invocation_failed)?;
    let model = require_target_model(target_model, &models)?;
    Ok(target_model_configuration(model, models, models_dir))
}

fn load_provider_configuration() -> ProvidersConfig {
    ProvidersConfig::load(&default_config_root().join("providers.toml")).unwrap_or_default()
}

fn resolve_models_directory(models_dir_override: Option<&Path>) -> PathBuf {
    models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir)
}

fn load_model_configuration(
    models_dir: &Path,
    providers: &ProvidersConfig,
) -> Result<HashMap<String, ModelConfig>, oulipoly_config::ModelError> {
    load_models(models_dir, Some(providers))
}

fn require_target_model(
    target_model: &str,
    models: &HashMap<String, ModelConfig>,
) -> Result<ModelConfig, ContinuationBlock> {
    let Some(model) = models.get(target_model).cloned() else {
        return Err(target_model_not_found(target_model));
    };
    Ok(model)
}

fn target_model_not_found(target_model: &str) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvalidEvidence,
        message: format_target_model_not_found(target_model),
    }
}

fn format_target_model_not_found(target_model: &str) -> String {
    format!("Fresh continuation target model not found: {target_model}")
}

fn target_model_configuration(
    model: ModelConfig,
    models: HashMap<String, ModelConfig>,
    models_dir: PathBuf,
) -> (ModelConfig, HashMap<String, ModelConfig>, PathBuf) {
    (model, models, models_dir)
}

fn report_outcome(outcome: &FreshContinuationOutcome) -> i32 {
    match outcome {
        FreshContinuationOutcome::Continued {
            continuation_id,
            handoff,
            ..
        } => {
            eprintln!(
                "Fresh continuation {continuation_id} published {} (sha256 {})",
                handoff.path.display(),
                handoff.sha256
            );
            0
        }
        FreshContinuationOutcome::Blocked { reason, .. } => {
            eprintln!("Fresh continuation blocked: {}", reason.message);
            1
        }
        FreshContinuationOutcome::Failed { reason, .. } => {
            eprintln!("Fresh continuation failed: {}", reason.message);
            1
        }
    }
}

fn format_block(block: ContinuationBlock) -> String {
    format!("Fresh continuation {:?}: {}", block.kind, block.message)
}

fn invocation_failed(error: impl std::fmt::Display) -> ContinuationBlock {
    ContinuationBlock {
        kind: ContinuationBlockKind::InvocationFailed,
        message: error.to_string(),
    }
}

fn resume_preparation_failed(exit_code: i32) -> ContinuationBlock {
    invocation_failed(format!("Resume preparation exited with status {exit_code}"))
}
