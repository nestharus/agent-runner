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
mod lifecycle_proof;
mod locate;
mod provider_client;
mod request;
mod turns;
mod types;

pub use dispatch::{
    capture, capture_for_lifecycle, enumerate_sessions, locate_transcript,
    locate_transcript_with_raw_metadata, read_turns, read_turns_for_lifecycle,
    read_user_turn_observations,
};
pub use lifecycle_proof::dispatch_aware_no_ref_lifecycle_proof;
pub use turns::{assert_turn_count_diagnostic, ingest_owned_turns};
pub use types::{
    NoRefProofOutput, NoRefProofRequest, S7A_NEUTRAL_SETTINGS_ID, SessionProviderCaptureRequest,
    SessionProviderCaptureResult, SessionProviderEnumerateEntry, SessionProviderEnumerateRequest,
    SessionProviderEnumerateResult, SessionProviderEnumerateSource, SessionProviderError,
    SessionProviderIdentity, SessionProviderLifecycleContext, SessionProviderLocateRequest,
    SessionProviderLocatedTranscript, SessionProviderReadTurnsRequest,
    SessionProviderReadTurnsResult, SessionProviderTurn,
};
