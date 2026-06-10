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
use oulipoly_state::StateDb;
use oulipoly_state::repositories::ResumeRepository;

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
        Ok(RoutingServiceOutput {
            provider_index: crate::balancer::select_provider(
                request.model,
                request.state,
                request.ctx,
            )
            .map_err(|error| ServiceError::Unavailable {
                message: error.to_string(),
            })?,
        })
    }
}

impl InvocationLifecycleServicePort for ProductionInvocationLifecycleService {
    fn start_invocation(
        &self,
        request: InvocationLifecycleStartRequest<'_>,
    ) -> Result<InvocationLifecycleStartOutput, ServiceError> {
        request
            .state
            .start_invocation(request.start)
            .map(|invocation_row_id| InvocationLifecycleStartOutput { invocation_row_id })
            .map_err(|message| ServiceError::Dependency { message })
    }

    fn finalize_invocation(
        &self,
        request: InvocationLifecycleFinalizeRequest<'_>,
    ) -> Result<InvocationLifecycleFinalizeOutput, ServiceError> {
        request
            .state
            .finalize_invocation(
                request.invocation_row_id,
                request.success,
                request.exit_code,
                request.error_category,
                request.terminal_reason,
            )
            .map(|_| InvocationLifecycleFinalizeOutput)
            .map_err(|message| ServiceError::Dependency { message })
    }
}

impl ResumeServicePort for ProductionResumeService {
    fn resolve_resume(
        &self,
        request: ResumeServiceRequest<'_>,
    ) -> Result<ResumeServiceOutput, ServiceError> {
        match <StateDb as ResumeRepository>::resolve_resume(
            request.state,
            request.models,
            request.input,
            request.model_override,
        ) {
            Ok(resolved) => Ok(ResumeServiceOutput::ResumeResolved { resolved }),
            Err(error) => Ok(ResumeServiceOutput::ResumeRejected { error }),
        }
    }

    fn record_acceptance(
        &self,
        request: ResumeAcceptanceRequest<'_>,
    ) -> Result<ResumeAcceptanceOutput, ServiceError> {
        request
            .state
            .update_resume_acceptance(request.invocation_row_id, request.status, request.evidence)
            .map(|_| ResumeAcceptanceOutput)
            .map_err(|message| ServiceError::Dependency { message })
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
        if let Some(identity) = request.external_provider {
            let result = crate::session_external_provider::export_session(
                self.provider_registry.as_ref(),
                identity,
                &request.session_id,
            );
            return Ok(SessionExportServiceOutput { result });
        }

        let result = crate::session_export::resolve_export_session_metadata(&request.session_id)
            .and_then(|metadata| {
                crate::session_export::read_canonical_transcript(&metadata)
                    .and_then(|records| crate::session_export::canonical_jsonl_bytes(&records))
            });
        Ok(SessionExportServiceOutput { result })
    }
}

impl SessionReplaceServicePort for ProductionSessionReplaceService {
    fn replace_session(
        &self,
        request: SessionReplaceServiceRequest,
    ) -> Result<SessionReplaceServiceOutput, ServiceError> {
        if let Some(identity) = request.external_provider {
            let result = crate::session_external_provider::replace_session(
                self.provider_registry.as_ref(),
                identity,
                &request.session_id,
                &request.source,
                request.preimage_sha256.as_deref(),
            );
            return Ok(SessionReplaceServiceOutput { result });
        }

        let input_path = match &request.source {
            crate::session_replace::ReplaceSource::File(path) => Some(path.as_path()),
            crate::session_replace::ReplaceSource::Stdin => None,
        };
        let result = crate::session_replace::run_import_replace(
            &request.session_id,
            input_path,
            request.preimage_sha256.as_deref(),
        );
        Ok(SessionReplaceServiceOutput { result })
    }
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
