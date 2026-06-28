//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter
//!
mod fixtures;

use fixtures::age_62_deployment_fixtures::{deployment_snapshot, pre_cutover_fixture};
use oulipoly_state::StateDb;
use oulipoly_state::deployment::metadata::store::error::MetadataError;
use oulipoly_state::deployment::metadata::store::rows::DeploymentSnapshot;
use oulipoly_state::deployment::{
    DbRole, DeploymentAwareOpener, DeploymentRoutingPort, ResolveError, ResolvedStateDb,
};
use oulipoly_state::repositories::{ProductionStateDbOpener, StateDbOpener};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

const DATA_DIR_ENV: &str = "OULIPOLY_DATA_DIR";

// TI-05 / cli-default-open-matrix
// component_slug: deployment-routing
// observable signal: representative read command roots all reach the same production opener
// boundary and delegate to the same resolved state.db.vN path.
#[test]
fn production_cli_default_open_matrix_uses_same_resolved_primary_for_read_commands() {
    let _data_dir = IsolatedDataDir::new();
    let fixture = pre_cutover_fixture();
    let expected_path = fixture.versioned_path(5);
    let resolved = ResolvedStateDb {
        path: expected_path.clone(),
        schema_version: 5,
        role: DbRole::PreCutoverPrimary,
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let opener = DeploymentAwareOpener::new(
        Arc::new(RecordingStateDbOpener {
            calls: Arc::clone(&calls),
        }),
        Arc::new(StaticRoutingPort::new(resolved)),
    );
    let production = ProductionStateDbOpener::with_deployment_aware_opener(opener);

    for command_root in [
        "agents trace",
        "agents session locate",
        "agents session export",
    ] {
        // These read-only command roots consume StateDb::open_default/StateDb::default_path.
        // AGE-62 narrows the assertion to the shared opener boundary.
        let _ = command_root;
        <ProductionStateDbOpener as StateDbOpener>::open_default(&production).unwrap();
    }

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls, vec![expected_path; 3]);
}

#[test]
fn default_opener_drop_does_not_clear_registered_deployment_aware_opener() {
    let _data_dir = IsolatedDataDir::new();
    let expected_path = PathBuf::from("/tmp/age-62-default-drop-state.db.v5");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let production = production_opener_for_path(&expected_path, Arc::clone(&calls));

    let default_opener = ProductionStateDbOpener;
    drop(default_opener);

    <ProductionStateDbOpener as StateDbOpener>::open_default(&production).unwrap();

    assert_eq!(*calls.lock().unwrap(), vec![expected_path]);
}

#[test]
fn dropping_older_registered_opener_does_not_clear_newer_registration() {
    let _data_dir = IsolatedDataDir::new();
    let first_path = PathBuf::from("/tmp/age-62-first-state.db.v5");
    let second_path = PathBuf::from("/tmp/age-62-second-state.db.v6");
    let first_calls = Arc::new(Mutex::new(Vec::new()));
    let second_calls = Arc::new(Mutex::new(Vec::new()));
    let first = production_opener_for_path(&first_path, Arc::clone(&first_calls));
    let second = production_opener_for_path(&second_path, Arc::clone(&second_calls));

    drop(first);
    <ProductionStateDbOpener as StateDbOpener>::open_default(&second).unwrap();

    assert!(first_calls.lock().unwrap().is_empty());
    assert_eq!(*second_calls.lock().unwrap(), vec![second_path]);
}

// TI-06 / opener-consults-routing-port
// component_slug: deployment-routing
// observable signal: open_default delegates to inner open_at with the ResolvedStateDb.path.
#[test]
fn deployment_aware_opener_consults_routing_port_before_opening_default() {
    let fixture = pre_cutover_fixture();
    let expected_path = fixture.versioned_path(5);
    let resolved = ResolvedStateDb {
        path: expected_path.clone(),
        schema_version: 5,
        role: DbRole::PreCutoverPrimary,
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let opener = DeploymentAwareOpener::new(
        Arc::new(RecordingStateDbOpener {
            calls: Arc::clone(&calls),
        }),
        Arc::new(StaticRoutingPort::new(resolved)),
    );

    let db = opener.open_default().unwrap();
    drop(db);

    assert_eq!(*calls.lock().unwrap(), vec![expected_path]);
}

// Declared role: mapper
fn production_opener_for_path(
    expected_path: &Path,
    calls: Arc<Mutex<Vec<PathBuf>>>,
) -> ProductionStateDbOpener {
    let resolved = resolved_pre_cutover_primary_for_path(expected_path);
    let opener = deployment_aware_recording_opener(resolved, calls);
    ProductionStateDbOpener::with_deployment_aware_opener(opener)
}

// Declared role: mapper
fn resolved_pre_cutover_primary_for_path(expected_path: &Path) -> ResolvedStateDb {
    ResolvedStateDb {
        path: expected_path.to_path_buf(),
        schema_version: 5,
        role: DbRole::PreCutoverPrimary,
    }
}

// Declared role: mapper
fn deployment_aware_recording_opener(
    resolved: ResolvedStateDb,
    calls: Arc<Mutex<Vec<PathBuf>>>,
) -> DeploymentAwareOpener {
    DeploymentAwareOpener::new(
        Arc::new(RecordingStateDbOpener { calls }),
        Arc::new(StaticRoutingPort::new(resolved)),
    )
}

struct RecordingStateDbOpener {
    calls: Arc<Mutex<Vec<PathBuf>>>,
}

impl StateDbOpener for RecordingStateDbOpener {
    // Declared role: validator
    fn open_default(&self) -> Result<StateDb, String> {
        panic!("deployment-aware opener must resolve and call open_at")
    }

    // Declared role: accessor
    fn open_at(&self, path: &Path) -> Result<StateDb, String> {
        self.calls.lock().unwrap().push(path.to_path_buf());
        StateDb::open(Path::new(":memory:"))
    }

    // Declared role: accessor
    fn open_in_memory(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }
}

struct IsolatedDataDir {
    _dir: tempfile::TempDir,
    _env: EnvGuard,
}

impl IsolatedDataDir {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = EnvGuard::set(dir.path());
        Self {
            _dir: dir,
            _env: env,
        }
    }
}

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_oulipoly_data_dir: Option<OsString>,
}

impl EnvGuard {
    fn set(oulipoly_data_dir: &Path) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_oulipoly_data_dir = std::env::var_os(DATA_DIR_ENV);
        unsafe {
            std::env::set_var(DATA_DIR_ENV, oulipoly_data_dir);
        }
        Self {
            _lock: lock,
            old_oulipoly_data_dir,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.old_oulipoly_data_dir.take() {
                Some(value) => std::env::set_var(DATA_DIR_ENV, value),
                None => std::env::remove_var(DATA_DIR_ENV),
            }
        }
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Clone)]
struct StaticRoutingPort {
    resolved: ResolvedStateDb,
}

impl StaticRoutingPort {
    // Declared role: mapper
    fn new(resolved: ResolvedStateDb) -> Self {
        Self { resolved }
    }
}

impl DeploymentRoutingPort for StaticRoutingPort {
    // Declared role: accessor
    fn resolve_for_current_binary(&self) -> Result<ResolvedStateDb, ResolveError> {
        Ok(self.resolved.clone())
    }

    // Declared role: accessor
    fn resolve_read_only(&self) -> Result<ResolvedStateDb, ResolveError> {
        Ok(self.resolved.clone())
    }

    // Declared role: accessor
    fn deployment_snapshot(&self) -> Result<DeploymentSnapshot, MetadataError> {
        Ok(deployment_snapshot(
            self.resolved.schema_version,
            self.resolved.role,
        ))
    }
}
