//! ## Declared roles
//!
//! `orchestration`, `validator`.
//!
//! ## Component declared roles
//! ```yaml
//! component_declared_roles:
//!   - component: scratchpad-core
//!     paths:
//!       - crates/oulipoly-agent-scratchpad/src/lib.rs
//!       - crates/oulipoly-agent-scratchpad/src/core_api.rs
//!       - crates/oulipoly-agent-scratchpad/src/application.rs
//!       - crates/oulipoly-agent-scratchpad/src/retirement_status.rs
//!     roles:
//!       - orchestration
//!       - validator
//!       - accessor
//!       - predicate
//!       - mapper
//!       - formatter
//!       - filter
//! ```

use std::path::Path;

use chrono::{DateTime, Utc};
use oulipoly_agent_store::Store;

use crate::application::ScratchpadApplication;
use crate::compatibility_ddl::install_store_aliases;
use crate::store_adapter::StoreScratchpadPersistence;
use crate::{
    DeleteReceipt, DeleteRequest, GcReport, GcRequest, ListRequest, PublishReceipt, PublishRequest,
    ReadRequest, SCRATCHPAD_PREFIX, Scratchpad, ScratchpadError, ScratchpadMeta, ScratchpadName,
    ScratchpadRecord, WriteReceipt, WriteRequest,
};

impl ScratchpadName {
    pub fn new(value: impl Into<String>) -> Result<Self, ScratchpadError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ScratchpadError::InvalidInput(
                "scratchpad name must not be empty".to_string(),
            ));
        }
        if value.starts_with(SCRATCHPAD_PREFIX) {
            return Err(ScratchpadError::InvalidInput(format!(
                "scratchpad name must not start with reserved prefix {SCRATCHPAD_PREFIX}"
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn system_current_utc() -> DateTime<Utc> {
    std::time::SystemTime::now().into()
}

impl Scratchpad {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, ScratchpadError> {
        let db_path = db_path.as_ref();
        let store = Store::open(db_path)?;
        install_store_aliases(db_path)?;
        let persistence = StoreScratchpadPersistence::new(store);
        let application =
            ScratchpadApplication::new(persistence, system_current_utc as fn() -> DateTime<Utc>);
        Ok(Self { application })
    }

    pub fn write(&self, req: WriteRequest) -> Result<WriteReceipt, ScratchpadError> {
        self.application.write(req)
    }

    pub fn read(&self, req: ReadRequest) -> Result<ScratchpadRecord, ScratchpadError> {
        self.application.read(req)
    }

    pub fn list(&self, req: ListRequest) -> Result<Vec<ScratchpadMeta>, ScratchpadError> {
        self.application.list(req)
    }

    pub fn delete(&self, req: DeleteRequest) -> Result<DeleteReceipt, ScratchpadError> {
        self.application.delete(req)
    }

    pub fn publish(&self, req: PublishRequest) -> Result<PublishReceipt, ScratchpadError> {
        self.application.publish(req)
    }

    pub fn gc(&self, req: GcRequest) -> Result<GcReport, ScratchpadError> {
        self.application.gc(req)
    }
}
