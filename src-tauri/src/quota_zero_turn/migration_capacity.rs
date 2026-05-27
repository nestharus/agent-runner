//! Migration quota-capacity resolution for resume migration candidates.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-204 (map row H13). Output-preserving.
//!
//! ## Declared roles
//!
//! `predicate`, `accessor`, `filter`, `formatter`
//!
//! - `filter`: `filter_quota_exhausted_migration_candidates` retains only providers that
//!   still have quota capacity (plus the active provider unconditionally).
//! - `predicate`: `quota_state_has_capacity` / `resume_migration_candidate_has_quota` decide
//!   capacity from a `QuotaRecord` (AGE-163 `exhausted_at` / `next_available_at` semantics).
//! - `accessor`: `read_provider_quota_state` / `provider_quota_state_for_migration` read the
//!   provider quota record from the state DB.

use oulipoly_config::ModelConfig;
use oulipoly_state::StateDb;

pub(crate) fn filter_quota_exhausted_migration_candidates(
    state: &StateDb,
    migration_model: &mut ModelConfig,
    active_provider: &str,
) {
    migration_model.providers.retain(|provider| {
        if provider.name == active_provider {
            return true;
        }
        resume_migration_candidate_has_quota(state, &provider.name)
    });
}

fn resume_migration_candidate_has_quota(state: &StateDb, provider_name: &str) -> bool {
    provider_quota_state_for_migration(state, provider_name)
        .map(quota_state_has_capacity)
        .unwrap_or_else(migration_candidate_default_capacity_after_quota_read_error)
}

fn provider_quota_state_for_migration(
    state: &StateDb,
    provider_name: &str,
) -> Option<Option<oulipoly_state::QuotaRecord>> {
    handle_provider_quota_state_for_migration(
        provider_name,
        read_provider_quota_state(state, provider_name),
    )
}

fn handle_provider_quota_state_for_migration(
    provider_name: &str,
    result: Result<Option<oulipoly_state::QuotaRecord>, String>,
) -> Option<Option<oulipoly_state::QuotaRecord>> {
    result.map_or_else(
        |err| {
            emit_quota_inspection_warning(provider_name, &err);
            None
        },
        Some,
    )
}

fn migration_candidate_default_capacity_after_quota_read_error() -> bool {
    true
}

fn read_provider_quota_state(
    state: &StateDb,
    provider_name: &str,
) -> Result<Option<oulipoly_state::QuotaRecord>, String> {
    state.get_quota(provider_name)
}

fn quota_state_has_capacity(quota: Option<oulipoly_state::QuotaRecord>) -> bool {
    // AGE-163 WU-A.4: the typed forensics writer lands durable
    // unavailability on `next_available_at`. A provider has capacity iff
    // neither the legacy `exhausted_at` flag nor an unelapsed
    // `next_available_at` cooldown is set.
    let Some(record) = quota else {
        return true;
    };
    if record.exhausted_at.is_some() {
        return false;
    }
    record
        .next_available_at
        .is_none_or(|ts| ts <= chrono::Utc::now())
}

fn emit_quota_inspection_warning(provider_name: &str, err: &str) {
    eprintln!("Warning: Failed to inspect quota state for {provider_name}: {err}");
}
