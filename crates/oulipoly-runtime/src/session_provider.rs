//! S7a external-provider transcript/session dispatch adapter.
//!
//! Providers return facts over the provider JSON contract. This module maps
//! those facts into runtime-owned transcript, turn, and capture shapes; callers
//! remain responsible for host-owned state transitions.
//!
//! ## Declared roles
//! orchestration, accessor, validator, mapper, formatter, parser, filter, predicate

mod dispatch;
mod enumerate;
mod host;
mod ingest;
mod lifecycle_proof;
mod locate;
mod provider_client;
mod request;
mod turns;
mod types;
mod worker;

pub(crate) use dispatch::capture_live_report_with_client;
pub(crate) use dispatch::locate_transcript_with_raw_metadata_with_cancellation;
pub use dispatch::{
    capture, capture_for_lifecycle, capture_live_report, enumerate_sessions, locate_transcript,
    locate_transcript_with_raw_metadata, read_turn_page,
};
pub use ingest::{canonical_stream_key, ingest_one_canonical_turn_page};
pub use lifecycle_proof::dispatch_aware_no_ref_lifecycle_proof;
pub use types::{
    NoRefProofOutput, NoRefProofRequest, SessionProviderCaptureRequest,
    SessionProviderCaptureResult, SessionProviderEnumerateEntry, SessionProviderEnumerateRequest,
    SessionProviderEnumerateResult, SessionProviderEnumerateSource, SessionProviderError,
    SessionProviderIdentity, SessionProviderLifecycleContext, SessionProviderLiveCaptureRequest,
    SessionProviderLocateRequest, SessionProviderLocatedTranscript, SessionProviderPageCursor,
    SessionProviderPageTurn, SessionProviderReadPageRequest, SessionProviderReadPageResult,
    SessionProviderTurnProjection, SessionTurnIngestQuantumRequest,
};
pub use worker::{
    SessionTurnIngestDriverRequest, SessionTurnIngestQuantumOutcome,
    run_one_session_turn_ingest_quantum, run_session_turn_ingest_quantum_for_key,
};

/// Fixed paging refusals require resolution and explicit host-consumer rearming.
/// Arbitrary provider messages/codes are not diagnostic authority.
pub(crate) fn fixed_paging_stop_reason(token: &str) -> Option<&'static str> {
    match token {
        "session_turn_page_budget_too_small" => Some("session_turn_page_budget_too_small"),
        "session_turn_record_ceiling_exceeded" => Some("session_turn_record_ceiling_exceeded"),
        "session_turn_staging_capacity_exceeded" => Some("session_turn_staging_capacity_exceeded"),
        "session_turn_paging_paused" => Some("session_turn_paging_paused"),
        "codex_rollout_capacity" => Some("codex_rollout_capacity"),
        _ => None,
    }
}
