//! Role: orchestration.
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs
//!     role: intrinsic-surface
//!     Domain: external-provider dispatch orchestration
//!     Owns:
//!       - provider artifact lookup and capability gate sequence
//!       - sibling error_mapper coupling
//!       - sibling request_builder coupling
//!       - terminal-classify handoff hook
//!       - launch-result mapper handoff
//! ```

use super::capability_gate::gate_required_capabilities;
use super::client_invoker::{invoke_provider_launch, invoke_provider_policy};
use super::context::{AccountSelection, ExternalProviderDispatchContext};
use super::error_mapper::{
    invalid_provider_input_error, map_provider_client_error, map_registry_error,
    protocol_service_error, provider_client_error_is_rotatable, service_error,
};
use super::launch_result_mapper::{
    PROVIDER_SESSION_MARKER, launch_provider_session_id,
    map_launch_result_with_terminal_classification, marker_provider_session_id,
};
use super::policy_transform::apply_policy_transform;
use super::request_builder::{build_launch_candidate, build_launch_request, build_policy_request};
use super::terminal_classify_handoff::classify_after_launch_success;
use crate::executor::ExecutionResult;
use crate::executor::cli::spawn_identity::{
    RunningRuntimeGeneration, SpawnIdentityContext, SpawnRuntimeMode, backfill_captured_session_id,
    child_custody_test_fault, context_from_parent_invocation_env, mark_runtime_generation_exited,
    mark_runtime_generation_orderly_completed, mark_runtime_generation_spawn_failed,
    record_child_identity, register_runtime_generation_starting,
};
use crate::provider_registry::ProviderRegistry;
use crate::services::ServiceError;
use oulipoly_provider::client::ProcessSpawnObserver;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::ProcessStatus;
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_provider::stream::LaunchResult;
use oulipoly_provider::stream::{DecodedLaunchEvent, LaunchEventObserver};
use std::sync::{Arc, Mutex};

type RecordedLaunchGeneration = Arc<Mutex<Option<Result<RunningRuntimeGeneration, String>>>>;

/// A single account attempt either succeeded, hit a deterministic terminal
/// failure (fail fast), or hit a rotatable transport-class failure (try the
/// next pool account).
struct AccountAttemptError {
    service_error: ServiceError,
    rotatable: bool,
}

fn terminal_attempt_error(service_error: ServiceError) -> AccountAttemptError {
    AccountAttemptError {
        service_error,
        rotatable: false,
    }
}

fn classify_provider_client_attempt_error(error: ProviderClientError) -> AccountAttemptError {
    AccountAttemptError {
        rotatable: provider_client_error_is_rotatable(&error),
        service_error: map_provider_client_error(error),
    }
}

pub(crate) fn dispatch(
    registry: &ProviderRegistry,
    context: ExternalProviderDispatchContext,
) -> Result<ExecutionResult, ServiceError> {
    // Artifact lookup, describe, and the capability gate are model-scoped: every
    // account in the pool shares one provider artifact, so they run once and
    // their failures are terminal (rotating accounts cannot fix a missing or
    // runtime-disabled artifact).
    let artifact = registry
        .enabled_artifact_for_model(&context.model.name)
        .map_err(map_registry_error)?;
    let describe = registry
        .describe_model_provider(&context.model.name)
        .map_err(map_registry_error)?;
    gate_required_capabilities(&describe).map_err(service_error)?;
    let provider_supports_prompt_acceptance_v1 = describe.capabilities.prompt_acceptance_v1;

    // FIX #32: rotate over the pool on transport-timeout / account-unavailable
    // classes, terminal-failing only once every account has been tried. The
    // order is bounded by the pool size — the originally selected account first,
    // then the remaining accounts in declaration order.
    let order = account_rotation_order(&context);
    let last_index = order.len().saturating_sub(1);
    for (position, account) in order.into_iter().enumerate() {
        let account_context = context.with_account(account);
        match attempt_account_dispatch(
            registry,
            &artifact,
            &account_context,
            provider_supports_prompt_acceptance_v1,
        ) {
            Ok(result) => return Ok(result),
            Err(attempt) => {
                if attempt.rotatable && position < last_index {
                    continue;
                }
                return Err(attempt.service_error);
            }
        }
    }

    // `account_rotation_order` always yields at least the selected account, so
    // the loop above always returns; this guards a degenerate empty pool.
    Err(protocol_service_error("empty_provider_pool"))
}

fn account_rotation_order(context: &ExternalProviderDispatchContext) -> Vec<AccountSelection> {
    // Selected account first, preserving the caller-provided (possibly
    // canonicalized) provider config for that index.
    let mut order = vec![AccountSelection {
        provider: context.provider.clone(),
        provider_index: context.provider_index,
    }];
    for (index, provider) in context.model.providers.iter().enumerate() {
        if index != context.provider_index {
            order.push(AccountSelection {
                provider: provider.clone(),
                provider_index: index,
            });
        }
    }
    order
}

fn attempt_account_dispatch(
    registry: &ProviderRegistry,
    artifact: &ProviderArtifactRef,
    context: &ExternalProviderDispatchContext,
    provider_supports_prompt_acceptance_v1: bool,
) -> Result<ExecutionResult, AccountAttemptError> {
    let spawn_identity = external_launch_spawn_identity_context(context);
    let recorded_generation = recorded_launch_generation();
    let spawn_observer =
        external_launch_spawn_observer(spawn_identity.as_ref(), Arc::clone(&recorded_generation));
    let launch_event_observer =
        external_launch_event_observer(spawn_identity.as_ref(), Arc::clone(&recorded_generation));
    let client = registry.client_factory().client_for_with_observers(
        artifact.clone(),
        spawn_observer,
        launch_event_observer,
    );
    let candidate = build_launch_candidate(context)
        .map_err(|message| terminal_attempt_error(invalid_provider_input_error(message)))?;
    let policy_request = build_policy_request(context, &candidate, registry.host_options())
        .map_err(|_| terminal_attempt_error(protocol_service_error("schema_invalid_request")))?;
    let policy_result = invoke_provider_policy(&client, policy_request)
        .map_err(classify_provider_client_attempt_error)?;
    let candidate = apply_policy_transform(candidate, policy_result)
        .map_err(|error| terminal_attempt_error(service_error(error)))?;
    let launch_prompt_acceptance_v1_enabled =
        provider_supports_prompt_acceptance_v1 && candidate.prompt_acceptance.is_some();
    let launch_request = build_launch_request(
        context,
        &candidate,
        registry.host_options(),
        launch_prompt_acceptance_v1_enabled,
    )
    .map_err(|_| terminal_attempt_error(protocol_service_error("schema_invalid_request")))?;
    register_runtime_generation_starting(spawn_identity.as_ref()).map_err(|_| {
        terminal_attempt_error(protocol_service_error(
            "runtime_generation_registration_failed",
        ))
    })?;
    let launch_result = match invoke_provider_launch(&client, launch_request) {
        Ok(result) => result,
        Err(error) => {
            finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            return Err(classify_provider_client_attempt_error(error));
        }
    };
    if spawn_identity.is_some() {
        if require_recorded_external_generation(&recorded_generation).is_err() {
            finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            return Err(terminal_attempt_error(protocol_service_error(
                "runtime_generation_bind_failed",
            )));
        }
        if backfill_external_launch_session_id(
            spawn_identity.as_ref(),
            &recorded_generation,
            &launch_result,
        )
        .is_err()
        {
            finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            return Err(terminal_attempt_error(protocol_service_error(
                "runtime_generation_attach_failed",
            )));
        }
        let exit_code = launch_exit_code(&launch_result.exit.status);
        if mark_runtime_generation_orderly_completed(spawn_identity.as_ref(), exit_code, exit_code)
            .is_err()
        {
            finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            return Err(terminal_attempt_error(protocol_service_error(
                "runtime_generation_exit_failed",
            )));
        }
    }
    let classification = classify_after_launch_success(registry, context, &launch_result);

    Ok(map_launch_result_with_terminal_classification(
        launch_result,
        context.provider_index,
        &context.provider.name,
        classification,
        launch_prompt_acceptance_v1_enabled,
    ))
}

fn launch_exit_code(status: &ProcessStatus) -> Option<i32> {
    match status {
        ProcessStatus::Exited { code } => Some(*code),
        _ => None,
    }
}

fn external_launch_spawn_identity_context(
    context: &ExternalProviderDispatchContext,
) -> Option<SpawnIdentityContext> {
    context_from_parent_invocation_env(
        context.parent_invocation_env.as_deref(),
        &context.provider.name,
        Some(&context.model.name),
        context.start_known_provider_session_id.as_deref(),
        SpawnRuntimeMode::Headless,
        context.working_dir.as_deref(),
        context.models_dir.as_deref(),
    )
}

fn recorded_launch_generation() -> RecordedLaunchGeneration {
    Arc::new(Mutex::new(None))
}

fn external_launch_spawn_observer(
    context: Option<&SpawnIdentityContext>,
    recorded_generation: RecordedLaunchGeneration,
) -> Option<ProcessSpawnObserver> {
    let context = context.cloned()?;
    Some(ProcessSpawnObserver::new(move |child_id| {
        child_custody_test_fault("external_spawn_observer")?;
        let generation = record_child_identity(child_id, Some(&context)).and_then(|generation| {
            generation.ok_or_else(|| "Missing external runtime generation".to_string())
        });
        remember_recorded_launch_generation(&recorded_generation, generation)
    }))
}

fn external_launch_event_observer(
    context: Option<&SpawnIdentityContext>,
    recorded_generation: RecordedLaunchGeneration,
) -> Option<LaunchEventObserver> {
    let context = context.cloned()?;
    Some(LaunchEventObserver::new(move |event| {
        backfill_external_launch_session_marker(&context, &recorded_generation, event);
    }))
}

fn backfill_external_launch_session_marker(
    context: &SpawnIdentityContext,
    recorded_generation: &RecordedLaunchGeneration,
    event: &DecodedLaunchEvent,
) {
    let DecodedLaunchEvent::Marker { name, value, .. } = event else {
        return;
    };
    if name != PROVIDER_SESSION_MARKER {
        return;
    }
    let Some(session_id) = marker_provider_session_id(value) else {
        return;
    };
    let generation = recorded_generation
        .lock()
        .ok()
        .and_then(|generation| generation.as_ref()?.as_ref().ok().cloned());
    let _ = backfill_captured_session_id(Some(context), generation.as_ref(), &session_id);
}

fn remember_recorded_launch_generation(
    recorded_generation: &RecordedLaunchGeneration,
    generation: Result<RunningRuntimeGeneration, String>,
) -> Result<(), String> {
    let result = generation.clone().map(|_| ());
    *recorded_generation
        .lock()
        .map_err(|_| "External runtime generation lock poisoned".to_string())? = Some(generation);
    result
}

fn require_recorded_external_generation(
    recorded_generation: &RecordedLaunchGeneration,
) -> Result<RunningRuntimeGeneration, String> {
    recorded_generation
        .lock()
        .map_err(|_| "External runtime generation lock poisoned".to_string())?
        .clone()
        .ok_or_else(|| "External provider launch did not report a spawned process".to_string())?
}

fn finalize_failed_external_launch(
    context: Option<&SpawnIdentityContext>,
    recorded_generation: &RecordedLaunchGeneration,
) {
    let spawned = recorded_generation
        .lock()
        .ok()
        .and_then(|generation| generation.as_ref().map(Result::is_ok));
    if spawned == Some(true) {
        let _ = mark_runtime_generation_exited(context, None);
    } else {
        let _ = mark_runtime_generation_spawn_failed(context);
    }
}

fn backfill_external_launch_session_id(
    context: Option<&SpawnIdentityContext>,
    recorded_generation: &RecordedLaunchGeneration,
    result: &LaunchResult,
) -> Result<(), String> {
    let Some(session_id) = launch_provider_session_id(result) else {
        return Ok(());
    };
    let generation = require_recorded_external_generation(recorded_generation)?;
    backfill_captured_session_id(context, Some(&generation), &session_id)
}
