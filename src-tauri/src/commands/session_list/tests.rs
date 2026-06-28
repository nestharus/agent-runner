use super::formatter::format_session_list_row;
use chrono::{TimeZone, Utc};
use oulipoly_state::ImportedSessionListRow;

#[test]
fn session_list_row_formatter_sanitizes_cells_and_marks_import_state() {
    let row = ImportedSessionListRow {
        chain_id: "chain\none".to_string(),
        active_provider: "provider-a".to_string(),
        active_provider_session_id: "native\tone".to_string(),
        title: Some("Title\rOne".to_string()),
        cwd: None,
        last_used_or_updated_at: Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
        turn_count: 3,
        is_imported: true,
    };

    assert_eq!(
        format_session_list_row(&row),
        "chain one\tprovider-a\tnative one\tTitle One\t-\t2026-06-01T00:00:00+00:00\t3\tyes"
    );
}
