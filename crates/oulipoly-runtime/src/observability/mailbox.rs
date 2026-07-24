//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`
//!
//! Read-only session runtime, mailbox, and wake-claim projection.

use crate::observability::dto::{
    CancelRef, InspectRef, LivenessStatus, MailboxState, MonitorDiagnostic,
    MonitorDiagnosticSeverity, MonitorNode, MonitorNodeKind, MonitorStatus, WakeState,
};
use crate::observability::invocation::{invocation_node_id, session_node_id};
use crate::observability::limits::SnapshotLimits;
use crate::observability::liveness::{
    runtime_is_stale, runtime_liveness_status, stale_runtime_diagnostic, wake_claim_liveness,
    wake_claim_no_pid_is_stale,
};
use crate::observability::service::ObservabilityRoot;
use crate::observability::state_access::storage_diagnostic;
use oulipoly_state::mailbox::{
    MailboxDb, MailboxRow, SessionRuntimeReadOnlyLiveness, SessionRuntimeRow,
    WAKE_SWEEP_ABANDONED_ERROR, WakeClaimRow,
};
use oulipoly_state::pid_identity::PidIdentityDb;
use std::collections::HashSet;

pub(crate) struct MailboxProjection {
    pub(crate) nodes: Vec<MonitorNode>,
    pub(crate) diagnostics: Vec<MonitorDiagnostic>,
    pub(crate) rows: Vec<MailboxRow>,
    pub(crate) pending_count: usize,
}

pub(crate) fn project_mailbox(
    mailbox: Option<&MailboxDb>,
    pid: Option<&PidIdentityDb>,
    root: &ObservabilityRoot,
    session_id: Option<&str>,
    root_invocation_uuid: Option<&str>,
    invocation_uuids: &HashSet<String>,
    limits: SnapshotLimits,
) -> MailboxProjection {
    let mut projection = empty_mailbox_projection();
    let Some(session_id) = session_id else {
        return projection;
    };
    let runtime = runtime_row(mailbox, session_id, &mut projection.diagnostics);
    let runtime_liveness = runtime_liveness(mailbox, session_id, &mut projection.diagnostics);
    push_session_node(
        &mut projection,
        session_id,
        root,
        root_invocation_uuid,
        runtime.as_ref(),
        runtime_liveness,
    );
    if runtime_liveness_is_stale(runtime_liveness) {
        push_stale_runtime_diagnostic(&mut projection, session_id);
    }
    projection.rows = mailbox_rows(mailbox, session_id, limits, &mut projection.diagnostics);
    projection.pending_count = pending_count(&projection.rows);
    let claim = wake_claim(mailbox, session_id, &mut projection.diagnostics);
    projection.nodes.extend(mailbox_nodes(
        &projection.rows,
        session_id,
        invocation_uuids,
    ));
    if let Some(claim) = claim.as_ref() {
        push_wake_claim_node(
            &mut projection,
            claim,
            pid,
            limits.wake_claim_stale_after_seconds,
        );
    }
    projection.diagnostics.extend(wake_diagnostics(
        session_id,
        projection.pending_count,
        runtime.as_ref(),
        runtime_liveness,
        claim.as_ref(),
        pid,
        limits,
    ));
    projection
}

fn empty_mailbox_projection() -> MailboxProjection {
    MailboxProjection {
        nodes: Vec::new(),
        diagnostics: Vec::new(),
        rows: Vec::new(),
        pending_count: 0,
    }
}

fn push_session_node(
    projection: &mut MailboxProjection,
    session_id: &str,
    root: &ObservabilityRoot,
    root_invocation_uuid: Option<&str>,
    runtime: Option<&SessionRuntimeRow>,
    runtime_liveness: Option<SessionRuntimeReadOnlyLiveness>,
) {
    projection.nodes.push(session_node(
        session_id,
        root,
        root_invocation_uuid,
        runtime,
        runtime_liveness,
    ));
}

fn runtime_liveness_is_stale(liveness: Option<SessionRuntimeReadOnlyLiveness>) -> bool {
    liveness.is_some_and(runtime_is_stale)
}

fn push_stale_runtime_diagnostic(projection: &mut MailboxProjection, session_id: &str) {
    projection
        .diagnostics
        .push(stale_runtime_diagnostic(Some(session_node_id(session_id))));
}

fn push_wake_claim_node(
    projection: &mut MailboxProjection,
    claim: &WakeClaimRow,
    pid: Option<&PidIdentityDb>,
    stale_after_seconds: i64,
) {
    let liveness = wake_claim_liveness(claim, pid);
    projection
        .nodes
        .push(wake_claim_node(claim, liveness, stale_after_seconds));
}

fn runtime_row(
    mailbox: Option<&MailboxDb>,
    session_id: &str,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<SessionRuntimeRow> {
    let mailbox = mailbox?;
    match read_runtime_row(mailbox, session_id) {
        Ok(row) => row,
        Err(err) => {
            diagnostics.push(runtime_read_diagnostic(err));
            None
        }
    }
}

fn read_runtime_row(
    mailbox: &MailboxDb,
    session_id: &str,
) -> Result<Option<SessionRuntimeRow>, String> {
    mailbox.session_runtime(session_id)
}

fn runtime_read_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("mailbox:runtime-read", message)
}

fn runtime_liveness(
    mailbox: Option<&MailboxDb>,
    session_id: &str,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<SessionRuntimeReadOnlyLiveness> {
    let mailbox = mailbox?;
    match read_runtime_liveness(mailbox, session_id) {
        Ok(liveness) => Some(liveness),
        Err(err) => {
            diagnostics.push(runtime_liveness_read_diagnostic(err));
            None
        }
    }
}

fn read_runtime_liveness(
    mailbox: &MailboxDb,
    session_id: &str,
) -> Result<SessionRuntimeReadOnlyLiveness, String> {
    mailbox.classify_session_runtime_read_only(session_id)
}

fn runtime_liveness_read_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("mailbox:runtime-liveness-read-only", message)
}

fn mailbox_rows(
    mailbox: Option<&MailboxDb>,
    session_id: &str,
    limits: SnapshotLimits,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Vec<MailboxRow> {
    let Some(mailbox) = mailbox else {
        return Vec::new();
    };
    match read_bounded_mailbox_rows(mailbox, session_id, limits.mailbox_cap) {
        Ok(rows) => rows,
        Err(err) => {
            diagnostics.push(bounded_mailbox_read_diagnostic(err));
            Vec::new()
        }
    }
}

fn read_bounded_mailbox_rows(
    mailbox: &MailboxDb,
    session_id: &str,
    cap: usize,
) -> Result<Vec<MailboxRow>, String> {
    mailbox.list_mailbox_bounded(session_id, cap)
}

fn bounded_mailbox_read_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("mailbox:bounded-read", message)
}

fn wake_claim(
    mailbox: Option<&MailboxDb>,
    session_id: &str,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<WakeClaimRow> {
    let mailbox = mailbox?;
    match read_wake_claim(mailbox, session_id) {
        Ok(claim) => claim,
        Err(err) => {
            diagnostics.push(wake_claim_read_diagnostic(err));
            None
        }
    }
}

fn read_wake_claim(mailbox: &MailboxDb, session_id: &str) -> Result<Option<WakeClaimRow>, String> {
    mailbox.wake_claim(session_id)
}

fn wake_claim_read_diagnostic(message: String) -> MonitorDiagnostic {
    storage_diagnostic("mailbox:wake-claim-read", message)
}

fn session_node(
    session_id: &str,
    root: &ObservabilityRoot,
    root_invocation_uuid: Option<&str>,
    runtime: Option<&SessionRuntimeRow>,
    runtime_liveness: Option<SessionRuntimeReadOnlyLiveness>,
) -> MonitorNode {
    MonitorNode {
        id: session_node_id(session_id),
        parent_id: None,
        kind: MonitorNodeKind::Session,
        label: session_label(root, runtime),
        status: session_status(runtime, runtime_liveness),
        pid: runtime.and_then(|row| row.running_os_pid),
        pgid: None,
        liveness: runtime_liveness
            .map(runtime_liveness_status)
            .unwrap_or(LivenessStatus::Unknown),
        started_at: runtime.and_then(|row| row.turn_started_at.clone()),
        updated_at: runtime.map(|row| row.updated_at.clone()),
        completed_at: runtime.and_then(|row| row.turn_ended_at.clone()),
        last_output_excerpt: None,
        inspect_ref: None,
        cancel_ref: root_invocation_uuid.map(|uuid| CancelRef::ProviderSessionEnd {
            invocation_uuid: uuid.to_string(),
        }),
        wake: None,
        mailbox: None,
    }
}

fn mailbox_nodes(
    rows: &[MailboxRow],
    session_id: &str,
    invocation_uuids: &HashSet<String>,
) -> Vec<MonitorNode> {
    rows.iter()
        .map(|row| mailbox_node(row, session_id, invocation_uuids))
        .collect()
}

fn mailbox_node(
    row: &MailboxRow,
    session_id: &str,
    invocation_uuids: &HashSet<String>,
) -> MonitorNode {
    MonitorNode {
        id: mailbox_node_id(session_id, row.seq),
        parent_id: mailbox_parent_id(row, session_id, invocation_uuids),
        kind: MonitorNodeKind::MailboxNotification,
        label: mailbox_label(row),
        status: mailbox_status(row),
        pid: row.matched_os_pid,
        pgid: None,
        liveness: LivenessStatus::NotApplicable,
        started_at: Some(row.enqueued_at.clone()),
        updated_at: row.delivered_at.clone(),
        completed_at: row.delivered_at.clone(),
        last_output_excerpt: None,
        inspect_ref: Some(InspectRef::MailboxPayload { seq: row.seq }),
        cancel_ref: None,
        wake: None,
        mailbox: Some(mailbox_state(row)),
    }
}

fn mailbox_label(row: &MailboxRow) -> String {
    format!("mailbox {} {}", row.seq, row.handle)
}

fn wake_claim_node(
    claim: &WakeClaimRow,
    liveness: LivenessStatus,
    stale_after_seconds: i64,
) -> MonitorNode {
    MonitorNode {
        id: wake_node_id(&claim.session_id, &claim.claim_token),
        parent_id: Some(session_node_id(&claim.session_id)),
        kind: MonitorNodeKind::WakeClaim,
        label: wake_claim_label(claim),
        status: wake_status(claim, liveness, stale_after_seconds),
        pid: claim.wake_pid,
        pgid: None,
        liveness,
        started_at: Some(claim.claimed_at.clone()),
        updated_at: None,
        completed_at: None,
        last_output_excerpt: None,
        inspect_ref: Some(InspectRef::WakeClaim {
            session_id: claim.session_id.clone(),
        }),
        cancel_ref: None,
        wake: Some(wake_state(claim)),
        mailbox: None,
    }
}

fn wake_claim_label(claim: &WakeClaimRow) -> String {
    format!("wake claim {}", claim.claim_token)
}

fn wake_diagnostics(
    session_id: &str,
    pending_count: usize,
    runtime: Option<&SessionRuntimeRow>,
    runtime_liveness: Option<SessionRuntimeReadOnlyLiveness>,
    claim: Option<&WakeClaimRow>,
    pid: Option<&PidIdentityDb>,
    limits: SnapshotLimits,
) -> Vec<MonitorDiagnostic> {
    let mut diagnostics = Vec::new();
    if wake_needed_no_runtime(pending_count, runtime) {
        diagnostics.push(wake_needed_no_runtime_diagnostic(session_id));
    }
    if wake_needed_no_claim(pending_count, runtime_liveness, claim) {
        diagnostics.push(pending_no_claim_diagnostic(session_id));
    }
    if let Some(claim) = claim {
        diagnostics.extend(wake_claim_diagnostics(session_id, claim, pid, limits));
    }
    diagnostics
}

fn wake_needed_no_runtime(pending_count: usize, runtime: Option<&SessionRuntimeRow>) -> bool {
    pending_mailbox_without_runtime(pending_count, runtime)
}

fn wake_needed_no_claim(
    pending_count: usize,
    runtime_liveness: Option<SessionRuntimeReadOnlyLiveness>,
    claim: Option<&WakeClaimRow>,
) -> bool {
    pending_mailbox_idle_without_claim(pending_count, runtime_liveness, claim)
}

fn pending_mailbox_without_runtime(
    pending_count: usize,
    runtime: Option<&SessionRuntimeRow>,
) -> bool {
    pending_count > 0 && runtime.is_none()
}

fn pending_mailbox_idle_without_claim(
    pending_count: usize,
    runtime_liveness: Option<SessionRuntimeReadOnlyLiveness>,
    claim: Option<&WakeClaimRow>,
) -> bool {
    pending_count > 0
        && runtime_liveness == Some(SessionRuntimeReadOnlyLiveness::Idle)
        && claim.is_none()
}

fn wake_claim_diagnostics(
    session_id: &str,
    claim: &WakeClaimRow,
    pid: Option<&PidIdentityDb>,
    limits: SnapshotLimits,
) -> Vec<MonitorDiagnostic> {
    let mut diagnostics = Vec::new();
    if claim_has_stale_missing_pid(claim, limits.wake_claim_stale_after_seconds) {
        diagnostics.push(claim_no_pid_stale_diagnostic(session_id, claim));
    }
    if claim_has_stale_liveness(claim, pid) {
        diagnostics.push(claim_dead_diagnostic(session_id, claim));
    }
    diagnostics
}

fn claim_has_stale_missing_pid(claim: &WakeClaimRow, stale_after_seconds: i64) -> bool {
    wake_claim_no_pid_is_stale(claim, stale_after_seconds)
}

fn claim_has_stale_liveness(claim: &WakeClaimRow, pid: Option<&PidIdentityDb>) -> bool {
    wake_claim_liveness_is_stale(claim, pid)
}

fn wake_claim_liveness_is_stale(claim: &WakeClaimRow, pid: Option<&PidIdentityDb>) -> bool {
    matches!(
        wake_claim_liveness(claim, pid),
        LivenessStatus::Dead | LivenessStatus::PidReused
    )
}

fn wake_needed_no_runtime_diagnostic(session_id: &str) -> MonitorDiagnostic {
    wake_diagnostic(
        "wake-needed:no-runtime",
        "pending mailbox rows exist but no session_runtime row is present",
        Some(session_node_id(session_id)),
    )
}

fn pending_no_claim_diagnostic(session_id: &str) -> MonitorDiagnostic {
    wake_diagnostic(
        "stuck:pending-no-claim",
        "pending mailbox rows exist while the runtime is idle and no wake claim exists",
        Some(session_node_id(session_id)),
    )
}

fn claim_no_pid_stale_diagnostic(session_id: &str, claim: &WakeClaimRow) -> MonitorDiagnostic {
    wake_diagnostic(
        "stuck:claim-no-pid-stale",
        "wake claim has no wake PID and is older than the stale threshold",
        Some(wake_node_id(session_id, &claim.claim_token)),
    )
}

fn claim_dead_diagnostic(session_id: &str, claim: &WakeClaimRow) -> MonitorDiagnostic {
    wake_diagnostic(
        "stuck:claim-dead",
        "wake claim PID is dead or no longer matches its recorded identity",
        Some(wake_node_id(session_id, &claim.claim_token)),
    )
}

pub(crate) fn mailbox_state(row: &MailboxRow) -> MailboxState {
    MailboxState {
        seq: row.seq,
        session_id: row.session_id.clone(),
        kind: row.kind.clone(),
        handle: row.handle.clone(),
        enqueued_at: row.enqueued_at.clone(),
        delivered_at: row.delivered_at.clone(),
        delivered_by_invocation_uuid: row.delivered_by_invocation_uuid.clone(),
        delivery_attempts: row.delivery_attempts,
        delivery_error: row.delivery_error.clone(),
        owner_invocation_uuid: row.owner_invocation_uuid.clone(),
        state_dir: row.state_dir.clone(),
        meta_path: row.meta_path.clone(),
        log_path: row.log_path.clone(),
        rc_path: row.rc_path.clone(),
        rc: row.rc,
    }
}

fn wake_state(claim: &WakeClaimRow) -> WakeState {
    WakeState {
        session_id: claim.session_id.clone(),
        claim_token: claim.claim_token.clone(),
        claimed_at: claim.claimed_at.clone(),
        wake_pid: claim.wake_pid,
        wake_invocation_uuid: claim.wake_invocation_uuid.clone(),
        reason: claim.reason.clone(),
        auto_wake_count: claim.auto_wake_count,
        min_pending_seq_at_claim: claim.min_pending_seq_at_claim,
        max_pending_seq_at_claim: claim.max_pending_seq_at_claim,
    }
}

pub(crate) fn mailbox_node_id(session_id: &str, seq: i64) -> String {
    format!("mailbox:{session_id}:{seq}")
}

pub(crate) fn wake_node_id(session_id: &str, claim_token: &str) -> String {
    format!("wake:{session_id}:{claim_token}")
}

fn mailbox_parent_id(
    row: &MailboxRow,
    session_id: &str,
    invocation_uuids: &HashSet<String>,
) -> Option<String> {
    known_mailbox_owner_uuid(row, invocation_uuids)
        .map(mailbox_owner_parent_id)
        .or_else(|| Some(mailbox_session_parent_id(session_id)))
}

fn known_mailbox_owner_uuid<'a>(
    row: &'a MailboxRow,
    invocation_uuids: &HashSet<String>,
) -> Option<&'a str> {
    row.owner_invocation_uuid
        .as_deref()
        .filter(|uuid| invocation_uuids.contains(*uuid))
}

fn mailbox_owner_parent_id(uuid: &str) -> String {
    invocation_node_id(uuid)
}

fn mailbox_session_parent_id(session_id: &str) -> String {
    session_node_id(session_id)
}

fn session_label(root: &ObservabilityRoot, runtime: Option<&SessionRuntimeRow>) -> String {
    let provider = runtime
        .and_then(|row| row.provider_name.as_deref())
        .or(root.provider_name.as_deref())
        .unwrap_or("unknown-provider");
    let model = runtime
        .and_then(|row| row.model_name.as_deref())
        .or(root.model_name.as_deref())
        .unwrap_or("unknown-model");
    format!("session {provider}/{model}")
}

fn session_status(
    runtime: Option<&SessionRuntimeRow>,
    runtime_liveness: Option<SessionRuntimeReadOnlyLiveness>,
) -> MonitorStatus {
    if runtime_liveness.is_some_and(runtime_is_stale) {
        return MonitorStatus::Stale;
    }
    match runtime.map(|row| row.run_state.as_str()) {
        Some("running") => MonitorStatus::Running,
        Some("idle") => MonitorStatus::Idle,
        Some(_) => MonitorStatus::Unknown,
        None => MonitorStatus::Unknown,
    }
}

fn mailbox_status(row: &MailboxRow) -> MonitorStatus {
    if row.delivered_at.is_some() {
        MonitorStatus::Delivered
    } else if row.delivery_error.as_deref() == Some(WAKE_SWEEP_ABANDONED_ERROR) {
        MonitorStatus::Failed
    } else {
        MonitorStatus::Pending
    }
}

fn wake_status(
    claim: &WakeClaimRow,
    liveness: LivenessStatus,
    stale_after_seconds: i64,
) -> MonitorStatus {
    if wake_claim_no_pid_is_stale(claim, stale_after_seconds) {
        return MonitorStatus::Stale;
    }
    match liveness {
        LivenessStatus::VerifiedLive | LivenessStatus::UnverifiedLive => MonitorStatus::Running,
        LivenessStatus::Dead | LivenessStatus::PidReused => MonitorStatus::Stale,
        LivenessStatus::NotApplicable => MonitorStatus::Pending,
        LivenessStatus::Unknown => MonitorStatus::Unknown,
    }
}

fn pending_count(rows: &[MailboxRow]) -> usize {
    rows.iter()
        .filter(|row| mailbox_status(row) == MonitorStatus::Pending)
        .count()
}

fn wake_diagnostic(code: &str, message: &str, node_id: Option<String>) -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: code.to_string(),
        severity: MonitorDiagnosticSeverity::Warning,
        message: message.to_string(),
        node_id,
    }
}
