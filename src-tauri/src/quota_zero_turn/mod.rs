//! Quota-capacity, zero-turn completion classification, and quota/error-category helpers.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-204 (slice B9 of the AGE-183 main.rs
//! decomposition program; map row H13). Output-preserving: bodies byte-identical to the
//! pre-AGE-204 main.rs definitions; only visibility + import targets change. The pure
//! zero-turn classification core stays in `crate::zero_turn_orchestration`; this module
//! holds the main-side adapters that wrap it plus the migration quota-capacity tree and
//! the quota/error-category decisioning helpers.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `predicate`, `accessor`, `formatter`, `filter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/quota_zero_turn/mod.rs
//!     role: adapter
//!     Translates:
//!       - AGE-204 H13: StateDb quota records -> migration-candidate capacity predicate
//!       - AGE-204 H13: session turn-count deltas -> zero-turn completion classification + signal mutation
//!       - AGE-204 H13: execution result / quota state -> error-category strings + retry-budget message
//! ```

mod completion_classification;
mod error_category;
mod migration_capacity;

pub(crate) use completion_classification::{
    apply_zero_turn_classification_to_result, apply_zero_turn_classification_to_signal_fields,
    host_observed_completion_from_interactive_result, host_observed_completion_from_result,
    is_confirmed_zero_turn_exhaustion, zero_turn_classification_for_action,
    zero_turn_classify_after_completion, zero_turn_classify_after_completion_with_recovery,
    zero_turn_late_bind_baseline, zero_turn_record_baseline,
};
pub(crate) use error_category::{
    balanced_result_error_category, error_category_is_quota_exhausted,
    format_quota_retry_budget_exhausted, quota_exhausted_category, resume_result_error_category,
};
pub(crate) use migration_capacity::filter_quota_exhausted_migration_candidates;
