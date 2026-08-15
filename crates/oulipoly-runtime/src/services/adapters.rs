//! ## Declared roles
//! orchestration, accessor, mapper, formatter
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/services/adapters.rs
//!     role: intrinsic-surface
//!     Domain: production service adapter wiring
//!     Owns:
//!       - production service struct construction
//!       - service-port implementation branching
//!       - built-in versus external-provider session export/replace dispatch selection
//!       - service DTO to runtime helper delegation
//! ```
//!
//! Production service adapters. This module owns concrete adapter structs and
//! trait implementations; branch-heavy service work is delegated to focused
//! helpers in sibling modules.

use super::dtos::*;
use super::error::ServiceError;
use super::ports::*;
use crate::provider_registry::ProviderRegistryHandle;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionRoutingService;

impl ProductionRoutingService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionInvocationLifecycleService;

impl ProductionInvocationLifecycleService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductionResumeService;

impl ProductionResumeService {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductionSessionLifecycleService {
    provider_registry: Option<ProviderRegistryHandle>,
}

impl ProductionSessionLifecycleService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry_handle(provider_registry: ProviderRegistryHandle) -> Self {
        Self {
            provider_registry: Some(provider_registry),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductionSessionImportService {
    provider_registry: Option<ProviderRegistryHandle>,
}

impl ProductionSessionImportService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry_handle(provider_registry: ProviderRegistryHandle) -> Self {
        Self {
            provider_registry: Some(provider_registry),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductionMigrationService {
    provider_registry: Option<ProviderRegistryHandle>,
}

impl ProductionMigrationService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry_handle(provider_registry: ProviderRegistryHandle) -> Self {
        Self {
            provider_registry: Some(provider_registry),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductionTraceService {
    _private: (),
}

impl ProductionTraceService {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductionSessionExportService {
    provider_registry: Option<ProviderRegistryHandle>,
}

impl ProductionSessionExportService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry_handle(provider_registry: ProviderRegistryHandle) -> Self {
        Self {
            provider_registry: Some(provider_registry),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProductionSessionReplaceService {
    provider_registry: Option<ProviderRegistryHandle>,
}

impl ProductionSessionReplaceService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_registry_handle(provider_registry: ProviderRegistryHandle) -> Self {
        Self {
            provider_registry: Some(provider_registry),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductionSessionLockService {
    _private: (),
}

impl ProductionSessionLockService {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RoutingServicePort for ProductionRoutingService {
    fn select_route(
        &self,
        request: RoutingServiceRequest<'_>,
    ) -> Result<RoutingServiceOutput, ServiceError> {
        map_routing_service_result(crate::balancer::select_provider(
            request.model,
            request.state,
            request.ctx,
        ))
    }
}

impl InvocationLifecycleServicePort for ProductionInvocationLifecycleService {
    fn start_invocation(
        &self,
        request: InvocationLifecycleStartRequest<'_>,
    ) -> Result<InvocationLifecycleStartOutput, ServiceError> {
        map_invocation_start_result(
            request
                .state
                .start_invocation_with_completion_registration_authority(request.start),
        )
    }

    fn finalize_invocation(
        &self,
        request: InvocationLifecycleFinalizeRequest<'_>,
    ) -> Result<InvocationLifecycleFinalizeOutput, ServiceError> {
        map_invocation_finalize_result(request.state.finalize_invocation_typed(
            request.invocation_row_id,
            request.success,
            request.exit_code,
            request.error_category,
            request.terminal_reason,
        ))
    }
}

impl ResumeServicePort for ProductionResumeService {
    fn resolve_resume(
        &self,
        request: ResumeServiceRequest<'_>,
    ) -> Result<ResumeServiceOutput, ServiceError> {
        Ok(super::resume::resolve_resume(request))
    }

    fn record_acceptance(
        &self,
        request: ResumeAcceptanceRequest<'_>,
    ) -> Result<ResumeAcceptanceOutput, ServiceError> {
        map_resume_acceptance_result(request.state.update_resume_acceptance(
            request.invocation_row_id,
            request.status,
            request.evidence,
        ))
    }
}

impl SessionLifecycleServicePort for ProductionSessionLifecycleService {
    fn ingest_session(
        &self,
        request: SessionLifecycleRequest<'_>,
    ) -> Result<SessionLifecycleOutput, ServiceError> {
        super::session_lifecycle::ingest_session_with_registry(
            request,
            self.provider_registry.as_ref(),
        )
    }
}

impl SessionImportServicePort for ProductionSessionImportService {
    fn import_sessions(
        &self,
        request: SessionImportServiceRequest<'_>,
    ) -> Result<SessionImportServiceOutput, ServiceError> {
        super::session_import::import_sessions_with_registry(
            request,
            self.provider_registry.as_ref(),
        )
    }
}

impl MigrationServicePort for ProductionMigrationService {
    fn migrate(
        &self,
        request: MigrationServiceRequest<'_>,
    ) -> Result<MigrationServiceOutput, ServiceError> {
        super::migration::migrate(request, self.provider_registry.as_ref())
    }
}

impl TraceServicePort for ProductionTraceService {
    fn trace(&self, request: TraceServiceRequest<'_>) -> Result<TraceServiceOutput, ServiceError> {
        Ok(super::trace_failure::trace_with_typed_failures(request))
    }
}

impl SessionExportServicePort for ProductionSessionExportService {
    fn export_session(
        &self,
        request: SessionExportServiceRequest,
    ) -> Result<SessionExportServiceOutput, ServiceError> {
        let SessionExportServiceRequest {
            session_id,
            external_provider,
        } = request;
        let result = match external_provider {
            Some(identity) => crate::session_external_provider::export_session(
                self.provider_registry.as_ref(),
                identity,
                &session_id,
            ),
            None => export_builtin_session(&session_id),
        };
        Ok(map_session_export_output(result))
    }
}

impl SessionReplaceServicePort for ProductionSessionReplaceService {
    fn replace_session(
        &self,
        request: SessionReplaceServiceRequest,
    ) -> Result<SessionReplaceServiceOutput, ServiceError> {
        let SessionReplaceServiceRequest {
            session_id,
            source,
            preimage_sha256,
            external_provider,
        } = request;
        let result = match external_provider {
            Some(identity) => crate::session_external_provider::replace_session(
                self.provider_registry.as_ref(),
                identity,
                &session_id,
                &source,
                preimage_sha256.as_deref(),
            ),
            None => crate::session_replace::run_import_replace(
                &session_id,
                replace_source_input_path(&source),
                preimage_sha256.as_deref(),
            ),
        };
        Ok(map_session_replace_output(result))
    }
}

fn map_routing_service_result(
    result: Result<usize, crate::balancer::RoutingError>,
) -> Result<RoutingServiceOutput, ServiceError> {
    result
        .map(|provider_index| RoutingServiceOutput { provider_index })
        .map_err(|error| ServiceError::Unavailable {
            message: error.to_string(),
            code: None,
        })
}

fn map_invocation_start_result(
    result: Result<oulipoly_state::InvocationStartWithCompletionAuthority, String>,
) -> Result<InvocationLifecycleStartOutput, ServiceError> {
    result
        .map(|start| InvocationLifecycleStartOutput {
            invocation_row_id: start.invocation_row_id,
            completion_registration_authority: start.completion_registration_authority,
        })
        .map_err(|message| ServiceError::Dependency { message })
}

fn map_invocation_finalize_result(
    result: Result<(), oulipoly_state::InvocationFinalizeError>,
) -> Result<InvocationLifecycleFinalizeOutput, ServiceError> {
    result
        .map(|_| InvocationLifecycleFinalizeOutput)
        .map_err(|error| match error {
            oulipoly_state::InvocationFinalizeError::Contention { message } => {
                ServiceError::Contention { message }
            }
            oulipoly_state::InvocationFinalizeError::Failure { message } => {
                ServiceError::Dependency { message }
            }
        })
}

fn map_resume_acceptance_result(
    result: Result<(), String>,
) -> Result<ResumeAcceptanceOutput, ServiceError> {
    result
        .map(|_| ResumeAcceptanceOutput)
        .map_err(|message| ServiceError::Dependency { message })
}

fn export_builtin_session(session_id: &str) -> Result<Vec<u8>, crate::session_export::ExportError> {
    let metadata = crate::session_export::resolve_export_session_metadata(session_id)?;
    let records = crate::session_export::read_canonical_transcript(&metadata)?;
    crate::session_export::canonical_jsonl_bytes(&records)
}

fn map_session_export_output(
    result: Result<Vec<u8>, crate::session_export::ExportError>,
) -> SessionExportServiceOutput {
    SessionExportServiceOutput { result }
}

fn replace_source_input_path(
    source: &crate::session_replace::ReplaceSource,
) -> Option<&std::path::Path> {
    match source {
        crate::session_replace::ReplaceSource::File(path) => Some(path.as_path()),
        crate::session_replace::ReplaceSource::Stdin => None,
    }
}

fn map_session_replace_output(
    result: Result<crate::session_replace::ReplaceReceipt, crate::session_replace::ReplaceError>,
) -> SessionReplaceServiceOutput {
    SessionReplaceServiceOutput { result }
}

impl SessionLockServicePort for ProductionSessionLockService {
    fn lock_session(
        &self,
        request: SessionLockServiceRequest,
    ) -> Result<SessionLockServiceOutput, ServiceError> {
        Ok(SessionLockServiceOutput {
            result: super::lock::lock_session(request),
        })
    }
}
