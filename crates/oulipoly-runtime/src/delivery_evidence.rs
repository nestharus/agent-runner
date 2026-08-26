//! Exact-fenced transport evidence below provider lifecycle confirmation.

use oulipoly_state::{
    AcknowledgementWrite, DeliveryEvidence, DeliveryEvidenceKind, SessionLifecycleRepository,
    SessionLifecycleResult,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PtyTransportAcknowledgementEvidence {
    pub evidence_id: String,
    pub delivery_attempt_id: String,
    pub session_id: String,
    pub turn_generation_id: String,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManualAcknowledgementEvidence {
    pub evidence_id: String,
    pub delivery_attempt_id: String,
    pub session_id: String,
    pub turn_generation_id: String,
    pub observed_at: i64,
}

impl PtyTransportAcknowledgementEvidence {
    pub fn record(
        &self,
        repository: &mut dyn SessionLifecycleRepository,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        repository.accept_pending_with_delivery_evidence(&DeliveryEvidence {
            evidence_id: self.evidence_id.clone(),
            kind: DeliveryEvidenceKind::PtyTransportAck,
            delivery_id: self.delivery_attempt_id.clone(),
            session_id: self.session_id.clone(),
            turn_generation_id: self.turn_generation_id.clone(),
            observed_at: self.observed_at,
        })
    }
}

impl ManualAcknowledgementEvidence {
    pub fn record(
        &self,
        repository: &mut dyn SessionLifecycleRepository,
    ) -> SessionLifecycleResult<AcknowledgementWrite> {
        repository.record_delivery_evidence(&DeliveryEvidence {
            evidence_id: self.evidence_id.clone(),
            kind: DeliveryEvidenceKind::ManualAcknowledgement,
            delivery_id: self.delivery_attempt_id.clone(),
            session_id: self.session_id.clone(),
            turn_generation_id: self.turn_generation_id.clone(),
            observed_at: self.observed_at,
        })
    }
}
