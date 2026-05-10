mod fixtures;

use fixtures::age_62_deployment_fixtures::{
    deployment_snapshot, missing_primary_snapshot, post_cutover_fixture, pre_cutover_fixture,
};
use oulipoly_state::StateDb;
use oulipoly_state::deployment::metadata::store::error::MetadataError;
use oulipoly_state::deployment::metadata::store::rows::DeploymentSnapshot;
use oulipoly_state::deployment::paths::StoreBackedRoutingPort;
use oulipoly_state::deployment::{
    DbRole, DeploymentAwareOpener, DeploymentMetadataStore, DeploymentPaths, DeploymentRoutingPort,
    ResolveError, ResolvedStateDb, SqliteDeploymentMetadataStore, StateDbDeploymentResolver,
};
use oulipoly_state::repositories::{ProductionStateDbOpener, StateDbOpener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// TI-01 / pre-cutover
// component_slug: deployment-paths-resolver
// observable signal: resolved path/schema/role names state.db.v5 and PreCutoverPrimary.
#[test]
fn resolver_pre_cutover_primary_returns_state_db_v5() {
    let fixture = pre_cutover_fixture();
    let resolver = resolver_for(&fixture.data_root);

    let resolved = resolver
        .resolve_for_current_binary(5, deployment_snapshot(5, DbRole::PreCutoverPrimary))
        .expect("pre-cutover primary should resolve");

    assert_eq!(
        resolved,
        ResolvedStateDb {
            path: fixture.versioned_path(5),
            schema_version: 5,
            role: DbRole::PreCutoverPrimary,
        }
    );
}

// TI-01 / post-cutover
// component_slug: deployment-paths-resolver
// observable signal: resolved path/schema/role names state.db.v6 and PostCutoverPrimary.
#[test]
fn resolver_post_cutover_primary_returns_state_db_v6() {
    let fixture = post_cutover_fixture();
    let resolver = resolver_for(&fixture.data_root);

    let resolved = resolver
        .resolve_for_current_binary(6, deployment_snapshot(6, DbRole::PostCutoverPrimary))
        .expect("post-cutover primary should resolve");

    assert_eq!(resolved.path, fixture.versioned_path(6));
    assert_eq!(resolved.schema_version, 6);
    assert_eq!(resolved.role, DbRole::PostCutoverPrimary);
}

// TI-01 / pre-cutover routing-port seam
// component_slug: deployment-routing
// observable signal: a seeded coordinator resolves through the routing port to state.db.v5.
#[test]
fn resolver_through_routing_port_resolves_pre_cutover_to_state_db_v5() {
    let fixture = pre_cutover_fixture();
    let store = SqliteDeploymentMetadataStore::open(&fixture.data_root).unwrap();
    let routing_port = StoreBackedRoutingPort::new(store, resolver_for(&fixture.data_root), 5);

    let resolved = routing_port
        .resolve_for_current_binary()
        .expect("pre-cutover primary should resolve through routing port");

    assert_eq!(
        resolved,
        ResolvedStateDb {
            path: fixture.versioned_path(5),
            schema_version: 5,
            role: DbRole::PreCutoverPrimary,
        }
    );
}

// TI-01 / post-cutover routing-port seam
// component_slug: deployment-routing
// observable signal: a seeded coordinator resolves through the routing port to state.db.v6.
#[test]
fn resolver_through_routing_port_resolves_post_cutover_to_state_db_v6() {
    let fixture = post_cutover_fixture();
    let store = SqliteDeploymentMetadataStore::open(&fixture.data_root).unwrap();
    let routing_port = StoreBackedRoutingPort::new(store, resolver_for(&fixture.data_root), 6);

    let resolved = routing_port
        .resolve_for_current_binary()
        .expect("post-cutover primary should resolve through routing port");

    assert_eq!(
        resolved,
        ResolvedStateDb {
            path: fixture.versioned_path(6),
            schema_version: 6,
            role: DbRole::PostCutoverPrimary,
        }
    );
}

// TI-11 / read-routes-by-pointer
// component_slug: deployment-paths-resolver
// observable signal: toggling primary_pointer role/version changes the selected versioned path
// through both the pure resolver and DeploymentAwareOpener::open_default.
#[test]
fn read_routes_by_primary_pointer_before_and_after_cutover() {
    let fixture = pre_cutover_fixture();
    let resolver = resolver_for(&fixture.data_root);

    let before_cutover = resolver
        .resolve_for_current_binary(5, deployment_snapshot(5, DbRole::PreCutoverPrimary))
        .expect("pre-cutover pointer should route to A");
    let after_cutover = resolver
        .resolve_for_current_binary(6, deployment_snapshot(6, DbRole::PostCutoverPrimary))
        .expect("post-cutover pointer should route to B");

    assert_eq!(before_cutover.path, fixture.versioned_path(5));
    assert_eq!(before_cutover.role, DbRole::PreCutoverPrimary);
    assert_eq!(after_cutover.path, fixture.versioned_path(6));
    assert_eq!(after_cutover.role, DbRole::PostCutoverPrimary);

    let before_opened = recorded_open_default_path(
        ResolvedStateDb {
            path: fixture.versioned_path(5),
            schema_version: 5,
            role: DbRole::PreCutoverPrimary,
        },
        &fixture.data_root,
    );
    let after_opened = recorded_open_default_path(
        ResolvedStateDb {
            path: fixture.versioned_path(6),
            schema_version: 6,
            role: DbRole::PostCutoverPrimary,
        },
        &fixture.data_root,
    );

    assert_eq!(before_opened, fixture.versioned_path(5));
    assert_eq!(after_opened, fixture.versioned_path(6));
}

// TI-11 / open-default-noop-select
// component_slug: deployment-routing
// observable signal: DeploymentAwareOpener::open_default uses the production StateDb opener
// to open the resolved versioned primary, and the returned handle can execute SELECT 1.
#[test]
fn deployment_aware_open_read_only_runs_noop_select_on_versioned_primary_before_and_after_cutover()
{
    let pre_cutover = pre_cutover_fixture();
    assert_read_only_default_noop_selects_from_versioned_primary(
        &pre_cutover.data_root,
        5,
        DbRole::PreCutoverPrimary,
        pre_cutover.versioned_path(5),
    );

    let post_cutover = post_cutover_fixture();
    assert_read_only_default_noop_selects_from_versioned_primary(
        &post_cutover.data_root,
        6,
        DbRole::PostCutoverPrimary,
        post_cutover.versioned_path(6),
    );
}

// TI-11 / never-read-from-queue
// component_slug: deployment-paths-resolver
// observable signal: resolver returns only seeded versioned DB paths and missing primary is explicit.
#[test]
fn resolver_never_names_a_queue_read_target_and_missing_pointer_is_no_primary() {
    let fixture = pre_cutover_fixture();
    let resolver = resolver_for(&fixture.data_root);
    let seeded_versioned_paths = vec![fixture.versioned_path(5), fixture.versioned_path(6)];

    for (schema_version, role, expected_path) in [
        (5, DbRole::Steady, fixture.versioned_path(5)),
        (5, DbRole::PreCutoverPrimary, fixture.versioned_path(5)),
        (6, DbRole::PostCutoverPrimary, fixture.versioned_path(6)),
        (6, DbRole::RetentionSecondary, fixture.versioned_path(6)),
    ] {
        let resolved = resolver
            .resolve_for_current_binary(schema_version, deployment_snapshot(schema_version, role))
            .expect("seeded primary should resolve");
        assert_versioned_read_target(&resolved.path, &seeded_versioned_paths);
        assert_eq!(resolved.path, expected_path);
    }

    assert_eq!(
        resolver.resolve_for_current_binary(5, missing_primary_snapshot()),
        Err(ResolveError::NoPrimary)
    );
}

fn resolver_for(data_root: &Path) -> StateDbDeploymentResolver {
    let mut paths = DeploymentPaths::new_from_data_root(data_root.to_path_buf());
    paths.add_versioned(5, data_root.join("state.db.v5"));
    paths.add_versioned(6, data_root.join("state.db.v6"));
    StateDbDeploymentResolver::new(paths)
}

#[allow(dead_code)]
fn _assert_pathbuf_signal(path: PathBuf) -> PathBuf {
    path
}

fn recorded_open_default_path(resolved: ResolvedStateDb, _data_root: &Path) -> PathBuf {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let snapshot = deployment_snapshot(resolved.schema_version, resolved.role);
    let opener = DeploymentAwareOpener::new(
        Arc::new(RecordingStateDbOpener {
            calls: Arc::clone(&calls),
        }),
        Arc::new(StaticRoutingPort::new(resolved, snapshot)),
    );

    let db = opener.open_default().unwrap();
    drop(db);

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    calls.into_iter().next().unwrap()
}

fn assert_read_only_default_noop_selects_from_versioned_primary(
    data_root: &Path,
    current_schema_version: u32,
    expected_role: DbRole,
    expected_path: PathBuf,
) {
    let routing_port = routing_port_for(data_root, current_schema_version);
    let resolved = resolve_seeded_primary(routing_port.as_ref());
    assert_resolved_primary(
        &resolved,
        &expected_path,
        current_schema_version,
        expected_role,
    );

    let routing_port_for_opener: Arc<dyn DeploymentRoutingPort> = routing_port;
    let opener =
        DeploymentAwareOpener::new(Arc::new(ProductionStateDbOpener), routing_port_for_opener);
    assert_read_only_db_can_noop_select(&opener, current_schema_version);
}

fn routing_port_for(data_root: &Path, current_schema_version: u32) -> Arc<StoreBackedRoutingPort> {
    let store = SqliteDeploymentMetadataStore::open(data_root).unwrap();
    Arc::new(StoreBackedRoutingPort::new(
        store,
        resolver_for(data_root),
        current_schema_version,
    ))
}

fn resolve_seeded_primary(routing_port: &dyn DeploymentRoutingPort) -> ResolvedStateDb {
    routing_port
        .resolve_for_current_binary()
        .expect("seeded primary should resolve before opening")
}

fn assert_resolved_primary(
    resolved: &ResolvedStateDb,
    expected_path: &Path,
    current_schema_version: u32,
    expected_role: DbRole,
) {
    assert_eq!(resolved.path, expected_path);
    assert_eq!(resolved.schema_version, current_schema_version);
    assert_eq!(resolved.role, expected_role);
}

fn assert_read_only_db_can_noop_select(
    opener: &DeploymentAwareOpener,
    current_schema_version: u32,
) {
    let db = opener.open_read_only_default().unwrap();
    assert_eq!(read_user_version(&db), i64::from(current_schema_version));
    assert_eq!(read_select_one(&db), 1);
}

fn read_user_version(db: &StateDb) -> i64 {
    db.connection()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn read_select_one(db: &StateDb) -> i64 {
    db.connection()
        .query_row("SELECT 1", [], |row| row.get(0))
        .unwrap()
}

fn assert_versioned_read_target(path: &Path, seeded_versioned_paths: &[PathBuf]) {
    assert!(
        !path.ends_with("state-deploy.db"),
        "read target must not be the coordinator DB: {}",
        path.display()
    );
    assert!(
        !path.to_string_lossy().contains("queue"),
        "read target must not be a queue path: {}",
        path.display()
    );
    assert!(
        seeded_versioned_paths.contains(&path.to_path_buf()),
        "read target must be one of the seeded versioned DB paths: {}",
        path.display()
    );
}

struct RecordingStateDbOpener {
    calls: Arc<Mutex<Vec<PathBuf>>>,
}

impl StateDbOpener for RecordingStateDbOpener {
    fn open_default(&self) -> Result<StateDb, String> {
        panic!("deployment-aware opener must resolve and call open_at")
    }

    fn open_at(&self, path: &Path) -> Result<StateDb, String> {
        self.calls.lock().unwrap().push(path.to_path_buf());
        StateDb::open(Path::new(":memory:"))
    }

    fn open_in_memory(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }
}

#[derive(Clone)]
struct StaticRoutingPort {
    resolved: ResolvedStateDb,
    snapshot: DeploymentSnapshot,
}

impl StaticRoutingPort {
    fn new(resolved: ResolvedStateDb, snapshot: DeploymentSnapshot) -> Self {
        Self { resolved, snapshot }
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
        Ok(self.snapshot.clone())
    }
}
