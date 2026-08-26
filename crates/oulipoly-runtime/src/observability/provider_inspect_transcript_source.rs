//! Provider-owned active-session transcript resolution for TUI inspect.

use crate::observability::limits::SnapshotLimits;
use crate::observability::transcript_source::ResolvedSessionTranscript;
use crate::provider_registry::ProviderRegistry;
use crate::session_metadata::TranscriptLookupMode;
use crate::session_provider::{self, SessionProviderIdentity, SessionProviderLocateRequest};
use oulipoly_core::CancellationToken;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct ProviderInspectTranscriptResolver {
    registry: Arc<ProviderRegistry>,
    identity: SessionProviderIdentity,
    active_session_id: String,
    effective_cwd: Option<PathBuf>,
}

impl ProviderInspectTranscriptResolver {
    pub(crate) fn new(
        registry: Arc<ProviderRegistry>,
        identity: SessionProviderIdentity,
        active_session_id: String,
        effective_cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            registry,
            identity,
            active_session_id,
            effective_cwd,
        }
    }

    pub(crate) fn resolve(
        &self,
        limits: SnapshotLimits,
        cancellation: &CancellationToken,
    ) -> Option<ResolvedSessionTranscript> {
        let located = session_provider::locate_transcript_with_raw_metadata_with_cancellation(
            SessionProviderLocateRequest {
                registry: self.registry.as_ref(),
                identity: self.identity.clone(),
                session_id: &self.active_session_id,
                lookup_mode: TranscriptLookupMode::RequireExisting,
                effective_cwd: self.effective_cwd.as_deref(),
                purpose: Some("inspect"),
                tail_bytes_hint: Some(limits.transcript_tail_bytes),
            },
            cancellation,
        )
        .ok()?;
        let format_id = located
            .format_id
            .filter(|format_id| !format_id.is_empty())?;
        Some(ResolvedSessionTranscript::with_metadata(
            located.path,
            Some(format_id),
            located.source_id,
        ))
    }
}
