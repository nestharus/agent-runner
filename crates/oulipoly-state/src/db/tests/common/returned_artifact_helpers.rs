//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//!
//! Role set: { accessor, formatter, mapper }

use super::super::*;
use super::*;
pub(in crate::db::tests) fn returned_artifact_ref(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> ReturnedArtifactRef {
    let workflow_run_id = format!("return:{invocation_uuid}");
    let version_id = format!("store://return/{invocation_uuid}/{artifact_name}/{version}");
    ReturnedArtifactRef {
        version_id,
        name: artifact_name.to_string(),
        store_address: oulipoly_agent_messenger::StoreAddress {
            workflow_run_id,
            artifact_name: artifact_name.to_string(),
            version,
        },
        sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        content_len: 123,
        format_hint: Some("text/plain".to_string()),
        verdict_line: Some("ok".to_string()),
        source: oulipoly_agent_messenger::ReturnedArtifactSource::Scratchpad {
            name: "notes".to_string(),
            version: 1,
        },
        producer_invocation_uuid: invocation_uuid,
        returned_at: ts("2026-04-17T08:00:00Z"),
    }
}
