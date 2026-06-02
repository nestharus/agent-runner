//! ## Declared roles
//! formatter

pub(super) fn target_active_conflict_message(conflict: &str) -> String {
    format!("target segment is active in chain {conflict}")
}

pub(super) fn required_field(field: &str) -> String {
    format!("{field} is required")
}

pub(super) fn snapshot_read_error(error: String) -> String {
    format!("failed to load chain segment snapshot: {error}")
}

pub(super) fn artifact_read_error(path: &str, error: std::io::Error) -> String {
    format!("failed to read host_state_plan artifact {path}: {error}")
}

pub(super) fn artifact_read_failed(error: std::io::Error) -> String {
    format!("failed to read artifact: {error}")
}

pub(super) fn artifact_path_required_for_sha256() -> String {
    "artifact path is required when sha256 is declared".to_string()
}

pub(super) fn artifact_sha256_mismatch() -> String {
    "artifact sha256 mismatch".to_string()
}

pub(super) fn host_state_plan_artifact_sha256_mismatch() -> String {
    "host_state_plan artifact sha256 mismatch".to_string()
}

pub(super) fn field_mismatch(field: &str) -> String {
    format!("{field} mismatch")
}
