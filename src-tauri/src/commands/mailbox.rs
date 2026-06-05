//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`

use oulipoly_state::mailbox::{MailboxDb, MailboxRow};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct MailboxListResponse {
    session_id: String,
    all: bool,
    rows: Vec<MailboxRow>,
}

pub(crate) fn run_list(session_id: &str, all: bool, json: bool) -> Result<i32, String> {
    let rows = list_rows(session_id, all)?;
    render_mailbox_list(session_id, all, rows, json)?;
    Ok(0)
}

fn render_mailbox_list(
    session_id: &str,
    all: bool,
    rows: Vec<MailboxRow>,
    json: bool,
) -> Result<(), String> {
    if json {
        print_json(&mailbox_list_response(session_id, all, rows))?;
    } else {
        print_human_rows(&rows);
    }
    Ok(())
}

fn mailbox_list_response(
    session_id: &str,
    all: bool,
    rows: Vec<MailboxRow>,
) -> MailboxListResponse {
    MailboxListResponse {
        session_id: session_id.to_string(),
        all,
        rows,
    }
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
            "seq={} kind={} handle={} rc={} delivered={}",
            row.seq,
            row.kind,
            row.handle,
            row.rc,
            row.delivered_at.as_deref().unwrap_or("-")
        );
    }
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|err| format!("Failed to serialize mailbox list JSON: {err}"))?;
    println!("{rendered}");
    Ok(())
}
