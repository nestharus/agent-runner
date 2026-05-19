//! ## Declared roles
//! accessor, filter, predicate, validator
//!
//! Resume-preview ambiguity policy for locate versus resume locate.

use super::MetadataError;

#[derive(Debug, Clone, Copy)]
pub(super) enum AmbiguityPolicy {
    Reject,
    UseStrictRecency,
}

pub(super) fn rejects_recent_ambiguity(policy: AmbiguityPolicy) -> bool {
    matches!(policy, AmbiguityPolicy::Reject)
}

pub(super) fn recency_cutoff_for_resume_previews() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::hours(24)
}

pub(super) fn count_recent_previews(
    previews: &[oulipoly_state::ChainPreview],
    cutoff: chrono::DateTime<chrono::Utc>,
) -> usize {
    previews
        .iter()
        .filter(|preview| preview.last_used_at >= cutoff)
        .count()
}

pub(super) fn reject_ambiguous_recent_matches(
    input: &str,
    recent_count: usize,
) -> Result<(), MetadataError> {
    if recent_count > 1 {
        return Err(MetadataError::AmbiguousSession {
            input: input.to_string(),
        });
    }
    Ok(())
}
