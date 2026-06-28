//! Declared roles: orchestration, accessor

use oulipoly_state::{ImportedSessionListRow, ReadOnlyOpenError, StateDb};

pub(crate) fn run_session_list(json: bool) -> Result<i32, String> {
    let rows = load_session_list_rows()?;
    super::formatter::render_session_list(&rows, json)?;
    Ok(0)
}

fn load_session_list_rows() -> Result<Vec<ImportedSessionListRow>, String> {
    let path = StateDb::default_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let state = StateDb::open_read_only(&path).map_err(format_session_list_open_error)?;
    state
        .imported_session_list()
        .map_err(super::formatter::format_session_list_load_error)
}

fn format_session_list_open_error(error: ReadOnlyOpenError) -> String {
    format!("Failed to open state DB read-only: {error:?}")
}
