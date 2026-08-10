//! ## Declared roles
//!
//! `accessor`
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/fresh_continuation.rs
//!     role: intrinsic-surface
//!     Domain: fresh_continuation_public_api
//!     Owns:
//!       - fresh-continuation child module declarations
//!       - fresh-continuation contract and production adapter re-exports
//! ```

mod contract;
mod coordinator;
mod evidence;
mod state_db_store;

pub use contract::{
    AcceptDecision, AcceptedContinuation, ArtifactIdentity, ContinuationArtifactSource,
    ContinuationBlock, ContinuationBlockKind, ContinuationEvidence, ContinuationEvidenceValidator,
    ContinuationHandoff, ContinuationStore, FreshContinuation, FreshContinuationOutcome,
    FreshContinuationRequest, FreshRunner, HandoffPublisher, InvocationAction,
    InvocationDisposition, InvocationOutcome, PublishedHandoff, ReservedInvocation,
    ResumeAcceptance, ResumeRunner, RunDecision, ValidatedContinuation, fresh_prompt,
};
pub use coordinator::FreshContinuationCoordinator;
pub use evidence::DefaultContinuationEvidenceValidator;
pub use state_db_store::StateDbContinuationStore;
