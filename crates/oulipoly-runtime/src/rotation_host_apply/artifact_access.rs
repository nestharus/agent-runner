//! ## Declared roles
//! accessor

pub(super) fn read_artifact_bytes(path: &str) -> Result<Vec<u8>, std::io::Error> {
    std::fs::read(path)
}

pub(super) fn expected_rotation_artifact_sha256(
    artifact: &oulipoly_provider::generated::Artifact,
) -> Option<&str> {
    artifact.sha256.as_deref()
}

pub(super) fn required_rotation_artifact_path(
    artifact: &oulipoly_provider::generated::Artifact,
) -> Result<&str, String> {
    artifact
        .path
        .as_deref()
        .ok_or_else(super::error_formatter::artifact_path_required_for_sha256)
}
