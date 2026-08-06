//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_config::SessionsConfig;
use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{
    ExactProcessEvidence, ExitRuntimeGenerationNonOrderly, GenerationMutation, MailboxDb,
    MailboxRow, RuntimeGenerationFence, RuntimeLifecycleState, RuntimeTerminalReason,
    SUBMITTED_INPUT_KIND, SessionGenerationProjection, SessionRuntimeRow, SessionRuntimeUpsert,
    mailbox_row_is_deliverable_pending,
};
use oulipoly_state::pid_identity::{ProcessIdentityObservation, observe_live_process_identity};
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

#[cfg(unix)]
use oulipoly_runtime::executor::cli::pty_broker::{
    PtyControlClientErrorKind, USER_INPUT_IDLE_INJECT_MS, append_notify_trace_record,
    inject_control_envelope, notify_trace_decision, notify_trace_inject_status,
    pty_delivery_ack_message, trace_token, unlink_control_socket_if_owned,
};

const MAILBOX_BATCH_MAX_ROWS: usize = 20;
const MAILBOX_PREFIX_MAX_BYTES: usize = 64 * 1024;
const DELIVERY_NONCE_PREFIX: &str = "[OULIPOLY-DELIVERY ";
const DELIVERY_NONCE_SUFFIX: &str = "]";
const DELIVERY_NONCE_LENGTH_PLACEHOLDER: &str = "00000000-0000-4000-8000-000000000000";

pub(crate) struct PreparedMailboxDelivery {
    pub answer: Option<String>,
    pub session_id: String,
    pub seqs: Vec<i64>,
    pub delivery_nonce: Option<String>,
    pub requires_turn_confirmation: bool,
}

pub(crate) struct PreparedPtyMailboxDelivery {
    pub envelope: String,
    pub attempt_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PtyMailboxDeliveryDiagnostic {
    pub(crate) attempted: bool,
    pub(crate) status: String,
    pub(crate) control_path: Option<String>,
    pub(crate) submitted: bool,
    pub(crate) delivered_seqs: Vec<i64>,
    pub(crate) remaining_pending: Option<usize>,
    pub(crate) message: Option<String>,
}

#[cfg(unix)]
struct PtyRuntimeAuthority {
    control_path: String,
    delivery_invocation_uuid: String,
}

#[cfg(unix)]
pub(crate) fn attempt_pty_mailbox_delivery_with_trigger(
    mailbox: &mut MailboxDb,
    session_id: &str,
    trigger: &str,
) -> PtyMailboxDeliveryDiagnostic {
    let diagnostic = attempt_pty_mailbox_delivery_inner(mailbox, session_id);
    trace_notify_pty_attempt(trigger, session_id, &diagnostic);
    diagnostic
}

#[cfg(unix)]
fn attempt_pty_mailbox_delivery_inner(
    mailbox: &mut MailboxDb,
    session_id: &str,
) -> PtyMailboxDeliveryDiagnostic {
    if matches!(mailbox.notifications_paused(session_id), Ok(true)) {
        return pty_status(
            false,
            "paused",
            None,
            Vec::new(),
            pending_count(mailbox, session_id),
            None,
        );
    }
    let authority = match pty_runtime_authority(mailbox, session_id) {
        Ok(authority) => authority,
        Err(diagnostic) => return diagnostic,
    };
    let control_path = authority.control_path;
    let delivery_invocation_uuid = authority.delivery_invocation_uuid;
    if let Err(err) = acknowledge_injected_pty_delivery_attempts(mailbox, session_id) {
        tracing::warn!(
            session_id,
            "Failed to acknowledge injected PTY delivery: {err}"
        );
    }
    let Some(prepared) =
        (match prepare_pty_mailbox_delivery(mailbox, session_id, &delivery_invocation_uuid) {
            Ok(prepared) => prepared,
            Err(err) => {
                return pty_status(
                    false,
                    "protocol_error",
                    Some(control_path),
                    Vec::new(),
                    None,
                    Some(err),
                );
            }
        })
    else {
        return pty_status(
            false,
            "no_pending",
            Some(control_path),
            Vec::new(),
            Some(0),
            None,
        );
    };
    if prepared.envelope.len() > mailbox_prefix_max_bytes() {
        return pty_status(
            false,
            "protocol_error",
            Some(control_path),
            Vec::new(),
            pending_count(mailbox, session_id),
            Some("oversize_frame".to_string()),
        );
    }
    match inject_control_envelope(&control_path, &prepared.envelope) {
        Ok(response)
            if response.ack
                && response.message == pty_delivery_ack_message(&prepared.attempt_id) =>
        {
            acknowledge_pty_batch_injected(mailbox, session_id, &prepared.attempt_id, control_path)
        }
        Ok(response) if response.ack => mark_unconfirmed_pty_ack(
            mailbox,
            session_id,
            &prepared.attempt_id,
            control_path,
            response.message,
        ),
        Ok(response) => {
            resolve_unacknowledged_pty_attempt_or_warn(mailbox, session_id, &prepared.attempt_id);
            let status = pty_nack_status(&response.message).to_string();
            pty_status(
                true,
                &status,
                Some(control_path),
                Vec::new(),
                pending_count(mailbox, session_id),
                Some(response.message),
            )
        }
        Err(err) => {
            if !pty_client_error_may_have_reached_broker(&err.kind) {
                resolve_unacknowledged_pty_attempt_or_warn(
                    mailbox,
                    session_id,
                    &prepared.attempt_id,
                );
            }
            pty_client_error_status(mailbox, session_id, control_path, err.kind, err.message)
        }
    }
}

#[cfg(unix)]
fn pty_runtime_authority(
    mailbox: &mut MailboxDb,
    session_id: &str,
) -> Result<PtyRuntimeAuthority, PtyMailboxDeliveryDiagnostic> {
    match mailbox.session_generation_projection(session_id) {
        Ok(SessionGenerationProjection::One(generation)) => {
            let control_path = generation.pty_control_path.clone();
            if terminalize_stale_runtime_generation(
                mailbox,
                session_id,
                control_path.as_deref().unwrap_or_default(),
            ) {
                return Err(pty_status(
                    false,
                    "stale_generation",
                    control_path,
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    Some("runtime generation process identity is no longer live".to_string()),
                ));
            }
            if generation.runtime_mode != "pty_interactive" {
                return Err(pty_status(
                    false,
                    "not_pty",
                    None,
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    None,
                ));
            }
            let Some(control_path) = control_path.filter(|path| !path.is_empty()) else {
                return Err(pty_status(
                    false,
                    "no_socket",
                    None,
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    None,
                ));
            };
            if generation.lifecycle_state != RuntimeLifecycleState::Running {
                return Err(pty_status(
                    false,
                    "no_socket",
                    Some(control_path),
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    Some(format!(
                        "runtime generation is {:?}",
                        generation.lifecycle_state
                    )),
                ));
            }
            Ok(PtyRuntimeAuthority {
                control_path,
                delivery_invocation_uuid: generation.spawn_invocation_uuid.clone(),
            })
        }
        Ok(SessionGenerationProjection::Multiple(_)) => Err(pty_status(
            false,
            "ambiguous_runtime",
            None,
            Vec::new(),
            pending_count(mailbox, session_id),
            Some("multiple nonterminal runtime generations".to_string()),
        )),
        Ok(SessionGenerationProjection::None) => legacy_pty_runtime_authority(mailbox, session_id),
        Err(err) => Err(pty_status(
            false,
            "no_runtime",
            None,
            Vec::new(),
            None,
            Some(err.to_string()),
        )),
    }
}

#[cfg(unix)]
fn legacy_pty_runtime_authority(
    mailbox: &MailboxDb,
    session_id: &str,
) -> Result<PtyRuntimeAuthority, PtyMailboxDeliveryDiagnostic> {
    let runtime = match mailbox.session_runtime(session_id) {
        Ok(Some(runtime)) => runtime,
        Ok(None) => {
            return Err(pty_status(
                false,
                "no_runtime",
                None,
                Vec::new(),
                pending_count(mailbox, session_id),
                None,
            ));
        }
        Err(err) => {
            return Err(pty_status(
                false,
                "no_runtime",
                None,
                Vec::new(),
                None,
                Some(err),
            ));
        }
    };
    if runtime.mode != "pty_interactive" {
        return Err(pty_status(
            false,
            "not_pty",
            None,
            Vec::new(),
            pending_count(mailbox, session_id),
            None,
        ));
    }
    let Some(control_path) = live_pty_control_path(&runtime) else {
        return Err(pty_status(
            false,
            "no_socket",
            runtime.pty_control_path,
            Vec::new(),
            pending_count(mailbox, session_id),
            None,
        ));
    };
    let Some(delivery_invocation_uuid) = runtime.running_invocation_uuid else {
        return Err(pty_status(
            false,
            "protocol_error",
            Some(control_path),
            Vec::new(),
            pending_count(mailbox, session_id),
            Some("running invocation missing".to_string()),
        ));
    };
    Ok(PtyRuntimeAuthority {
        control_path,
        delivery_invocation_uuid,
    })
}

#[cfg(unix)]
fn resolve_unacknowledged_pty_attempt_or_warn(
    mailbox: &mut MailboxDb,
    session_id: &str,
    attempt_id: &str,
) {
    if let Err(err) = mailbox.resolve_unacknowledged_delivery_attempt(attempt_id) {
        tracing::warn!(
            session_id,
            attempt_id,
            "Failed to resolve unacknowledged PTY delivery attempt: {err}"
        );
    }
}

#[cfg(not(unix))]
pub(crate) fn attempt_pty_mailbox_delivery_with_trigger(
    _mailbox: &mut MailboxDb,
    _session_id: &str,
    _trigger: &str,
) -> PtyMailboxDeliveryDiagnostic {
    pty_status(false, "not_pty", None, Vec::new(), None, None)
}

pub(crate) fn prepare_pty_mailbox_delivery(
    db: &mut MailboxDb,
    session_id: &str,
    delivery_invocation_uuid: &str,
) -> Result<Option<PreparedPtyMailboxDelivery>, String> {
    let pending = pending_mailbox_rows(db, session_id, None)?;
    if !has_pending_rows(&pending) {
        return Ok(None);
    }
    let batch = select_batch(&pending);
    let seqs = batch_seqs(&batch);
    let candidate_attempt_id = new_delivery_nonce();
    let attempt_id = db.register_or_reuse_delivery_attempt(
        &candidate_attempt_id,
        session_id,
        delivery_invocation_uuid,
        &seqs,
        batch.remaining_count,
    )?;
    let window = db
        .delivery_attempt_window(&attempt_id)?
        .ok_or_else(|| format!("Mailbox delivery attempt {attempt_id} disappeared"))?;
    if window.rows.is_empty() {
        return Ok(None);
    }
    let envelope = render_mailbox_prefix(&window.rows, window.remaining_count, &attempt_id)?;
    Ok(Some(PreparedPtyMailboxDelivery {
        envelope,
        attempt_id,
    }))
}

pub(crate) fn mailbox_prefix_max_bytes() -> usize {
    MAILBOX_PREFIX_MAX_BYTES
}

#[cfg(unix)]
fn live_pty_control_path(runtime: &SessionRuntimeRow) -> Option<String> {
    (runtime.run_state == "running")
        .then_some(runtime.pty_control_path.as_ref())
        .flatten()
        .filter(|path| !path.is_empty())
        .cloned()
}

#[cfg(unix)]
fn acknowledge_pty_batch_injected(
    mailbox: &mut MailboxDb,
    session_id: &str,
    attempt_id: &str,
    control_path: String,
) -> PtyMailboxDeliveryDiagnostic {
    let seqs = mailbox
        .delivery_attempt_window(attempt_id)
        .ok()
        .flatten()
        .map(|window| {
            window
                .rows
                .into_iter()
                .map(|row| row.seq)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let confirmation = mailbox
        .record_delivery_attempt_transport_ack(attempt_id)
        .and_then(|recorded| {
            if recorded {
                mailbox.confirm_delivery_attempt(attempt_id)
            } else {
                mailbox
                    .delivery_attempt_disposition(attempt_id)
                    .map(|disposition| {
                        matches!(
                            disposition,
                            Some(
                                oulipoly_state::mailbox::MailboxDeliveryAttemptDisposition::Resolved
                            )
                        )
                    })
            }
        });
    match confirmation {
        Ok(true) => pty_status(
            true,
            "acked",
            Some(control_path),
            seqs,
            pending_count(mailbox, session_id),
            Some("ok".to_string()),
        ),
        Ok(false) => pty_status(
            true,
            "mark_delivered_error",
            Some(control_path),
            Vec::new(),
            pending_count(mailbox, session_id),
            Some(format!("delivery attempt {attempt_id} is not registered")),
        ),
        Err(err) => pty_status(
            true,
            "mark_delivered_error",
            Some(control_path),
            Vec::new(),
            pending_count(mailbox, session_id),
            Some(err),
        ),
    }
}

#[cfg(unix)]
fn mark_unconfirmed_pty_ack(
    mailbox: &mut MailboxDb,
    session_id: &str,
    attempt_id: &str,
    control_path: String,
    message: String,
) -> PtyMailboxDeliveryDiagnostic {
    let seqs = mailbox
        .delivery_attempt_window(attempt_id)
        .ok()
        .flatten()
        .map(|window| {
            window
                .rows
                .into_iter()
                .map(|row| row.seq)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    resolve_unacknowledged_pty_attempt_or_warn(mailbox, session_id, attempt_id);
    let failure = mailbox
        .mark_delivery_failed(session_id, &seqs, "mailbox_delivery_unconfirmed")
        .err();
    pty_status(
        true,
        "unconfirmed_ack",
        Some(control_path),
        Vec::new(),
        pending_count(mailbox, session_id),
        Some(
            failure
                .map(|failure| format!("{message}; {failure}"))
                .unwrap_or(message),
        ),
    )
}

#[cfg(unix)]
fn pty_client_error_status(
    mailbox: &mut MailboxDb,
    session_id: &str,
    control_path: String,
    kind: PtyControlClientErrorKind,
    message: String,
) -> PtyMailboxDeliveryDiagnostic {
    let status = match kind {
        PtyControlClientErrorKind::Connect
            if terminalize_stale_runtime_generation(mailbox, session_id, &control_path) =>
        {
            "stale_generation"
        }
        PtyControlClientErrorKind::Connect => "connect_error",
        PtyControlClientErrorKind::Protocol
        | PtyControlClientErrorKind::Oversize
        | PtyControlClientErrorKind::EmptyPayload => "protocol_error",
    };
    pty_status(
        true,
        status,
        Some(control_path),
        Vec::new(),
        pending_count(mailbox, session_id),
        Some(message),
    )
}

#[cfg(unix)]
fn terminalize_stale_runtime_generation(
    mailbox: &mut MailboxDb,
    session_id: &str,
    control_path: &str,
) -> bool {
    let generation = match mailbox.session_generation_projection(session_id) {
        Ok(SessionGenerationProjection::One(generation)) => generation,
        Ok(SessionGenerationProjection::None | SessionGenerationProjection::Multiple(_))
        | Err(_) => return false,
    };
    let ExactProcessEvidence::Recorded(identity) = &generation.exact_process_evidence else {
        return false;
    };
    let stale = match observe_live_process_identity(identity.os_pid) {
        ProcessIdentityObservation::ExactLive(live) => live != *identity,
        ProcessIdentityObservation::Dead => true,
        ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => false,
    };
    if !stale {
        return false;
    }
    let fence = RuntimeGenerationFence {
        generation_id: &generation.generation_id,
        spawn_invocation_uuid: &generation.spawn_invocation_uuid,
    };
    let terminalized = mailbox
        .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
            fence,
            reason: RuntimeTerminalReason::RecoveredDead,
            exit_code: None,
        })
        .is_ok_and(|result| {
            matches!(
                result,
                GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_)
            )
        });
    let _ = mailbox.session_liveness(session_id);
    let _ = unlink_control_socket_if_owned(control_path);
    terminalized
}

#[cfg(unix)]
fn pty_client_error_may_have_reached_broker(kind: &PtyControlClientErrorKind) -> bool {
    matches!(kind, PtyControlClientErrorKind::Protocol)
}

fn pending_count(mailbox: &MailboxDb, session_id: &str) -> Option<usize> {
    pending_mailbox_rows(mailbox, session_id, None)
        .map(|rows| rows.len())
        .ok()
}

fn pty_status(
    attempted: bool,
    status: &str,
    control_path: Option<String>,
    delivered_seqs: Vec<i64>,
    remaining_pending: Option<usize>,
    message: Option<String>,
) -> PtyMailboxDeliveryDiagnostic {
    PtyMailboxDeliveryDiagnostic {
        attempted,
        status: status.to_string(),
        control_path,
        submitted: pty_status_implies_submit(status),
        delivered_seqs,
        remaining_pending,
        message,
    }
}

fn pty_status_implies_submit(status: &str) -> bool {
    matches!(status, "acked" | "mark_delivered_error")
}

fn acknowledge_injected_pty_delivery_attempts(
    mailbox: &mut MailboxDb,
    session_id: &str,
) -> Result<(), String> {
    for window in mailbox.accepted_delivery_attempt_windows(session_id)? {
        mailbox.confirm_delivery_attempt(&window.attempt_id)?;
    }
    Ok(())
}

pub(crate) fn finalize_pty_mailbox_delivery_handoff(
    _state: &StateDb,
    _sessions_cfg: &SessionsConfig,
    _provider_name: &str,
    session_id: Option<&str>,
    invocation_uuid: &str,
    exit_code: i32,
) -> Result<bool, String> {
    let Some(session_id) = session_id else {
        return Ok(false);
    };
    if let Some(mut mailbox) = MailboxDb::open_default_if_exists()? {
        acknowledge_injected_pty_delivery_attempts(&mut mailbox, session_id)?;
    }
    crate::wake_coordinator::mark_session_idle_after_turn(
        session_id,
        invocation_uuid,
        Some(exit_code),
    )?;
    Ok(true)
}

fn pty_nack_status(_message: &str) -> &str {
    "protocol_error"
}

pub(crate) fn trace_notify_enabled() -> bool {
    matches!(
        std::env::var("OULIPOLY_TRACE_NOTIFY").ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn trace_notify_pty_attempt(
    trigger: &str,
    session_id: &str,
    diagnostic: &PtyMailboxDeliveryDiagnostic,
) {
    let base_decision = if diagnostic.submitted {
        "inject"
    } else {
        "skip"
    };
    let trace_status = if diagnostic.status == "stale_generation" {
        "connect_error"
    } else {
        &diagnostic.status
    };
    let inject_status = notify_trace_inject_status(base_decision, trace_status);
    let reason = notify_trace_summary_reason(&diagnostic.status);
    let record = format!(
        "trigger={} session_id={} attempted={} input_empty=unknown at_boundary=unknown \
         mid_escape=unknown last_user_input_ms=unknown user_input_idle_ms=unknown \
         user_input_idle=unknown user_input_idle_threshold_ms={} boundary_probe=unknown \
         quiescent=unknown last_child_output_ms=unknown foreground=unknown \
         control_path_present={} decision={} inject_status={} submitted={} \
         delivered_count={} remaining_pending={} message={} reason={} consumed=unknown",
        trace_token(trigger),
        trace_token(session_id),
        diagnostic.attempted,
        USER_INPUT_IDLE_INJECT_MS,
        diagnostic
            .control_path
            .as_deref()
            .is_some_and(|path| !path.is_empty()),
        notify_trace_decision(base_decision, &inject_status),
        inject_status,
        diagnostic.submitted,
        diagnostic.delivered_seqs.len(),
        diagnostic
            .remaining_pending
            .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
        diagnostic
            .message
            .as_deref()
            .map(trace_token)
            .unwrap_or_else(|| "none".to_string()),
        reason,
    );
    append_notify_trace_record(&record);
    if trace_notify_enabled() {
        eprintln!("oulipoly_notify_trace {record}");
    }
}

fn notify_trace_summary_reason(status: &str) -> &'static str {
    match status {
        "acked" | "mark_delivered_error" => "control_ack",
        "connect_error" => "connect_error",
        "no_runtime" => "no_runtime",
        "not_pty_interactive" => "not_pty_interactive",
        _ => "protocol_or_delivery_error",
    }
}

pub(crate) fn prepare_headless_resume_delivery(
    resolved: &oulipoly_state::ResolvedResume,
    answer: Option<String>,
    models_dir: Option<&Path>,
) -> Result<PreparedMailboxDelivery, String> {
    let session_id = delivery_session_id(resolved);
    let Some(mut db) = open_mailbox_sidecar()? else {
        return Ok(empty_delivery(answer, session_id));
    };
    record_headless_session_runtime(&mut db, resolved, models_dir)?;
    if db.notifications_paused(&session_id)? {
        return Ok(empty_delivery(answer, session_id));
    }
    let pending = pending_mailbox_rows(&db, &session_id, Some(&resolved.chain_id))?;
    delivery_for_pending(session_id, pending, answer)
}

pub(crate) fn deliverable_pending_count(session_id: &str) -> Result<usize, String> {
    let Some(db) = open_mailbox_sidecar()? else {
        return Ok(0);
    };
    if db.notifications_paused(session_id)? {
        return Ok(0);
    }
    pending_mailbox_rows(&db, session_id, None).map(|rows| rows.len())
}

fn delivery_session_id(resolved: &oulipoly_state::ResolvedResume) -> String {
    resolved.active_session_id.clone()
}

fn pending_mailbox_rows(
    db: &MailboxDb,
    session_id: &str,
    chain_id: Option<&str>,
) -> Result<Vec<MailboxRow>, String> {
    let rows = db.list_pending_for_delivery(session_id, chain_id)?;
    for row in &rows {
        db.verify_mailbox_row_payload(row)
            .map_err(|err| format!("Mailbox row {} payload unavailable: {err}", row.seq))?;
    }
    Ok(deliverable_pending_rows(rows))
}

fn deliverable_pending_rows(rows: Vec<MailboxRow>) -> Vec<MailboxRow> {
    rows.into_iter()
        .filter(mailbox_row_is_deliverable_pending)
        .collect()
}

fn delivery_for_pending(
    session_id: String,
    pending: Vec<MailboxRow>,
    answer: Option<String>,
) -> Result<PreparedMailboxDelivery, String> {
    if !has_pending_rows(&pending) {
        return Ok(empty_delivery(answer, session_id));
    }

    let batch = select_batch(&pending);
    delivery_for_batch(session_id, batch, answer)
}

fn open_mailbox_sidecar() -> Result<Option<MailboxDb>, String> {
    MailboxDb::open_default_if_exists()
}

fn empty_delivery(answer: Option<String>, session_id: String) -> PreparedMailboxDelivery {
    PreparedMailboxDelivery {
        answer,
        session_id,
        seqs: Vec::new(),
        delivery_nonce: None,
        requires_turn_confirmation: false,
    }
}

fn has_pending_rows(rows: &[MailboxRow]) -> bool {
    !rows.is_empty()
}

fn batch_seqs(batch: &MailboxBatch) -> Vec<i64> {
    batch.rows.iter().map(|row| row.seq).collect()
}

fn delivery_for_batch(
    session_id: String,
    batch: MailboxBatch,
    answer: Option<String>,
) -> Result<PreparedMailboxDelivery, String> {
    let seqs = batch_seqs(&batch);
    let requires_turn_confirmation = batch
        .rows
        .iter()
        .any(|row| row.kind != SUBMITTED_INPUT_KIND);
    let delivery_nonce = new_delivery_nonce();
    let prefix = render_mailbox_prefix(&batch.rows, batch.remaining_count, &delivery_nonce)?;
    Ok(prepared_delivery(
        session_id,
        seqs,
        prefix,
        answer,
        delivery_nonce,
        requires_turn_confirmation,
    ))
}

fn prepared_delivery(
    session_id: String,
    seqs: Vec<i64>,
    prefix: String,
    answer: Option<String>,
    delivery_nonce: String,
    requires_turn_confirmation: bool,
) -> PreparedMailboxDelivery {
    PreparedMailboxDelivery {
        answer: Some(compose_answer(prefix, answer)),
        session_id,
        seqs,
        delivery_nonce: Some(delivery_nonce),
        requires_turn_confirmation,
    }
}

pub(crate) fn mark_headless_resume_delivered(
    session_id: &str,
    seqs: &[i64],
    delivered_by_invocation_uuid: &str,
) -> Result<(), String> {
    if seqs.is_empty() {
        return Ok(());
    }
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Err("mailbox sidecar missing while marking delivered rows".to_string());
    };
    db.mark_delivered(session_id, seqs, delivered_by_invocation_uuid)
}

pub(crate) fn mark_headless_resume_delivery_failed(
    session_id: &str,
    seqs: &[i64],
    delivery_error: &str,
) -> Result<(), String> {
    if seqs.is_empty() {
        return Ok(());
    }
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Err("mailbox sidecar missing while marking failed delivery rows".to_string());
    };
    db.mark_delivery_failed(session_id, seqs, delivery_error)
}

struct MailboxBatch {
    rows: Vec<MailboxRow>,
    remaining_count: usize,
}

fn record_headless_session_runtime(
    db: &mut MailboxDb,
    resolved: &oulipoly_state::ResolvedResume,
    models_dir: Option<&Path>,
) -> Result<(), String> {
    let models_dir = models_dir_string(models_dir);
    let input = headless_session_runtime_upsert(resolved, models_dir.as_deref());
    db.upsert_session_runtime(input)
}

fn headless_session_runtime_upsert<'a>(
    resolved: &'a oulipoly_state::ResolvedResume,
    models_dir: Option<&'a str>,
) -> SessionRuntimeUpsert<'a> {
    SessionRuntimeUpsert {
        session_id: &resolved.active_session_id,
        mode: "headless",
        invocation_uuid: None,
        provider_name: Some(&resolved.active_provider),
        model_name: resolved.model_name.as_deref(),
        pty_control_path: None,
        models_dir,
        effective_cwd: None,
        selected_auto_wake_max: None,
    }
}

fn models_dir_string(path: Option<&Path>) -> Option<String> {
    path.map(|path| path.to_string_lossy().into_owned())
}

fn select_batch(pending: &[MailboxRow]) -> MailboxBatch {
    let prefix_lengths = candidate_prefix_lengths(pending);
    select_batch_by_prefix_lengths(pending, &prefix_lengths)
}

fn candidate_prefix_lengths(pending: &[MailboxRow]) -> Vec<usize> {
    (1..=candidate_count(pending))
        .map(|row_count| candidate_prefix_len(pending, row_count))
        .collect()
}

fn candidate_count(pending: &[MailboxRow]) -> usize {
    pending.len().min(MAILBOX_BATCH_MAX_ROWS)
}

fn candidate_prefix_len(pending: &[MailboxRow], row_count: usize) -> usize {
    let remaining_count = pending.len().saturating_sub(row_count);
    notification_prefix_len(&pending[..row_count], remaining_count)
}

fn select_batch_by_prefix_lengths(
    pending: &[MailboxRow],
    prefix_lengths: &[usize],
) -> MailboxBatch {
    let selected_count = selected_batch_len(pending, prefix_lengths);
    let rows = pending
        .iter()
        .take(selected_count)
        .cloned()
        .collect::<Vec<_>>();
    MailboxBatch {
        remaining_count: pending.len().saturating_sub(rows.len()),
        rows,
    }
}

fn selected_batch_len(pending: &[MailboxRow], prefix_lengths: &[usize]) -> usize {
    if pending.is_empty() {
        return 0;
    }
    prefix_lengths
        .iter()
        .position(|length| *length > MAILBOX_PREFIX_MAX_BYTES)
        .map(|index| selected_len_before_limit(index + 1))
        .unwrap_or(prefix_lengths.len())
}

fn selected_len_before_limit(row_count: usize) -> usize {
    if row_count > 1 { row_count - 1 } else { 1 }
}

fn notification_prefix_len(rows: &[MailboxRow], remaining_count: usize) -> usize {
    render_mailbox_prefix(rows, remaining_count, DELIVERY_NONCE_LENGTH_PLACEHOLDER)
        .map(|prefix| prefix.len())
        .unwrap_or_else(|_| MAILBOX_PREFIX_MAX_BYTES.saturating_add(1))
}

fn render_mailbox_prefix(
    rows: &[MailboxRow],
    remaining_count: usize,
    delivery_nonce: &str,
) -> Result<String, String> {
    let mut rendered = String::new();
    let contains_input = rows.iter().any(|row| row.kind == SUBMITTED_INPUT_KIND);
    if contains_input {
        rendered.push_str("[OULIPOLY INBOX]\n");
        rendered.push_str("The following accepted inbox items are pending delivery.\n\n");
    } else {
        rendered.push_str("[OULIPOLY NOTIFICATIONS]\n");
        rendered.push_str(
            "The following background agent-bash workloads completed while this session was inactive.\n\n",
        );
    }
    for (index, row) in rows.iter().enumerate() {
        if row.kind == SUBMITTED_INPUT_KIND {
            render_submitted_input(&mut rendered, index, row)?;
        } else {
            render_notification(&mut rendered, index, row);
        }
    }
    if remaining_count > 0 {
        rendered.push_str(&format!(
            "{remaining_count} additional notification(s) remain queued for the next resume.\n\n"
        ));
    }
    if !contains_input {
        rendered.push_str("Use the paths above if you need details. Do not assume log content unless you inspect it.\n");
    }
    rendered.push_str(DELIVERY_NONCE_PREFIX);
    rendered.push_str(delivery_nonce);
    rendered.push_str(DELIVERY_NONCE_SUFFIX);
    rendered.push('\n');
    rendered.push_str(if contains_input {
        "[END OULIPOLY INBOX]"
    } else {
        "[END OULIPOLY NOTIFICATIONS]"
    });
    Ok(rendered)
}

fn render_notification(rendered: &mut String, index: usize, row: &MailboxRow) {
    rendered.push_str(&format!(
        "{}. kind: {}\n   handle: {}\n   rc: {}\n   state_dir: {}\n   meta: {}\n   log: {}\n   rc_file: {}\n\n",
        index + 1,
        sanitize(&row.kind),
        sanitize(&row.handle),
        row.rc,
        quote_path(&row.state_dir),
        quote_path(&row.meta_path),
        quote_path(&row.log_path),
        quote_path(&row.rc_path),
    ));
}

fn render_submitted_input(
    rendered: &mut String,
    index: usize,
    row: &MailboxRow,
) -> Result<(), String> {
    let path = row
        .payload_file_path
        .as_deref()
        .ok_or_else(|| format!("Input mailbox row {} has no payload file", row.seq))?;
    let payload = std::fs::read_to_string(path).map_err(|err| {
        format!(
            "Failed to read input mailbox row {} payload: {err}",
            row.seq
        )
    })?;
    rendered.push_str(&format!(
        "{}. kind: {}\n   item_id: {}\n   target: {}:{}\n   payload:\n{}\n\n",
        index + 1,
        sanitize(&row.kind),
        sanitize(&row.handle),
        sanitize(row.target_kind.as_deref().unwrap_or("unknown")),
        sanitize(row.target_id.as_deref().unwrap_or("unknown")),
        payload,
    ));
    Ok(())
}

fn new_delivery_nonce() -> String {
    Uuid::new_v4().to_string()
}

fn compose_answer(prefix: String, answer: Option<String>) -> String {
    match answer {
        Some(answer) => format!("{prefix}\n\n[USER RESUME PAYLOAD]\n{answer}"),
        None => prefix,
    }
}

fn quote_path(path: &str) -> String {
    format!("\"{}\"", sanitize(path))
}

fn sanitize(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_without_pending_preserves_answer_by_caller_contract() {
        let original = Some("byte-identical".to_string());
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27".to_string();
        let prepared = PreparedMailboxDelivery {
            answer: original.clone(),
            session_id,
            seqs: Vec::new(),
            delivery_nonce: None,
            requires_turn_confirmation: false,
        };

        assert_eq!(prepared.answer, original);
    }

    #[test]
    fn notification_prefix_includes_delivery_nonce_near_end() {
        let prefix = render_mailbox_prefix(&[], 0, "nonce-123").unwrap();

        assert!(
            prefix.contains("[OULIPOLY-DELIVERY nonce-123]\n[END OULIPOLY NOTIFICATIONS]"),
            "{prefix}"
        );
    }
}
