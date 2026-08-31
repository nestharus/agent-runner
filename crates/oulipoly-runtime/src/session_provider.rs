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

pub(crate) use dispatch::locate_transcript_with_raw_metadata_with_cancellation;
pub use dispatch::{
    capture, capture_for_lifecycle, capture_live_report, enumerate_sessions, locate_transcript,
    locate_transcript_with_raw_metadata, read_turn_page,
};
pub use ingest::{canonical_stream_key, ingest_one_canonical_turn_page};
pub use lifecycle_proof::dispatch_aware_no_ref_lifecycle_proof;
pub use types::{
    NoRefProofOutput, NoRefProofRequest, S7A_NEUTRAL_SETTINGS_ID, SessionProviderCaptureRequest,
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
