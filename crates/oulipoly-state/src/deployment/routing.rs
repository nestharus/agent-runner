use crate::StateDb;
use crate::db::ReadOnlyOpenError;
use crate::deployment::metadata::store::error::MetadataError;
use crate::deployment::metadata::store::rows::DeploymentSnapshot;
use crate::deployment::paths::{ResolveError, ResolvedStateDb};
use crate::repositories::StateDbOpener;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type ReadOnlyStateDb = StateDb;

#[derive(Debug, Clone)]
pub enum OpenError {
    ResolveFailed(ResolveError),
    OpenFailed(String),
    ReadOnlyOpenFailed(ReadOnlyOpenError),
}

impl From<ResolveError> for OpenError {
    fn from(err: ResolveError) -> Self {
        OpenError::ResolveFailed(err)
    }
}

impl From<ReadOnlyOpenError> for OpenError {
    fn from(err: ReadOnlyOpenError) -> Self {
        OpenError::ReadOnlyOpenFailed(err)
    }
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::ResolveFailed(err) => {
                write!(f, "Failed to resolve deployment primary: {err:?}")
            }
            OpenError::OpenFailed(err) => write!(f, "Failed to open resolved primary: {err}"),
            OpenError::ReadOnlyOpenFailed(err) => {
                write!(f, "Failed to open resolved primary read-only: {err:?}")
            }
        }
    }
}

impl std::error::Error for OpenError {}

impl From<OpenError> for String {
    fn from(err: OpenError) -> Self {
        err.to_string()
    }
}

pub trait DeploymentRoutingPort: Send + Sync {
    fn resolve_for_current_binary(&self) -> Result<ResolvedStateDb, ResolveError>;
    fn resolve_read_only(&self) -> Result<ResolvedStateDb, ResolveError>;
    fn deployment_snapshot(&self) -> Result<DeploymentSnapshot, MetadataError>;
}

#[derive(Clone)]
pub struct DeploymentAwareOpener {
    inner: Arc<dyn StateDbOpener>,
    routing: Arc<dyn DeploymentRoutingPort>,
}

impl DeploymentAwareOpener {
    pub fn new(inner: Arc<dyn StateDbOpener>, routing: Arc<dyn DeploymentRoutingPort>) -> Self {
        Self { inner, routing }
    }

    pub fn open_default(&self) -> Result<StateDb, OpenError> {
        let resolved = self.routing.resolve_for_current_binary()?;
        self.inner
            .open_at(&resolved.path)
            .map_err(OpenError::OpenFailed)
    }

    pub fn default_path(&self) -> Result<PathBuf, OpenError> {
        Ok(self.routing.resolve_for_current_binary()?.path)
    }

    pub fn open_read_only_default(&self) -> Result<ReadOnlyStateDb, OpenError> {
        let resolved = self.routing.resolve_read_only()?;
        Ok(StateDb::open_read_only(&resolved.path)?)
    }
}
