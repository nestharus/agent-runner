mod fixtures;

use fixtures::age_62_deployment_fixtures::{
    data_root_content_snapshot, deployment_snapshot, pre_cutover_fixture,
};
use oulipoly_state::deployment::metadata::store::error::MetadataError;
use oulipoly_state::deployment::metadata::store::rows::DeploymentSnapshot;
use oulipoly_state::deployment::{
    DbRole, DeploymentAwareOpener, DeploymentRoutingPort, ResolveError, ResolvedStateDb,
};
use oulipoly_state::repositories::StateDbOpener;
use oulipoly_state::{StateDb, schema_probe};
use std::path::Path;
use std::sync::Arc;

// TI-02 / readonly-no-side-effect
// component_slug: deployment-routing
// observable signal: file list and file bytes under data_root are unchanged by read-only open.
#[test]
fn open_read_only_default_does_not_change_data_root_contents() {
    let fixture = pre_cutover_fixture();
    let resolved = ResolvedStateDb {
        path: fixture.versioned_path(5),
        schema_version: 5,
        role: DbRole::PreCutoverPrimary,
    };
    let opener = DeploymentAwareOpener::new(
        Arc::new(PanicIfUsedOpener),
        Arc::new(StaticRoutingPort::new(resolved)),
    );
    let before = data_root_content_snapshot(&fixture.data_root);

    let handle = opener.open_read_only_default().unwrap();
    drop(handle);

    let after = data_root_content_snapshot(&fixture.data_root);
    assert_eq!(after, before);
}

// TI-02 / schema-probe-resolved-primary
// component_slug: deployment-routing
// observable signal: schema diagnostics inspect the resolved primary path and version.
#[test]
fn schema_probe_report_uses_resolved_primary_path_and_version_when_given_routing_port() {
    let fixture = pre_cutover_fixture();
    let resolved = ResolvedStateDb {
        path: fixture.versioned_path(5),
        schema_version: 5,
        role: DbRole::PreCutoverPrimary,
    };
    let routing = StaticRoutingPort::new(resolved.clone());

    let routed = routing
        .resolve_read_only()
        .expect("routing port should provide read-only primary");
    let db = StateDb::open_read_only(&routed.path).unwrap();
    let state_db = schema_probe::inspect_schema(db.connection(), routed.path.clone()).unwrap();
    let report = schema_probe::report_from_state_db(state_db);

    assert_eq!(report.state_db.path, resolved.path);
    assert_eq!(report.state_db.user_version, resolved.schema_version);
}

#[derive(Clone)]
struct StaticRoutingPort {
    resolved: ResolvedStateDb,
}

impl StaticRoutingPort {
    fn new(resolved: ResolvedStateDb) -> Self {
        Self { resolved }
    }
}

impl DeploymentRoutingPort for StaticRoutingPort {
    fn resolve_for_current_binary(&self) -> Result<ResolvedStateDb, ResolveError> {
        Ok(self.resolved.clone())
    }

    fn resolve_read_only(&self) -> Result<ResolvedStateDb, ResolveError> {
        Ok(self.resolved.clone())
    }

    fn deployment_snapshot(&self) -> Result<DeploymentSnapshot, MetadataError> {
        Ok(deployment_snapshot(
            self.resolved.schema_version,
            self.resolved.role,
        ))
    }
}

struct PanicIfUsedOpener;

impl StateDbOpener for PanicIfUsedOpener {
    fn open_default(&self) -> Result<StateDb, String> {
        panic!("read-only deployment open must not call open_default")
    }

    fn open_at(&self, path: &Path) -> Result<StateDb, String> {
        StateDb::open(path)
    }

    fn open_in_memory(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }
}
