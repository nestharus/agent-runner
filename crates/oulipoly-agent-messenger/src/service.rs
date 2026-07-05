//! ## Declared roles
//!
//! - orchestration
//!
//! Service seam over returned-artifact use cases.

use crate::MessengerError;
use crate::mapper::{
    metas_from_store, record_from_store, return_lookup, return_put_request, returned_from_put,
    returned_list_filter,
};
use crate::model::{
    ListReturnedRequest, ReturnPayload, ReturnRequest, ReturnedArtifact, ReturnedArtifactMeta,
    ReturnedArtifactRecord, ShowReturnedRequest, StoredReturnPayload,
};
use crate::repository::{
    AgentScratchpadRepository, AgentStoreRepository, FileReturnChannelWriter, ReturnChannelWriter,
    ReturnStoreRepository, ScratchpadRepository,
};
use crate::source::materialize_return_source;
use std::path::Path;

pub fn return_artifact(req: ReturnRequest) -> Result<ReturnedArtifact, MessengerError> {
    default_service().return_artifact(req)
}

pub fn list_returned(
    req: ListReturnedRequest,
) -> Result<Vec<ReturnedArtifactMeta>, MessengerError> {
    default_service().list_returned(req)
}

pub fn show_returned(req: ShowReturnedRequest) -> Result<ReturnedArtifactRecord, MessengerError> {
    default_service().show_returned(req)
}

struct MessengerService<StoreRepo, Scratchpads, Channels> {
    store: StoreRepo,
    scratchpads: Scratchpads,
    channels: Channels,
}

impl<StoreRepo, Scratchpads, Channels> MessengerService<StoreRepo, Scratchpads, Channels> {
    fn new(store: StoreRepo, scratchpads: Scratchpads, channels: Channels) -> Self {
        Self {
            store,
            scratchpads,
            channels,
        }
    }
}

impl<StoreRepo, Scratchpads, Channels> MessengerService<StoreRepo, Scratchpads, Channels>
where
    StoreRepo: ReturnStoreRepository,
    Scratchpads: ScratchpadRepository,
    Channels: ReturnChannelWriter,
{
    fn return_artifact(&self, req: ReturnRequest) -> Result<ReturnedArtifact, MessengerError> {
        let payload = materialize_return_source(&self.scratchpads, &req)?;
        let stored = self.store_payload(&req, payload)?;
        let returned = returned_from_put(stored.receipt, req.invocation_uuid, stored.source);
        self.append_return_channel_if_requested(req.return_channel.as_deref(), &returned)?;
        Ok(returned)
    }

    fn list_returned(
        &self,
        req: ListReturnedRequest,
    ) -> Result<Vec<ReturnedArtifactMeta>, MessengerError> {
        let filter = returned_list_filter(req.invocation_uuid, req.name);
        let rows = self.store.list(&req.db_path, filter)?;
        metas_from_store(rows)
    }

    fn show_returned(
        &self,
        req: ShowReturnedRequest,
    ) -> Result<ReturnedArtifactRecord, MessengerError> {
        let lookup = return_lookup(req)?;
        let record = self
            .store
            .get(&lookup.db_path, &lookup.key, lookup.version)?;
        record_from_store(record)
    }

    fn store_payload(
        &self,
        req: &ReturnRequest,
        payload: ReturnPayload,
    ) -> Result<StoredReturnPayload, MessengerError> {
        let source = payload.source.clone();
        let put_request = return_put_request(req, payload);
        let receipt = self.store.put(&req.db_path, put_request)?;
        Ok(StoredReturnPayload { receipt, source })
    }

    fn append_return_channel_if_requested(
        &self,
        path: Option<&Path>,
        returned: &ReturnedArtifact,
    ) -> Result<(), MessengerError> {
        if let Some(path) = path {
            return self.channels.append(path, returned);
        }
        Ok(())
    }
}

fn default_service()
-> MessengerService<AgentStoreRepository, AgentScratchpadRepository, FileReturnChannelWriter> {
    MessengerService::new(
        AgentStoreRepository,
        AgentScratchpadRepository,
        FileReturnChannelWriter,
    )
}
