mod db;
pub mod deployment;
pub mod migrations;
pub mod repositories;
pub mod schema;
pub mod schema_probe;

pub type StateDbError = String;

pub use crate::schema::{CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION};
pub use crate::schema_probe::{BinaryInfo, FeatureMap, SchemaProbeReport, StateDbReport};
pub use db::DbError;
pub use db::ProviderRecord;
pub use db::ReadOnlyOpenError;
pub use db::SessionTurnCounts;
pub use db::SessionTurnIngest;
pub use db::StateDb;
pub use db::{AccountRecord, AuthMethod, AuthStatus, CliProviderRecord};
pub use db::{
    BackfillReport, ChainPreview, ModelStore, ProviderSessionBinding, ResolvedResume, ResumeError,
    SessionMarkerPayload, TurnPreview, WrongIdKindInput,
};
pub use db::{CliMapping, DiscoveredModel, ModelParameter, ParamType};
pub use db::{CompositeInvocationId, InvocationRecord, InvocationStart, InvocationStatus};
pub use db::{QuotaRecord, QuotaWindow, QuotaWindowInput};
pub use deployment::{
    DbRole, DeploymentAwareOpener, DeploymentMetadataStore, DeploymentRoutingDecision,
    DeploymentRoutingPort, ResolveError, ResolvedStateDb, StoreBackedRoutingPort,
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
    pub struct StateDbRawConnectionEscapeMustNotCompile;
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, OnceLock};

    pub(crate) fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}
