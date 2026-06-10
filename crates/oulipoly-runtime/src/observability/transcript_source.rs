//! ## Declared roles
//!
//! `accessor`, `predicate`, `mapper`, `orchestration`
//!
//! Active-session transcript path resolution with caching. The transcript
//! locator hydrates by listing every transcript file under the provider's
//! storage, so resolving on every refresh would be costly; the resolved path is
//! cached for the session and missing-file lookups re-attempt only on a fixed
//! cadence.

use crate::session_metadata::{TranscriptLookupMode, resolve_jsonl_path_for_provider_with_mode};
use oulipoly_config::{SessionStorage, SessionsConfig};
use std::sync::{Mutex, MutexGuard};

/// Number of `resolve` calls to skip between re-attempts while the transcript
/// file does not yet exist. Bounds the expensive locator scan to at most one
/// attempt per this many snapshot refreshes until the file appears.
const MISS_RETRY_PERIOD: u64 = 40;

/// Resolves and caches the active session's transcript jsonl path from the
/// provider's configured session storage.
pub(crate) struct SessionTranscriptResolver {
    storage: Option<SessionStorage>,
    cache: Mutex<TranscriptCache>,
}

#[derive(Default)]
struct TranscriptCache {
    session_id: Option<String>,
    resolved: Option<String>,
    misses: u64,
}

impl SessionTranscriptResolver {
    pub(crate) fn new(storage: Option<SessionStorage>) -> Self {
        Self {
            storage,
            cache: Mutex::new(TranscriptCache::default()),
        }
    }

    /// Resolve the transcript jsonl path for the active session, reusing a
    /// cached result when available. Returns `None` when the provider/session is
    /// unknown, no storage is configured, or the transcript file does not yet
    /// exist.
    pub(crate) fn resolve(
        &self,
        provider_name: Option<&str>,
        session_id: Option<&str>,
    ) -> Option<String> {
        let (provider_name, session_id) = self.resolvable_identity(provider_name, session_id)?;
        self.resolve_identity(provider_name, session_id)
    }

    fn resolve_identity(&self, provider_name: &str, session_id: &str) -> Option<String> {
        let mut cache = self.lock_cache();
        prepare_cache_for_session(&mut cache, session_id);
        if let Some(path) = cached_transcript_path(&cache) {
            return Some(path);
        }
        if !cache_attempt_due(&mut cache) {
            return None;
        }
        let resolved = self.locate(provider_name, session_id);
        cache.store_resolved(resolved.clone());
        resolved
    }

    fn resolvable_identity<'a>(
        &self,
        provider_name: Option<&'a str>,
        session_id: Option<&'a str>,
    ) -> Option<(&'a str, &'a str)> {
        configured_storage(&self.storage)?;
        required_transcript_identity(provider_name, session_id)
    }

    fn lock_cache(&self) -> MutexGuard<'_, TranscriptCache> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn locate(&self, provider_name: &str, session_id: &str) -> Option<String> {
        locate_transcript_path(self.storage.as_ref(), provider_name, session_id).map(path_string)
    }
}

fn configured_storage(storage: &Option<SessionStorage>) -> Option<&SessionStorage> {
    storage.as_ref()
}

fn required_transcript_identity<'a>(
    provider_name: Option<&'a str>,
    session_id: Option<&'a str>,
) -> Option<(&'a str, &'a str)> {
    Some((provider_name?, session_id?))
}

fn locate_transcript_path(
    storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let sessions_cfg = SessionsConfig::default();
    resolve_jsonl_path_for_provider_with_mode(
        &sessions_cfg,
        storage,
        provider_name,
        session_id,
        TranscriptLookupMode::RequireExisting,
    )
    .ok()
}

fn path_string(path: std::path::PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

fn prepare_cache_for_session(cache: &mut TranscriptCache, session_id: &str) {
    if !cache.session_matches(session_id) {
        cache.reset_for_session(session_id);
    }
}

fn cached_transcript_path(cache: &TranscriptCache) -> Option<String> {
    cache.cached()
}

fn cache_attempt_due(cache: &mut TranscriptCache) -> bool {
    let due = cache.attempt_due();
    cache.advance();
    due
}

impl TranscriptCache {
    fn session_matches(&self, session_id: &str) -> bool {
        self.session_id.as_deref() == Some(session_id)
    }

    fn reset_for_session(&mut self, session_id: &str) {
        self.session_id = Some(session_id.to_string());
        self.resolved = None;
        self.misses = 0;
    }

    fn cached(&self) -> Option<String> {
        self.resolved.clone()
    }

    fn attempt_due(&self) -> bool {
        self.misses.is_multiple_of(MISS_RETRY_PERIOD)
    }

    fn advance(&mut self) {
        self.misses = self.misses.saturating_add(1);
    }

    fn store_resolved(&mut self, resolved: Option<String>) {
        if let Some(path) = resolved {
            self.resolved = Some(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver_without_storage() -> SessionTranscriptResolver {
        SessionTranscriptResolver::new(None)
    }

    #[test]
    fn resolve_without_storage_is_none() {
        let resolver = resolver_without_storage();
        assert_eq!(resolver.resolve(Some("provider-x"), Some("abc")), None);
    }

    #[test]
    fn resolve_without_identity_is_none() {
        let resolver = SessionTranscriptResolver::new(Some(SessionStorage::ClaudeCode {
            projects_dir: std::path::PathBuf::from("/nonexistent-projects"),
        }));
        assert_eq!(resolver.resolve(None, Some("abc")), None);
        assert_eq!(resolver.resolve(Some("provider-x"), None), None);
    }

    #[test]
    fn cache_serves_resolved_path_without_relocating() {
        let mut cache = TranscriptCache::default();
        cache.reset_for_session("s1");
        cache.store_resolved(Some("/path/s1.jsonl".to_string()));
        assert_eq!(cache.cached().as_deref(), Some("/path/s1.jsonl"));
    }

    #[test]
    fn cache_resets_when_session_changes() {
        let mut cache = TranscriptCache::default();
        cache.reset_for_session("s1");
        cache.store_resolved(Some("/path/s1.jsonl".to_string()));
        assert!(cache.session_matches("s1"));
        assert!(!cache.session_matches("s2"));
        cache.reset_for_session("s2");
        assert_eq!(cache.cached(), None);
        assert_eq!(cache.session_id.as_deref(), Some("s2"));
    }

    #[test]
    fn miss_attempts_follow_fixed_cadence() {
        let mut cache = TranscriptCache::default();
        cache.reset_for_session("s1");
        // First attempt is due, then skipped until the retry period elapses.
        assert!(cache.attempt_due());
        cache.advance();
        for _ in 1..MISS_RETRY_PERIOD {
            assert!(!cache.attempt_due());
            cache.advance();
        }
        assert!(cache.attempt_due());
    }
}
