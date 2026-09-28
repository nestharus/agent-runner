#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_state::mailbox::{BrokerSidecar, EmptyV30BootstrapIdentity, FreshV30Lane};
use rusqlite::Connection;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;

#[test]
fn root_mapped_empty_broker_bootstrap_reopens_exact_identity_and_refuses_conflicts() {
    if std::env::var_os("AGE319_EMPTY_BOOTSTRAP_CHILD").is_none() {
        let status = Command::new("unshare")
            .arg("-Ur")
            .arg(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("root_mapped_empty_broker_bootstrap_reopens_exact_identity_and_refuses_conflicts")
            .arg("--nocapture")
            .env("AGE319_EMPTY_BOOTSTRAP_CHILD", "1")
            .status()
            .expect("root-mapped user namespace required");
        assert!(status.success(), "root-mapped bootstrap child failed");
        return;
    }

    let private = tempfile::tempdir().unwrap();
    let parent = private.path();
    let root = parent.join("broker-state");
    let identity = EmptyV30BootstrapIdentity::bootstrap_at(&root).unwrap();
    assert_eq!(
        EmptyV30BootstrapIdentity::bootstrap_at(&root).unwrap(),
        identity
    );
    assert_eq!(
        EmptyV30BootstrapIdentity::readback_at(&root).unwrap(),
        identity
    );
    let sidecar =
        BrokerSidecar::open_existing(&root.join("sidecar/pid-identity.db"), &root).unwrap();
    assert_eq!(sidecar.source_generation(), identity.source_generation);
    assert_eq!(sidecar.domain_id().unwrap(), identity.domain_id);
    let file = sidecar.bound_state_file_identity().unwrap();
    assert_eq!(
        (file.device, file.inode),
        (identity.state_device, identity.state_inode)
    );
    assert_eq!(
        FreshV30Lane::open_at(&root).unwrap().identity(),
        &identity.fresh_lane
    );
    let state = Connection::open(root.join("state.db")).unwrap();
    let version: i64 = state
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, i64::from(oulipoly_state::CURRENT_SCHEMA_VERSION));
    let invocations: i64 = state
        .query_row("SELECT count(*) FROM invocations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(invocations, 0);
    drop(state);
    drop(sidecar);
    for path in [
        root.join("state.db"),
        root.join("sidecar/pid-identity.db"),
        root.join("sidecar/state-source.json"),
        root.join("v30/state.db"),
        root.join("v30/sidecar/state-source.json"),
        root.join("empty-v30-bootstrap-v1.json"),
    ] {
        let meta = fs::symlink_metadata(path).unwrap();
        assert_eq!(meta.uid(), 0);
        assert_eq!(meta.mode() & 0o077, 0);
    }

    let incompatible = parent.join("existing-incompatible");
    fs::create_dir(&incompatible).unwrap();
    fs::set_permissions(&incompatible, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(incompatible.join("state.db"), b"foreign state").unwrap();
    assert!(EmptyV30BootstrapIdentity::bootstrap_at(&incompatible).is_err());
    assert!(!incompatible.join("sidecar").exists());

    let partial_parent = parent.join("partial-parent");
    fs::create_dir(&partial_parent).unwrap();
    fs::set_permissions(&partial_parent, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(partial_parent.join(".empty-v30-bootstrap-interrupted")).unwrap();
    assert!(EmptyV30BootstrapIdentity::bootstrap_at(&partial_parent.join("broker")).is_err());
    assert!(!partial_parent.join("broker").exists());

    // Replacing a bound State inode cannot be mistaken for a retry.
    let state_path = root.join("state.db");
    fs::rename(&state_path, root.join("state.db.removed")).unwrap();
    fs::write(&state_path, b"conflicting state").unwrap();
    fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(EmptyV30BootstrapIdentity::bootstrap_at(&root).is_err());
    fs::remove_file(&state_path).unwrap();
    fs::rename(root.join("state.db.removed"), &state_path).unwrap();
    fs::set_permissions(&state_path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(EmptyV30BootstrapIdentity::bootstrap_at(&root).is_err());
    fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        EmptyV30BootstrapIdentity::readback_at(&root).unwrap(),
        identity
    );

    let sidecar_path = root.join("sidecar/pid-identity.db");
    fs::rename(&sidecar_path, root.join("sidecar/pid-identity.db.removed")).unwrap();
    fs::write(&sidecar_path, b"conflicting sidecar").unwrap();
    fs::set_permissions(&sidecar_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(EmptyV30BootstrapIdentity::bootstrap_at(&root).is_err());
    fs::remove_file(&sidecar_path).unwrap();
    fs::rename(root.join("sidecar/pid-identity.db.removed"), &sidecar_path).unwrap();
    assert_eq!(
        EmptyV30BootstrapIdentity::readback_at(&root).unwrap(),
        identity
    );

    let binding_path = root.join("sidecar/state-source.json");
    let binding = fs::read(&binding_path).unwrap();
    fs::remove_file(&binding_path).unwrap();
    assert!(EmptyV30BootstrapIdentity::bootstrap_at(&root).is_err());
    fs::write(&binding_path, binding).unwrap();
    fs::set_permissions(&binding_path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        EmptyV30BootstrapIdentity::readback_at(&root).unwrap(),
        identity
    );

    fs::remove_file(root.join("empty-v30-bootstrap-v1.json")).unwrap();
    assert!(EmptyV30BootstrapIdentity::bootstrap_at(&root).is_err());
}
