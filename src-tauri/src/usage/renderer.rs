use crate::usage::row::{RowState, UsageRow, UsageWindow};
use std::io::{self, Write};

pub(crate) fn render(rows: &[UsageRow], writer: &mut impl Write) -> io::Result<()> {
    let mut table_rows = Vec::new();
    table_rows.push([
        "Account".to_string(),
        "Vendor".to_string(),
        "Window".to_string(),
        "Used / Limit".to_string(),
        "Remaining".to_string(),
    ]);

    for row in rows {
        match &row.row_state {
            RowState::HasWindows => {
                for window in &row.windows {
                    table_rows.push([
                        row.account_id.clone(),
                        row.vendor.clone(),
                        window_label(window),
                        format_used_limit(window),
                        format_remaining(window),
                    ]);
                }
            }
            RowState::NoUsageApi => table_rows.push(state_row(row, "(no usage api)")),
            RowState::Error(message) => {
                table_rows.push(state_row(row, &format!("(error: {message})")))
            }
            RowState::InFlight => table_rows.push(state_row(row, "(in flight)")),
            RowState::NoWindows => table_rows.push(state_row(row, "(no windows)")),
        }
    }

    let widths = column_widths(&table_rows);
    for row in table_rows {
        writeln!(
            writer,
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        )?;
    }
    Ok(())
}

fn state_row(row: &UsageRow, label: &str) -> [String; 5] {
    [
        row.account_id.clone(),
        row.vendor.clone(),
        label.to_string(),
        "-".to_string(),
        "-".to_string(),
    ]
}

fn window_label(window: &UsageWindow) -> String {
    window.label.clone()
}

fn format_used_limit(window: &UsageWindow) -> String {
    match window.limit {
        Some(limit) => {
            let used = (window.used_percent * limit as f64).round() as u64;
            append_unit(&format!("{used} / {limit}"), window.unit.as_deref())
        }
        None => format!("{} / 100%", format_percent(window.used_percent)),
    }
}

fn format_remaining(window: &UsageWindow) -> String {
    match window.remaining {
        Some(remaining) => append_unit(&remaining.to_string(), window.unit.as_deref()),
        None => format_percent(1.0 - window.used_percent),
    }
}

fn append_unit(value: &str, unit: Option<&str>) -> String {
    match unit {
        Some(unit) if !unit.is_empty() => format!("{value} {unit}"),
        _ => value.to_string(),
    }
}

fn format_percent(value: f64) -> String {
    let pct = (value * 100.0).clamp(0.0, 100.0);
    if (pct - pct.round()).abs() < 1e-6 {
        format!("{:.0}%", pct)
    } else {
        format!("{pct:.1}%")
    }
}

fn column_widths(rows: &[[String; 5]]) -> [usize; 5] {
    let mut widths = [0; 5];
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            widths[index] = widths[index].max(value.len());
        }
    }
    widths
}
