//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`

use oulipoly_state::mailbox::{MailboxDb, MailboxRow};
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
    db.list_mailbox(session_id, all)
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
