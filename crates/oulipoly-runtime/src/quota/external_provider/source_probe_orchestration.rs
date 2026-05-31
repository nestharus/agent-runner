//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `predicate`

use super::capability_gate::validate_quota_capability;
use super::client_invoker::{invoke_quota_probe, invoke_quota_refresh_auth, invoke_quota_source};
use super::error_mapper::{
    failed_outcome, probe_retry_error, provider_client_error, registry_error,
};
use super::errors::ExternalQuotaError;
use super::request_builder::{
    build_quota_probe_request, build_quota_refresh_auth_request, build_quota_source_request,
};
use super::terminal_state_mapper::{probe_result_unavailable, source_result_without_source};
use super::window_projection::project_quota_probe_windows;
use super::window_shape::validate_probe_windows;
use crate::provider_registry::{
    DescribeHostOptions, ProviderRegistry, ProviderRegistryError, ProviderRegistryHandle,
};
use crate::quota::{InFlightGuard, RefreshOutcome};
use crate::services::{QuotaServiceExternalProviderIdentity, QuotaServiceRequest};
use oulipoly_provider::client::ProviderClient;
use oulipoly_provider::generated::{DescribeResult, QuotaProbeResult, QuotaSourceResult};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_state::QuotaWindowInput;
use serde_json::Value;
use std::sync::Arc;

pub(crate) fn refresh_external_provider_quota(
    registry_handle: &ProviderRegistryHandle,
    request: QuotaServiceRequest<'_>,
    identity: QuotaServiceExternalProviderIdentity,
) -> RefreshOutcome {
    let _guard = match claim_external_quota_refresh(request.in_flight, &request.provider_name) {
        Ok(guard) => guard,
        Err(outcome) => return outcome,
    };
    let registry = current_registry(registry_handle);
    let artifact = match enabled_artifact_for_identity(&registry, &identity) {
        Ok(artifact) => artifact,
        Err(error) => return registry_failure_outcome(error),
    };
    let describe = match describe_identity_provider(&registry, &identity) {
        Ok(describe) => describe,
        Err(error) => return registry_failure_outcome(error),
    };
    if let Some(outcome) = invalid_capability_outcome(&describe) {
        return outcome;
    }
    let client = client_for_artifact(&registry, artifact);
    run_external_quota_sequence(&client, registry.host_options(), request, &identity)
}

fn claim_external_quota_refresh<'a>(
    in_flight: &'a crate::quota::InFlight,
    provider_name: &str,
) -> Result<InFlightGuard<'a>, RefreshOutcome> {
    in_flight
        .try_claim(provider_name)
        .ok_or(RefreshOutcome::AlreadyInFlight)
}

fn current_registry(registry_handle: &ProviderRegistryHandle) -> Arc<ProviderRegistry> {
    registry_handle.current()
}

fn enabled_artifact_for_identity(
    registry: &ProviderRegistry,
    identity: &QuotaServiceExternalProviderIdentity,
) -> Result<ProviderArtifactRef, ProviderRegistryError> {
    registry.enabled_artifact_for_model(&identity.model_name)
}

fn describe_identity_provider(
    registry: &ProviderRegistry,
    identity: &QuotaServiceExternalProviderIdentity,
) -> Result<DescribeResult, ProviderRegistryError> {
    registry.describe_model_provider(&identity.model_name)
}

fn registry_failure_outcome(error: ProviderRegistryError) -> RefreshOutcome {
    failed_outcome(registry_error(error))
}

fn invalid_capability_outcome(describe: &DescribeResult) -> Option<RefreshOutcome> {
    validate_quota_capability(describe)
        .err()
        .map(failed_outcome)
}

fn client_for_artifact(
    registry: &ProviderRegistry,
    artifact: ProviderArtifactRef,
) -> ProviderClient {
    registry.client_factory().client_for(artifact)
}

fn run_external_quota_sequence(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
    request: QuotaServiceRequest<'_>,
    identity: &QuotaServiceExternalProviderIdentity,
) -> RefreshOutcome {
    let source = match run_quota_source(client, host_options, identity) {
        Ok(source) => source,
        Err(error) => return failed_outcome(error),
    };
    if let Some(outcome) = source_terminal_outcome(&source) {
        return outcome;
    }
    run_probe_sequence(client, host_options, request, identity)
}

fn run_quota_source(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
    identity: &QuotaServiceExternalProviderIdentity,
) -> Result<QuotaSourceResult, ExternalQuotaError> {
    let request = quota_source_request(identity, host_options)?;
    invoke_external_quota_source(client, request)
}

fn quota_source_request(
    identity: &QuotaServiceExternalProviderIdentity,
    host_options: &DescribeHostOptions,
) -> Result<Value, ExternalQuotaError> {
    build_quota_source_request(identity, host_options)
        .map_err(|_| ExternalQuotaError::schema_invalid_request())
}

fn invoke_external_quota_source(
    client: &ProviderClient,
    request: Value,
) -> Result<QuotaSourceResult, ExternalQuotaError> {
    invoke_quota_source(client, request).map_err(provider_client_error)
}

fn source_terminal_outcome(source: &QuotaSourceResult) -> Option<RefreshOutcome> {
    if source.has_source {
        return None;
    }
    Some(source_result_without_source())
}

fn run_probe_sequence(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
    request: QuotaServiceRequest<'_>,
    identity: &QuotaServiceExternalProviderIdentity,
) -> RefreshOutcome {
    let first = invoke_probe(client, host_options, identity);
    if should_retry_probe(&first, request.state, &request.provider_name) {
        return run_refresh_auth_retry(client, host_options, request, identity, first);
    }
    probe_outcome(first, request)
}

fn run_refresh_auth_retry(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
    request: QuotaServiceRequest<'_>,
    identity: &QuotaServiceExternalProviderIdentity,
    first: Result<QuotaProbeResult, ExternalQuotaError>,
) -> RefreshOutcome {
    let refresh_error = refresh_auth_error(client, host_options, identity);
    let retry = invoke_probe(client, host_options, identity);
    retry_probe_outcome(first, refresh_error, retry, request)
}

fn refresh_auth_error(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
    identity: &QuotaServiceExternalProviderIdentity,
) -> Option<ExternalQuotaError> {
    run_quota_refresh_auth(client, host_options, identity).err()
}

fn run_quota_refresh_auth(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
    identity: &QuotaServiceExternalProviderIdentity,
) -> Result<(), ExternalQuotaError> {
    let request = quota_refresh_auth_request(identity, host_options)?;
    invoke_external_quota_refresh_auth(client, request)
}

fn quota_refresh_auth_request(
    identity: &QuotaServiceExternalProviderIdentity,
    host_options: &DescribeHostOptions,
) -> Result<Value, ExternalQuotaError> {
    build_quota_refresh_auth_request(identity, host_options)
        .map_err(|_| ExternalQuotaError::schema_invalid_request())
}

fn invoke_external_quota_refresh_auth(
    client: &ProviderClient,
    request: Value,
) -> Result<(), ExternalQuotaError> {
    invoke_quota_refresh_auth(client, request)
        .map(|_| ())
        .map_err(provider_client_error)
}

fn invoke_probe(
    client: &ProviderClient,
    host_options: &DescribeHostOptions,
    identity: &QuotaServiceExternalProviderIdentity,
) -> Result<QuotaProbeResult, ExternalQuotaError> {
    let request = quota_probe_request(identity, host_options)?;
    invoke_external_quota_probe(client, request)
}

fn quota_probe_request(
    identity: &QuotaServiceExternalProviderIdentity,
    host_options: &DescribeHostOptions,
) -> Result<Value, ExternalQuotaError> {
    build_quota_probe_request(identity, host_options)
        .map_err(|_| ExternalQuotaError::schema_invalid_request())
}

fn invoke_external_quota_probe(
    client: &ProviderClient,
    request: Value,
) -> Result<QuotaProbeResult, ExternalQuotaError> {
    invoke_quota_probe(client, request).map_err(provider_client_error)
}

fn should_retry_probe(
    result: &Result<QuotaProbeResult, ExternalQuotaError>,
    state: &oulipoly_state::StateDb,
    provider_name: &str,
) -> bool {
    match result {
        Err(_) => true,
        Ok(result) if !result.available => true,
        Ok(result) if result.windows.is_empty() => has_prior_windows(state, provider_name),
        Ok(_) => false,
    }
}

fn retry_probe_outcome(
    first: Result<QuotaProbeResult, ExternalQuotaError>,
    refresh_error: Option<ExternalQuotaError>,
    retry: Result<QuotaProbeResult, ExternalQuotaError>,
    request: QuotaServiceRequest<'_>,
) -> RefreshOutcome {
    match retry {
        Ok(result) => persist_probe_result(result, request),
        Err(error) => failed_outcome(probe_retry_error(first, refresh_error, error)),
    }
}

fn probe_outcome(
    result: Result<QuotaProbeResult, ExternalQuotaError>,
    request: QuotaServiceRequest<'_>,
) -> RefreshOutcome {
    match result {
        Ok(result) => persist_probe_result(result, request),
        Err(error) => failed_outcome(error),
    }
}

fn persist_probe_result(
    result: QuotaProbeResult,
    request: QuotaServiceRequest<'_>,
) -> RefreshOutcome {
    if let Some(outcome) = probe_terminal_outcome(&result) {
        return outcome;
    }
    let windows = match validate_probe_windows(&result) {
        Ok(windows) => windows,
        Err(error) => return failed_outcome(error),
    };
    let windows = match project_quota_probe_windows(&windows) {
        Ok(windows) => windows,
        Err(error) => return failed_outcome(error),
    };
    persist_windows(request.state, &request.provider_name, windows)
}

fn probe_terminal_outcome(result: &QuotaProbeResult) -> Option<RefreshOutcome> {
    if result.available {
        return None;
    }
    Some(probe_result_unavailable(result))
}

fn persist_windows(
    state: &oulipoly_state::StateDb,
    provider_name: &str,
    windows: Vec<QuotaWindowInput>,
) -> RefreshOutcome {
    let persist_result = upsert_quota_windows(state, provider_name, &windows);
    refresh_outcome_from_persist_result(persist_result, windows)
}

fn upsert_quota_windows(
    state: &oulipoly_state::StateDb,
    provider_name: &str,
    windows: &[QuotaWindowInput],
) -> Result<(), String> {
    state.upsert_quota_refresh(provider_name, windows)
}

fn refresh_outcome_from_persist_result(
    result: Result<(), String>,
    windows: Vec<QuotaWindowInput>,
) -> RefreshOutcome {
    if let Err(error) = result {
        return RefreshOutcome::Failed(error);
    }
    RefreshOutcome::Updated { windows }
}

fn has_prior_windows(state: &oulipoly_state::StateDb, provider_name: &str) -> bool {
    state
        .get_windows(provider_name)
        .map(|windows| !windows.is_empty())
        .unwrap_or(false)
}
