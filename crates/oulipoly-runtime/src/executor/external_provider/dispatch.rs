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

use super::attempt::ProviderLaunchFailure;
use super::capability_gate::gate_required_capabilities;
use super::client_invoker::{invoke_provider_launch, invoke_provider_policy};
use super::context::ExternalProviderDispatchContext;
use super::error_mapper::{
    invalid_provider_input_error, map_provider_client_error, map_registry_error,
    protocol_service_error, service_error,
};
use super::launch_result_mapper::{
    LaunchOutputArtifacts, PROVIDER_SESSION_MARKER, launch_failure_provider_session_id,
    launch_provider_session_id, map_launch_result_with_terminal_classification,
    map_missing_final_exit_with_prompt_acceptance, marker_provider_session_id,
};
use super::output_spool_observer::observe_output;
use super::policy_transform::apply_policy_transform;
use super::request_builder::{
    RETURN_CHANNEL_ENV, build_launch_candidate, build_launch_request, build_policy_request,
};
use super::terminal_classify_handoff::classify_after_launch_success;
use crate::executor::cli::spawn_identity::{
    GenerationOperationError, GenerationOperationOutcome, RunningRuntimeGeneration,
    SpawnIdentityContext, SpawnRuntimeMode, attach_captured_session_id, child_custody_test_fault,
    context_from_parent_invocation_env, exit_runtime_generation_outcome,
    mark_runtime_generation_orderly_completed, record_child_identity,
    register_runtime_generation_starting,
};
use crate::executor::cli::{prepare_return_channel, read_and_cleanup_return_channel};
use crate::executor::{ExecutionOutputSpool, ExecutionResult, ExternalProviderSessionAuthority};
use crate::provider_registry::ProviderRegistry;
use crate::services::ServiceError;
use crate::session_authority::{
    AuthoritativeSessionObservation, SessionAuthorityExpectation, VerifiedSessionAuthority,
    verify_session_authority,
};
use oulipoly_provider::client::ProcessSpawnObserver;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::ProcessStatus;
use oulipoly_provider::stream::{DecodedLaunchEvent, LaunchEventObserver, LaunchResult};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct LiveAttachmentFailure {
    verified: VerifiedSessionAuthority,
    cause: GenerationOperationError,
}

type RecordedAttachmentFailure = Arc<Mutex<Option<LiveAttachmentFailure>>>;

type RecordedLaunchGeneration = Arc<Mutex<Option<Result<RunningRuntimeGeneration, String>>>>;

/// A single account attempt either succeeded, hit a deterministic terminal
/// failure (fail fast), or hit a rotatable transport-class failure (try the
/// next pool account).
pub(super) struct AccountAttemptError {
    pub(super) service_error: ServiceError,
    pub(super) failure: Box<ProviderLaunchFailure>,
}

pub(super) fn terminal_attempt_error(service_error: ServiceError) -> AccountAttemptError {
    AccountAttemptError {
        failure: Box::new(ProviderLaunchFailure::Execution(service_error.clone())),
        service_error,
    }
}

fn classify_provider_client_attempt_error(error: ProviderClientError) -> AccountAttemptError {
    AccountAttemptError {
        failure: Box::new(ProviderLaunchFailure::Provider(error.clone())),
        service_error: map_provider_client_error(error),
    }
}

pub(crate) fn dispatch(
    registry: &ProviderRegistry,
    context: ExternalProviderDispatchContext,
) -> Result<ExecutionResult, ServiceError> {
    attempt_account_dispatch(registry, &context).map_err(|attempt| attempt.service_error)
}

pub(super) fn attempt_account_dispatch(
    registry: &ProviderRegistry,
    context: &ExternalProviderDispatchContext,
) -> Result<ExecutionResult, AccountAttemptError> {
    let endpoint = registry
        .preflight_account_with_custody(
            &context.provider.name,
            context.attempt.as_ref().map(|a| a.actors.clone()),
        )
        .map_err(|error| {
            let source = match &error {
                crate::provider_registry::ProviderRegistryError::ProviderTransport {
                    source,
                    ..
                }
                | crate::provider_registry::ProviderRegistryError::ProviderProtocol {
                    source,
                    ..
                }
                | crate::provider_registry::ProviderRegistryError::ProviderDescribeFailed {
                    source,
                    ..
                } => Some((**source).clone()),
                _ => None,
            };
            let mut mapped = terminal_attempt_error(map_registry_error(error));
            if let Some(source) = source {
                mapped.failure = Box::new(ProviderLaunchFailure::Provider(source));
            }
            mapped
        })?;
    if let Some(attempt) = &context.attempt {
        attempt.bind_endpoint(&endpoint).map_err(|_| {
            terminal_attempt_error(protocol_service_error("endpoint_identity_bind_failed"))
        })?;
    }
    let settings_id = endpoint
        .settings_id()
        .map_err(|error| terminal_attempt_error(map_registry_error(error)))?;
    let account_context = context.with_settings_id(settings_id);
    let context = &account_context;
    let output_spool = ExecutionOutputSpool::new().map_err(|_| {
        terminal_attempt_error(protocol_service_error("launch_output_spool_create_failed"))
    })?;
    if let Some(attempt) = &context.attempt {
        attempt
            .evidence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .output = Some(output_spool.clone());
    }
    let spawn_identity = external_launch_spawn_identity_context(context);
    let recorded_generation = recorded_launch_generation();
    let spawn_observer =
        external_launch_spawn_observer(spawn_identity.as_ref(), Arc::clone(&recorded_generation));
    let attachment_failure = Arc::new(Mutex::new(None));
    let launch_event_observer = external_launch_event_observer(
        output_spool.clone(),
        Arc::clone(&attachment_failure),
        spawn_identity.clone(),
        Arc::clone(&recorded_generation),
        endpoint.account_name().to_string(),
        context.clone(),
    );
    let client = registry
        .client_factory()
        .client_from_pinned_with_observers(
            endpoint.client(),
            spawn_observer,
            launch_event_observer,
            context.attempt.as_ref().map(|a| a.actors.clone()),
        )
        .map_err(classify_provider_client_attempt_error)?;
    let describe = endpoint.capabilities();
    let provider_instance_id = format!("{}-instance", describe.provider_id);
    let session_authority = ExternalProviderSessionAuthority {
        account_name: endpoint.account_name().to_string(),
        provider_instance_id: provider_instance_id.clone(),
        settings_id: settings_id.to_string(),
    };
    gate_required_capabilities(describe)
        .map_err(|error| terminal_attempt_error(service_error(error)))?;
    if !describe.capabilities.launch_output_v1 {
        return Err(terminal_attempt_error(protocol_service_error(
            "complete_launch_output_unsupported",
        )));
    }
    let provider_supports_prompt_acceptance_v1 = describe.capabilities.prompt_acceptance_v1;
    let candidate = build_launch_candidate(context)
        .map_err(|message| terminal_attempt_error(invalid_provider_input_error(message)))?;
    let policy_request = build_policy_request(
        context,
        &candidate,
        &provider_instance_id,
        registry.host_options(),
    )
    .map_err(|_| terminal_attempt_error(protocol_service_error("schema_invalid_request")))?;
    let policy_result = invoke_provider_policy(&client, policy_request)
        .map_err(classify_provider_client_attempt_error)?;
    let mut candidate = apply_policy_transform(candidate, policy_result)
        .map_err(|error| terminal_attempt_error(service_error(error)))?;
    let return_channel = if let Some(attempt) = &context.attempt {
        let allocation = &attempt.allocation;
        Some(
            crate::executor::ReturnChannel::for_attempt(
                &allocation.channel_root,
                allocation.parent_invocation_uuid,
                allocation.lease.owner.logical_launch_id,
                allocation.lease.owner.attempt_id,
                allocation.lease.owner.invocation_uuid,
            )
            .map_err(|_| {
                terminal_attempt_error(protocol_service_error("return_channel_create_failed"))
            })?,
        )
    } else {
        prepare_return_channel(context.parent_invocation_env.as_deref()).map_err(|_| {
            terminal_attempt_error(protocol_service_error("return_channel_create_failed"))
        })?
    };
    if let Some(return_channel) = return_channel.as_ref() {
        candidate.env.insert(
            RETURN_CHANNEL_ENV.to_string(),
            return_channel.path().display().to_string(),
        );
    }
    let launch_prompt_acceptance_v1_enabled =
        provider_supports_prompt_acceptance_v1 && candidate.prompt_acceptance.is_some();
    let launch_request = build_launch_request(
        context,
        &candidate,
        &provider_instance_id,
        endpoint.family(),
        registry.host_options(),
        launch_prompt_acceptance_v1_enabled,
        describe.capabilities.launch_output_v1,
    )
    .map_err(|_| terminal_attempt_error(protocol_service_error("schema_invalid_request")))?;
    if let Some(attempt) = &context.attempt {
        let mut evidence = attempt.evidence.lock().unwrap_or_else(|e| e.into_inner());
        evidence.prompt = launch_request
            .get("params")
            .and_then(|p| p.get("prompt_acceptance"))
            .and_then(|p| serde_json::from_value(p.clone()).ok());
    }
    if context.attempt.is_none() {
        register_runtime_generation_starting(spawn_identity.as_ref()).map_err(|_| {
            terminal_attempt_error(protocol_service_error(
                "runtime_generation_registration_failed",
            ))
        })?;
    }
    let standalone_channel = if let Some(attempt) = &context.attempt {
        attempt
            .evidence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .channel = return_channel;
        None
    } else {
        return_channel
    };
    let launch_outcome = invoke_provider_launch(&client, launch_request);
    let returned_artifacts = match read_and_cleanup_return_channel(standalone_channel) {
        Ok(artifacts) => artifacts,
        Err(message) if launch_outcome.is_ok() => {
            let _ = finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            return Err(terminal_attempt_error(ServiceError::Dependency { message }));
        }
        Err(message) => {
            tracing::warn!(
                message,
                "Return channel quarantined; retaining the original provider failure"
            );
            Vec::new()
        }
    };
    let launch_result = match launch_outcome {
        Ok(result) => result,
        Err(error) => {
            let cleanup =
                finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            // The observer records verified typed custody before returning its transport error.
            // Do not infer attachment failure from provider-controlled diagnostics.
            let live_failure = attachment_failure
                .lock()
                .ok()
                .and_then(|failure| failure.clone());
            if let Some(failure) = live_failure {
                output_spool.mark_incomplete();
                let result = ExecutionResult {
                    stdout: Vec::new(),
                    stderr: String::new(),
                    output_spool: Some(output_spool),
                    exit_code: -1,
                    provider_index: context.provider_index,
                    session_capture: crate::executor::SessionCaptureResult {
                        session_id: None,
                        method: crate::executor::SessionCaptureMethod::ExternalProviderLaunch(
                            session_authority.clone(),
                        ),
                    },
                    resume_acceptance: None,
                    terminal_reason: None,
                    terminal_signal: None,
                    produced_assistant_response: false,
                    prompt_acceptance_attestation: None,
                    captured_child_invocations: Vec::new(),
                    returned_artifacts,
                };
                let mut result = failed_finalization_result(
                    result,
                    Some(&failure.verified),
                    "runtime_generation_attach_failed",
                    failure.cause,
                    cleanup,
                );
                let signal = result.terminal_signal.as_mut().expect("failure signal");
                signal.provider_name = context.provider.name.clone();
                signal.evidence.push_str(
                    ";output=incomplete;output_artifacts=<invocation_uuid>.partial.{stdout,stderr}",
                );
                return Ok(result);
            }
            let verified_failure_session = match verify_optional_failure_session(
                context,
                &endpoint,
                launch_failure_provider_session_id(&error).as_deref(),
            ) {
                Ok(verified) => verified,
                Err(error) => {
                    return Err(terminal_attempt_error(protocol_service_error(
                        error.protocol_kind(),
                    )));
                }
            };
            if let Some(mut result) = map_missing_final_exit_with_prompt_acceptance(
                &error,
                verified_failure_session.as_ref(),
                context.provider_index,
                &context.provider.name,
                launch_prompt_acceptance_v1_enabled,
                returned_artifacts,
                &session_authority,
            ) {
                // The verified session authorizes failure mapping, not complete
                // output. Retain exactly the observer's decoded prefix.
                output_spool.mark_incomplete();
                result.output_spool = Some(output_spool);
                if let Some(signal) = &mut result.terminal_signal {
                    signal.evidence.push_str(";output=incomplete;output_artifacts=<invocation_uuid>.partial.{stdout,stderr}");
                }
                return Ok(result);
            }
            return Err(classify_provider_client_attempt_error(error));
        }
    };
    let verified_session = match verify_launch_session_authority(context, &endpoint, &launch_result)
    {
        Ok(verified) => verified,
        Err(error) => {
            let _ = finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            return Err(terminal_attempt_error(protocol_service_error(
                error.protocol_kind(),
            )));
        }
    };
    if spawn_identity.is_some() {
        if require_recorded_external_generation(&recorded_generation).is_err() {
            let _ = finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            return Err(terminal_attempt_error(protocol_service_error(
                "runtime_generation_bind_failed",
            )));
        }
        let attachment = backfill_external_launch_session_id(
            spawn_identity.as_ref(),
            &recorded_generation,
            verified_session.as_ref(),
        );
        let failure = match attachment {
            Err(error) => Some(("runtime_generation_attach_failed", error)),
            Ok(_) => {
                let exit_code = launch_exit_code(&launch_result.exit.status);
                mark_runtime_generation_orderly_completed(
                    spawn_identity.as_ref(),
                    exit_code,
                    exit_code,
                )
                .err()
                .map(|error| ("runtime_generation_exit_failed", error))
            }
        };
        if let Some((stage, error)) = failure {
            let cleanup =
                finalize_failed_external_launch(spawn_identity.as_ref(), &recorded_generation);
            let result = map_launch_result_with_terminal_classification(
                launch_result,
                context.provider_index,
                &context.provider.name,
                None,
                launch_prompt_acceptance_v1_enabled,
                LaunchOutputArtifacts {
                    spool: output_spool,
                    returned_artifacts,
                },
                &session_authority,
            );
            return Ok(failed_finalization_result(
                result,
                verified_session.as_ref(),
                stage,
                error,
                cleanup,
            ));
        }
    }

    let classification =
        classify_after_launch_success(registry, &client, describe, context, &launch_result);

    Ok(map_launch_result_with_terminal_classification(
        launch_result,
        context.provider_index,
        &context.provider.name,
        classification,
        launch_prompt_acceptance_v1_enabled,
        LaunchOutputArtifacts {
            spool: output_spool,
            returned_artifacts,
        },
        &session_authority,
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
    if let Some(attempt) = &context.attempt {
        return Some(attempt.spawn.clone());
    }
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
        remember_recorded_launch_generation(&recorded_generation, generation)?;
        child_custody_test_fault("external_status_poll")
    }))
}

fn external_launch_event_observer(
    output_spool: ExecutionOutputSpool,
    attachment_failure: RecordedAttachmentFailure,
    spawn_identity: Option<SpawnIdentityContext>,
    recorded_generation: RecordedLaunchGeneration,
    observed_account_name: String,
    dispatch_context: ExternalProviderDispatchContext,
) -> Option<LaunchEventObserver> {
    Some(LaunchEventObserver::new(move |event| {
        if let Some(attempt) = &dispatch_context.attempt {
            attempt.observe(&dispatch_context, event)?;
        }
        observe_output(&output_spool, event)?;
        bind_external_launch_session_from_event(
            spawn_identity.as_ref(),
            &recorded_generation,
            &attachment_failure,
            &dispatch_context.provider.name,
            dispatch_context.start_known_provider_session_id.as_deref(),
            &observed_account_name,
            event,
        )
    }))
}

fn bind_external_launch_session_from_event(
    context: Option<&SpawnIdentityContext>,
    recorded_generation: &RecordedLaunchGeneration,
    attachment_failure: &RecordedAttachmentFailure,
    account_name: &str,
    expected_provider_session_id: Option<&str>,
    observed_account_name: &str,
    event: &DecodedLaunchEvent,
) -> Result<(), String> {
    let Some(provider_session_id) = provider_session_id_from_launch_event(event) else {
        return Ok(());
    };
    let verified = verify_session_authority(
        SessionAuthorityExpectation {
            account_name,
            provider_session_id: expected_provider_session_id,
        },
        Some(AuthoritativeSessionObservation {
            account_name: observed_account_name,
            provider_session_id: &provider_session_id,
        }),
    )
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "provider session marker produced no verified authority".to_string())?;
    let attachment = require_recorded_external_generation(recorded_generation)
        .map_err(|_| GenerationOperationError::MissingGeneration)
        .and_then(|generation| {
            attach_captured_session_id(context, Some(&generation), verified.provider_session_id())
        });
    match attachment {
        Ok(_) => Ok(()),
        Err(cause) => {
            *attachment_failure
                .lock()
                .map_err(|_| "attachment failure custody unavailable".to_string())? =
                Some(LiveAttachmentFailure { verified, cause });
            Err("runtime_generation_attach_failed".to_string())
        }
    }
}

pub(super) fn provider_session_id_from_launch_event(event: &DecodedLaunchEvent) -> Option<String> {
    let DecodedLaunchEvent::Marker { name, value, .. } = event else {
        return None;
    };
    (name == PROVIDER_SESSION_MARKER)
        .then(|| marker_provider_session_id(value))
        .flatten()
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
) -> Result<GenerationOperationOutcome, GenerationOperationError> {
    let spawned = recorded_generation
        .lock()
        .map_err(|_| GenerationOperationError::Unknown)?
        .as_ref()
        .map(Result::is_ok);
    let reason = if spawned == Some(true) {
        oulipoly_state::mailbox::RuntimeTerminalReason::AbnormalTermination
    } else {
        oulipoly_state::mailbox::RuntimeTerminalReason::StartupFailed
    };
    let outcome = exit_runtime_generation_outcome(context, reason, None);
    // Also retain cleanup evidence on the earlier provider/authority failure paths.
    tracing::warn!(cleanup = ?outcome, "External launch failure cleanup");
    outcome
}

fn backfill_external_launch_session_id(
    context: Option<&SpawnIdentityContext>,
    recorded_generation: &RecordedLaunchGeneration,
    verified: Option<&VerifiedSessionAuthority>,
) -> Result<GenerationOperationOutcome, GenerationOperationError> {
    let Some(verified) = verified else {
        return Ok(GenerationOperationOutcome::NotRequired);
    };
    let generation = require_recorded_external_generation(recorded_generation)
        .map_err(|_| GenerationOperationError::MissingGeneration)?;
    attach_captured_session_id(context, Some(&generation), verified.provider_session_id())
}

fn failed_finalization_result(
    mut result: ExecutionResult,
    verified: Option<&VerifiedSessionAuthority>,
    stage: &'static str,
    error: GenerationOperationError,
    cleanup: Result<GenerationOperationOutcome, GenerationOperationError>,
) -> ExecutionResult {
    use crate::executor::terminal_signal::{TerminalSignal, TerminalSignalKind};
    result.exit_code = -1;
    result.terminal_reason = Some(stage.to_string());
    result.terminal_signal = Some(TerminalSignal {
        kind: TerminalSignalKind::SpawnError,
        provider_name: result
            .terminal_signal
            .as_ref()
            .map(|signal| signal.provider_name.clone())
            .unwrap_or_default(),
        evidence: format!("{stage};cause={error};cleanup={cleanup:?}"),
        observed_at: std::time::SystemTime::now(),
    });
    // Only authority-verified public identity survives a host finalization failure.
    result.session_capture.session_id =
        verified.map(|session| session.provider_session_id().to_string());
    result
}

fn verify_launch_session_authority(
    context: &ExternalProviderDispatchContext,
    endpoint: &crate::provider_registry::PinnedProviderEndpoint,
    result: &LaunchResult,
) -> Result<Option<VerifiedSessionAuthority>, crate::session_authority::SessionAuthorityError> {
    let observed_session_id = launch_provider_session_id(result);
    verify_session_authority(
        SessionAuthorityExpectation {
            account_name: &context.provider.name,
            provider_session_id: context.start_known_provider_session_id.as_deref(),
        },
        observed_session_id
            .as_deref()
            .map(|provider_session_id| AuthoritativeSessionObservation {
                account_name: endpoint.account_name(),
                provider_session_id,
            }),
    )
}

fn verify_optional_failure_session(
    context: &ExternalProviderDispatchContext,
    endpoint: &crate::provider_registry::PinnedProviderEndpoint,
    observed_session_id: Option<&str>,
) -> Result<Option<VerifiedSessionAuthority>, crate::session_authority::SessionAuthorityError> {
    let Some(observed_session_id) = observed_session_id else {
        return Ok(None);
    };
    verify_session_authority(
        SessionAuthorityExpectation {
            account_name: &context.provider.name,
            provider_session_id: context.start_known_provider_session_id.as_deref(),
        },
        Some(AuthoritativeSessionObservation {
            account_name: endpoint.account_name(),
            provider_session_id: observed_session_id,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed_fixture_output() -> ExecutionOutputSpool {
        use oulipoly_provider::generated::{LAUNCH_OUTPUT_COMPLETE_MARKER_V1, LAUNCH_OUTPUT_V1};
        use sha2::{Digest, Sha256};
        let spool = ExecutionOutputSpool::new().unwrap();
        spool
            .observe(&DecodedLaunchEvent::Stdout {
                seq: 1,
                data: vec![0, 255, 42],
            })
            .unwrap();
        spool.observe(&DecodedLaunchEvent::Marker {
            seq: 2, name: LAUNCH_OUTPUT_COMPLETE_MARKER_V1.into(),
            value: serde_json::json!({
                "protocol": LAUNCH_OUTPUT_V1,
                "stdout": { "bytes": 3, "sha256": format!("{:x}", Sha256::digest([0, 255, 42])) },
                "stderr": { "bytes": 0, "sha256": format!("{:x}", Sha256::digest([])) },
                "data_event_count": 1
            }),
        }).unwrap();
        spool
            .observe(&DecodedLaunchEvent::Exit(
                oulipoly_provider::stream::LaunchExit {
                    seq: 3,
                    status: ProcessStatus::Exited { code: 0 },
                    terminal_signal: oulipoly_provider::generated::TerminalSignal {
                        kind: oulipoly_provider::generated::TerminalSignalKind::CleanExit,
                        evidence: None,
                        observed_at_unix_ms: 1,
                    },
                    session: None,
                },
            ))
            .unwrap();
        spool
    }

    #[test]
    fn attachment_failure_retains_verified_identity_output_and_failure_semantics() {
        use crate::executor::terminal_signal::TerminalSignalKind;
        use crate::executor::{SessionCaptureMethod, SessionCaptureResult};
        let verified = verify_session_authority(
            SessionAuthorityExpectation {
                account_name: "fixture",
                provider_session_id: Some("public-session"),
            },
            Some(AuthoritativeSessionObservation {
                account_name: "fixture",
                provider_session_id: "public-session",
            }),
        )
        .unwrap()
        .unwrap();
        let original = ExecutionResult {
            stdout: vec![0, 255, 42],
            stderr: "authentic stderr".into(),
            output_spool: Some(sealed_fixture_output()),
            exit_code: 0,
            provider_index: 2,
            session_capture: SessionCaptureResult {
                session_id: Some("unverified".into()),
                method: SessionCaptureMethod::ExternalProviderLaunch(
                    ExternalProviderSessionAuthority {
                        account_name: "fixture".into(),
                        provider_instance_id: "fixture-instance".into(),
                        settings_id: "fixture-settings".into(),
                    },
                ),
            },
            resume_acceptance: None,
            terminal_reason: None,
            terminal_signal: None,
            produced_assistant_response: true,
            prompt_acceptance_attestation: None,
            captured_child_invocations: vec![],
            returned_artifacts: vec![serde_json::from_value(serde_json::json!({
                "version_id": "store://return/11111111-1111-4111-8111-111111111111/fixture/1",
                "name": "fixture",
                "store_address": { "workflow_run_id": "return:11111111-1111-4111-8111-111111111111", "artifact_name": "fixture", "version": 1 },
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "content_len": 3, "format_hint": null, "verdict_line": null,
                "source": { "kind": "inline_bytes" },
                "producer_invocation_uuid": "11111111-1111-4111-8111-111111111111",
                "returned_at": "2026-09-07T00:00:00Z"
            })).unwrap()],
        };
        for cleanup in [
            Ok(GenerationOperationOutcome::Applied),
            Err(GenerationOperationError::StorageFailure),
        ] {
            let result = failed_finalization_result(
                original.clone(),
                Some(&verified),
                "runtime_generation_attach_failed",
                GenerationOperationError::Rejected(
                    oulipoly_state::mailbox::GenerationRejection::SessionConflict,
                ),
                cleanup.clone(),
            );
            assert_eq!(result.exit_code, -1);
            assert_eq!(
                result.terminal_reason.as_deref(),
                Some("runtime_generation_attach_failed")
            );
            assert_eq!(
                result.session_capture.session_id.as_deref(),
                Some("public-session")
            );
            let signal = result.terminal_signal.as_ref().unwrap();
            assert_eq!(signal.kind, TerminalSignalKind::SpawnError);
            assert!(signal.evidence.contains("SessionConflict"));
            assert!(signal.evidence.contains(&format!("cleanup={cleanup:?}")));
            assert!(!signal.evidence.contains("public-session"));
            assert_eq!(result.stdout, original.stdout);
            assert_eq!(result.stderr, original.stderr);
            assert_eq!(result.output_spool, original.output_spool);
            assert_eq!(result.returned_artifacts, original.returned_artifacts);
            assert!(result.produced_assistant_response);
        }
        let dir = tempfile::tempdir().unwrap();
        let state = oulipoly_state::StateDb::open(&dir.path().join("state.db")).unwrap();
        let invocation_uuid = "11111111-1111-4111-8111-111111111111";
        let invocation_id = state
            .start_invocation(&oulipoly_state::InvocationStart {
                invocation_uuid: invocation_uuid.into(),
                model_name: "fixture".into(),
                provider_name: "fixture".into(),
                provider_index: 2,
                parent_invocation_id: None,
            })
            .unwrap();
        let mut failed = failed_finalization_result(
            original.clone(),
            Some(&verified),
            "runtime_generation_attach_failed",
            GenerationOperationError::StorageFailure,
            Err(GenerationOperationError::StorageFailure),
        );
        failed
            .retain_failed_finalization_evidence(&state, invocation_id, invocation_uuid)
            .unwrap();
        assert_eq!(
            state.list_returned_artifacts(invocation_id).unwrap(),
            original.returned_artifacts
        );
        let paths = state
            .invocation_output_artifact_paths(invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(paths.stdout).unwrap(), original.stdout);
        assert_eq!(failed.exit_code, -1);
        // Failure of one retention channel cannot skip the other or overwrite the cause.
        failed.output_spool = Some(ExecutionOutputSpool::new().unwrap());
        // Keep producer authority, but require a NEW reference absent before this call.
        let mut new_reference = original.returned_artifacts[0].clone();
        new_reference.name = "new-after-output-failure".into();
        new_reference.store_address.artifact_name = new_reference.name.clone();
        new_reference.version_id =
            format!("store://return/{invocation_uuid}/{}/1", new_reference.name);
        assert!(
            !state
                .list_returned_artifacts(invocation_id)
                .unwrap()
                .contains(&new_reference)
        );
        failed.returned_artifacts = vec![new_reference.clone()];
        assert_eq!(
            failed.retain_failed_finalization_evidence(&state, invocation_id, invocation_uuid),
            Err("finalization_evidence: artifacts=retained;output=storage_failure")
        );
        assert_eq!(
            state.list_returned_artifacts(invocation_id).unwrap(),
            vec![new_reference]
        );
        assert_eq!(
            failed.terminal_reason.as_deref(),
            Some("runtime_generation_attach_failed")
        );
        assert_eq!(failed.exit_code, -1);
        let result = failed_finalization_result(
            original,
            None,
            "runtime_generation_exit_failed",
            GenerationOperationError::Unknown,
            Ok(GenerationOperationOutcome::AlreadyApplied),
        );
        assert_eq!(result.session_capture.session_id, None);
        assert!(result.terminal_signal.unwrap().evidence.contains("Unknown"));
    }

    #[test]
    fn live_marker_authority_rejection_cannot_create_attachment_failure_custody() {
        let failures = Arc::new(Mutex::new(None));
        let generation = recorded_launch_generation();
        let marker = DecodedLaunchEvent::Marker {
            seq: 1,
            name: PROVIDER_SESSION_MARKER.into(),
            value: serde_json::json!({"provider_session_id": "observed"}),
        };
        for (expected, observed_account) in [(Some("expected"), "account"), (None, "foreign")] {
            assert!(
                bind_external_launch_session_from_event(
                    None,
                    &generation,
                    &failures,
                    "account",
                    expected,
                    observed_account,
                    &marker,
                )
                .is_err()
            );
            assert!(failures.lock().unwrap().is_none());
        }
        assert!(
            bind_external_launch_session_from_event(
                None,
                &generation,
                &failures,
                "account",
                None,
                "account",
                &marker,
            )
            .is_err()
        );
        let retained = failures.lock().unwrap().clone().unwrap();
        assert_eq!(retained.cause, GenerationOperationError::MissingGeneration);
        assert_eq!(retained.verified.provider_session_id(), "observed");
    }
}
