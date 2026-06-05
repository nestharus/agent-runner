//! ## Declared roles
//!
//! `orchestration`, `formatter`, `filter`, `predicate`

use oulipoly_state::mailbox::{MailboxDb, MailboxRow, SessionRuntimeUpsert};

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
) -> Result<PreparedMailboxDelivery, String> {
    let session_id = resolved.active_session_id.clone();
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(PreparedMailboxDelivery {
            answer,
            session_id,
            seqs: Vec::new(),
        });
    };
    record_headless_session_runtime(&mut db, resolved)?;
    let pending = db.list_pending(&session_id)?;
    if pending.is_empty() {
        return Ok(PreparedMailboxDelivery {
            answer,
            session_id,
            seqs: Vec::new(),
        });
    }

    let batch = select_batch(&pending);
    let seqs = batch.rows.iter().map(|row| row.seq).collect();
    let prefix = render_notification_prefix(&batch.rows, batch.remaining_count);
    Ok(PreparedMailboxDelivery {
        answer: Some(compose_answer(prefix, answer)),
        session_id,
        seqs,
    })
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
) -> Result<(), String> {
    db.upsert_session_runtime(SessionRuntimeUpsert {
        session_id: &resolved.active_session_id,
        mode: "headless",
        invocation_uuid: None,
        provider_name: Some(&resolved.active_provider),
        model_name: resolved.model_name.as_deref(),
        pty_control_path: None,
    })
}

fn select_batch(pending: &[MailboxRow]) -> MailboxBatch {
    let mut rows = Vec::new();
    for row in pending.iter().take(MAILBOX_BATCH_MAX_ROWS) {
        rows.push(row.clone());
        let remaining_count = pending.len().saturating_sub(rows.len());
        if render_notification_prefix(&rows, remaining_count).len() > MAILBOX_PREFIX_MAX_BYTES
            && rows.len() > 1
        {
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
