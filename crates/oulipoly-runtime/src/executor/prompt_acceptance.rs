//! Host-owned promotion of provider prompt-acceptance attestations.
//!
//! ## Declared roles
//!
//! Roles: validator, formatter.

use oulipoly_provider::generated::{PROMPT_ACCEPTANCE_V1, PromptAcceptedMarkerValueV1};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedPromptAcceptance<'a> {
    pub provider_session_id: &'a str,
    pub prompt_sha256: &'a str,
    pub delivery_nonce: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidatedPromptAcceptance {
    DeliveryNonceAndPromptSha256,
    PromptSha256,
}

impl ValidatedPromptAcceptance {
    pub fn evidence(self) -> &'static str {
        match self {
            Self::DeliveryNonceAndPromptSha256 => {
                "validated prompt acceptance: exact session, delivery nonce, and prompt SHA-256"
            }
            Self::PromptSha256 => "validated prompt acceptance: exact session and prompt SHA-256",
        }
    }
}

pub fn promote_prompt_acceptance_attestation(
    expected: ExpectedPromptAcceptance<'_>,
    attestation: &PromptAcceptedMarkerValueV1,
) -> Option<ValidatedPromptAcceptance> {
    if attestation.protocol != PROMPT_ACCEPTANCE_V1
        || attestation.provider_session_id != expected.provider_session_id
        || attestation.prompt_sha256 != expected.prompt_sha256
    {
        return None;
    }
    match expected.delivery_nonce {
        Some(expected_nonce) => (attestation.delivery_nonce.as_deref() == Some(expected_nonce))
            .then_some(ValidatedPromptAcceptance::DeliveryNonceAndPromptSha256),
        None => Some(ValidatedPromptAcceptance::PromptSha256),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExpectedPromptAcceptance, ValidatedPromptAcceptance, promote_prompt_acceptance_attestation,
    };
    use oulipoly_provider::generated::{PROMPT_ACCEPTANCE_V1, PromptAcceptedMarkerValueV1};

    fn attestation() -> PromptAcceptedMarkerValueV1 {
        PromptAcceptedMarkerValueV1 {
            protocol: PROMPT_ACCEPTANCE_V1.to_string(),
            provider_session_id: "session-1".to_string(),
            prompt_sha256: "prompt-hash".to_string(),
            delivery_nonce: Some("delivery-1".to_string()),
            source: None,
            message_id: None,
        }
    }

    #[test]
    fn promotion_requires_exact_protocol_session_prompt_and_applicable_nonce() {
        let expected = ExpectedPromptAcceptance {
            provider_session_id: "session-1",
            prompt_sha256: "prompt-hash",
            delivery_nonce: Some("delivery-1"),
        };
        assert_eq!(
            promote_prompt_acceptance_attestation(expected, &attestation()),
            Some(ValidatedPromptAcceptance::DeliveryNonceAndPromptSha256)
        );

        for candidate in [
            PromptAcceptedMarkerValueV1 {
                protocol: "oulipoly.prompt_acceptance/v2".to_string(),
                ..attestation()
            },
            PromptAcceptedMarkerValueV1 {
                provider_session_id: "other-session".to_string(),
                ..attestation()
            },
            PromptAcceptedMarkerValueV1 {
                prompt_sha256: "other-hash".to_string(),
                ..attestation()
            },
            PromptAcceptedMarkerValueV1 {
                delivery_nonce: Some("other-delivery".to_string()),
                ..attestation()
            },
        ] {
            assert_eq!(
                promote_prompt_acceptance_attestation(expected, &candidate),
                None
            );
        }
    }

    #[test]
    fn manual_prompt_promotion_requires_exact_prompt_without_a_nonce() {
        let expected = ExpectedPromptAcceptance {
            provider_session_id: "session-1",
            prompt_sha256: "prompt-hash",
            delivery_nonce: None,
        };
        assert_eq!(
            promote_prompt_acceptance_attestation(expected, &attestation()),
            Some(ValidatedPromptAcceptance::PromptSha256)
        );
    }
}
