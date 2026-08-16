//! ## Declared roles
//! accessor
//!
//! Runtime services module root. It preserves the historical
//! `oulipoly_runtime::services::*` public surface while sibling modules own
//! DTOs, port traits, production adapters, and focused implementation helpers.

mod adapters;
mod dtos;
pub mod error;
mod invocation_lifecycle_finalize;
mod lock;
mod marker;
mod migration;
mod ports;
mod resume;
mod session_import;
mod session_lifecycle;
mod session_warning;
mod session_window;
mod trace_failure;

pub use adapters::{
    ProductionInvocationLifecycleService, ProductionMigrationService, ProductionResumeService,
    ProductionRoutingService, ProductionSessionExportService, ProductionSessionImportService,
    ProductionSessionLifecycleService, ProductionSessionLockService,
    ProductionSessionReplaceService, ProductionTraceService,
};
pub use dtos::*;
pub use error::ServiceError;
pub use invocation_lifecycle_finalize::{
    SUCCESS_FINALIZE_MAX_ATTEMPTS, finalize_retained_outcome_with_contention_retry,
};
pub use ports::*;

pub(crate) fn emit_live_session_marker(
    state: &oulipoly_state::StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
) -> Result<oulipoly_state::SessionMarkerPayload, String> {
    let mut stderr = std::io::stderr();
    marker::emit_known_session_payload_for_service(
        state,
        &mut stderr,
        invocation_row_id,
        invocation_uuid,
        session_id,
        capture_method,
    )
    .map_err(|err| err.to_string())
    .and_then(|payload| payload.ok_or_else(|| "Live-session marker was not emitted".to_string()))
}
