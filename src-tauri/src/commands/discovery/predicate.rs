//! ## Declared roles
//!
//! `predicate`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/discovery/predicate.rs
//!     role: adapter
//!     Translates:
//!       - non-empty discovery stale-delete guard contract
//! ```

use oulipoly_runtime::discovery::DiscoveryResult;

pub fn has_discovered_models(result: &DiscoveryResult) -> bool {
    !result.models.is_empty()
}
