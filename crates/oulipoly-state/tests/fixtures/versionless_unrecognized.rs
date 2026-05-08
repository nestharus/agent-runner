use super::open;
use std::path::Path;

pub fn build_versionless_unrecognized_db(path: &Path) {
    let conn = open(path);
    conn.execute_batch(
        "
        PRAGMA user_version = 0;
        CREATE TABLE unrelated_application_table (
            id INTEGER PRIMARY KEY,
            payload TEXT NOT NULL
        );
        INSERT INTO unrelated_application_table (payload) VALUES ('must not be mutated');
        ",
    )
    .unwrap();
}
