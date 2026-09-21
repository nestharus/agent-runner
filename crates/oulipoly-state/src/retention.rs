//! Policy and execution contracts for bounded historical retention.
//!
//! This module owns decisions, not scheduling.  AGE-377 may drive the bounded
//! operations exposed by State, mailbox, and event-generation adapters, but it
//! must not replace these fail-closed policy decisions with age-only logic.

use crate::diagnostic_recorder::{
    DiagnosticPhase, PhaseObservation, RecordStatus, SpanStart, process_recorder,
};
use crate::event_store::{
    Digest32, GenerationEligibilityFacts, GenerationId, GenerationMaintenanceTarget,
    PartitionLifecycleState, RetirementReceipt, WriterInstanceId,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DEFAULT_TERMINAL_RETENTION_DAYS: i64 = 30;
pub const DEFAULT_TERMINAL_RETENTION_MICROS: i64 =
    DEFAULT_TERMINAL_RETENTION_DAYS * 24 * 60 * 60 * 1_000_000;
pub const RETENTION_POLICY_VERSION: &str = "age372.v1";

/// Independently configurable authoritative roots.  The event families are
/// listed separately even though a mixed event generation is retired as one
/// directory; the generation uses the longest policy of every possible event
/// family and therefore cannot weaken a family-specific policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionFamily {
    Invocation,
    ProviderLogicalLaunch,
    ProviderLaunchAttempt,
    CompletedTurn,
    Mailbox,
    MailboxDeliveryAttempt,
    CompletionEvent,
    CompletionEventListener,
    RuntimeGeneration,
    DiagnosticEvent,
    TraceEvent,
    MetricEvent,
    LogEvent,
    MaintenanceEvent,
}

impl RetentionFamily {
    pub const ALL: [Self; 14] = [
        Self::Invocation,
        Self::ProviderLogicalLaunch,
        Self::ProviderLaunchAttempt,
        Self::CompletedTurn,
        Self::Mailbox,
        Self::MailboxDeliveryAttempt,
        Self::CompletionEvent,
        Self::CompletionEventListener,
        Self::RuntimeGeneration,
        Self::DiagnosticEvent,
        Self::TraceEvent,
        Self::MetricEvent,
        Self::LogEvent,
        Self::MaintenanceEvent,
    ];

    pub const EVENT_FAMILIES: [Self; 5] = [
        Self::DiagnosticEvent,
        Self::TraceEvent,
        Self::MetricEvent,
        Self::LogEvent,
        Self::MaintenanceEvent,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Invocation => "invocation",
            Self::ProviderLogicalLaunch => "provider_logical_launch",
            Self::ProviderLaunchAttempt => "provider_launch_attempt",
            Self::CompletedTurn => "completed_turn",
            Self::Mailbox => "mailbox",
            Self::MailboxDeliveryAttempt => "mailbox_delivery_attempt",
            Self::CompletionEvent => "completion_event",
            Self::CompletionEventListener => "completion_event_listener",
            Self::RuntimeGeneration => "runtime_generation",
            Self::DiagnosticEvent => "diagnostic_event",
            Self::TraceEvent => "trace_event",
            Self::MetricEvent => "metric_event",
            Self::LogEvent => "log_event",
            Self::MaintenanceEvent => "maintenance_event",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionBoundary {
    /// A record is eligible when `authoritative_time <= as_of - horizon`.
    /// Thus an age exactly equal to the horizon is eligible.
    Inclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyRetentionPolicy {
    pub family: RetentionFamily,
    pub horizon_micros: i64,
    pub boundary: RetentionBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub version: String,
    pub families: Vec<FamilyRetentionPolicy>,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self::default_30_days()
    }
}

impl RetentionPolicy {
    pub fn default_30_days() -> Self {
        Self {
            version: RETENTION_POLICY_VERSION.to_string(),
            families: RetentionFamily::ALL
                .into_iter()
                .map(|family| FamilyRetentionPolicy {
                    family,
                    horizon_micros: DEFAULT_TERMINAL_RETENTION_MICROS,
                    boundary: RetentionBoundary::Inclusive,
                })
                .collect(),
        }
    }

    pub fn validate(&self) -> Result<(), RetentionPolicyError> {
        if self.version.is_empty()
            || self.version.len() > 64
            || !self
                .version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(RetentionPolicyError::InvalidVersion);
        }
        let mut observed = BTreeSet::new();
        for rule in &self.families {
            if rule.horizon_micros <= 0 || !observed.insert(rule.family) {
                return Err(RetentionPolicyError::InvalidFamilyRule(rule.family));
            }
        }
        if observed != RetentionFamily::ALL.into_iter().collect() {
            return Err(RetentionPolicyError::IncompleteFamilyRules);
        }
        Ok(())
    }

    pub fn rule(
        &self,
        family: RetentionFamily,
    ) -> Result<&FamilyRetentionPolicy, RetentionPolicyError> {
        self.validate()?;
        self.families
            .iter()
            .find(|rule| rule.family == family)
            .ok_or(RetentionPolicyError::IncompleteFamilyRules)
    }

    pub fn cutoff_unix_micros(
        &self,
        family: RetentionFamily,
        as_of_unix_micros: i64,
    ) -> Result<i64, RetentionPolicyError> {
        if as_of_unix_micros < 0 {
            return Err(RetentionPolicyError::InvalidAsOf);
        }
        as_of_unix_micros
            .checked_sub(self.rule(family)?.horizon_micros)
            .ok_or(RetentionPolicyError::CutoffUnderflow)
    }

    /// A mixed event generation uses the longest configured event-family
    /// horizon.  This remains fail-closed without scanning the generation to
    /// infer which event family happens to be present.
    pub fn event_generation_cutoff_unix_micros(
        &self,
        as_of_unix_micros: i64,
    ) -> Result<i64, RetentionPolicyError> {
        self.validate()?;
        if as_of_unix_micros < 0 {
            return Err(RetentionPolicyError::InvalidAsOf);
        }
        let horizon = RetentionFamily::EVENT_FAMILIES
            .into_iter()
            .map(|family| self.rule(family).map(|rule| rule.horizon_micros))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .ok_or(RetentionPolicyError::IncompleteFamilyRules)?;
        as_of_unix_micros
            .checked_sub(horizon)
            .ok_or(RetentionPolicyError::CutoffUnderflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicyError {
    InvalidVersion,
    InvalidFamilyRule(RetentionFamily),
    IncompleteFamilyRules,
    InvalidRotatedHistoryFamily(RetentionFamily),
    InvalidAsOf,
    CutoffUnderflow,
}

impl std::fmt::Display for RetentionPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid retention policy: {self:?}")
    }
}

impl std::error::Error for RetentionPolicyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreservationReason {
    Active,
    NonTerminal,
    Unresolved,
    Unacknowledged,
    Inherited,
    RecoveryAuthoritative,
    UnknownAge,
    ClockAnomaly,
    WithinRetentionHorizon,
    SelectedHead,
    Hold,
    Corrupt,
    UnvalidatedPreparedManifest,
    UnvalidatedSealedManifest,
    UnclassifiedLegacyInput,
    MissingRowCount,
    MissingIngestionWatermark,
    MissingOccurrenceWatermark,
    LeasePresent,
    IncompleteImport,
    UnsupportedLegacyInput,
    IdentityMismatch,
    MissingSealedDigest,
    StaleCandidate,
    WriterBusy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionProjectionStatus {
    Pending,
    Blocked,
    Eligible,
    LegacyUnknown,
    ClockAnomaly,
    InheritsParent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordRetentionFacts {
    pub family: RetentionFamily,
    pub terminal: bool,
    pub projection_status: RetentionProjectionStatus,
    pub authoritative_at_unix_micros: Option<i64>,
    pub unresolved: bool,
    pub unacknowledged: bool,
    pub inherited: bool,
    pub recovery_authoritative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RetentionDecision {
    Eligible {
        authoritative_at_unix_micros: i64,
        cutoff_unix_micros: i64,
    },
    Preserve {
        reasons: Vec<PreservationReason>,
        cutoff_unix_micros: Option<i64>,
    },
}

impl RetentionDecision {
    pub fn eligible(&self) -> bool {
        matches!(self, Self::Eligible { .. })
    }

    pub fn preservation_reasons(&self) -> &[PreservationReason] {
        match self {
            Self::Eligible { .. } => &[],
            Self::Preserve { reasons, .. } => reasons,
        }
    }
}

pub fn evaluate_record_retention(
    policy: &RetentionPolicy,
    facts: &RecordRetentionFacts,
    as_of_unix_micros: i64,
) -> Result<RetentionDecision, RetentionPolicyError> {
    let cutoff = policy.cutoff_unix_micros(facts.family, as_of_unix_micros)?;
    let mut reasons = Vec::new();
    if !facts.terminal {
        reasons.push(PreservationReason::NonTerminal);
    }
    match facts.projection_status {
        RetentionProjectionStatus::Pending => reasons.push(PreservationReason::Active),
        RetentionProjectionStatus::Blocked => reasons.push(PreservationReason::Unresolved),
        RetentionProjectionStatus::LegacyUnknown => reasons.push(PreservationReason::UnknownAge),
        RetentionProjectionStatus::ClockAnomaly => reasons.push(PreservationReason::ClockAnomaly),
        RetentionProjectionStatus::InheritsParent => reasons.push(PreservationReason::Inherited),
        RetentionProjectionStatus::Eligible => {}
    }
    if facts.unresolved && !reasons.contains(&PreservationReason::Unresolved) {
        reasons.push(PreservationReason::Unresolved);
    }
    if facts.unacknowledged {
        reasons.push(PreservationReason::Unacknowledged);
    }
    if facts.inherited && !reasons.contains(&PreservationReason::Inherited) {
        reasons.push(PreservationReason::Inherited);
    }
    if facts.recovery_authoritative {
        reasons.push(PreservationReason::RecoveryAuthoritative);
    }
    let Some(authoritative_at) = facts.authoritative_at_unix_micros else {
        if !reasons.contains(&PreservationReason::UnknownAge) {
            reasons.push(PreservationReason::UnknownAge);
        }
        return Ok(RetentionDecision::Preserve {
            reasons,
            cutoff_unix_micros: Some(cutoff),
        });
    };
    if facts.projection_status != RetentionProjectionStatus::Eligible {
        return Ok(RetentionDecision::Preserve {
            reasons,
            cutoff_unix_micros: Some(cutoff),
        });
    }
    if authoritative_at > cutoff {
        reasons.push(PreservationReason::WithinRetentionHorizon);
    }
    if reasons.is_empty() {
        Ok(RetentionDecision::Eligible {
            authoritative_at_unix_micros: authoritative_at,
            cutoff_unix_micros: cutoff,
        })
    } else {
        Ok(RetentionDecision::Preserve {
            reasons,
            cutoff_unix_micros: Some(cutoff),
        })
    }
}

pub fn evaluate_event_generation_retention(
    policy: &RetentionPolicy,
    facts: &GenerationEligibilityFacts,
    as_of_unix_micros: i64,
) -> Result<RetentionDecision, RetentionPolicyError> {
    let cutoff = policy.event_generation_cutoff_unix_micros(as_of_unix_micros)?;
    let mut reasons = Vec::new();
    if facts.state != PartitionLifecycleState::Closed {
        reasons.push(if facts.state == PartitionLifecycleState::Writable {
            PreservationReason::Active
        } else {
            PreservationReason::NonTerminal
        });
    }
    if !facts.prepared_manifest_valid {
        reasons.push(PreservationReason::UnvalidatedPreparedManifest);
    }
    if !facts.sealed_manifest_valid {
        reasons.push(PreservationReason::UnvalidatedSealedManifest);
    }
    if facts.selected_head {
        reasons.push(PreservationReason::SelectedHead);
    }
    if facts.hold_present {
        reasons.push(PreservationReason::Hold);
    }
    if facts.corruption_present {
        reasons.push(PreservationReason::Corrupt);
    }
    if facts.unclassified_legacy_input {
        reasons.push(PreservationReason::UnclassifiedLegacyInput);
    }
    let Some(closed_at) = facts.closed_at_unix_micros else {
        reasons.push(PreservationReason::UnknownAge);
        return Ok(RetentionDecision::Preserve {
            reasons,
            cutoff_unix_micros: Some(cutoff),
        });
    };
    let reference = match (facts.row_count, facts.max_ingested_at_unix_micros) {
        (Some(0), None) => closed_at,
        (Some(_), Some(max_ingested)) => closed_at.max(max_ingested),
        (None, _) => {
            reasons.push(PreservationReason::MissingRowCount);
            closed_at
        }
        (Some(_), None) => {
            reasons.push(PreservationReason::MissingIngestionWatermark);
            closed_at
        }
    };
    if reference > cutoff {
        reasons.push(PreservationReason::WithinRetentionHorizon);
    }
    if reasons.is_empty() {
        Ok(RetentionDecision::Eligible {
            authoritative_at_unix_micros: reference,
            cutoff_unix_micros: cutoff,
        })
    } else {
        Ok(RetentionDecision::Preserve {
            reasons,
            cutoff_unix_micros: Some(cutoff),
        })
    }
}

/// Facts for a closed rotated history artifact outside the event-generation
/// directory protocol, principally preservation-mode JSONL awaiting or having
/// completed import. These facts grant no filesystem deletion authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotatedHistoryFacts {
    pub family: RetentionFamily,
    pub closed: bool,
    pub closed_at_unix_micros: Option<i64>,
    pub max_occurrence_at_unix_micros: Option<i64>,
    pub record_count: Option<u64>,
    pub legacy_import_required: bool,
    pub import_receipt_valid: bool,
    pub import_completed_at_unix_micros: Option<i64>,
    pub hold_present: bool,
    pub lease_present: bool,
    pub corruption_present: bool,
    pub unsupported_or_torn_input: bool,
    pub recovery_authoritative: bool,
}

pub fn evaluate_rotated_history_retention(
    policy: &RetentionPolicy,
    facts: &RotatedHistoryFacts,
    as_of_unix_micros: i64,
) -> Result<RetentionDecision, RetentionPolicyError> {
    if !RetentionFamily::EVENT_FAMILIES.contains(&facts.family) {
        return Err(RetentionPolicyError::InvalidRotatedHistoryFamily(
            facts.family,
        ));
    }
    let cutoff = policy.cutoff_unix_micros(facts.family, as_of_unix_micros)?;
    let mut reasons = Vec::new();
    if !facts.closed {
        reasons.push(PreservationReason::Active);
    }
    if facts.hold_present {
        reasons.push(PreservationReason::Hold);
    }
    if facts.lease_present {
        reasons.push(PreservationReason::LeasePresent);
    }
    if facts.corruption_present {
        reasons.push(PreservationReason::Corrupt);
    }
    if facts.unsupported_or_torn_input {
        reasons.push(PreservationReason::UnsupportedLegacyInput);
    }
    if facts.recovery_authoritative {
        reasons.push(PreservationReason::RecoveryAuthoritative);
    }
    if facts.legacy_import_required && !facts.import_receipt_valid {
        reasons.push(PreservationReason::IncompleteImport);
    }
    let Some(closed_at) = facts.closed_at_unix_micros else {
        reasons.push(PreservationReason::UnknownAge);
        return Ok(RetentionDecision::Preserve {
            reasons,
            cutoff_unix_micros: Some(cutoff),
        });
    };
    let mut reference = match (facts.record_count, facts.max_occurrence_at_unix_micros) {
        (Some(0), None) => closed_at,
        (Some(_), Some(max_occurrence)) => closed_at.max(max_occurrence),
        (None, _) => {
            reasons.push(PreservationReason::MissingRowCount);
            closed_at
        }
        (Some(_), None) => {
            reasons.push(PreservationReason::MissingOccurrenceWatermark);
            closed_at
        }
    };
    if facts.legacy_import_required {
        match facts.import_completed_at_unix_micros {
            Some(imported_at) if facts.import_receipt_valid => {
                reference = reference.max(imported_at);
            }
            _ => {
                if !reasons.contains(&PreservationReason::IncompleteImport) {
                    reasons.push(PreservationReason::IncompleteImport);
                }
            }
        }
    }
    if reference > cutoff {
        reasons.push(PreservationReason::WithinRetentionHorizon);
    }
    if reasons.is_empty() {
        Ok(RetentionDecision::Eligible {
            authoritative_at_unix_micros: reference,
            cutoff_unix_micros: cutoff,
        })
    } else {
        Ok(RetentionDecision::Preserve {
            reasons,
            cutoff_unix_micros: Some(cutoff),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRetirementApproval {
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub prepared_manifest_sha256: Digest32,
    pub sealed_manifest_sha256: Digest32,
    pub policy_version: String,
    pub authoritative_at_unix_micros: i64,
    pub cutoff_unix_micros: i64,
}

impl GenerationRetirementApproval {
    /// Build the existing AGE-376 create-once receipt shape. The caller still
    /// must acquire the exact maintenance lease, validate both head slots, and
    /// perform the AGE-377 move before publishing this receipt.
    pub fn retirement_receipt(&self, retirement_epoch: i64) -> RetirementReceipt {
        RetirementReceipt::new(
            self.writer_instance_id,
            self.generation_id,
            self.prepared_manifest_sha256,
            self.sealed_manifest_sha256,
            self.policy_version.clone(),
            self.cutoff_unix_micros,
            retirement_epoch,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "authorization", rename_all = "snake_case")]
pub enum GenerationRetirementAuthorization {
    Approved(GenerationRetirementApproval),
    Preserve(RetentionDecision),
}

pub fn authorize_event_generation_retirement(
    policy: &RetentionPolicy,
    facts: &GenerationEligibilityFacts,
    target: &GenerationMaintenanceTarget,
    as_of_unix_micros: i64,
) -> Result<GenerationRetirementAuthorization, RetentionPolicyError> {
    let decision = evaluate_event_generation_retention(policy, facts, as_of_unix_micros)?;
    let RetentionDecision::Eligible {
        authoritative_at_unix_micros,
        cutoff_unix_micros,
    } = decision
    else {
        return Ok(GenerationRetirementAuthorization::Preserve(decision));
    };
    let mut reasons = Vec::new();
    if target.writer_instance_id != facts.writer_instance_id
        || target.generation_id != facts.generation_id
        || target.state != PartitionLifecycleState::Closed
    {
        reasons.push(PreservationReason::IdentityMismatch);
    }
    let Some(sealed_manifest_sha256) = target.sealed_manifest_sha256 else {
        reasons.push(PreservationReason::MissingSealedDigest);
        return Ok(GenerationRetirementAuthorization::Preserve(
            RetentionDecision::Preserve {
                reasons,
                cutoff_unix_micros: Some(cutoff_unix_micros),
            },
        ));
    };
    if !reasons.is_empty() {
        return Ok(GenerationRetirementAuthorization::Preserve(
            RetentionDecision::Preserve {
                reasons,
                cutoff_unix_micros: Some(cutoff_unix_micros),
            },
        ));
    }
    Ok(GenerationRetirementAuthorization::Approved(
        GenerationRetirementApproval {
            writer_instance_id: facts.writer_instance_id,
            generation_id: facts.generation_id,
            prepared_manifest_sha256: target.prepared_manifest_sha256,
            sealed_manifest_sha256,
            policy_version: policy.version.clone(),
            authoritative_at_unix_micros,
            cutoff_unix_micros,
        },
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionBatchCursor {
    pub policy_version: String,
    pub family: RetentionFamily,
    pub cutoff_unix_micros: i64,
    pub after_authoritative_at: String,
    pub after_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionBatchRequest {
    pub policy: RetentionPolicy,
    pub family: RetentionFamily,
    pub as_of_unix_micros: i64,
    pub limit: usize,
    pub cursor: Option<RetentionBatchCursor>,
}

impl RetentionBatchRequest {
    pub fn validate(&self) -> Result<i64, String> {
        if self.limit == 0 {
            return Err("retention batch limit must be positive".to_string());
        }
        let cutoff = self
            .policy
            .cutoff_unix_micros(self.family, self.as_of_unix_micros)
            .map_err(|error| error.to_string())?;
        if let Some(cursor) = &self.cursor
            && (cursor.policy_version != self.policy.version
                || cursor.family != self.family
                || cursor.cutoff_unix_micros != cutoff)
        {
            return Err("retention cursor does not match the policy/family/cutoff snapshot".into());
        }
        Ok(cutoff)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionBatchStatus {
    Complete,
    MoreWork,
    Busy,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionGap {
    pub stage: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionObservationDelivery {
    Appended,
    Queued,
    Fallback,
    Gap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionBatchOutcome {
    pub policy_version: String,
    pub family: RetentionFamily,
    pub cutoff_unix_micros: i64,
    pub status: RetentionBatchStatus,
    pub candidates_examined: usize,
    pub records_deleted: usize,
    pub artifacts_retired: usize,
    pub records_preserved: usize,
    pub preservation_reasons: BTreeMap<PreservationReason, usize>,
    pub next_cursor: Option<RetentionBatchCursor>,
    pub gaps: Vec<RetentionGap>,
    pub observation_delivery: Option<RetentionObservationDelivery>,
}

impl RetentionBatchOutcome {
    pub fn new(request: &RetentionBatchRequest, cutoff_unix_micros: i64) -> Self {
        Self {
            policy_version: request.policy.version.clone(),
            family: request.family,
            cutoff_unix_micros,
            status: RetentionBatchStatus::Complete,
            candidates_examined: 0,
            records_deleted: 0,
            artifacts_retired: 0,
            records_preserved: 0,
            preservation_reasons: BTreeMap::new(),
            next_cursor: request.cursor.clone(),
            gaps: Vec::new(),
            observation_delivery: None,
        }
    }

    pub fn preserve(&mut self, reason: PreservationReason) {
        self.records_preserved += 1;
        *self.preservation_reasons.entry(reason).or_default() += 1;
    }

    pub fn gap(&mut self, stage: impl Into<String>, reason: impl Into<String>) {
        self.gaps.push(RetentionGap {
            stage: stage.into(),
            reason: reason.into(),
        });
        if self.status != RetentionBatchStatus::Busy {
            self.status = RetentionBatchStatus::Partial;
        }
    }

    /// Emit after every live transaction is closed.  Failure is reflected in
    /// this returned contract and never changes the deletion decision.
    pub fn emit_independent_observation(&mut self) {
        let phase = match self.status {
            RetentionBatchStatus::Busy => DiagnosticPhase::Contention,
            RetentionBatchStatus::Partial => DiagnosticPhase::Failed,
            RetentionBatchStatus::Complete | RetentionBatchStatus::MoreWork => {
                DiagnosticPhase::Released
            }
        };
        let mut observation = if self.records_deleted > 0 {
            PhaseObservation::committed()
        } else {
            PhaseObservation::terminal()
        };
        observation = observation
            .with_cause(format!("status_{:?}", self.status).to_ascii_lowercase())
            .with_cause(format!("candidates_{}", self.candidates_examined))
            .with_cause(format!("deleted_{}", self.records_deleted))
            .with_cause(format!("artifacts_{}", self.artifacts_retired))
            .with_cause(format!("preserved_{}", self.records_preserved))
            .with_cause(format!("gaps_{}", self.gaps.len()));
        let start = SpanStart::new("retention_batch", "historical_maintenance")
            .with_lifecycle_phase(self.family.as_str())
            .with_identifier("policy_version", &self.policy_version);
        let status =
            process_recorder().with_requested_span(start, |span| span.record(phase, observation));
        self.observation_delivery = Some(match status {
            RecordStatus::Appended => RetentionObservationDelivery::Appended,
            RecordStatus::Queued => RetentionObservationDelivery::Queued,
            RecordStatus::FallbackAppended { .. } | RecordStatus::FallbackQueued { .. } => {
                RetentionObservationDelivery::Fallback
            }
            RecordStatus::Disabled | RecordStatus::Failed { .. } => {
                self.gaps.push(RetentionGap {
                    stage: "retention_observation".to_string(),
                    reason: "independent_observation_not_durable".to_string(),
                });
                RetentionObservationDelivery::Gap
            }
        });
    }
}

pub fn parse_authoritative_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp_micros())
        .filter(|value| *value >= 0)
}

pub fn format_timestamp_micros(value: i64) -> Result<String, String> {
    DateTime::<Utc>::from_timestamp_micros(value)
        .ok_or_else(|| "retention timestamp is outside the supported UTC range".to_string())
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_recorder::{FlightRecorder, RecorderConfig, with_test_process_recorder};
    use crate::event_store::{GenerationId, WriterInstanceId};
    use std::path::PathBuf;

    const NOW: i64 = 4_000_000_000_000_000;

    fn eligible_record(at: Option<i64>) -> RecordRetentionFacts {
        RecordRetentionFacts {
            family: RetentionFamily::Invocation,
            terminal: true,
            projection_status: RetentionProjectionStatus::Eligible,
            authoritative_at_unix_micros: at,
            unresolved: false,
            unacknowledged: false,
            inherited: false,
            recovery_authoritative: false,
        }
    }

    #[test]
    fn thirty_day_boundary_is_inclusive() {
        let policy = RetentionPolicy::default();
        assert_eq!(policy.families.len(), RetentionFamily::ALL.len());
        assert!(policy.families.iter().all(|rule| {
            rule.horizon_micros == DEFAULT_TERMINAL_RETENTION_MICROS
                && rule.boundary == RetentionBoundary::Inclusive
        }));
        let cutoff = NOW - DEFAULT_TERMINAL_RETENTION_MICROS;
        assert!(
            evaluate_record_retention(&policy, &eligible_record(Some(cutoff - 1)), NOW)
                .unwrap()
                .eligible()
        );
        assert!(
            evaluate_record_retention(&policy, &eligible_record(Some(cutoff)), NOW)
                .unwrap()
                .eligible()
        );
        assert_eq!(
            evaluate_record_retention(&policy, &eligible_record(Some(cutoff + 1)), NOW)
                .unwrap()
                .preservation_reasons(),
            &[PreservationReason::WithinRetentionHorizon]
        );
    }

    #[test]
    fn every_live_or_unknown_authority_category_fails_closed() {
        let policy = RetentionPolicy::default();
        let cutoff = NOW - DEFAULT_TERMINAL_RETENTION_MICROS - 1;
        let cases = [
            (
                RecordRetentionFacts {
                    terminal: false,
                    ..eligible_record(Some(cutoff))
                },
                PreservationReason::NonTerminal,
            ),
            (
                RecordRetentionFacts {
                    projection_status: RetentionProjectionStatus::Pending,
                    ..eligible_record(Some(cutoff))
                },
                PreservationReason::Active,
            ),
            (
                RecordRetentionFacts {
                    unresolved: true,
                    ..eligible_record(Some(cutoff))
                },
                PreservationReason::Unresolved,
            ),
            (
                RecordRetentionFacts {
                    unacknowledged: true,
                    ..eligible_record(Some(cutoff))
                },
                PreservationReason::Unacknowledged,
            ),
            (
                RecordRetentionFacts {
                    inherited: true,
                    projection_status: RetentionProjectionStatus::InheritsParent,
                    ..eligible_record(Some(cutoff))
                },
                PreservationReason::Inherited,
            ),
            (
                RecordRetentionFacts {
                    recovery_authoritative: true,
                    ..eligible_record(Some(cutoff))
                },
                PreservationReason::RecoveryAuthoritative,
            ),
            (
                RecordRetentionFacts {
                    projection_status: RetentionProjectionStatus::LegacyUnknown,
                    authoritative_at_unix_micros: None,
                    ..eligible_record(None)
                },
                PreservationReason::UnknownAge,
            ),
            (
                RecordRetentionFacts {
                    projection_status: RetentionProjectionStatus::ClockAnomaly,
                    ..eligible_record(Some(cutoff))
                },
                PreservationReason::ClockAnomaly,
            ),
            (
                eligible_record(Some(NOW - DEFAULT_TERMINAL_RETENTION_MICROS + 1)),
                PreservationReason::WithinRetentionHorizon,
            ),
        ];
        for (facts, reason) in cases {
            let decision = evaluate_record_retention(&policy, &facts, NOW).unwrap();
            assert!(decision.preservation_reasons().contains(&reason));
        }
    }

    #[test]
    fn generation_policy_requires_every_age_and_seal_fact() {
        let policy = RetentionPolicy::default();
        let cutoff = NOW - DEFAULT_TERMINAL_RETENTION_MICROS;
        let mut facts = GenerationEligibilityFacts {
            writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
            generation_id: GenerationId::from_bytes([2; 16]),
            state: PartitionLifecycleState::Closed,
            prepared_manifest_valid: true,
            sealed_manifest_valid: true,
            selected_head: false,
            hold_present: false,
            corruption_present: false,
            unclassified_legacy_input: false,
            closed_at_unix_micros: Some(cutoff - 2),
            max_ingested_at_unix_micros: Some(cutoff),
            row_count: Some(1),
        };
        assert!(
            evaluate_event_generation_retention(&policy, &facts, NOW)
                .unwrap()
                .eligible()
        );
        facts.selected_head = true;
        assert_eq!(
            evaluate_event_generation_retention(&policy, &facts, NOW)
                .unwrap()
                .preservation_reasons(),
            &[PreservationReason::SelectedHead]
        );
        facts.selected_head = false;
        facts.max_ingested_at_unix_micros = None;
        assert!(
            evaluate_event_generation_retention(&policy, &facts, NOW)
                .unwrap()
                .preservation_reasons()
                .contains(&PreservationReason::MissingIngestionWatermark)
        );

        let base = GenerationEligibilityFacts {
            writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
            generation_id: GenerationId::from_bytes([2; 16]),
            state: PartitionLifecycleState::Closed,
            prepared_manifest_valid: true,
            sealed_manifest_valid: true,
            selected_head: false,
            hold_present: false,
            corruption_present: false,
            unclassified_legacy_input: false,
            closed_at_unix_micros: Some(cutoff),
            max_ingested_at_unix_micros: None,
            row_count: Some(0),
        };
        let cases = [
            (
                GenerationEligibilityFacts {
                    state: PartitionLifecycleState::Writable,
                    ..base.clone()
                },
                PreservationReason::Active,
            ),
            (
                GenerationEligibilityFacts {
                    prepared_manifest_valid: false,
                    ..base.clone()
                },
                PreservationReason::UnvalidatedPreparedManifest,
            ),
            (
                GenerationEligibilityFacts {
                    sealed_manifest_valid: false,
                    ..base.clone()
                },
                PreservationReason::UnvalidatedSealedManifest,
            ),
            (
                GenerationEligibilityFacts {
                    hold_present: true,
                    ..base.clone()
                },
                PreservationReason::Hold,
            ),
            (
                GenerationEligibilityFacts {
                    corruption_present: true,
                    ..base.clone()
                },
                PreservationReason::Corrupt,
            ),
            (
                GenerationEligibilityFacts {
                    unclassified_legacy_input: true,
                    ..base.clone()
                },
                PreservationReason::UnclassifiedLegacyInput,
            ),
            (
                GenerationEligibilityFacts {
                    closed_at_unix_micros: None,
                    ..base
                },
                PreservationReason::UnknownAge,
            ),
        ];
        for (facts, reason) in cases {
            assert!(
                evaluate_event_generation_retention(&policy, &facts, NOW)
                    .unwrap()
                    .preservation_reasons()
                    .contains(&reason)
            );
        }
    }

    #[test]
    fn generation_authorization_binds_exact_target_and_existing_receipt_contract() {
        let policy = RetentionPolicy::default();
        let cutoff = NOW - DEFAULT_TERMINAL_RETENTION_MICROS;
        let facts = GenerationEligibilityFacts {
            writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
            generation_id: GenerationId::from_bytes([2; 16]),
            state: PartitionLifecycleState::Closed,
            prepared_manifest_valid: true,
            sealed_manifest_valid: true,
            selected_head: false,
            hold_present: false,
            corruption_present: false,
            unclassified_legacy_input: false,
            closed_at_unix_micros: Some(cutoff),
            max_ingested_at_unix_micros: None,
            row_count: Some(0),
        };
        let mut target = GenerationMaintenanceTarget {
            writer_instance_id: facts.writer_instance_id,
            generation_id: facts.generation_id,
            generation_directory: PathBuf::from("generation"),
            lease_path: PathBuf::from("lease"),
            database_relative_name: crate::event_store::DATABASE_FILE_NAME.to_string(),
            prepared_manifest_sha256: Digest32::from_bytes([3; 32]),
            sealed_manifest_sha256: Some(Digest32::from_bytes([4; 32])),
            database_file_sha256: Some(Digest32::from_bytes([5; 32])),
            state: PartitionLifecycleState::Closed,
        };
        let approval =
            match authorize_event_generation_retirement(&policy, &facts, &target, NOW).unwrap() {
                GenerationRetirementAuthorization::Approved(approval) => approval,
                other => panic!("unexpected authorization: {other:?}"),
            };
        let receipt = approval.retirement_receipt(7);
        receipt.validate().unwrap();
        assert_eq!(receipt.generation_id, facts.generation_id);
        assert_eq!(receipt.retention_cutoff_unix_micros, cutoff);

        target.generation_id = GenerationId::from_bytes([9; 16]);
        let decision =
            authorize_event_generation_retirement(&policy, &facts, &target, NOW).unwrap();
        assert!(matches!(
            decision,
            GenerationRetirementAuthorization::Preserve(RetentionDecision::Preserve {
                reasons,
                ..
            }) if reasons.contains(&PreservationReason::IdentityMismatch)
        ));
    }

    #[test]
    fn rotated_legacy_history_requires_closed_imported_exact_age_evidence() {
        let policy = RetentionPolicy::default();
        let cutoff = NOW - DEFAULT_TERMINAL_RETENTION_MICROS;
        let eligible = RotatedHistoryFacts {
            family: RetentionFamily::DiagnosticEvent,
            closed: true,
            closed_at_unix_micros: Some(cutoff - 2),
            max_occurrence_at_unix_micros: Some(cutoff - 1),
            record_count: Some(1),
            legacy_import_required: true,
            import_receipt_valid: true,
            import_completed_at_unix_micros: Some(cutoff),
            hold_present: false,
            lease_present: false,
            corruption_present: false,
            unsupported_or_torn_input: false,
            recovery_authoritative: false,
        };
        assert!(
            evaluate_rotated_history_retention(&policy, &eligible, NOW)
                .unwrap()
                .eligible()
        );

        let cases = [
            (
                RotatedHistoryFacts {
                    closed: false,
                    ..eligible.clone()
                },
                PreservationReason::Active,
            ),
            (
                RotatedHistoryFacts {
                    closed_at_unix_micros: None,
                    ..eligible.clone()
                },
                PreservationReason::UnknownAge,
            ),
            (
                RotatedHistoryFacts {
                    import_receipt_valid: false,
                    ..eligible.clone()
                },
                PreservationReason::IncompleteImport,
            ),
            (
                RotatedHistoryFacts {
                    lease_present: true,
                    ..eligible.clone()
                },
                PreservationReason::LeasePresent,
            ),
            (
                RotatedHistoryFacts {
                    unsupported_or_torn_input: true,
                    ..eligible.clone()
                },
                PreservationReason::UnsupportedLegacyInput,
            ),
            (
                RotatedHistoryFacts {
                    import_completed_at_unix_micros: Some(cutoff + 1),
                    ..eligible
                },
                PreservationReason::WithinRetentionHorizon,
            ),
        ];
        for (facts, reason) in cases {
            assert!(
                evaluate_rotated_history_retention(&policy, &facts, NOW)
                    .unwrap()
                    .preservation_reasons()
                    .contains(&reason)
            );
        }
    }

    #[test]
    fn selected_or_unvalidated_generation_never_receives_approval() {
        let policy = RetentionPolicy::default();
        let cutoff = NOW - DEFAULT_TERMINAL_RETENTION_MICROS;
        let mut facts = GenerationEligibilityFacts {
            writer_instance_id: WriterInstanceId::from_bytes([1; 16]),
            generation_id: GenerationId::from_bytes([2; 16]),
            state: PartitionLifecycleState::Closed,
            prepared_manifest_valid: true,
            sealed_manifest_valid: true,
            selected_head: true,
            hold_present: false,
            corruption_present: false,
            unclassified_legacy_input: false,
            closed_at_unix_micros: Some(cutoff),
            max_ingested_at_unix_micros: None,
            row_count: Some(0),
        };
        let target = GenerationMaintenanceTarget {
            writer_instance_id: facts.writer_instance_id,
            generation_id: facts.generation_id,
            generation_directory: PathBuf::from("generation"),
            lease_path: PathBuf::from("lease"),
            database_relative_name: crate::event_store::DATABASE_FILE_NAME.to_string(),
            prepared_manifest_sha256: Digest32::from_bytes([3; 32]),
            sealed_manifest_sha256: Some(Digest32::from_bytes([4; 32])),
            database_file_sha256: Some(Digest32::from_bytes([5; 32])),
            state: PartitionLifecycleState::Closed,
        };
        assert!(matches!(
            authorize_event_generation_retirement(&policy, &facts, &target, NOW).unwrap(),
            GenerationRetirementAuthorization::Preserve(_)
        ));
        facts.selected_head = false;
        facts.sealed_manifest_valid = false;
        assert!(matches!(
            authorize_event_generation_retirement(&policy, &facts, &target, NOW).unwrap(),
            GenerationRetirementAuthorization::Preserve(_)
        ));
    }

    #[test]
    fn cursor_must_match_an_exact_policy_snapshot() {
        let policy = RetentionPolicy::default();
        let cutoff = policy
            .cutoff_unix_micros(RetentionFamily::CompletedTurn, NOW)
            .unwrap();
        let mut request = RetentionBatchRequest {
            policy: policy.clone(),
            family: RetentionFamily::CompletedTurn,
            as_of_unix_micros: NOW,
            limit: 1,
            cursor: Some(RetentionBatchCursor {
                policy_version: policy.version.clone(),
                family: RetentionFamily::CompletedTurn,
                cutoff_unix_micros: cutoff,
                after_authoritative_at: "2020-01-01T00:00:00Z".to_string(),
                after_key: "one".to_string(),
            }),
        };
        assert_eq!(request.validate().unwrap(), cutoff);
        request.as_of_unix_micros += 1;
        assert!(request.validate().is_err());
    }

    #[test]
    fn independent_observation_success_and_gap_are_part_of_the_outcome() {
        let policy = RetentionPolicy::default();
        let request = RetentionBatchRequest {
            policy: policy.clone(),
            family: RetentionFamily::CompletedTurn,
            as_of_unix_micros: NOW,
            limit: 1,
            cursor: None,
        };
        let cutoff = request.validate().unwrap();
        let mut partial = RetentionBatchOutcome::new(&request, cutoff);
        partial.gap("candidate_delete", "injected_failure");
        assert_eq!(partial.status, RetentionBatchStatus::Partial);
        assert_eq!(partial.gaps.len(), 1);

        let directory = tempfile::tempdir().unwrap();
        let recorder = FlightRecorder::open(directory.path(), RecorderConfig::default()).unwrap();
        let mut observed = RetentionBatchOutcome::new(&request, cutoff);
        with_test_process_recorder(recorder.clone(), || {
            observed.emit_independent_observation();
        });
        assert!(matches!(
            observed.observation_delivery,
            Some(RetentionObservationDelivery::Queued | RetentionObservationDelivery::Appended)
        ));
        assert!(observed.gaps.is_empty());
        recorder.drain_deferred_for_test().unwrap();

        let mut gap = RetentionBatchOutcome::new(&request, cutoff);
        gap.emit_independent_observation();
        assert_eq!(
            gap.observation_delivery,
            Some(RetentionObservationDelivery::Gap)
        );
        assert!(gap.gaps.iter().any(|item| {
            item.stage == "retention_observation"
                && item.reason == "independent_observation_not_durable"
        }));
    }
}
