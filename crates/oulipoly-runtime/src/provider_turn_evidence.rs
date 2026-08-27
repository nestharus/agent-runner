//! Provider evidence interpretation, trust classification, and persistence encoding.
//!
//! ## Declared roles
//!
//! `formatter`, `mapper`, `validator`

use oulipoly_provider::generated::PromptAcceptedMarkerValueV1;
use oulipoly_state::TurnFence;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::executor::ResumeAcceptanceStatus;
use crate::executor::prompt_acceptance::{
    ExpectedPromptAcceptance, promote_prompt_acceptance_attestation,
};
use crate::executor::terminal_signal::TerminalSignalKind;
use crate::provider_turn_adapter::{ProviderTurnAdapterError, ProviderTurnLaunch};
use crate::provider_turn_execution::{ProviderExecutionOutcome, ProviderExecutionStatus};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderEvidence {
    ProcessLaunched,
    TransportAccepted,
    PromptAcceptanceAttestation(PromptAcceptedMarkerValueV1),
    ResumeAccepted {
        provider_session_id: String,
        evidence: String,
    },
    IngestedUserTurn {
        provider_session_id: String,
        turn_id: String,
    },
    AssistantOutput {
        provider_session_id: String,
    },
    AssistantOutputAbsent,
    IngestedAssistantTurn {
        provider_session_id: String,
        turn_id: String,
    },
    AffirmativeProviderCompletion {
        provider_session_id: String,
        evidence: String,
    },
    TerminalSignal {
        kind: TerminalSignalKind,
        evidence: String,
    },
    ProviderRejected {
        reason: String,
    },
    QuotaExhausted {
        reason: String,
    },
    Malformed {
        reason: String,
    },
    Manual {
        evidence: String,
    },
    ResumeCompletionUnconfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FencedProviderEvidence {
    pub fence: TurnFence,
    pub evidence: ProviderEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceStrength {
    Informational,
    Submitted,
    Confirmed,
}

pub fn classify_provider_evidence(
    expected_fence: &TurnFence,
    expected_session_id: &str,
    expected_prompt_sha256: Option<&str>,
    expected_delivery_nonce: Option<&str>,
    evidence: &FencedProviderEvidence,
) -> Result<EvidenceStrength, ProviderTurnAdapterError> {
    validate_evidence_fence(expected_fence, evidence)?;
    let strength = match &evidence.evidence {
        ProviderEvidence::PromptAcceptanceAttestation(attestation) => {
            prompt_acceptance_evidence_strength(
                expected_session_id,
                expected_prompt_sha256,
                expected_delivery_nonce,
                attestation,
            )
        }
        ProviderEvidence::ResumeAccepted {
            provider_session_id,
            evidence,
        } if provider_session_id == expected_session_id && !evidence.trim().is_empty() => {
            EvidenceStrength::Submitted
        }
        ProviderEvidence::IngestedUserTurn {
            provider_session_id,
            turn_id,
        } if provider_session_id == expected_session_id && !turn_id.trim().is_empty() => {
            EvidenceStrength::Submitted
        }
        ProviderEvidence::AssistantOutput {
            provider_session_id,
        } if provider_session_id == expected_session_id => EvidenceStrength::Confirmed,
        ProviderEvidence::IngestedAssistantTurn {
            provider_session_id,
            turn_id,
        } if provider_session_id == expected_session_id && !turn_id.trim().is_empty() => {
            EvidenceStrength::Confirmed
        }
        ProviderEvidence::AffirmativeProviderCompletion {
            provider_session_id,
            evidence,
        } if provider_session_id == expected_session_id && !evidence.trim().is_empty() => {
            EvidenceStrength::Confirmed
        }
        _ => EvidenceStrength::Informational,
    };
    Ok(strength)
}

fn prompt_acceptance_evidence_strength(
    expected_session_id: &str,
    expected_prompt_sha256: Option<&str>,
    expected_delivery_nonce: Option<&str>,
    attestation: &PromptAcceptedMarkerValueV1,
) -> EvidenceStrength {
    let Some(expected_prompt_sha256) = expected_prompt_sha256 else {
        return EvidenceStrength::Informational;
    };
    if promote_prompt_acceptance_attestation(
        ExpectedPromptAcceptance {
            provider_session_id: expected_session_id,
            prompt_sha256: expected_prompt_sha256,
            delivery_nonce: expected_delivery_nonce,
        },
        attestation,
    )
    .is_some()
    {
        EvidenceStrength::Submitted
    } else {
        EvidenceStrength::Informational
    }
}

pub fn prompt_sha256(prompt: &str) -> String {
    let digest = Sha256::digest(prompt.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn validate_evidence_fence(
    expected: &TurnFence,
    evidence: &FencedProviderEvidence,
) -> Result<(), ProviderTurnAdapterError> {
    if evidence.fence != *expected {
        return Err(ProviderTurnAdapterError::InvalidFence("provider evidence"));
    }
    Ok(())
}

pub(crate) fn evidence_from_execution(
    launch: &ProviderTurnLaunch,
    fence: &TurnFence,
    execution: &ProviderExecutionOutcome,
) -> Vec<FencedProviderEvidence> {
    let mut evidence = Vec::new();
    let Some(result) = execution.result.as_ref() else {
        evidence.push(fenced(fence, failure_evidence(execution)));
        return evidence;
    };
    evidence.push(fenced(fence, ProviderEvidence::ProcessLaunched));
    if let Some(attestation) = result.prompt_acceptance_attestation.as_ref() {
        evidence.push(fenced(
            fence,
            ProviderEvidence::PromptAcceptanceAttestation(attestation.clone()),
        ));
    }
    if let Some(acceptance) = result.resume_acceptance.as_ref() {
        let fact = match acceptance.status {
            ResumeAcceptanceStatus::Accepted => ProviderEvidence::ResumeAccepted {
                provider_session_id: launch.mailbox_batch.session_id.clone(),
                evidence: acceptance.evidence.clone().unwrap_or_default(),
            },
            ResumeAcceptanceStatus::Rejected => ProviderEvidence::ProviderRejected {
                reason: acceptance.evidence.clone().unwrap_or_default(),
            },
            ResumeAcceptanceStatus::Unconfirmed => ProviderEvidence::ResumeCompletionUnconfirmed,
        };
        evidence.push(fenced(fence, fact));
    }
    evidence.push(fenced(
        fence,
        if result.produced_assistant_response {
            ProviderEvidence::AssistantOutput {
                provider_session_id: launch.mailbox_batch.session_id.clone(),
            }
        } else {
            ProviderEvidence::AssistantOutputAbsent
        },
    ));
    if let Some(signal) = result.terminal_signal.as_ref() {
        evidence.push(fenced(
            fence,
            ProviderEvidence::TerminalSignal {
                kind: signal.kind,
                evidence: signal.evidence.clone(),
            },
        ));
    }
    if execution.status == ProviderExecutionStatus::ResumeCompletionUnconfirmed {
        evidence.push(fenced(fence, ProviderEvidence::ResumeCompletionUnconfirmed));
    }
    evidence
}

fn fenced(fence: &TurnFence, evidence: ProviderEvidence) -> FencedProviderEvidence {
    FencedProviderEvidence {
        fence: fence.clone(),
        evidence,
    }
}

fn failure_evidence(execution: &ProviderExecutionOutcome) -> ProviderEvidence {
    let reason = execution
        .error
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{:?}", execution.status));
    match execution.status {
        ProviderExecutionStatus::ProviderRejected => ProviderEvidence::ProviderRejected { reason },
        ProviderExecutionStatus::QuotaExhausted => ProviderEvidence::QuotaExhausted { reason },
        ProviderExecutionStatus::MalformedEvidence => ProviderEvidence::Malformed { reason },
        ProviderExecutionStatus::ResumeCompletionUnconfirmed => {
            ProviderEvidence::ResumeCompletionUnconfirmed
        }
        ProviderExecutionStatus::Completed
        | ProviderExecutionStatus::AbnormalExit
        | ProviderExecutionStatus::LaunchFailed => ProviderEvidence::TerminalSignal {
            kind: TerminalSignalKind::Unknown,
            evidence: reason,
        },
    }
}

pub(crate) fn acknowledgement_evidence(
    launch: &ProviderTurnLaunch,
    fence: &TurnFence,
    evidence: &[FencedProviderEvidence],
) -> Result<(Option<String>, Option<String>), ProviderTurnAdapterError> {
    if launch.mailbox_batch.delivery_ids.is_empty() {
        return Ok((None, None));
    }
    let prompt_hash = launch.request.prompt().map(prompt_sha256);
    let submitted = select_evidence(
        EvidenceStrength::Submitted,
        fence,
        launch,
        prompt_hash.as_deref(),
        evidence,
    )?;
    let confirmed = select_evidence(
        EvidenceStrength::Confirmed,
        fence,
        launch,
        prompt_hash.as_deref(),
        evidence,
    )?;
    // Confirmation is stronger than submission. Persist the stronger fact at
    // both monotonic stages when the provider exposes no separate submit fact.
    let submission = submitted.as_deref().or(confirmed.as_deref());
    Ok((submission.map(ToOwned::to_owned), confirmed))
}

fn select_evidence(
    strength: EvidenceStrength,
    fence: &TurnFence,
    launch: &ProviderTurnLaunch,
    prompt_hash: Option<&str>,
    evidence: &[FencedProviderEvidence],
) -> Result<Option<String>, ProviderTurnAdapterError> {
    let mut candidates = evidence
        .iter()
        .filter_map(|item| {
            let classified = classify_provider_evidence(
                fence,
                &launch.mailbox_batch.session_id,
                prompt_hash,
                launch.mailbox_batch.delivery_nonce.as_deref(),
                item,
            );
            match classified {
                Ok(actual) if actual == strength => Some(Ok((
                    evidence_rank(&item.evidence),
                    encode_evidence(&item.evidence),
                ))),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    candidates.sort();
    Ok(candidates.into_iter().next().map(|(_, encoded)| encoded))
}

fn evidence_rank(evidence: &ProviderEvidence) -> u8 {
    match evidence {
        ProviderEvidence::PromptAcceptanceAttestation(_) => 0,
        ProviderEvidence::IngestedUserTurn { .. } => 1,
        ProviderEvidence::ResumeAccepted { .. } => 2,
        ProviderEvidence::IngestedAssistantTurn { .. } => 0,
        ProviderEvidence::AssistantOutput { .. } => 1,
        ProviderEvidence::AffirmativeProviderCompletion { .. } => 2,
        ProviderEvidence::ProcessLaunched
        | ProviderEvidence::TransportAccepted
        | ProviderEvidence::AssistantOutputAbsent
        | ProviderEvidence::TerminalSignal { .. }
        | ProviderEvidence::ProviderRejected { .. }
        | ProviderEvidence::QuotaExhausted { .. }
        | ProviderEvidence::Malformed { .. }
        | ProviderEvidence::Manual { .. }
        | ProviderEvidence::ResumeCompletionUnconfirmed => u8::MAX,
    }
}

fn encode_evidence(evidence: &ProviderEvidence) -> String {
    let value = match evidence {
        ProviderEvidence::PromptAcceptanceAttestation(attestation) => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "prompt_acceptance_attestation",
            "protocol": attestation.protocol,
            "provider_session_id": attestation.provider_session_id,
            "prompt_sha256": attestation.prompt_sha256,
            "delivery_nonce": attestation.delivery_nonce,
            "source": attestation.source,
            "message_id": attestation.message_id,
        }),
        ProviderEvidence::ResumeAccepted {
            provider_session_id,
            evidence,
        } => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "resume_accepted",
            "provider_session_id": provider_session_id,
            "evidence": evidence,
        }),
        ProviderEvidence::IngestedUserTurn {
            provider_session_id,
            turn_id,
        } => turn_evidence("ingested_user_turn", provider_session_id, turn_id),
        ProviderEvidence::AssistantOutput {
            provider_session_id,
        } => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "assistant_output",
            "provider_session_id": provider_session_id,
        }),
        ProviderEvidence::IngestedAssistantTurn {
            provider_session_id,
            turn_id,
        } => turn_evidence("ingested_assistant_turn", provider_session_id, turn_id),
        ProviderEvidence::AffirmativeProviderCompletion {
            provider_session_id,
            evidence,
        } => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "affirmative_provider_completion",
            "provider_session_id": provider_session_id,
            "evidence": evidence,
        }),
        _ => json!({
            "schema": "oulipoly.provider-turn-evidence/v1",
            "kind": "informational",
            "evidence": format!("{evidence:?}"),
        }),
    };
    serde_json::to_string(&value).expect("provider evidence JSON serializes")
}

fn turn_evidence(kind: &str, provider_session_id: &str, turn_id: &str) -> Value {
    json!({
        "schema": "oulipoly.provider-turn-evidence/v1",
        "kind": kind,
        "provider_session_id": provider_session_id,
        "turn_id": turn_id,
    })
}
