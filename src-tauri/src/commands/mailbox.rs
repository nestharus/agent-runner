//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`

use oulipoly_state::mailbox::{
    DeliveredPayloadCompactionReport, DeliveredPayloadCompactionStats, MailboxDb, MailboxRow,
    TerminalHistoryPruneReport, TerminalHistoryRetentionStats,
};
use serde::Serialize;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Serialize)]
struct MailboxListResponse {
    session_id: String,
    all: bool,
    rows: Vec<MailboxRow>,
}

#[derive(Debug, Serialize)]
struct MailboxShowResponse {
    session_id: String,
    row: MailboxRow,
    artifacts: Option<MailboxArtifacts>,
}

#[derive(Debug, Serialize)]
struct MailboxArtifacts {
    meta: String,
    log: String,
    rc: String,
}

#[derive(Debug, Serialize)]
struct MailboxStatusResponse {
    session_id: String,
    paused: bool,
    pending_count: usize,
    deliverable_count: usize,
    min_pending_seq: Option<i64>,
    max_pending_seq: Option<i64>,
}

#[derive(Debug, Serialize)]
struct MailboxAckResponse {
    session_id: String,
    from_seq: i64,
    to_seq: i64,
    acknowledged_count: usize,
    remaining_pending: usize,
}

#[derive(Debug, Serialize)]
struct MailboxCompactionResponse {
    applied: bool,
    limit: usize,
    before: DeliveredPayloadCompactionStats,
    report: Option<DeliveredPayloadCompactionReport>,
    after: DeliveredPayloadCompactionStats,
}

#[derive(Debug, Serialize)]
struct TerminalHistoryPruneResponse {
    applied: bool,
    vacuumed: bool,
    limit: usize,
    before: TerminalHistoryRetentionStats,
    report: Option<TerminalHistoryPruneReport>,
    after: TerminalHistoryRetentionStats,
}

pub(crate) fn run_list(session_id: &str, all: bool, json: bool) -> Result<i32, String> {
    let rows = list_rows(session_id, all)?;
    render_mailbox_list(session_id, all, rows, json)?;
    Ok(0)
}

pub(crate) fn run_search(
    session_id: &str,
    query: &str,
    all: bool,
    limit: usize,
    json: bool,
) -> Result<i32, String> {
    let query = query.to_lowercase();
    let rows = list_rows(session_id, all)?
        .into_iter()
        .filter(|row| mailbox_row_matches(row, &query))
        .take(limit)
        .collect();
    render_mailbox_list(session_id, all, rows, json)?;
    Ok(0)
}

pub(crate) fn run_show(
    session_id: &str,
    seq: Option<i64>,
    handle: Option<&str>,
    include_artifacts: bool,
    max_bytes: usize,
    json: bool,
) -> Result<i32, String> {
    let row = list_rows(session_id, true)?
        .into_iter()
        .find(|row| seq == Some(row.seq) || handle == Some(row.handle.as_str()))
        .ok_or_else(|| "Mailbox notification not found".to_string())?;
    let artifacts = include_artifacts
        .then(|| read_artifacts(&row, max_bytes))
        .transpose()?;
    let response = MailboxShowResponse {
        session_id: session_id.to_string(),
        row,
        artifacts,
    };
    if json {
        print_json(&response)?;
    } else {
        print_show_human(&response);
    }
    Ok(0)
}

pub(crate) fn run_status(session_id: &str, json: bool) -> Result<i32, String> {
    render_status(&mailbox_status(session_id)?, json)?;
    Ok(0)
}

pub(crate) fn run_pause(session_id: &str, paused: bool, json: bool) -> Result<i32, String> {
    let mut db = MailboxDb::open_default()?;
    db.set_notifications_paused(session_id, paused)?;
    drop(db);
    render_status(&mailbox_status(session_id)?, json)?;
    Ok(0)
}

pub(crate) fn run_ack(
    session_id: &str,
    from_seq: i64,
    to_seq: i64,
    delivered_by: &str,
    json: bool,
) -> Result<i32, String> {
    let mut db = MailboxDb::open_default()?;
    let acknowledged_count = db.acknowledge_range(session_id, from_seq, to_seq, delivered_by)?;
    let remaining_pending = db.list_pending(session_id)?.len();
    let response = MailboxAckResponse {
        session_id: session_id.to_string(),
        from_seq,
        to_seq,
        acknowledged_count,
        remaining_pending,
    };
    if json {
        print_json(&response)?;
    } else {
        println!(
            "session={} acknowledged={} range={}..={} remaining={}",
            response.session_id,
            response.acknowledged_count,
            response.from_seq,
            response.to_seq,
            response.remaining_pending
        );
    }
    Ok(0)
}

fn mailbox_status(session_id: &str) -> Result<MailboxStatusResponse, String> {
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(MailboxStatusResponse {
            session_id: session_id.to_string(),
            paused: false,
            pending_count: 0,
            deliverable_count: 0,
            min_pending_seq: None,
            max_pending_seq: None,
        });
    };
    let paused = db.notifications_paused(session_id)?;
    let pending = db.list_pending(session_id)?;
    let min_pending_seq = pending.first().map(|row| row.seq);
    let max_pending_seq = pending.last().map(|row| row.seq);
    let deliverable_count = crate::mailbox_delivery::deliverable_pending_count(session_id)?;
    Ok(MailboxStatusResponse {
        session_id: session_id.to_string(),
        paused,
        pending_count: pending.len(),
        deliverable_count,
        min_pending_seq,
        max_pending_seq,
    })
}

fn render_status(response: &MailboxStatusResponse, json: bool) -> Result<(), String> {
    if json {
        print_json(response)
    } else {
        println!(
            "session={} paused={} pending={} deliverable={} min_seq={} max_seq={}",
            response.session_id,
            response.paused,
            response.pending_count,
            response.deliverable_count,
            optional_seq(response.min_pending_seq),
            optional_seq(response.max_pending_seq)
        );
        Ok(())
    }
}

fn optional_seq(seq: Option<i64>) -> String {
    seq.map_or_else(|| "-".to_string(), |seq| seq.to_string())
}

fn mailbox_row_matches(row: &MailboxRow, query: &str) -> bool {
    row.seq.to_string().contains(query)
        || row.kind.to_lowercase().contains(query)
        || row.handle.to_lowercase().contains(query)
        || row.payload_json.to_lowercase().contains(query)
        || row.state_dir.to_lowercase().contains(query)
        || row.meta_path.to_lowercase().contains(query)
        || row.log_path.to_lowercase().contains(query)
        || row.rc_path.to_lowercase().contains(query)
}

fn read_artifacts(row: &MailboxRow, max_bytes: usize) -> Result<MailboxArtifacts, String> {
    Ok(MailboxArtifacts {
        meta: read_bounded(Path::new(&row.meta_path), max_bytes)?,
        log: read_bounded(Path::new(&row.log_path), max_bytes)?,
        rc: read_bounded(Path::new(&row.rc_path), max_bytes)?,
    })
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<String, String> {
    let file = File::open(path)
        .map_err(|err| format!("Failed to open mailbox artifact {}: {err}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Failed to read mailbox artifact {}: {err}", path.display()))?;
    let truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    let mut rendered = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        rendered.push_str("\n[truncated]");
    }
    Ok(rendered)
}

fn print_show_human(response: &MailboxShowResponse) {
    print_human_rows(std::slice::from_ref(&response.row));
    println!("payload={}", response.row.payload_json);
    if let Some(artifacts) = &response.artifacts {
        println!("\n[meta]\n{}", artifacts.meta);
        println!("\n[log]\n{}", artifacts.log);
        println!("\n[rc]\n{}", artifacts.rc);
    }
}

fn render_mailbox_list(
    session_id: &str,
    all: bool,
    rows: Vec<MailboxRow>,
    json: bool,
) -> Result<(), String> {
    if json {
        print_json(&MailboxListResponse {
            session_id: session_id.to_string(),
            all,
            rows,
        })?;
    } else {
        print_human_rows(&rows);
    }
    Ok(())
}

fn list_rows(session_id: &str, all: bool) -> Result<Vec<MailboxRow>, String> {
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        return Ok(Vec::new());
    };
    let mut rows = db.list_mailbox(session_id, all)?;
    for row in &mut rows {
        row.payload_json = db.payloads().hydrate_agent_bash_payload_json(row)?;
    }
    Ok(rows)
}

pub(crate) fn run_compact_delivered(limit: usize, apply: bool, json: bool) -> Result<i32, String> {
    let Some(db) = MailboxDb::open_default_if_exists()? else {
        let empty = DeliveredPayloadCompactionStats::default();
        render_compaction(
            MailboxCompactionResponse {
                applied: apply,
                limit,
                before: empty,
                report: apply.then(DeliveredPayloadCompactionReport::default),
                after: empty,
            },
            json,
        )?;
        return Ok(0);
    };
    let before = db.payloads().delivered_payload_compaction_stats()?;
    let report = apply
        .then(|| db.payloads().compact_delivered_payloads(limit))
        .transpose()?;
    let after = db.payloads().delivered_payload_compaction_stats()?;
    render_compaction(
        MailboxCompactionResponse {
            applied: apply,
            limit,
            before,
            report,
            after,
        },
        json,
    )?;
    Ok(0)
}

pub(crate) fn run_prune_terminal(
    limit: usize,
    apply: bool,
    vacuum: bool,
    json: bool,
) -> Result<i32, String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return render_terminal_prune(
            TerminalHistoryPruneResponse {
                applied: apply,
                vacuumed: false,
                limit,
                before: TerminalHistoryRetentionStats::default(),
                report: apply.then(TerminalHistoryPruneReport::default),
                after: TerminalHistoryRetentionStats::default(),
            },
            json,
        )
        .map(|()| 0);
    };
    let before = db.terminal_history_retention_stats()?;
    let report = apply
        .then(|| prune_terminal_history_bounded(&mut db, limit))
        .transpose()?;
    if vacuum {
        db.vacuum_terminal_history()?;
    }
    let after = db.terminal_history_retention_stats()?;
    render_terminal_prune(
        TerminalHistoryPruneResponse {
            applied: apply,
            vacuumed: vacuum,
            limit,
            before,
            report,
            after,
        },
        json,
    )?;
    Ok(0)
}

fn prune_terminal_history_bounded(
    db: &mut MailboxDb,
    limit: usize,
) -> Result<TerminalHistoryPruneReport, String> {
    const BATCH_SIZE: usize = 4_096;
    let mut report = TerminalHistoryPruneReport::default();
    loop {
        let pruned = report
            .mailbox_rows_deleted
            .max(report.delivery_attempts_deleted)
            .max(report.payload_files_deleted);
        let batch_limit = limit.saturating_sub(pruned).min(BATCH_SIZE);
        if batch_limit == 0 {
            break;
        }
        let batch = db.prune_terminal_history(batch_limit)?;
        let made_progress = batch.mailbox_rows_deleted > 0
            || batch.delivery_attempts_deleted > 0
            || batch.payload_files_deleted > 0;
        merge_terminal_prune_report(&mut report, batch);
        if !made_progress {
            break;
        }
    }
    Ok(report)
}

fn merge_terminal_prune_report(
    report: &mut TerminalHistoryPruneReport,
    batch: TerminalHistoryPruneReport,
) {
    report.mailbox_rows_deleted += batch.mailbox_rows_deleted;
    report.listeners_detached += batch.listeners_detached;
    report.delivery_attempts_deleted += batch.delivery_attempts_deleted;
    report.delivery_attempt_items_deleted += batch.delivery_attempt_items_deleted;
    report.payload_files_deleted += batch.payload_files_deleted;
    report.payload_bytes_reclaimed += batch.payload_bytes_reclaimed;
}

fn render_terminal_prune(response: TerminalHistoryPruneResponse, json: bool) -> Result<(), String> {
    if json {
        return print_json(&response);
    }
    let report = response.report.unwrap_or_default();
    println!(
        "applied={} vacuumed={} limit={} eligible_mailbox_rows={} eligible_attempts={} reclaimable_payloads={} mailbox_rows_deleted={} attempts_deleted={} attempt_items_deleted={} listeners_detached={} payload_files_deleted={} payload_bytes_reclaimed={} remaining_mailbox_rows={} remaining_attempts={} remaining_payloads={}",
        response.applied,
        response.vacuumed,
        response.limit,
        response.before.prunable_mailbox_rows,
        response.before.prunable_delivery_attempts,
        response.before.reclaimable_payload_files,
        report.mailbox_rows_deleted,
        report.delivery_attempts_deleted,
        report.delivery_attempt_items_deleted,
        report.listeners_detached,
        report.payload_files_deleted,
        report.payload_bytes_reclaimed,
        response.after.prunable_mailbox_rows,
        response.after.prunable_delivery_attempts,
        response.after.reclaimable_payload_files,
    );
    Ok(())
}

fn render_compaction(response: MailboxCompactionResponse, json: bool) -> Result<(), String> {
    if json {
        return print_json(&response);
    }
    println!(
        "applied={} limit={} eligible_rows={} inline_bytes={} compacted_rows={} reclaimed_bytes={} remaining_rows={} remaining_inline_bytes={}",
        response.applied,
        response.limit,
        response.before.eligible_rows,
        response.before.inline_bytes,
        response
            .report
            .map(|report| report.compacted_rows)
            .unwrap_or(0),
        response
            .report
            .map(|report| report.inline_bytes_reclaimed)
            .unwrap_or(0),
        response.after.eligible_rows,
        response.after.inline_bytes,
    );
    Ok(())
}

fn print_human_rows(rows: &[MailboxRow]) {
    for row in rows {
        println!(
            "seq={} kind={} handle={} target_kind={} target_id={} rc={} delivered={} error={} payload={} sha256={} bytes={} retention={}",
            row.seq,
            row.kind,
            row.handle,
            row.target_kind.as_deref().unwrap_or("-"),
            row.target_id.as_deref().unwrap_or("-"),
            row.rc,
            row.delivered_at.as_deref().unwrap_or("-"),
            row.delivery_error.as_deref().unwrap_or("-"),
            row.payload_file_path.as_deref().unwrap_or("-"),
            row.payload_sha256.as_deref().unwrap_or("-"),
            row.payload_byte_len
                .map(|length| length.to_string())
                .as_deref()
                .unwrap_or("-"),
            row.payload_retention_policy.as_deref().unwrap_or("-"),
        );
    }
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|err| format!("Failed to serialize mailbox JSON: {err}"))?;
    println!("{rendered}");
    Ok(())
}
