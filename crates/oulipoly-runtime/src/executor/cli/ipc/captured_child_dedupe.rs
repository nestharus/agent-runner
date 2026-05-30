//! ## Declared roles
//!
//! Roles: filter.
//!
//! - filter: suppresses duplicate captured-child markers by composite source
//!   and id while preserving first-seen order.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/captured_child_dedupe.rs
//!     role: adapter
//!     Translates:
//!       - captured-child-marker-contract
//!       - composite-invocation-id-contract
//! ```

use oulipoly_state::CompositeInvocationId;

pub(super) fn mark_captured_child_seen(
    seen: &mut std::collections::HashSet<(String, String)>,
    composite_id: &CompositeInvocationId,
) -> bool {
    seen.insert((composite_id.source.clone(), composite_id.id.clone()))
}
