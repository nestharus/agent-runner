//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_state::mailbox::{
    MailboxDb, MailboxRow, SessionRuntimeRow, SessionRuntimeUpsert, WAKE_SWEEP_ABANDONED_ERROR,
};
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

#[cfg(unix)]
use oulipoly_runtime::executor::cli::pty_broker::{
    PtyControlClientErrorKind, USER_INPUT_IDLE_INJECT_MS, append_notify_trace_record,
    inject_control_envelope, notify_trace_decision, notify_trace_inject_status,
    render_mailbox_notification_envelope, trace_token,
};

const MAILBOX_BATCH_MAX_ROWS: usize = 20;
const MAILBOX_PREFIX_MAX_BYTES: usize = 64 * 1024;
const MAILBOX_DELIVERY_UNCONFIRMED: &str = "mailbox_delivery_unconfirmed";
const MAX_UNCONFIRMED_DELIVERY_ATTEMPTS: i64 = 2;
const DELIVERY_NONCE_LENGTH_PLACEHOLDER: &str = "00000000-0000-4000-8000-000000000000";

pub(crate) struct PreparedMailboxDelivery {
    pub answer: Option<String>,
    pub session_id: String,
    pub seqs: Vec<i64>,
    pub delivery_nonce: Option<String>,
}

pub(crate) struct PreparedPtyMailboxDelivery {
    pub envelope: String,
    pub seqs: Vec<i64>,
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
    let runtime = match mailbox.session_runtime(session_id) {
        Ok(Some(runtime)) => runtime,
        Ok(None) => {
            return pty_status(
                false,
                "no_runtime",
                None,
                Vec::new(),
                pending_count(mailbox, session_id),
                None,
            );
        }
        Err(err) => return pty_status(false, "no_runtime", None, Vec::new(), None, Some(err)),
    };
    if runtime.mode != "pty_interactive" {
        return pty_status(
            false,
            "not_pty",
            None,
            Vec::new(),
            pending_count(mailbox, session_id),
            None,
        );
    }
    let Some(control_path) = live_pty_control_path(&runtime) else {
        return pty_status(
            false,
            "no_socket",
            runtime.pty_control_path,
            Vec::new(),
            pending_count(mailbox, session_id),
            None,
        );
    };
    let Some(delivery_invocation_uuid) = runtime.running_invocation_uuid.as_deref() else {
        return pty_status(
            false,
            "protocol_error",
            Some(control_path),
            Vec::new(),
            pending_count(mailbox, session_id),
            Some("running invocation missing".to_string()),
        );
    };
    let Some(prepared) =
        (match prepare_pty_mailbox_delivery(mailbox, session_id, delivery_invocation_uuid) {
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
        Ok(response) if response.ack => mark_pty_batch_delivered(
            mailbox,
            session_id,
            &prepared.seqs,
            &prepared.attempt_id,
            control_path,
        ),
        Ok(response) => {
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
            pty_client_error_status(mailbox, session_id, control_path, err.kind, err.message)
        }
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
    let pending = pending_mailbox_rows(db, session_id)?;
    if !has_pending_rows(&pending) {
        return Ok(None);
    }
    let batch = select_batch(&pending);
    let seqs = batch_seqs(&batch);
    let attempt_id = new_delivery_nonce();
    db.register_delivery_attempt(
        &attempt_id,
        session_id,
        delivery_invocation_uuid,
        &seqs,
        batch.remaining_count,
    )?;
    let window = db
        .delivery_attempt_window(&attempt_id)?
        .ok_or_else(|| format!("Mailbox delivery attempt {attempt_id} disappeared"))?;
    if window.rows.is_empty() {
        db.acknowledge_delivery_attempt(&attempt_id)?;
        return Ok(None);
    }
    let seqs = window.rows.iter().map(|row| row.seq).collect();
    let envelope = render_notification_prefix(&window.rows, window.remaining_count, &attempt_id);
    Ok(Some(PreparedPtyMailboxDelivery {
        envelope,
        seqs,
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
fn mark_pty_batch_delivered(
    mailbox: &mut MailboxDb,
    session_id: &str,
    seqs: &[i64],
    attempt_id: &str,
    control_path: String,
) -> PtyMailboxDeliveryDiagnostic {
    match mailbox.acknowledge_delivery_attempt(attempt_id) {
        Ok(true) => pty_status(
            true,
            "acked",
            Some(control_path),
            seqs.to_vec(),
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
fn pty_client_error_status(
    mailbox: &MailboxDb,
    session_id: &str,
    control_path: String,
    kind: PtyControlClientErrorKind,
    message: String,
) -> PtyMailboxDeliveryDiagnostic {
    let status = match kind {
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

fn pending_count(mailbox: &MailboxDb, session_id: &str) -> Option<usize> {
    mailbox.list_pending(session_id).map(|rows| rows.len()).ok()
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

fn pty_nack_status(message: &str) -> &str {
    match message {
        "unsafe_mid_line"
        | "unsafe_child_output_active"
        | "unsafe_foreground_process"
        | "unsafe_foreground_unknown" => message,
        _ => "protocol_error",
    }
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
    let inject_status = notify_trace_inject_status(base_decision, &diagnostic.status);
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
        "unsafe_mid_line" => "unsafe_mid_line",
        "unsafe_child_output_active" => "child_output_active",
        "unsafe_foreground_process" => "foreground_process",
        "unsafe_foreground_unknown" => "foreground_unknown",
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
    let pending = pending_mailbox_rows(&db, &session_id)?;
    Ok(delivery_for_pending(session_id, pending, answer))
}

pub(crate) fn deliverable_pending_count(session_id: &str) -> Result<usize, String> {
    let Some(db) = open_mailbox_sidecar()? else {
        return Ok(0);
    };
    if db.notifications_paused(session_id)? {
        return Ok(0);
    }
    pending_mailbox_rows(&db, session_id).map(|rows| rows.len())
}

fn delivery_session_id(resolved: &oulipoly_state::ResolvedResume) -> String {
    resolved.active_session_id.clone()
}

fn pending_mailbox_rows(db: &MailboxDb, session_id: &str) -> Result<Vec<MailboxRow>, String> {
    db.list_pending(session_id).map(deliverable_pending_rows)
}

fn deliverable_pending_rows(rows: Vec<MailboxRow>) -> Vec<MailboxRow> {
    rows.into_iter()
        .filter(mailbox_row_is_deliverable_pending)
        .collect()
}

fn mailbox_row_is_deliverable_pending(row: &MailboxRow) -> bool {
    row.delivery_error.as_deref() != Some(WAKE_SWEEP_ABANDONED_ERROR)
        && (row.delivery_error.as_deref() != Some(MAILBOX_DELIVERY_UNCONFIRMED)
            || row.delivery_attempts < MAX_UNCONFIRMED_DELIVERY_ATTEMPTS)
}

fn delivery_for_pending(
    session_id: String,
    pending: Vec<MailboxRow>,
    answer: Option<String>,
) -> PreparedMailboxDelivery {
    if !has_pending_rows(&pending) {
        return empty_delivery(answer, session_id);
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
) -> PreparedMailboxDelivery {
    let seqs = batch_seqs(&batch);
    let delivery_nonce = new_delivery_nonce();
    let prefix = render_notification_prefix(&batch.rows, batch.remaining_count, &delivery_nonce);
    prepared_delivery(session_id, seqs, prefix, answer, delivery_nonce)
}

fn prepared_delivery(
    session_id: String,
    seqs: Vec<i64>,
    prefix: String,
    answer: Option<String>,
    delivery_nonce: String,
) -> PreparedMailboxDelivery {
    PreparedMailboxDelivery {
        answer: Some(compose_answer(prefix, answer)),
        session_id,
        seqs,
        delivery_nonce: Some(delivery_nonce),
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
    render_notification_prefix(rows, remaining_count, DELIVERY_NONCE_LENGTH_PLACEHOLDER).len()
}

fn render_notification_prefix(
    rows: &[MailboxRow],
    remaining_count: usize,
    delivery_nonce: &str,
) -> String {
    render_mailbox_notification_envelope(rows, remaining_count, delivery_nonce)
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
        };

        assert_eq!(prepared.answer, original);
    }

    #[test]
    fn notification_prefix_includes_delivery_nonce_near_end() {
        let prefix = render_notification_prefix(&[], 0, "nonce-123");

        assert!(
            prefix.contains("[OULIPOLY-DELIVERY nonce-123]\n[END OULIPOLY NOTIFICATIONS]"),
            "{prefix}"
        );
    }
}
