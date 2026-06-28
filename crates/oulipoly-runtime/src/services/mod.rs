//! ## Declared roles
//! accessor
//!
//! Runtime services module root. It preserves the historical
//! `oulipoly_runtime::services::*` public surface while sibling modules own
//! DTOs, port traits, production adapters, and focused implementation helpers.

mod adapters;
mod dtos;
pub mod error;
mod lock;
mod marker;
mod migration;
mod ports;
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
pub use ports::*;
