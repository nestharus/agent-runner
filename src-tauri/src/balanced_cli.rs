//! One-shot balanced CLI helper boundaries.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`

use oulipoly_state::StateDb;

pub(super) fn mark_provider_exhausted(state: &StateDb, provider_name: &str) {
    state
        .mark_exhausted(provider_name)
        .unwrap_or_else(|e| eprintln!("Warning: Failed to mark provider exhausted: {e}"));
}
