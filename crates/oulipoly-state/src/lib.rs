mod db;
pub mod schema_probe;

pub use crate::schema_probe::{BinaryInfo, FeatureMap, SchemaProbeReport, StateDbReport};
pub use db::ReadOnlyOpenError;
pub use db::SessionTurnCounts;
pub use db::SessionTurnIngest;
pub use db::StateDb;
pub use db::{AccountRecord, AuthMethod, AuthStatus, CliProviderRecord};
pub use db::{BackfillReport, ChainPreview, ModelStore, ResolvedResume, ResumeError, TurnPreview};
pub use db::{CliMapping, DiscoveredModel, ModelParameter, ParamType};
pub use db::{CompositeInvocationId, InvocationRecord, InvocationStart, InvocationStatus};
pub use db::{QuotaRecord, QuotaWindow, QuotaWindowInput};

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, OnceLock};

    pub(crate) fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
}
