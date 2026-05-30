//! ## Declared roles
//!
//! Roles: none.
//!
//! Functionless module inventory; no A1 function-role claim.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/policy.rs
//!     role: adapter
//!     Translates:
//!       - provider-policy-config-contract
//!       - executor-policy-launch-contract
//! ```

mod messages;
mod orchestration;
mod predicates;
mod validation;

pub(super) use orchestration::{apply_provider_policy, provider_policy_launch_parts};
