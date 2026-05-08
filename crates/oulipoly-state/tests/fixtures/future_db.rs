use super::{create_full_state_schema, open};
use oulipoly_state::schema::CURRENT_SCHEMA_VERSION;
use std::path::Path;

pub fn build_future_db(path: &Path) {
    let conn = open(path);
    create_full_state_schema(&conn, CURRENT_SCHEMA_VERSION + 1);
}
