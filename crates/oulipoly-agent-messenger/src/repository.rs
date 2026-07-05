//! ## Declared roles
//!
//! - orchestration
//!
//! Repository traits and default adapters for store, scratchpad, and channel I/O.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-agent-messenger/src/repository.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-agent-store repository contract
//!       - oulipoly-agent-scratchpad repository contract
//!       - agent-messenger return-channel writer contract
//!       - std filesystem path and append contract
//! ```

use crate::MessengerError;
use crate::channel;
use crate::model::ReturnedArtifact;
use oulipoly_agent_scratchpad::{ReadRequest, Scratchpad, ScratchpadRecord};
use oulipoly_agent_store::{
    ArtifactKey, ArtifactMeta, ArtifactRecord, ListFilter, PutReceipt, PutRequest, Store,
};
use std::path::Path;

pub(crate) trait ReturnStoreRepository {
    fn put(&self, db_path: &Path, req: PutRequest) -> Result<PutReceipt, MessengerError>;
    fn list(&self, db_path: &Path, filter: ListFilter)
    -> Result<Vec<ArtifactMeta>, MessengerError>;
    fn get(
        &self,
        db_path: &Path,
        key: &ArtifactKey,
        version: Option<u64>,
    ) -> Result<ArtifactRecord, MessengerError>;
}

pub(crate) trait ScratchpadRepository {
    fn read(&self, db_path: &Path, req: ReadRequest) -> Result<ScratchpadRecord, MessengerError>;
}

pub(crate) trait ReturnChannelWriter {
    fn append(&self, path: &Path, receipt: &ReturnedArtifact) -> Result<(), MessengerError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct AgentStoreRepository;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct AgentScratchpadRepository;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FileReturnChannelWriter;

impl ReturnStoreRepository for AgentStoreRepository {
    fn put(&self, db_path: &Path, req: PutRequest) -> Result<PutReceipt, MessengerError> {
        Store::open(db_path)?.put(req).map_err(MessengerError::from)
    }

    fn list(
        &self,
        db_path: &Path,
        filter: ListFilter,
    ) -> Result<Vec<ArtifactMeta>, MessengerError> {
        Store::open(db_path)?
            .list(filter)
            .map_err(MessengerError::from)
    }

    fn get(
        &self,
        db_path: &Path,
        key: &ArtifactKey,
        version: Option<u64>,
    ) -> Result<ArtifactRecord, MessengerError> {
        Store::open(db_path)?
            .get(key, version)
            .map_err(MessengerError::from)
    }
}

impl ScratchpadRepository for AgentScratchpadRepository {
    fn read(&self, db_path: &Path, req: ReadRequest) -> Result<ScratchpadRecord, MessengerError> {
        Scratchpad::open(db_path)?
            .read(req)
            .map_err(MessengerError::from)
    }
}

impl ReturnChannelWriter for FileReturnChannelWriter {
    fn append(&self, path: &Path, receipt: &ReturnedArtifact) -> Result<(), MessengerError> {
        channel::append_return_channel(path, receipt)
    }
}
