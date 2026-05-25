//! Declared role: validator

pub(super) fn validate_migrate_rebuild_flag(rebuild: bool) -> Result<(), String> {
    if rebuild {
        Ok(())
    } else {
        Err("missing required flag: --rebuild".to_string())
    }
}
