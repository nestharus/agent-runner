#![allow(dead_code, unused_imports, unused_variables)]

pub mod metadata;
pub mod paths;
pub mod routing;
pub mod row_version;

pub use metadata::{
    COORDINATOR_SCHEMA_VERSION, DeploymentMetadataStore, SchemaError,
    SqliteDeploymentMetadataStore, ensure_coordinator_schema,
};
pub use paths::{
    DbRole, DeploymentPaths, DeploymentRoutingDecision, ResolveError, ResolvedStateDb,
    StateDbDeploymentResolver, StoreBackedRoutingPort,
};
pub use routing::{DeploymentAwareOpener, DeploymentRoutingPort, OpenError, ReadOnlyStateDb};
