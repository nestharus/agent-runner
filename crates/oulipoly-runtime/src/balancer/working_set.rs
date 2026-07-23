//! ## Declared roles
//!
//! `predicate`, `orchestration`, `accessor`, `mapper`.

use crate::migration::MigrationError;
use chrono::{DateTime, Utc};
use oulipoly_config::ModelConfig;
use oulipoly_state::{QuotaRecord, StateDb};

/// AGE-163 WU-A.1 working-set membership predicate. A provider is in the
/// working set iff its `next_available_at` is null or has elapsed: the
/// post-failure forensics writer sets this column to push a provider out of
/// rotation for a typed cooldown window.
pub fn working_set_member(quota: Option<&QuotaRecord>, now: DateTime<Utc>) -> bool {
    next_available_at(quota).is_none_or(|ts| ts <= now)
}

/// AGE-163 WU-A.3 round-robin candidate selection. Walks the model's
/// provider pool starting after the persisted round-robin cursor, filters
/// through `working_set_member`, and returns the first eligible index.
/// `exclude_provider_index` skips a candidate (e.g. the one that just
/// failed). On success, advances the persisted cursor. Returns `Ok(None)`
/// when the working set is exhausted.
pub fn select_next_working_candidate(
    state: &StateDb,
    model: &ModelConfig,
    now: DateTime<Utc>,
    exclude_provider_index: Option<usize>,
) -> Result<Option<usize>, MigrationError> {
    let pool_len = model.providers.len();
    if pool_len == 0 {
        return Ok(None);
    }
    let start = next_working_set_scan_start(state, model, pool_len)?;
    first_working_candidate(state, model, now, exclude_provider_index, start, 0)
}

fn next_available_at(quota: Option<&QuotaRecord>) -> Option<DateTime<Utc>> {
    quota.and_then(|q| q.next_available_at)
}

fn next_working_set_scan_start(
    state: &StateDb,
    model: &ModelConfig,
    pool_len: usize,
) -> Result<usize, MigrationError> {
    let cursor = round_robin_cursor(state, model)?.unwrap_or(usize::MAX);
    Ok(scan_start_for_cursor(cursor, pool_len))
}

fn round_robin_cursor(
    state: &StateDb,
    model: &ModelConfig,
) -> Result<Option<usize>, MigrationError> {
    state
        .next_round_robin_index_for_model(&model.name)
        .map_err(|message| MigrationError::Db { message })
}

fn scan_start_for_cursor(cursor: usize, pool_len: usize) -> usize {
    if cursor == usize::MAX {
        return 0;
    }
    (cursor + 1) % pool_len
}

fn first_working_candidate(
    state: &StateDb,
    model: &ModelConfig,
    now: DateTime<Utc>,
    exclude_provider_index: Option<usize>,
    start: usize,
    offset: usize,
) -> Result<Option<usize>, MigrationError> {
    if offset == model.providers.len() {
        return Ok(None);
    }
    let candidate_index = candidate_index_at_offset(start, offset, model.providers.len());
    let candidate =
        select_working_candidate(state, model, candidate_index, exclude_provider_index, now)?;
    if candidate.is_some() {
        return Ok(candidate);
    }
    first_working_candidate(state, model, now, exclude_provider_index, start, offset + 1)
}

fn candidate_index_at_offset(start: usize, offset: usize, pool_len: usize) -> usize {
    (start + offset) % pool_len
}

fn candidate_is_excluded(candidate_index: usize, exclude_provider_index: Option<usize>) -> bool {
    Some(candidate_index) == exclude_provider_index
}

fn select_working_candidate(
    state: &StateDb,
    model: &ModelConfig,
    candidate_index: usize,
    exclude_provider_index: Option<usize>,
    now: DateTime<Utc>,
) -> Result<Option<usize>, MigrationError> {
    if candidate_is_excluded(candidate_index, exclude_provider_index) {
        return Ok(None);
    }
    if candidate_is_unavailable(state, model, candidate_index, now)? {
        return Ok(None);
    }
    advance_working_set_cursor(state, model, candidate_index, now)?;
    Ok(Some(candidate_index))
}

fn candidate_is_unavailable(
    state: &StateDb,
    model: &ModelConfig,
    candidate_index: usize,
    now: DateTime<Utc>,
) -> Result<bool, MigrationError> {
    let quota = candidate_quota(state, model, candidate_index)?;
    Ok(!working_set_member(quota.as_ref(), now))
}

fn candidate_quota(
    state: &StateDb,
    model: &ModelConfig,
    candidate_index: usize,
) -> Result<Option<QuotaRecord>, MigrationError> {
    let provider = &model.providers[candidate_index];
    state
        .get_quota(&provider.name)
        .map_err(|message| MigrationError::Db { message })
}

fn advance_working_set_cursor(
    state: &StateDb,
    model: &ModelConfig,
    candidate_index: usize,
    now: DateTime<Utc>,
) -> Result<(), MigrationError> {
    state
        .advance_round_robin_index(&model.name, candidate_index, now)
        .map_err(|message| MigrationError::Db { message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oulipoly_config::{ProviderConfig, SessionStorage, model::PromptMode};
    use std::path::{Path, PathBuf};

    fn quota_record_with_next_available_at(
        next_available_at: Option<DateTime<Utc>>,
    ) -> QuotaRecord {
        QuotaRecord {
            provider_name: "p".to_string(),
            calls_since_refresh: 0,
            refreshed_at: None,
            exhausted_at: None,
            topology_peak_live_window_count: 0,
            last_topology_probe_at: None,
            next_available_at,
            last_refresh_at: None,
            failure_class: None,
        }
    }

    #[test]
    fn working_set_member_true_when_next_available_at_null() {
        let now = Utc::now();
        let q = quota_record_with_next_available_at(None);
        assert!(working_set_member(Some(&q), now));
        assert!(working_set_member(None, now));
    }

    #[test]
    fn working_set_member_true_when_next_available_at_past() {
        let now = Utc::now();
        let q = quota_record_with_next_available_at(Some(now - Duration::hours(1)));
        assert!(working_set_member(Some(&q), now));
    }

    #[test]
    fn working_set_member_false_when_next_available_at_future() {
        let now = Utc::now();
        let q = quota_record_with_next_available_at(Some(now + Duration::hours(1)));
        assert!(!working_set_member(Some(&q), now));
    }

    fn working_set_model(provider_names: &[&str]) -> ModelConfig {
        ModelConfig {
            name: "working-set-fixture".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: provider_names
                .iter()
                .map(|name| ProviderConfig {
                    environment: Default::default(),
                    unset_environment: Default::default(),
                    name: (*name).to_string(),
                    command: (*name).to_string(),
                    args: Vec::new(),
                    interactive_args: Some(vec!["launch".to_string()]),
                    resume: None,
                    session_capture: None,
                    resume_acceptance: None,
                    session_storage: Some(SessionStorage::ClaudeCode {
                        projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
                    }),
                    system_prompt_override: None,
                    tool_restrictions: None,
                    invocation_mode: Default::default(),
                })
                .collect(),
            inputs: Vec::new(),
            provider: None,
        }
    }

    #[test]
    fn select_next_working_candidate_round_robins_through_working_set() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = working_set_model(&["a", "b", "c"]);
        let now = Utc::now();

        let first = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(first, Some(0));

        let second = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(second, Some(1));

        let third = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(third, Some(2));

        let fourth = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(fourth, Some(0), "cursor wraps around the pool");
    }

    #[test]
    fn select_next_working_candidate_skips_exclude_index() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = working_set_model(&["a", "b"]);
        let now = Utc::now();

        let picked = select_next_working_candidate(&db, &model, now, Some(0)).unwrap();
        assert_eq!(picked, Some(1));
    }

    #[test]
    fn select_next_working_candidate_returns_none_when_all_unavailable() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = working_set_model(&["a", "b"]);
        let now = Utc::now();
        let future = now + Duration::hours(1);
        db.record_provider_unavailable("a", Some(future), "RollingWindow5h")
            .unwrap();
        db.record_provider_unavailable("b", Some(future), "RollingWindow5h")
            .unwrap();

        let picked = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(picked, None);
    }
}
