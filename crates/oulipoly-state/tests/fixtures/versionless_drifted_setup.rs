use super::v3_setup_only_db::build_setup_only_db;
use std::path::Path;

pub fn build_versionless_drifted_setup_db(path: &Path) {
    build_setup_only_db(path, 0, false);
}
