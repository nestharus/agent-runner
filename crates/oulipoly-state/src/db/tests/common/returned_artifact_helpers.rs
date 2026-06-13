//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//!
//! Role set: { accessor, formatter, mapper }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/common/returned_artifact_helpers.rs
//!     role: intrinsic-surface
//!     Domain: returned-artifact-helpers-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::super::*;
use super::*;
pub(in crate::db::tests) fn returned_artifact_ref(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> ReturnedArtifactRef {
    let workflow_run_id = returned_artifact_workflow_run_id(invocation_uuid);
    let version_id = returned_artifact_version_id(invocation_uuid, artifact_name, version);
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

fn returned_artifact_workflow_run_id(invocation_uuid: Uuid) -> String {
    format!("return:{invocation_uuid}")
}

fn returned_artifact_version_id(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> String {
    format!("store://return/{invocation_uuid}/{artifact_name}/{version}")
}
