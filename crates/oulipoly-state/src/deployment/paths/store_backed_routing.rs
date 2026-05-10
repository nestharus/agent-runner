use crate::deployment::metadata::DeploymentMetadataStore;
use crate::deployment::metadata::store::error::MetadataError;
use crate::deployment::metadata::store::rows::DeploymentSnapshot;
use crate::deployment::routing::DeploymentRoutingPort;

use super::resolver::StateDbDeploymentResolver;
use super::types::{ResolveError, ResolvedStateDb};

pub struct StoreBackedRoutingPort {
    store: Box<dyn DeploymentMetadataStore>,
    resolver: StateDbDeploymentResolver,
    current_schema_version: u32,
}

impl StoreBackedRoutingPort {
    pub fn new(
        store: Box<dyn DeploymentMetadataStore>,
        resolver: StateDbDeploymentResolver,
        current_schema_version: u32,
    ) -> Self {
        Self {
            store,
            resolver,
            current_schema_version,
        }
    }
}

impl DeploymentRoutingPort for StoreBackedRoutingPort {
    fn resolve_for_current_binary(&self) -> Result<ResolvedStateDb, ResolveError> {
        let snapshot = self.store.snapshot().map_err(|_| ResolveError::NoPrimary)?;
        self.resolver
            .resolve_for_current_binary(self.current_schema_version, snapshot)
    }

    fn resolve_read_only(&self) -> Result<ResolvedStateDb, ResolveError> {
        let snapshot = self.store.snapshot().map_err(|_| ResolveError::NoPrimary)?;
        self.resolver
            .resolve_read_only(self.current_schema_version, snapshot)
    }

    fn deployment_snapshot(&self) -> Result<DeploymentSnapshot, MetadataError> {
        self.store.snapshot()
    }
}
