//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`

use oulipoly_state::mailbox::{MailboxDb, MailboxRow, SessionRuntimeUpsert};
use std::path::Path;

const MAILBOX_BATCH_MAX_ROWS: usize = 20;
const MAILBOX_PREFIX_MAX_BYTES: usize = 64 * 1024;

pub(crate) struct PreparedMailboxDelivery {
    pub answer: Option<String>,
    pub session_id: String,
    pub seqs: Vec<i64>,
}

pub(crate) fn prepare_headless_resume_delivery(
    resolved: &oulipoly_state::ResolvedResume,
    answer: Option<String>,
    models_dir: Option<&Path>,
) -> Result<PreparedMailboxDelivery, String> {
    let session_id = resolved.active_session_id.clone();
    let Some(mut db) = open_mailbox_sidecar()? else {
        return Ok(empty_delivery(answer, session_id));
    };
    record_headless_session_runtime(&mut db, resolved, models_dir)?;
    let pending = db.list_pending(&session_id)?;
    if !has_pending_rows(&pending) {
        return Ok(empty_delivery(answer, session_id));
    }

    let batch = select_batch(&pending);
    let seqs = batch_seqs(&batch);
    let prefix = render_notification_prefix(&batch.rows, batch.remaining_count);
    Ok(prepared_delivery(session_id, seqs, prefix, answer))
}

fn open_mailbox_sidecar() -> Result<Option<MailboxDb>, String> {
    MailboxDb::open_default_if_exists()
}

fn empty_delivery(answer: Option<String>, session_id: String) -> PreparedMailboxDelivery {
    PreparedMailboxDelivery {
        answer,
        session_id,
        seqs: Vec::new(),
    }
}

fn has_pending_rows(rows: &[MailboxRow]) -> bool {
    !rows.is_empty()
}

fn batch_seqs(batch: &MailboxBatch) -> Vec<i64> {
    batch.rows.iter().map(|row| row.seq).collect()
}

fn prepared_delivery(
    session_id: String,
    seqs: Vec<i64>,
    prefix: String,
    answer: Option<String>,
) -> PreparedMailboxDelivery {
    PreparedMailboxDelivery {
        answer: Some(compose_answer(prefix, answer)),
        session_id,
        seqs,
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
    let mut rows = Vec::new();
    for row in pending.iter().take(MAILBOX_BATCH_MAX_ROWS) {
        rows.push(row.clone());
        let remaining_count = pending.len().saturating_sub(rows.len());
        if batch_exceeds_prefix_limit(&rows, remaining_count) {
            rows.pop();
            break;
        }
    }
    if rows.is_empty() && !pending.is_empty() {
        rows.push(pending[0].clone());
    }
    MailboxBatch {
        remaining_count: pending.len().saturating_sub(rows.len()),
        rows,
    }
}

fn batch_exceeds_prefix_limit(rows: &[MailboxRow], remaining_count: usize) -> bool {
    notification_prefix_len(rows, remaining_count) > MAILBOX_PREFIX_MAX_BYTES && rows.len() > 1
}

fn notification_prefix_len(rows: &[MailboxRow], remaining_count: usize) -> usize {
    render_notification_prefix(rows, remaining_count).len()
}

fn render_notification_prefix(rows: &[MailboxRow], remaining_count: usize) -> String {
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
    rendered.push_str("[END OULIPOLY NOTIFICATIONS]");
    rendered
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
        };

        assert_eq!(prepared.answer, original);
    }
}
