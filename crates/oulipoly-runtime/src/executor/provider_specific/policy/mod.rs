//! ## Declared roles
//!
//! Roles: none.
//!
//! Functionless module inventory; no A1 function-role claim.

mod claude;
mod codex;

pub(in crate::executor) use claude::append_claude_provider_policy;
pub(in crate::executor) use codex::append_codex_provider_policy;
