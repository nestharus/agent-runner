//! ## Declared roles
//! accessor, mapper, validator, orchestration, filter, predicate
//!
//! State resume lookup helpers and effective provider resolution.

use super::MetadataError;
use super::errors::{
    operational_error, provider_model_mismatch_message, provider_resolution_error,
    retired_provider_message, session_not_found_error,
};
use oulipoly_config::{ModelConfig, ProviderConfig, ProvidersConfig};
use oulipoly_state::{ChainSegmentRow, ResolvedResume, StateDb};

pub(super) fn fetch_resume_previews(
    state: &StateDb,
    input: &str,
) -> Result<Vec<oulipoly_state::ChainPreview>, String> {
    state.resume_previews(input)
}

pub(super) fn fetch_active_segment_id(
    state: &StateDb,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<Option<i64>, String> {
    state.active_segment_id_for_chain_provider_session(
        &resolved.chain_id,
        &resolved.active_provider,
        &resolved.active_session_id,
    )
}

pub(super) fn active_segment_id_to_metadata_error_or_value(
    active_segment_id: Option<i64>,
    chain_id: &str,
) -> Result<i64, MetadataError> {
    active_segment_id.ok_or_else(|| session_not_found_error(chain_id))
}

pub(super) fn effective_provider_for_resolved(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ProviderConfig, MetadataError> {
    if let Some(model) = resolved.model.as_ref() {
        let model_provider = active_model_provider(model, &resolved.active_provider)
            .ok_or_else(|| provider_mismatch_error(model, &resolved.active_provider))?;
        return effective_model_provider(providers_cfg, model_provider, &resolved.active_provider);
    }
    runtime_provider(providers_cfg, &resolved.active_provider)
}

fn active_model_provider<'a>(
    model: &'a ModelConfig,
    active_provider: &str,
) -> Option<&'a ProviderConfig> {
    model
        .providers
        .iter()
        .find(|provider| provider.name == active_provider)
}

fn provider_mismatch_error(model: &ModelConfig, active_provider: &str) -> MetadataError {
    operational_error(provider_model_mismatch_message(
        &model.name,
        active_provider,
    ))
}

fn effective_model_provider(
    providers_cfg: &ProvidersConfig,
    model_provider: &ProviderConfig,
    active_provider: &str,
) -> Result<ProviderConfig, MetadataError> {
    let (provider, _) = providers_cfg
        .effective_provider(model_provider)
        .map_err(|message| provider_resolution_error(active_provider, message))?;
    Ok(provider)
}

fn runtime_provider(
    providers_cfg: &ProvidersConfig,
    active_provider: &str,
) -> Result<ProviderConfig, MetadataError> {
    let (provider, _) = providers_cfg
        .runtime_provider(active_provider)
        .map_err(|message| provider_resolution_error(active_provider, message))?;
    Ok(provider)
}

/// When the active segment's provider is no longer resumable, fall back to the
/// most-recent earlier segment of the same chain that is — keeping the same
/// conversation lineage rather than guessing a provider. Errors with an
/// actionable message when no segment of the chain is resumable.
pub(super) fn rebase_resolved_to_resumable_segment<F>(
    state: &StateDb,
    resolved: ResolvedResume,
    is_resumable: F,
) -> Result<ResolvedResume, MetadataError>
where
    F: Fn(&str, &str) -> bool,
{
    if is_resumable(&resolved.active_provider, &resolved.active_session_id) {
        return Ok(resolved);
    }
    let segments = list_chain_segments_desc(state, &resolved.chain_id)?;
    match fallback_segment(&segments, &resolved, &is_resumable) {
        Some(segment) => Ok(rebased_resolved(resolved, segment)),
        None => Err(retired_provider_error(&resolved)),
    }
}

fn list_chain_segments_desc(
    state: &StateDb,
    chain_id: &str,
) -> Result<Vec<ChainSegmentRow>, MetadataError> {
    state
        .list_chain_segments_desc(chain_id)
        .map_err(operational_error)
}

fn fallback_segment<'a, F>(
    segments: &'a [ChainSegmentRow],
    resolved: &ResolvedResume,
    is_resumable: &F,
) -> Option<&'a ChainSegmentRow>
where
    F: Fn(&str, &str) -> bool,
{
    segments
        .iter()
        .filter(|segment| !is_active_segment(segment, resolved))
        .find(|segment| is_resumable(&segment.provider_name, &segment.session_id))
}

fn is_active_segment(segment: &ChainSegmentRow, resolved: &ResolvedResume) -> bool {
    segment.provider_name == resolved.active_provider
        && segment.session_id == resolved.active_session_id
}

fn rebased_resolved(resolved: ResolvedResume, segment: &ChainSegmentRow) -> ResolvedResume {
    ResolvedResume {
        active_provider: segment.provider_name.clone(),
        active_session_id: segment.session_id.clone(),
        ..resolved
    }
}

fn retired_provider_error(resolved: &ResolvedResume) -> MetadataError {
    operational_error(retired_provider_message(
        &resolved.active_provider,
        &resolved.active_session_id,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(provider: &str, session: &str) -> ResolvedResume {
        ResolvedResume {
            chain_id: "chain".to_string(),
            model_name: Some("model".to_string()),
            model: None,
            active_provider: provider.to_string(),
            active_session_id: session.to_string(),
        }
    }

    fn segment(provider: &str, session: &str, started_at: &str) -> ChainSegmentRow {
        ChainSegmentRow {
            provider_name: provider.to_string(),
            session_id: session.to_string(),
            started_at: started_at.to_string(),
        }
    }

    #[test]
    fn fallback_picks_first_resumable_non_active_segment() {
        let active = resolved("retired", "s3");
        let segments = vec![
            segment("retired", "s3", "2026-06-03"),
            segment("registered", "s2", "2026-06-02"),
            segment("registered", "s1", "2026-06-01"),
        ];
        let pick = fallback_segment(&segments, &active, &|provider, _| provider == "registered");
        assert_eq!(pick.map(|s| s.session_id.as_str()), Some("s2"));
    }

    #[test]
    fn fallback_skips_the_active_segment_even_if_resumable() {
        // The active segment already failed resolution; never re-select it.
        let active = resolved("retired", "s2");
        let segments = vec![
            segment("retired", "s2", "2026-06-02"),
            segment("registered", "s1", "2026-06-01"),
        ];
        let pick = fallback_segment(&segments, &active, &|_, _| true);
        assert_eq!(pick.map(|s| s.session_id.as_str()), Some("s1"));
    }

    #[test]
    fn fallback_returns_none_when_no_segment_is_resumable() {
        let active = resolved("retired", "s2");
        let segments = vec![
            segment("retired", "s2", "2026-06-02"),
            segment("alsoretired", "s1", "2026-06-01"),
        ];
        let pick = fallback_segment(&segments, &active, &|_, _| false);
        assert!(pick.is_none());
    }

    #[test]
    fn rebased_resolved_swaps_provider_and_session_keeps_chain_and_model() {
        let rebased = rebased_resolved(
            resolved("retired", "s2"),
            &segment("registered", "s1", "2026-06-01"),
        );
        assert_eq!(rebased.active_provider, "registered");
        assert_eq!(rebased.active_session_id, "s1");
        assert_eq!(rebased.chain_id, "chain");
        assert_eq!(rebased.model_name.as_deref(), Some("model"));
    }

    #[test]
    fn is_active_segment_matches_provider_and_session() {
        let active = resolved("p", "s");
        assert!(is_active_segment(&segment("p", "s", "t"), &active));
        assert!(!is_active_segment(&segment("p", "other", "t"), &active));
        assert!(!is_active_segment(&segment("other", "s", "t"), &active));
    }
}
