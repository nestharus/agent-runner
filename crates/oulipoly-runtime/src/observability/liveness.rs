//! ## Declared roles
//!
//! `accessor`, `mapper`, `orchestration`, `parser`, `predicate`, `validator`
//!
//! Read-only process and runtime liveness classification.

use crate::observability::dto::{LivenessStatus, MonitorDiagnostic, MonitorDiagnosticSeverity};
use chrono::{DateTime, Duration, Utc};
use oulipoly_state::mailbox::{SessionRuntimeReadOnlyLiveness, WakeClaimRow};
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRow, ProcessIdentity, ProcessIdentityObservation,
};

pub(crate) fn pid_row_liveness(row: &PidIdentityRow) -> LivenessStatus {
    process_identity_liveness(&row.identity())
}

pub(crate) fn process_identity_liveness(identity: &ProcessIdentity) -> LivenessStatus {
    live_process_identity_liveness(identity, read_process_identity(identity.os_pid))
}

fn read_process_identity(pid: i64) -> ProcessIdentityObservation {
    oulipoly_state::pid_identity::observe_live_process_identity(pid)
}

fn live_process_identity_liveness(
    identity: &ProcessIdentity,
    live: ProcessIdentityObservation,
) -> LivenessStatus {
    match live {
        ProcessIdentityObservation::ExactLive(live) if live == *identity => {
            LivenessStatus::VerifiedLive
        }
        ProcessIdentityObservation::ExactLive(_) => LivenessStatus::PidReused,
        ProcessIdentityObservation::Dead => LivenessStatus::Dead,
        ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => {
            LivenessStatus::Unknown
        }
    }
}

pub(crate) fn unverified_pid_liveness(pid: Option<i64>) -> LivenessStatus {
    let Some(pid) = pid else {
        return LivenessStatus::NotApplicable;
    };
    unverified_process_identity_liveness(read_process_identity(pid))
}

fn unverified_process_identity_liveness(live: ProcessIdentityObservation) -> LivenessStatus {
    match live {
        ProcessIdentityObservation::ExactLive(_) => LivenessStatus::UnverifiedLive,
        ProcessIdentityObservation::Dead => LivenessStatus::Dead,
        ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => {
            LivenessStatus::Unknown
        }
    }
}

pub(crate) fn runtime_liveness_status(liveness: SessionRuntimeReadOnlyLiveness) -> LivenessStatus {
    match liveness {
        SessionRuntimeReadOnlyLiveness::Busy => LivenessStatus::VerifiedLive,
        SessionRuntimeReadOnlyLiveness::Idle => LivenessStatus::NotApplicable,
        SessionRuntimeReadOnlyLiveness::StaleDead => LivenessStatus::Dead,
        SessionRuntimeReadOnlyLiveness::StalePidReused => LivenessStatus::PidReused,
        SessionRuntimeReadOnlyLiveness::StaleMissingInvocation
        | SessionRuntimeReadOnlyLiveness::StaleMissingIdentity => LivenessStatus::Unknown,
    }
}

pub(crate) fn runtime_is_stale(liveness: SessionRuntimeReadOnlyLiveness) -> bool {
    matches!(
        liveness,
        SessionRuntimeReadOnlyLiveness::StaleMissingInvocation
            | SessionRuntimeReadOnlyLiveness::StaleMissingIdentity
            | SessionRuntimeReadOnlyLiveness::StaleDead
            | SessionRuntimeReadOnlyLiveness::StalePidReused
    )
}

pub(crate) fn wake_claim_liveness(
    claim: &WakeClaimRow,
    pid_db: Option<&PidIdentityDb>,
) -> LivenessStatus {
    wake_claim_liveness_from_evidence(claim, pid_db, read_wake_claim_live_identity(claim))
}

fn read_wake_claim_live_identity(claim: &WakeClaimRow) -> Option<ProcessIdentityObservation> {
    let wake_pid = wake_claim_pid(claim)?;
    Some(read_wake_live_identity(wake_pid))
}

fn wake_claim_liveness_from_evidence(
    claim: &WakeClaimRow,
    pid_db: Option<&PidIdentityDb>,
    live: Option<ProcessIdentityObservation>,
) -> LivenessStatus {
    let Some(live) = live else {
        return LivenessStatus::NotApplicable;
    };
    let live = match live {
        ProcessIdentityObservation::ExactLive(live) => live,
        ProcessIdentityObservation::Dead => return LivenessStatus::Dead,
        ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => {
            return LivenessStatus::Unknown;
        }
    };
    wake_claim_live_status(wake_live_identity_matches_claim(claim, pid_db, &live))
}

fn wake_claim_pid(claim: &WakeClaimRow) -> Option<i64> {
    claim.wake_pid
}

fn read_wake_live_identity(pid: i64) -> ProcessIdentityObservation {
    oulipoly_state::pid_identity::observe_live_process_identity(pid)
}

fn wake_claim_live_status(identity_matches: bool) -> LivenessStatus {
    if identity_matches {
        LivenessStatus::VerifiedLive
    } else {
        LivenessStatus::PidReused
    }
}

pub(crate) fn wake_claim_no_pid_is_stale(claim: &WakeClaimRow, stale_after_seconds: i64) -> bool {
    if wake_claim_has_pid(claim) {
        return false;
    }
    let Some(claimed_at) = wake_claimed_at(claim) else {
        return true;
    };
    wake_claim_age_exceeds(claimed_at, stale_after_seconds)
}

fn wake_claim_has_pid(claim: &WakeClaimRow) -> bool {
    claim.wake_pid.is_some()
}

fn wake_claimed_at(claim: &WakeClaimRow) -> Option<DateTime<Utc>> {
    parse_rfc3339_utc(&claim.claimed_at)
}

fn wake_claim_age_exceeds(claimed_at: DateTime<Utc>, stale_after_seconds: i64) -> bool {
    Utc::now().signed_duration_since(claimed_at) > Duration::seconds(stale_after_seconds)
}

pub(crate) fn stale_runtime_diagnostic(node_id: Option<String>) -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: "stale-runtime".to_string(),
        severity: MonitorDiagnosticSeverity::Warning,
        message: "session_runtime is marked running but its recorded PID identity is not live"
            .to_string(),
        node_id,
    }
}

fn wake_live_identity_matches_claim(
    claim: &WakeClaimRow,
    pid_db: Option<&PidIdentityDb>,
    live: &ProcessIdentity,
) -> bool {
    let Some(pid_db) = pid_db else {
        return false;
    };
    wake_live_identity_row(pid_db, live).is_some_and(|row| wake_pid_row_matches_claim(claim, &row))
}

fn wake_live_identity_row(
    pid_db: &PidIdentityDb,
    live: &ProcessIdentity,
) -> Option<PidIdentityRow> {
    pid_db.lookup_by_identity(live).ok().flatten()
}

fn wake_pid_row_matches_claim(claim: &WakeClaimRow, row: &PidIdentityRow) -> bool {
    row.invocation_uuid == claim.claim_token && row.session_id.as_deref() == Some(&claim.session_id)
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    parse_rfc3339(value).map(utc_datetime)
}

fn parse_rfc3339(value: &str) -> Option<DateTime<chrono::FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

fn utc_datetime(value: DateTime<chrono::FixedOffset>) -> DateTime<Utc> {
    value.with_timezone(&Utc)
}
