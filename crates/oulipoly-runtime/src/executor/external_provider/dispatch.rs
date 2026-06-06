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
use super::context::ExternalProviderDispatchContext;
use super::error_mapper::{
    invalid_provider_input_error, map_provider_client_error, map_registry_error,
    protocol_service_error, service_error,
};
use super::launch_result_mapper::{
    launch_provider_session_id, map_launch_result_with_terminal_classification,
};
use super::policy_transform::apply_policy_transform;
use super::request_builder::{build_launch_candidate, build_launch_request, build_policy_request};
use super::terminal_classify_handoff::classify_after_launch_success;
use crate::executor::ExecutionResult;
use crate::executor::cli::spawn_identity::{
    SpawnIdentityContext, SpawnRuntimeMode, backfill_captured_session_id,
    context_from_parent_invocation_env, record_child_identity,
};
use crate::provider_registry::ProviderRegistry;
use crate::services::ServiceError;
use oulipoly_provider::client::ProcessSpawnObserver;
use oulipoly_provider::stream::LaunchResult;
use oulipoly_state::pid_identity::ProcessIdentity;
use std::sync::{Arc, Mutex};

type RecordedLaunchIdentity = Arc<Mutex<Option<ProcessIdentity>>>;

pub(crate) fn dispatch(
    registry: &ProviderRegistry,
    context: ExternalProviderDispatchContext,
) -> Result<ExecutionResult, ServiceError> {
    let artifact = registry
        .enabled_artifact_for_model(&context.model.name)
        .map_err(map_registry_error)?;
    let describe = registry
        .describe_model_provider(&context.model.name)
        .map_err(map_registry_error)?;
    gate_required_capabilities(&describe).map_err(service_error)?;

    let spawn_identity = external_launch_spawn_identity_context(&context);
    let recorded_identity = recorded_launch_identity();
    let spawn_observer =
        external_launch_spawn_observer(spawn_identity.as_ref(), Arc::clone(&recorded_identity));
    let client = registry
        .client_factory()
        .client_for_with_spawn_observer(artifact, spawn_observer);
    let candidate = build_launch_candidate(&context).map_err(invalid_provider_input_error)?;
    let policy_request = build_policy_request(&context, &candidate)
        .map_err(|_| protocol_service_error("schema_invalid_request"))?;
    let policy_result =
        invoke_provider_policy(&client, policy_request).map_err(map_provider_client_error)?;
    let candidate = apply_policy_transform(candidate, policy_result).map_err(service_error)?;
    let launch_request = build_launch_request(&context, &candidate)
        .map_err(|_| protocol_service_error("schema_invalid_request"))?;
    let launch_result =
        invoke_provider_launch(&client, launch_request).map_err(map_provider_client_error)?;
    backfill_external_launch_session_id(
        spawn_identity.as_ref(),
        &recorded_identity,
        &launch_result,
    );
    let classification = classify_after_launch_success(registry, &context, &launch_result);

    Ok(map_launch_result_with_terminal_classification(
        launch_result,
        context.provider_index,
        &context.provider.name,
        classification,
    ))
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
    )
}

fn recorded_launch_identity() -> RecordedLaunchIdentity {
    Arc::new(Mutex::new(None))
}

fn external_launch_spawn_observer(
    context: Option<&SpawnIdentityContext>,
    recorded_identity: RecordedLaunchIdentity,
) -> Option<ProcessSpawnObserver> {
    let context = context.cloned()?;
    Some(ProcessSpawnObserver::new(move |child_id| {
        let identity = record_child_identity(child_id, Some(&context));
        remember_recorded_launch_identity(&recorded_identity, identity);
    }))
}

fn remember_recorded_launch_identity(
    recorded_identity: &RecordedLaunchIdentity,
    identity: Option<ProcessIdentity>,
) {
    if let Ok(mut recorded_identity) = recorded_identity.lock() {
        *recorded_identity = identity;
    }
}

fn backfill_external_launch_session_id(
    context: Option<&SpawnIdentityContext>,
    recorded_identity: &RecordedLaunchIdentity,
    result: &LaunchResult,
) {
    let Some(session_id) = launch_provider_session_id(result) else {
        return;
    };
    let identity = recorded_identity
        .lock()
        .ok()
        .and_then(|identity| identity.clone());
    backfill_captured_session_id(context, identity.as_ref(), &session_id);
}
