//! Host-owned promotion of provider prompt-acceptance attestations.
//!
//! ## Declared roles
//!
//! Roles: validator, accessor.

use oulipoly_provider::generated::{PROMPT_ACCEPTANCE_V1, PromptAcceptedMarkerValueV1};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedPromptAcceptance<'a> {
    pub provider_session_id: &'a str,
    pub prompt_sha256: &'a str,
    pub delivery_nonce: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedPromptAcceptance {
    protocol: String,
    provider_session_id: String,
    prompt_sha256: String,
    delivery_nonce: Option<String>,
}

impl ValidatedPromptAcceptance {
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    pub fn provider_session_id(&self) -> &str {
        &self.provider_session_id
    }

    pub fn prompt_sha256(&self) -> &str {
        &self.prompt_sha256
    }

    pub fn delivery_nonce(&self) -> Option<&str> {
        self.delivery_nonce.as_deref()
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
    if expected.delivery_nonce.is_some()
        && attestation.delivery_nonce.as_deref() != expected.delivery_nonce
    {
        return None;
    }
    Some(ValidatedPromptAcceptance {
        protocol: PROMPT_ACCEPTANCE_V1.to_string(),
        provider_session_id: expected.provider_session_id.to_string(),
        prompt_sha256: expected.prompt_sha256.to_string(),
        delivery_nonce: expected.delivery_nonce.map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::{ExpectedPromptAcceptance, promote_prompt_acceptance_attestation};
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
        let validated = promote_prompt_acceptance_attestation(expected, &attestation())
            .expect("exact attestation must promote");
        assert_eq!(validated.protocol(), PROMPT_ACCEPTANCE_V1);
        assert_eq!(validated.provider_session_id(), "session-1");
        assert_eq!(validated.prompt_sha256(), "prompt-hash");
        assert_eq!(validated.delivery_nonce(), Some("delivery-1"));

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
        let validated = promote_prompt_acceptance_attestation(expected, &attestation())
            .expect("exact manual attestation must promote");
        assert_eq!(validated.protocol(), PROMPT_ACCEPTANCE_V1);
        assert_eq!(validated.provider_session_id(), "session-1");
        assert_eq!(validated.prompt_sha256(), "prompt-hash");
        assert_eq!(validated.delivery_nonce(), None);
    }
}
