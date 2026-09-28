#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::protocol::{
    EntryRoute, observe_entry_gate_at, observe_installed_pair_at,
};
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

struct Images {
    directory: PathBuf,
    runner: PathBuf,
    broker: PathBuf,
    launcher: PathBuf,
    bash: PathBuf,
    manifest: PathBuf,
    pair: InstalledPair,
}

impl Images {
    fn new(parent: &Path) -> Self {
        let directory = parent.join("installed");
        fs::create_dir(&directory).unwrap();
        let runner = std::env::current_exe().unwrap();
        let broker = directory.join("oulipoly-kernel-broker");
        fs::copy(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"), &broker).unwrap();
        let launcher = directory.join("oulipoly-installed-launcher");
        fs::copy(env!("CARGO_BIN_EXE_oulipoly-installed-launcher"), &launcher).unwrap();
        let bash = directory.join("agent-bash");
        fs::copy(&broker, &bash).unwrap();
        let manifest = directory.join("install-v1.json");
        let pair = InstalledPair {
            schema: 2,
            version: env!("CARGO_PKG_VERSION").into(),
            generation: uuid::Uuid::new_v4().to_string(),
            runner_sha256: digest(&runner),
            broker_sha256: digest(&broker),
            launcher_sha256: Some(digest(&launcher)),
            bash_sha256: Some(digest(&bash)),
        };
        fs::write(&manifest, serde_json::to_vec(&pair).unwrap()).unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            directory,
            runner,
            broker,
            launcher,
            bash,
            manifest,
            pair,
        }
    }

    fn paths(&self) -> PairPaths<'_> {
        PairPaths {
            manifest: &self.manifest,
            runner: &self.runner,
            broker: &self.broker,
            launcher: &self.launcher,
            bash: &self.bash,
        }
    }

    fn restore_manifest(&self) {
        fs::write(&self.manifest, serde_json::to_vec(&self.pair).unwrap()).unwrap();
        fs::set_permissions(&self.manifest, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn broker(root: &Path, socket: &Path, images: &Images) -> Child {
    Command::new(&images.broker)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", root)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &images.runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &images.manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &images.broker)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1",
            &images.launcher,
        )
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &images.bash)
        .spawn()
        .unwrap()
}

fn wait_route(child: &mut Child, socket: &Path, expected: EntryRoute) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(route) = observe_entry_gate_at(socket) {
            assert_eq!(route, expected);
            return;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("Broker exited before route readback: {status}");
        }
        assert!(Instant::now() < deadline, "Broker route timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn root_mapped_first_install_activation_is_exact_and_restart_stable() {
    if std::env::var_os("AGE319_FIRST_ACTIVATION_CHILD").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("root_mapped_first_install_activation_is_exact_and_restart_stable")
            .arg("--nocapture")
            .env("AGE319_FIRST_ACTIVATION_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success(), "root-mapped activation fixture failed");
        return;
    }
    let private = tempfile::tempdir().unwrap();
    let images = Images::new(private.path());
    let root = private.path().join("broker-state");
    let source = EmptyV30BootstrapIdentity::bootstrap_at(&root).unwrap();
    let partial = root.join(".first-install-activation-interrupted");
    fs::write(&partial, b"partial").unwrap();
    assert!(FirstInstallActivation::activate_at(&root, images.paths(), true).is_err());
    let partial_socket = private.path().join("partial.sock");
    let mut partial_start = broker(&root, &partial_socket, &images);
    assert!(
        !partial_start.wait().unwrap().success(),
        "partial activation stage admitted Broker"
    );
    fs::remove_file(&partial).unwrap();
    let mut old_pair = images.pair.clone();
    old_pair.schema = 1;
    old_pair.bash_sha256 = None;
    fs::write(&images.manifest, serde_json::to_vec(&old_pair).unwrap()).unwrap();
    assert!(FirstInstallActivation::activate_at(&root, images.paths(), true).is_err());
    images.restore_manifest();
    let original_bash = fs::read(&images.bash).unwrap();
    fs::write(&images.bash, b"changed-before-activation").unwrap();
    assert!(FirstInstallActivation::activate_at(&root, images.paths(), true).is_err());
    fs::write(&images.bash, &original_bash).unwrap();
    fs::set_permissions(&images.bash, fs::Permissions::from_mode(0o722)).unwrap();
    assert!(FirstInstallActivation::activate_at(&root, images.paths(), true).is_err());
    fs::set_permissions(&images.bash, fs::Permissions::from_mode(0o700)).unwrap();
    let activated = FirstInstallActivation::activate_at(&root, images.paths(), true).unwrap();
    assert_eq!(activated.source, source);
    assert_eq!(activated.pair_generation, images.pair.generation);
    assert_ne!(
        activated.source.source_generation,
        activated.pair_generation
    );
    assert_eq!(
        FirstInstallActivation::activate_at(&root, images.paths(), true).unwrap(),
        activated
    );

    let socket = private.path().join("control.sock");
    for _ in 0..2 {
        let mut child = broker(&root, &socket, &images);
        wait_route(&mut child, &socket, EntryRoute::FreshOnlyOpen);
        let pair = observe_installed_pair_at(&socket).unwrap_or_else(|error| {
            let raw = oulipoly_kernel_broker::protocol::request_at(
                &socket,
                oulipoly_kernel_broker::protocol::Operation::ObserveInstalledPair,
            )
            .unwrap();
            panic!("installed pair observation failed: {error}; raw={raw:?}");
        });
        assert_eq!(pair.route, EntryRoute::FreshOnlyOpen);
        assert_eq!(pair.generation, images.pair.generation);
        assert_eq!(
            pair.source_generation.as_deref(),
            Some(source.source_generation.as_str())
        );
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            FirstInstallActivation::readback_at(&root, images.paths(), true).unwrap(),
            activated
        );
    }

    let nonempty_root = private.path().join("nonempty-state");
    EmptyV30BootstrapIdentity::bootstrap_at(&nonempty_root).unwrap();
    rusqlite::Connection::open(nonempty_root.join("state.db"))
        .unwrap()
        .execute_batch("CREATE TABLE preactivation_debt(value TEXT); INSERT INTO preactivation_debt VALUES('old');")
        .unwrap();
    assert!(FirstInstallActivation::activate_at(&nonempty_root, images.paths(), true).is_err());
    assert!(
        !nonempty_root
            .join("first-install-activation-v1.json")
            .exists()
    );

    let unactivated_root = private.path().join("unactivated-state");
    EmptyV30BootstrapIdentity::bootstrap_at(&unactivated_root).unwrap();
    let unactivated_socket = private.path().join("unactivated.sock");
    let mut unactivated = broker(&unactivated_root, &unactivated_socket, &images);
    wait_route(
        &mut unactivated,
        &unactivated_socket,
        EntryRoute::BrokerV30Closed,
    );
    unactivated.kill().unwrap();
    unactivated.wait().unwrap();
    assert!(FirstInstallActivation::activate_at(&unactivated_root, images.paths(), true).is_err());

    let old_root = private.path().join("old-root");
    fs::create_dir(&old_root).unwrap();
    fs::set_permissions(&old_root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(FirstInstallActivation::activate_at(&old_root, images.paths(), true).is_err());
    let old_socket = private.path().join("old.sock");
    let mut old = broker(&old_root, &old_socket, &images);
    wait_route(&mut old, &old_socket, EntryRoute::BrokerV30Closed);
    old.kill().unwrap();
    old.wait().unwrap();

    let mut changed_pair = images.pair.clone();
    changed_pair.generation = uuid::Uuid::new_v4().to_string();
    fs::write(&images.manifest, serde_json::to_vec(&changed_pair).unwrap()).unwrap();
    assert!(FirstInstallActivation::readback_at(&root, images.paths(), true).is_err());
    images.restore_manifest();
    assert_eq!(
        FirstInstallActivation::readback_at(&root, images.paths(), true).unwrap(),
        activated
    );

    let record_path = root.join("first-install-activation-v1.json");
    let record_bytes = fs::read(&record_path).unwrap();
    fs::write(&record_path, b"{}").unwrap();
    assert!(FirstInstallActivation::readback_at(&root, images.paths(), true).is_err());
    let mut partial_record = broker(&root, &socket, &images);
    assert!(
        !partial_record.wait().unwrap().success(),
        "partial activation record admitted Broker"
    );
    fs::write(&record_path, record_bytes).unwrap();
    assert_eq!(
        FirstInstallActivation::readback_at(&root, images.paths(), true).unwrap(),
        activated
    );

    // The binding includes named inodes as well as digests. Equal bytes at
    // a newly installed path are a different image and refuse readback.
    let replacement = images.directory.join("replacement");
    fs::copy(&images.bash, &replacement).unwrap();
    fs::rename(&replacement, &images.bash).unwrap();
    assert!(FirstInstallActivation::readback_at(&root, images.paths(), true).is_err());
    let mut child = broker(&root, &socket, &images);
    assert!(
        !child.wait().unwrap().success(),
        "changed Bash image admitted Broker"
    );
}
