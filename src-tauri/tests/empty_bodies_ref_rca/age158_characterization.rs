use rusqlite::params;
use std::path::Path;

use super::{PROVIDER, RcaFixture, SESSION_ID, TS_USER, sh_path};

#[test]
fn age158_sh_path_escapes_embedded_single_quotes() {
    assert_eq!(
        sh_path(Path::new("alpha' beta/gamma'")),
        r#"'alpha'\'' beta/gamma'\'''"#
    );
}

#[test]
fn age158_fetch_turn_body_reports_missing_row_shape() {
    let fixture = RcaFixture::new();

    assert_eq!(
        fixture.fetch_turn_body("missing-turn").unwrap_err(),
        "body column must be queryable from session_turns: Query returned no rows"
    );
}

#[test]
fn age158_fetch_turn_body_reports_invalid_json_shape() {
    let fixture = RcaFixture::new();
    fixture
        .conn()
        .execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role,
                 parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
             VALUES (?1, ?2, 'invalid-json-turn', ?3, 'assistant', NULL, 0, 0, '', ?3, ?4)",
            params![PROVIDER, SESSION_ID, TS_USER, "not-json"],
        )
        .unwrap();

    assert_eq!(
        fixture.fetch_turn_body("invalid-json-turn").unwrap_err(),
        "body JSON must parse: expected ident at line 1 column 2"
    );
}
