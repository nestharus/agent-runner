//! AGE-217 S6a external-provider dispatch adapter.
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - surface_id: age217_s6a_external_provider_dispatch
//!     component: crates/oulipoly-runtime/src/executor/external_provider/mod.rs
//!     role: orchestration
//!     translates:
//!       - provider-registry-selection
//!       - provider-policy-contract
//!       - provider-launch-contract
//!       - runtime-execution-result
//!       - cancellation-disposition
//!     contract_limit: 5
//! ```

mod capability_gate;
mod client_invoker;
pub(crate) mod context;
mod dispatch;
mod error_formatter;
mod error_mapper;
mod errors;
mod launch_result_mapper;
mod policy_transform;
mod request_builder;
mod terminal_cancel_mapper;
mod terminal_classify_handoff;

pub(crate) use dispatch::dispatch;
