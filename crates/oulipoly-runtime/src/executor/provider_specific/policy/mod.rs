//! ## Declared roles
//!
//! Roles: none.
//!
//! Functionless module inventory; no A1 function-role claim.

mod codex;
mod host_policy;

pub(in crate::executor) use codex::append_codex_provider_policy;
pub(in crate::executor) use host_policy::append_host_provider_policy;
