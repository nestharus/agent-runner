use crate::deployment::metadata::store::rows::DeploymentSnapshot;

use super::resolver_validators::{validate_inputs, validate_versioned_path};
use super::types::{DbRole, DeploymentPaths, ResolveError, ResolvedStateDb, make_resolved};
use std::path::PathBuf;

pub struct StateDbDeploymentResolver {
    paths: DeploymentPaths,
}

impl StateDbDeploymentResolver {
    pub fn new(paths: DeploymentPaths) -> Self {
        Self { paths }
    }

    pub fn resolve_for_current_binary(
        &self,
        current_schema_version: u32,
        deployment: DeploymentSnapshot,
    ) -> Result<ResolvedStateDb, ResolveError> {
        self.resolve(current_schema_version, deployment)
    }

    pub fn resolve_read_only(
        &self,
        current_schema_version: u32,
        deployment: DeploymentSnapshot,
    ) -> Result<ResolvedStateDb, ResolveError> {
        self.resolve(current_schema_version, deployment)
    }

    fn resolve(
        &self,
        current_schema_version: u32,
        deployment: DeploymentSnapshot,
    ) -> Result<ResolvedStateDb, ResolveError> {
        let (role, schema_version) = validate_inputs(&deployment, current_schema_version)?;
        self.build_resolved(role, schema_version)
    }

    fn build_resolved(
        &self,
        role: DbRole,
        schema_version: u32,
    ) -> Result<ResolvedStateDb, ResolveError> {
        let path = self.lookup_versioned_path(schema_version);
        let path = validate_versioned_path(path, schema_version)?;
        Ok(make_resolved(path, schema_version, role))
    }

    fn lookup_versioned_path(&self, schema_version: u32) -> Option<PathBuf> {
        self.paths
            .versioned_path_for(schema_version)
            .map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::metadata::store::rows::{DeploymentSnapshot, PrimaryPointer};
    use chrono::Utc;
    use std::path::PathBuf;

    fn snapshot(schema_version: u32, role: DbRole) -> DeploymentSnapshot {
        DeploymentSnapshot {
            primary: PrimaryPointer {
                schema_version,
                deployment_id: None,
                role,
                updated_at: Utc::now(),
            },
            active_deployment: None,
            queue_states: Vec::new(),
            retention: None,
        }
    }

    // component_slug: deployment-paths-resolver
    #[test]
    fn resolver_maps_all_db_roles_to_declared_versioned_primary_paths() {
        let mut paths = DeploymentPaths::new_from_data_root(PathBuf::from("/tmp/age-62"));
        paths.add_versioned(5, PathBuf::from("/tmp/age-62/state.db.v5"));
        paths.add_versioned(6, PathBuf::from("/tmp/age-62/state.db.v6"));
        paths.add_versioned(7, PathBuf::from("/tmp/age-62/state.db.v7"));
        paths.add_versioned(8, PathBuf::from("/tmp/age-62/state.db.v8"));
        let resolver = StateDbDeploymentResolver::new(paths);

        let cases = [
            (DbRole::Steady, 5, "/tmp/age-62/state.db.v5"),
            (DbRole::PreCutoverPrimary, 6, "/tmp/age-62/state.db.v6"),
            (DbRole::PostCutoverPrimary, 7, "/tmp/age-62/state.db.v7"),
            (DbRole::RetentionSecondary, 8, "/tmp/age-62/state.db.v8"),
        ];

        for (role, version, expected_path) in cases {
            let resolved = resolver
                .resolve_for_current_binary(version, snapshot(version, role))
                .expect("resolver should map declared primary pointer");
            assert_eq!(resolved.role, role);
            assert_eq!(resolved.schema_version, version);
            assert_eq!(resolved.path, PathBuf::from(expected_path));
        }
    }

    #[test]
    fn resolver_reports_no_primary_for_missing_primary_pointer_invariant() {
        let resolver = StateDbDeploymentResolver::new(DeploymentPaths::new_from_data_root(
            PathBuf::from("/tmp/age-62"),
        ));
        let mut missing = snapshot(5, DbRole::Steady);
        missing.primary.schema_version = 0;

        let result = resolver.resolve_for_current_binary(5, missing);

        assert_eq!(result, Err(ResolveError::NoPrimary));
    }
}
