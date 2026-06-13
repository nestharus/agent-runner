//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_lifecycle_finalize_context.rs
//!     role: intrinsic-surface
//!     Domain: invocation-lifecycle-finalize-context-persistence
//!     Owns:
//!       - the StateDb invocation-lifecycle-finalize-context persistence surface this concern extends, split
//!         from the StateDb facade with the public API preserved
//!       - intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: StateDb, LifecycleInvocationRow, FinalizeLifecycleInput, lc_log_adapter (FinalizeContext, RawArtifactPaths), active_lifecycle_session_id
//! ```
//!
//! Invocation finalize lifecycle-log context mapping.

use super::*;

impl StateDb {
    pub(super) fn finalize_context(
        &self,
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        input: FinalizeLifecycleInput<'_>,
    ) -> lc_log_adapter::FinalizeContext {
        let row_invocation_uuid = Self::load_invocation_uuid_for_finalize(row);
        let fallback_invocation_uuid = Self::format_fallback_invocation_uuid(id);
        let invocation_uuid =
            Self::select_finalize_invocation_uuid(row_invocation_uuid, fallback_invocation_uuid);
        let session_id = Self::load_session_id_for_invocation(row);
        let chain_id_result = self.load_chain_id_for_invocation(id);
        let chain_id = Self::map_lifecycle_chain_id(chain_id_result);
        let raw_artifact_paths = self.load_raw_paths_for_finalize(&invocation_uuid);
        Self::build_finalize_context(
            id,
            row,
            invocation_uuid,
            session_id,
            chain_id,
            raw_artifact_paths,
            input,
        )
    }

    pub(super) fn load_invocation_uuid_for_finalize(
        row: Option<&LifecycleInvocationRow>,
    ) -> Option<String> {
        row.map(Self::clone_lifecycle_invocation_uuid)
    }

    pub(super) fn select_finalize_invocation_uuid(
        row_invocation_uuid: Option<String>,
        fallback_invocation_uuid: String,
    ) -> String {
        row_invocation_uuid.unwrap_or(fallback_invocation_uuid)
    }

    pub(super) fn clone_lifecycle_invocation_uuid(row: &LifecycleInvocationRow) -> String {
        row.invocation_uuid.clone()
    }

    pub(super) fn format_fallback_invocation_uuid(row_id: i64) -> String {
        format!("unresolved-invocation-row-{row_id}")
    }

    pub(super) fn load_session_id_for_invocation(
        row: Option<&LifecycleInvocationRow>,
    ) -> Option<String> {
        row.and_then(active_lifecycle_session_id)
    }

    pub(super) fn load_raw_paths_for_finalize(
        &self,
        invocation_uuid: &str,
    ) -> Option<lc_log_adapter::RawArtifactPaths> {
        self.raw_paths_for(invocation_uuid)
    }

    pub(super) fn build_finalize_context(
        id: i64,
        row: Option<&LifecycleInvocationRow>,
        invocation_uuid: String,
        session_id: Option<String>,
        chain_id: Option<String>,
        raw_artifact_paths: Option<lc_log_adapter::RawArtifactPaths>,
        input: FinalizeLifecycleInput<'_>,
    ) -> lc_log_adapter::FinalizeContext {
        lc_log_adapter::FinalizeContext {
            invocation_uuid,
            provider_source: row.and_then(|row| row.provider_name.clone()),
            chain_id,
            session_id,
            latency_us: 0,
            invocation_row_id: row.map(|_| id),
            terminal_status_attempt: input.terminal_status_attempt.to_string(),
            exit_code: input.exit_code,
            error_category: input.error_category.map(str::to_string),
            terminal_reason: input.terminal_reason.map(str::to_string),
            raw_artifact_paths,
            operation_result: input.operation_result,
        }
    }
}
