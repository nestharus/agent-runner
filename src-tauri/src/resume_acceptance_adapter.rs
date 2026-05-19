//! Typed resume-acceptance category mapping for CLI resume handling.
//!
//! ## Declared roles
//!
//! `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/resume_acceptance_adapter.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly_runtime::executor::ResumeAcceptanceResult contract
//!       - oulipoly_runtime::diagnostics ResumeSessionMismatch category contract
//!       - src-tauri CLI headless-resume error-category contract
//! ```

use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::{ResumeAcceptanceResult, ResumeAcceptanceStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeAcceptanceCategory {
    SessionMismatch,
    OtherRejected,
    AcceptedOrUnconfirmed,
    Absent,
}

pub(crate) fn classify(acceptance: Option<&ResumeAcceptanceResult>) -> ResumeAcceptanceCategory {
    let Some(acceptance) = acceptance else {
        return ResumeAcceptanceCategory::Absent;
    };

    if acceptance.status != ResumeAcceptanceStatus::Rejected {
        return ResumeAcceptanceCategory::AcceptedOrUnconfirmed;
    }

    if legacy_evidence_is_session_mismatch(acceptance.evidence.as_deref()) {
        ResumeAcceptanceCategory::SessionMismatch
    } else {
        ResumeAcceptanceCategory::OtherRejected
    }
}

fn legacy_evidence_is_session_mismatch(evidence: Option<&str>) -> bool {
    evidence
        .is_some_and(|evidence| evidence.contains(ErrorCategory::ResumeSessionMismatch.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(evidence: Option<&str>) -> ResumeAcceptanceResult {
        ResumeAcceptanceResult {
            status: ResumeAcceptanceStatus::Rejected,
            evidence: evidence.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn age151_resume_acceptance_rejected_mismatch_maps_to_typed_session_mismatch_category() {
        let acceptance = rejected(Some(ErrorCategory::ResumeSessionMismatch.as_str()));

        assert_eq!(
            classify(Some(&acceptance)),
            ResumeAcceptanceCategory::SessionMismatch
        );
    }

    #[test]
    fn age151_resume_acceptance_absent_none_evidence_accepted_and_unrelated_rejected_are_not_mismatch()
     {
        let none_evidence = rejected(None);
        let accepted = ResumeAcceptanceResult {
            status: ResumeAcceptanceStatus::Accepted,
            evidence: Some(ErrorCategory::ResumeSessionMismatch.as_str().to_string()),
        };
        let unconfirmed = ResumeAcceptanceResult {
            status: ResumeAcceptanceStatus::Unconfirmed,
            evidence: Some(ErrorCategory::ResumeSessionMismatch.as_str().to_string()),
        };
        let unrelated_rejected = rejected(Some("provider refused unrelated resume state"));

        assert_eq!(classify(None), ResumeAcceptanceCategory::Absent);
        assert_eq!(
            classify(Some(&none_evidence)),
            ResumeAcceptanceCategory::OtherRejected
        );
        assert_eq!(
            classify(Some(&accepted)),
            ResumeAcceptanceCategory::AcceptedOrUnconfirmed
        );
        assert_eq!(
            classify(Some(&unconfirmed)),
            ResumeAcceptanceCategory::AcceptedOrUnconfirmed
        );
        assert_eq!(
            classify(Some(&unrelated_rejected)),
            ResumeAcceptanceCategory::OtherRejected
        );
    }
}
