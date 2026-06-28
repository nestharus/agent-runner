//! Declared roles: formatter, predicate, mapper

use oulipoly_state::ImportedSessionListRow;

const SESSION_LIST_HEADER: &str = "CHAIN_ID\tACTIVE_PROVIDER\tACTIVE_PROVIDER_SESSION_ID\tTITLE\tCWD\tLAST_USED_OR_UPDATED_AT\tTURN_COUNT\tIS_IMPORTED";

pub(super) fn render_session_list(
    rows: &[ImportedSessionListRow],
    json: bool,
) -> Result<(), String> {
    if json {
        render_session_list_json(rows)
    } else {
        render_session_list_table(rows);
        Ok(())
    }
}

fn render_session_list_json(rows: &[ImportedSessionListRow]) -> Result<(), String> {
    serde_json::to_writer(std::io::stdout(), rows).map_err(format_session_list_json_error)?;
    println!();
    Ok(())
}

fn render_session_list_table(rows: &[ImportedSessionListRow]) {
    if rows.is_empty() {
        println!("No sessions found");
        return;
    }
    println!("{SESSION_LIST_HEADER}");
    for row in rows {
        println!("{}", format_session_list_row(row));
    }
}

pub(super) fn format_session_list_row(row: &ImportedSessionListRow) -> String {
    [
        cell(&row.chain_id),
        cell(&row.active_provider),
        cell(&row.active_provider_session_id),
        optional_cell(row.title.as_deref()),
        optional_cell(row.cwd.as_deref()),
        row.last_used_or_updated_at.to_rfc3339(),
        row.turn_count.to_string(),
        imported_cell(row.is_imported),
    ]
    .join("\t")
}

fn cell(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch == '\t' || ch == '\n' || ch == '\r' {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

fn optional_cell(value: Option<&str>) -> String {
    value.map(cell).unwrap_or_else(|| "-".to_string())
}

fn imported_cell(is_imported: bool) -> String {
    if is_imported { "yes" } else { "no" }.to_string()
}

pub(super) fn format_session_list_load_error(error: impl std::fmt::Display) -> String {
    format!("Failed to list sessions: {error}")
}

fn format_session_list_json_error(error: serde_json::Error) -> String {
    format!("Failed to serialize session list: {error}")
}
