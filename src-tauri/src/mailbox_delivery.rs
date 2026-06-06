//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_state::mailbox::{MailboxDb, MailboxRow, SessionRuntimeUpsert};
use std::path::Path;
use uuid::Uuid;

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
}

pub(crate) struct PreparedPtyMailboxDelivery {
    pub envelope: String,
    pub seqs: Vec<i64>,
}

pub(crate) fn prepare_pty_mailbox_delivery(
    db: &MailboxDb,
    session_id: &str,
) -> Result<Option<PreparedPtyMailboxDelivery>, String> {
    let pending = pending_mailbox_rows(db, session_id)?;
    if !has_pending_rows(&pending) {
        return Ok(None);
    }
    let batch = select_batch(&pending);
    let seqs = batch_seqs(&batch);
    let envelope =
        render_notification_prefix(&batch.rows, batch.remaining_count, &new_delivery_nonce());
    Ok(Some(PreparedPtyMailboxDelivery { envelope, seqs }))
}

pub(crate) fn mailbox_prefix_max_bytes() -> usize {
    MAILBOX_PREFIX_MAX_BYTES
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
    let pending = pending_mailbox_rows(&db, &session_id)?;
    Ok(delivery_for_pending(session_id, pending, answer))
}

fn delivery_session_id(resolved: &oulipoly_state::ResolvedResume) -> String {
    resolved.active_session_id.clone()
}

fn pending_mailbox_rows(db: &MailboxDb, session_id: &str) -> Result<Vec<MailboxRow>, String> {
    db.list_pending(session_id)
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
    let mut rendered = String::new();
    rendered.push_str("[OULIPOLY NOTIFICATIONS]\n");
    rendered.push_str(
        "The following background agent-bash workloads completed while this session was inactive.\n\n",
    );
    for (index, row) in rows.iter().enumerate() {
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
    if remaining_count > 0 {
        rendered.push_str(&format!(
            "{remaining_count} additional notification(s) remain queued for the next resume.\n\n"
        ));
    }
    rendered.push_str("Use the paths above if you need details. Do not assume log content unless you inspect it.\n");
    rendered.push_str(DELIVERY_NONCE_PREFIX);
    rendered.push_str(delivery_nonce);
    rendered.push_str(DELIVERY_NONCE_SUFFIX);
    rendered.push('\n');
    rendered.push_str("[END OULIPOLY NOTIFICATIONS]");
    rendered
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
