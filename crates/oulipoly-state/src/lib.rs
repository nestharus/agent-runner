//! ## Declared roles
//!
//! - accessor
//! - validator
//!
//! Role set: { accessor, validator }
//!
//! Per ACR-249/ACR-250 lib.rs is the declared root-API carrier with a cohesion role set
//! bounded by re-export consumer policy and doctest compile-fail validation. Intrinsic-surface
//! declaration `oulipoly_state_root_compatibility_api` covers the documented root-public DB
//! compatibility facade; see `the AGE-160 proposal § Intrinsic-surface declarations`.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/lib.rs
//!     role: intrinsic-surface
//!     Domain: oulipoly_state_root_compatibility_api
//!     Owns:
//!       - the crate root-public DB compatibility facade: every `pub use db::...`
//!         re-export (StateDb, DbError, LegacyProviderNames, ProviderRecord,
//!         InvocationRecord/Start/Status, QuotaRecord/Window, SessionTurn*,
//!         ResumeInputMatch, ResumeNativeCandidate, ReadOnlyOpenError, and the
//!         remaining db concern surface) is the
//!         documented stable consumer API this root carrier owns and re-exports
//!         unchanged from the decomposed `db` module tree
//!       - the root-public typed continuation DTO and repository error module
//!       - root-public deployment, invocation_marker, mailbox, migrations, paths,
//!         pid_identity, repositories, result_envelope, schema, and schema_probe modules
//! ```

mod chain_segments;
pub mod continuation;
mod db;
pub mod deployment;
pub mod invocation_marker;
mod lifecycle_log;
pub mod mailbox;
pub mod migrations;
pub mod paths;
pub mod pid_identity;
mod read_only_snapshot;
pub mod repositories;
pub mod result_envelope;
pub mod schema;
pub mod schema_probe;

pub type StateDbError = String;

pub use crate::schema::{CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION};
pub use chain_segments::ChainSegmentRow;
pub use db::DbError;
pub use db::LegacyProviderNames;
pub use db::ProviderRecord;
pub use db::ReadOnlyOpenError;
pub use db::SessionTurnCounts;
pub use db::SessionTurnIngest;
pub use db::StateDb;
pub use db::StateDbRebuildAuthority;
pub use db::StateDbWriterAuthority;
pub use db::StateReadConnection;
pub use db::{AccountRecord, AuthMethod, AuthStatus, CliProviderRecord};
pub use db::{
    AcknowledgementStage, AcknowledgementWrite, DeliveryAcknowledgement, DeliveryEvidence,
    DeliveryEvidenceKind, DispositionWrite, EventDisposition, ExactProcessIdentity,
    ExternalIngress, ExternalIngressWrite, LeaseAcquire, LeaseReplace, LifecycleEvent,
    NewLifecycleEvent, ProviderTurnGeneration, SessionLifecycleError, SessionLifecycleRepository,
    SessionLifecycleResult, SessionReconstruction, SupervisorFence, SupervisorLease, TurnFence,
    TurnState,
};
pub use db::{
    ActiveChainSegmentSnapshot, ChainSegmentRotationInput, QuotaRecord, QuotaWindow,
    QuotaWindowInput,
};
pub use db::{
    BackfillReport, ChainPreview, ModelStore, ProviderSessionBinding, RESUME_INPUT_MAX_LEN,
    ResolvedResume, ResumeError, ResumeInputMatch, ResumeNativeCandidate, SessionMarkerPayload,
    TurnPreview, WrongIdKindInput,
};
pub use db::{CliMapping, DiscoveredModel, ModelParameter, ParamType};
pub use db::{CompactSummaryEvidence, OwnedTurnEvent, OwnedTurnEventRow};
pub use db::{
    CompletionObligationAdmission, CompletionObligationAdmissionResult,
    CompletionObligationAuthority, CompletionObligationExpectation, EffectiveTerminalDisposition,
    ListenerSettlementClass, OwnedCompletionEventState, OwnerLineageRelationship,
    OwnershipAuthorityError, OwnershipAuthoritySnapshot, RecoveryDisposition,
    SettlementVerifierIdentity, SidecarGenerationState,
};
pub use db::{
    ImportedSessionDisplayMetadata, ImportedSessionDisplayMetadataUpsert, ImportedSessionListRow,
};
pub use db::{InvocationRecord, InvocationStart, InvocationStatus};
pub use db::{ProviderTurnEffectInput, ProviderTurnEffectWrite};
pub use db::{
    SessionTurnReplacement, SessionTurnRestoreRow, SessionTurnsReplacement, SessionTurnsRestore,
};
pub use invocation_marker::CompositeInvocationId;
pub use lifecycle_log::{LifecycleEventSink, NoopLifecycleEventSink};
pub use mailbox::{
    InboxTarget, InboxTargetKind, MAILBOX_PAYLOAD_RETENTION_POLICY, PublishedMailboxPayload,
    SUBMITTED_INPUT_KIND, SubmittedInputEnqueue, submitted_input_handle,
};
pub use result_envelope::{
    ResultEnvelopeFailureIdentity, ResultEnvelopeInput, result_envelope_payload,
};

#[cfg(doctest)]
pub mod age_32_connection_boundary_doctest {
    /// ```compile_fail
    /// use oulipoly_state::StateDb;
    ///
    /// let mut state = StateDb::open_default()?;
    /// let raw_connection = state.into_connection();
    /// let _escaped: rusqlite::Connection = raw_connection;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// This fails specifically at the forbidden method call if a default state
    /// DB can be opened; open failure is not hidden behind `?` conversion.
    ///
    /// ```compile_fail
    /// use oulipoly_state::StateDb;
    ///
    /// let state = StateDb::open_default().unwrap();
    /// let raw_connection = state.into_connection();
    /// let _escaped: rusqlite::Connection = raw_connection;
    /// ```
    ///
    /// ```compile_fail
    /// use oulipoly_state::StateDb;
    ///
    /// let mut state = StateDb::open_default()?;
    /// let escaped: &mut rusqlite::Connection = state.connection_mut();
    /// escaped.execute_batch("CREATE TABLE bypass (id INTEGER)")?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// This fails specifically at the forbidden method call if a default state
    /// DB can be opened; open failure is not hidden behind `?` conversion.
    ///
    /// ```compile_fail
    /// use oulipoly_state::StateDb;
    ///
    /// let mut state = StateDb::open_default().unwrap();
    /// let escaped: &mut rusqlite::Connection = state.connection_mut();
    /// escaped.execute_batch("CREATE TABLE bypass (id INTEGER)").unwrap();
    /// ```
    ///
    /// ```compile_fail
    /// use oulipoly_state::StateDb;
    ///
    /// let state = StateDb::open_default().unwrap();
    /// state.connection().execute_batch(
    ///     "DROP TRIGGER trg_invocation_completion_obligations_append_only_delete;
    ///      DELETE FROM invocation_completion_obligations;",
    /// ).unwrap();
    /// ```
    ///
    /// ```compile_fail
    /// use oulipoly_state::StateDb;
    ///
    /// let mut state = StateDb::open_default().unwrap();
    /// state.with_write_txn(|tx| {
    ///     tx.execute_batch(
    ///         "DROP TRIGGER trg_invocation_completion_obligations_append_only_delete;
    ///          DELETE FROM invocation_completion_obligations;",
    ///     ).map_err(|error| error.to_string())
    /// }).unwrap();
    /// ```
    ///
    /// ```compile_fail
    /// use oulipoly_state::{CompletionObligationAdmission, StateDb};
    ///
    /// let state = StateDb::open_default().unwrap();
    /// state.record_completion_obligation(CompletionObligationAdmission {
    ///     admission_id: "forbidden-state-only-admission",
    ///     invocation_uuid: "11111111-1111-4111-8111-111111111111",
    ///     event_id: "forbidden-event",
    ///     owner_invocation_uuid: "11111111-1111-4111-8111-111111111111",
    ///     owner_session_id: "forbidden-session",
    ///     expected_sidecar_generation: "22222222-2222-4222-8222-222222222222",
    /// }).unwrap();
    /// ```
    pub struct StateDbRawConnectionEscapeMustNotCompile;
}

#[cfg(test)]
mod age160_root_reexport_tests {
    /// AGE-160 risk: A6 lib.rs↔db root compatibility facade.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track.
    ///
    /// Documented consumer reason: the kept root DB symbols are consumed by
    /// runtime, Tauri, setup, examples, integration tests, or exposed by
    /// public StateDb method signatures per the AGE-160 entrypoints inventory.
    #[test]
    fn age160_root_reexport_documented_consumers_db_surface() {
        let root_type_names = [
            std::any::type_name::<crate::StateDb>(),
            std::any::type_name::<crate::ReadOnlyOpenError>(),
            std::any::type_name::<crate::CompositeInvocationId>(),
            std::any::type_name::<crate::InvocationStart>(),
            std::any::type_name::<crate::InvocationRecord>(),
            std::any::type_name::<crate::InvocationStatus>(),
            std::any::type_name::<crate::QuotaRecord>(),
            std::any::type_name::<crate::QuotaWindow>(),
            std::any::type_name::<crate::QuotaWindowInput>(),
            std::any::type_name::<crate::SessionTurnIngest>(),
            std::any::type_name::<crate::ChainPreview>(),
            std::any::type_name::<crate::ModelStore>(),
            std::any::type_name::<crate::ProviderSessionBinding>(),
            std::any::type_name::<crate::ResolvedResume>(),
            std::any::type_name::<crate::ResumeError>(),
            std::any::type_name::<crate::ResumeInputMatch>(),
            std::any::type_name::<crate::ResumeNativeCandidate>(),
            std::any::type_name::<crate::SessionMarkerPayload>(),
            std::any::type_name::<crate::TurnPreview>(),
            std::any::type_name::<crate::AccountRecord>(),
            std::any::type_name::<crate::AuthMethod>(),
            std::any::type_name::<crate::AuthStatus>(),
            std::any::type_name::<crate::CliProviderRecord>(),
            std::any::type_name::<crate::BackfillReport>(),
            std::any::type_name::<crate::WrongIdKindInput>(),
            std::any::type_name::<crate::CliMapping>(),
            std::any::type_name::<crate::DiscoveredModel>(),
            std::any::type_name::<crate::ModelParameter>(),
            std::any::type_name::<crate::ParamType>(),
            std::any::type_name::<crate::CompactSummaryEvidence>(),
            std::any::type_name::<crate::OwnedTurnEventRow>(),
            std::any::type_name::<crate::ImportedSessionListRow>(),
            std::any::type_name::<dyn crate::LifecycleEventSink>(),
            std::any::type_name::<crate::NoopLifecycleEventSink>(),
        ];

        assert!(
            root_type_names.iter().all(|name| !name.is_empty()),
            "root compatibility symbols must resolve through crate::<symbol>: {root_type_names:#?}"
        );
        let _: i32 = crate::CURRENT_SCHEMA_VERSION;
        let _: i32 = crate::MINIMUM_SUPPORTED_SCHEMA_VERSION;
    }

    /// AGE-160 risk: A6 lib.rs↔deployment and lib.rs↔schema_probe root narrowing.
    /// Selected level: unit.
    /// Source: the AGE-160 proposal § Test-intent track; validates A8/A9.
    ///
    /// Rust cannot assert a non-resolvable root path from a normal unit test
    /// without adding a compile-fail harness. This structural assertion is the
    /// Step 6b contract: root `pub use` lines are absent while module-qualified
    /// public paths continue to compile.
    #[test]
    fn age160_root_reexport_documented_consumers_deployment_and_schema_probe_removed_or_module_qualified()
     {
        let lib_rs = include_str!("lib.rs");
        let deployment_root_reexport = concat!("pub use deployment", "::");
        let schema_probe_root_reexport = concat!("pub use schema_probe", "::");
        let crate_schema_probe_root_reexport = concat!("pub use crate::schema_probe", "::");
        assert!(
            !lib_rs.contains(deployment_root_reexport),
            "deployment types must be reached through crate::deployment::<symbol>, not root re-exports"
        );
        assert!(
            !lib_rs.contains(crate_schema_probe_root_reexport)
                && !lib_rs.contains(schema_probe_root_reexport),
            "schema-probe DTOs must be reached through crate::schema_probe::<symbol>, not root re-exports"
        );

        let deployment_paths = [
            std::any::type_name::<crate::deployment::DbRole>(),
            std::any::type_name::<crate::deployment::DeploymentAwareOpener>(),
            std::any::type_name::<dyn crate::deployment::DeploymentMetadataStore>(),
            std::any::type_name::<crate::deployment::DeploymentRoutingDecision>(),
            std::any::type_name::<dyn crate::deployment::DeploymentRoutingPort>(),
            std::any::type_name::<crate::deployment::ResolveError>(),
            std::any::type_name::<crate::deployment::ResolvedStateDb>(),
            std::any::type_name::<crate::deployment::StoreBackedRoutingPort>(),
        ];
        let schema_probe_paths = [
            std::any::type_name::<crate::schema_probe::BinaryInfo>(),
            std::any::type_name::<crate::schema_probe::FeatureMap>(),
            std::any::type_name::<crate::schema_probe::SchemaProbeReport>(),
            std::any::type_name::<crate::schema_probe::StateDbReport>(),
        ];
        assert!(
            deployment_paths
                .iter()
                .all(|name| name.contains("deployment")),
            "deployment module-qualified symbols must remain public: {deployment_paths:#?}"
        );
        assert!(
            schema_probe_paths.iter().all(|name| !name.is_empty()),
            "schema-probe module-qualified symbols must remain public: {schema_probe_paths:#?}"
        );
    }
}
