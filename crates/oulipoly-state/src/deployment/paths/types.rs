use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DbRole {
    Steady,
    PreCutoverPrimary,
    PostCutoverPrimary,
    RetentionSecondary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentPaths {
    pub data_root: PathBuf,
    pub alias: PathBuf,
    pub coordinator: PathBuf,
    pub versioned: BTreeMap<u32, PathBuf>,
}

impl DeploymentPaths {
    pub fn new_from_data_root(data_root: PathBuf) -> Self {
        Self {
            alias: data_root.join("state.db"),
            coordinator: data_root.join("state-deploy.db"),
            data_root,
            versioned: BTreeMap::new(),
        }
    }

    pub fn coordinator_path(&self) -> &Path {
        &self.coordinator
    }

    pub fn versioned_path_for(&self, schema_version: u32) -> Option<&Path> {
        self.versioned.get(&schema_version).map(PathBuf::as_path)
    }

    pub fn add_versioned(&mut self, schema_version: u32, path: PathBuf) {
        self.versioned.insert(schema_version, path);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedStateDb {
    pub path: PathBuf,
    pub schema_version: u32,
    pub role: DbRole,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveError {
    NoPrimary,
    UnknownVersion(u32),
    RoleMismatch,
}

pub(super) fn make_resolved(path: PathBuf, schema_version: u32, role: DbRole) -> ResolvedStateDb {
    ResolvedStateDb {
        path,
        schema_version,
        role,
    }
}
