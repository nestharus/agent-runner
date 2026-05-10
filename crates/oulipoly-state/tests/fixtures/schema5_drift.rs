use super::schema4_invocations::build_schema4_invocation_fixture;
use super::schema5_invocations::apply_schema5_dual_id_shape;
use rusqlite::Connection;
use std::path::Path;

pub fn build_schema4_with_dual_id_columns_or_index_drift(path: &Path) {
    build_schema4_invocation_fixture(path);
    let conn = Connection::open(path).unwrap();
    apply_schema5_dual_id_shape(&conn);
    conn.pragma_update(None, "user_version", 4).unwrap();
}

pub fn build_current_missing_resume_input_id_column(path: &Path) {
    build_schema4_invocation_fixture(path);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "
        ALTER TABLE invocations ADD COLUMN provider_session_id TEXT;
        ALTER TABLE invocations ADD COLUMN provider_session_capture_method TEXT;
        UPDATE invocations
           SET provider_session_id = session_id,
               provider_session_capture_method = session_capture_method
         WHERE session_id IS NOT NULL
           AND (session_capture_method IS NULL OR session_capture_method <> 'resumed');
        ",
    )
    .unwrap();
    conn.pragma_update(None, "user_version", 5).unwrap();
}

pub fn build_schema4_with_existing_provider_session_index(path: &Path) {
    build_schema4_invocation_fixture(path);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "
        ALTER TABLE invocations ADD COLUMN provider_session_id TEXT;
        ALTER TABLE invocations ADD COLUMN resume_input_id TEXT;
        ALTER TABLE invocations ADD COLUMN provider_session_capture_method TEXT;
        CREATE INDEX idx_invocations_provider_provider_session
            ON invocations(provider_name, provider_index, provider_session_id)
            WHERE provider_session_id IS NOT NULL;
        ",
    )
    .unwrap();
    conn.pragma_update(None, "user_version", 4).unwrap();
}

pub fn build_failing_0005_duplicate_column_or_index(path: &Path) {
    build_schema4_invocation_fixture(path);
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("ALTER TABLE invocations ADD COLUMN provider_session_id TEXT;")
        .unwrap();
    conn.pragma_update(None, "user_version", 4).unwrap();
}
