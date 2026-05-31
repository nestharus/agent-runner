//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/external_provider/mod.rs
//!     role: intrinsic-surface
//!     Domain: external_quota_dispatch_module
//!     Owns:
//!       - quota::external_provider
//!       - refresh_external_provider_quota
//!       - source/probe/refresh-auth orchestration module boundary
//!       - provider-name persistence handoff
//! ```

mod capability_gate;
mod client_invoker;
mod error_format;
mod error_mapper;
mod errors;
mod request_builder;
mod request_id_format;
mod source_probe_orchestration;
mod terminal_state_mapper;
mod window_projection;
mod window_shape;

pub(crate) use source_probe_orchestration::refresh_external_provider_quota;
