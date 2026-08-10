use std::collections::HashMap;
use std::path::Path;

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
    if request.origin_session_id != session_id {
        return Err("Fresh continuation origin session does not match --resume".to_string());
    }
    if working_dir.is_some_and(|path| path != request.worktree) {
        return Err("Fresh continuation worktree does not match --project".to_string());
    }

    let store_state = ProductionStateDbOpener.open_default()?;
    let observation_state = ProductionStateDbOpener.open_default()?;
    let effective_worktree = request.worktree.clone();
    let outcome = execute_with_callbacks(
        request,
        store_state,
        &observation_state,
        |reserved, _| {
            let mut prepared = match prepare_resume(
                agent_runtime_services,
                model_name,
                session_id,
                target_kind,
                prompt,
                file,
                submission_token,
                Some(&effective_worktree),
                models_dir_override,
            )
            .map_err(invocation_failed)?
            {
                Ok(prepared) => prepared,
                Err(exit_code) => {
                    return Err(invocation_failed(format!(
                        "Resume preparation exited with status {exit_code}"
                    )));
                }
            };
            run_prepared_resume(
                agent_runtime_services,
                &mut prepared,
                Some(reserved),
                None,
                session_id,
                Some(&effective_worktree),
            )
            .map(|_| ())
            .map_err(invocation_failed)
        },
        |reserved, context, resume| {
            let (model, all_models, models_dir) =
                load_target_model(&context.request.target_model, models_dir_override)?;
            let fresh_prompt = continuation_request::fresh_prompt(context, resume);
            run_reserved_with_balancing(
                agent_runtime_services,
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
        },
    );
    Ok(report_outcome(&outcome))
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
    let providers =
        ProvidersConfig::load(&default_config_root().join("providers.toml")).unwrap_or_default();
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let models = load_models(&models_dir, Some(&providers)).map_err(invocation_failed)?;
    let model = models
        .get(target_model)
        .cloned()
        .ok_or_else(|| ContinuationBlock {
            kind: ContinuationBlockKind::InvalidEvidence,
            message: format!("Fresh continuation target model not found: {target_model}"),
        })?;
    Ok((model, models, models_dir))
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
