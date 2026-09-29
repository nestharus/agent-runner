#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::protocol::{EntryRoute, Operation, observe_entry_gate_at, request_at};
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[test]
fn root_mapped_connected_l_reserves_exact_e_and_direct_runner_refuses() {
    let Ok(runner_bin) = std::env::var("AGE319_CONNECTED_RUNNER_BIN") else {
        return;
    };
    if std::env::var_os("AGE319_CONNECTED_CHILD_TEST").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "root_mapped_connected_l_reserves_exact_e_and_direct_runner_refuses",
                "--nocapture",
            ])
            .env("AGE319_CONNECTED_CHILD_TEST", "1")
            .env("AGE319_CONNECTED_RUNNER_BIN", runner_bin)
            .status()
            .unwrap();
        assert!(status.success(), "root-mapped connected fixture failed");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let installed = temp.path().join("installed");
    fs::create_dir(&installed).unwrap();
    let runner = installed.join("oulipoly-agent-runner");
    let broker_image = installed.join("oulipoly-kernel-broker");
    let launcher = installed.join("oulipoly-installed-launcher");
    let bash = installed.join("agent-bash");
    fs::copy(&runner_bin, &runner).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"), &broker_image).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_oulipoly-installed-launcher"), &launcher).unwrap();
    fs::copy(&broker_image, &bash).unwrap();
    let manifest = installed.join("install-v1.json");
    let pair = InstalledPair {
        schema: 2,
        version: env!("CARGO_PKG_VERSION").into(),
        generation: uuid::Uuid::new_v4().to_string(),
        runner_sha256: digest(&runner),
        broker_sha256: digest(&broker_image),
        launcher_sha256: Some(digest(&launcher)),
        bash_sha256: Some(digest(&bash)),
    };
    fs::write(&manifest, serde_json::to_vec(&pair).unwrap()).unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let state = temp.path().join("state");
    let source = EmptyV30BootstrapIdentity::bootstrap_at(&state).unwrap();
    FirstInstallActivation::activate_at(
        &state,
        PairPaths {
            manifest: &manifest,
            runner: &runner,
            broker: &broker_image,
            launcher: &launcher,
            bash: &bash,
        },
        true,
    )
    .unwrap();
    let socket = temp.path().join("control.sock");
    let mut broker = Command::new(&broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &bash)
        .env("OULIPOLY_AGE319_PRIVATE_CONNECTED_CONTROL_V1", "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while observe_entry_gate_at(&socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
        assert!(broker.try_wait().unwrap().is_none(), "Broker exited");
        assert!(Instant::now() < deadline, "Broker did not activate");
        std::thread::sleep(Duration::from_millis(20));
    }
    let direct = |copied: bool| {
        let mut command = Command::new(&runner);
        command
            .arg("--help")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if copied {
            command.env("OULIPOLY_KERNEL_CONNECTED_CONTROL_FD_V1", "3");
        }
        assert!(
            !command.status().unwrap().success(),
            "direct Runner entered"
        );
    };
    direct(false);
    direct(true);
    let forged_socket = || {
        let (candidate, _peer) = UnixStream::pair().unwrap();
        let inherited = candidate.as_raw_fd();
        let mut command = Command::new(&runner);
        command
            .arg("--help")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_CONNECTED_CONTROL_FD_V1", "3")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(inherited, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        assert!(
            !command.status().unwrap().success(),
            "unrelated socket forged grant"
        );
    };
    forged_socket();
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 0);
    let pause = temp.path().join("connected-pause");
    fs::create_dir(&pause).unwrap();
    let launcher_status = Command::new(&launcher)
        .arg("--help")
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_AGE319_PRIVATE_DUPLICATE_L_V1", "1")
        .env("AGE319_CONNECTED_PAUSE_DIR", &pause)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1",
            &pair.generation,
        )
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(launcher_status.code(), Some(70)); // L remains pending/unknown.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pause.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "control Runner did not accept connected grant"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        observe_entry_gate_at(&socket).unwrap(),
        EntryRoute::FreshOnlyOpen
    );
    assert!(
        request_at(&socket, Operation::ReserveV30Entry)
            .unwrap()
            .starts_with("error ")
    );
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 0);
    fs::write(pause.join("release"), b"release").unwrap();
    let root_id = loop {
        let mut files = fs::read_dir(state.join("installed-launches")).unwrap();
        if let Some(file) = files.next() {
            let record: serde_json::Value =
                serde_json::from_slice(&fs::read(file.unwrap().path()).unwrap()).unwrap();
            break record["root_id"].as_str().unwrap().to_owned();
        }
        assert!(Instant::now() < deadline, "L did not publish");
        std::thread::sleep(Duration::from_millis(20));
    };
    let entry = state.join("entries").join(format!("{root_id}.json"));
    while !entry.exists() {
        assert!(
            Instant::now() < deadline,
            "connected E did not reserve exact ledger root"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let reserved: serde_json::Value = serde_json::from_slice(&fs::read(&entry).unwrap()).unwrap();
    assert_eq!(reserved["root_id"], root_id);
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    assert!(
        request_at(&socket, Operation::ReserveV30Entry)
            .unwrap()
            .starts_with("error ")
    );
    direct(false);
    direct(true);
    forged_socket();
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    assert_eq!(
        source.source_generation,
        EmptyV30BootstrapIdentity::readback_at(&state)
            .unwrap()
            .source_generation
    );
    broker.kill().unwrap();
    broker.wait().unwrap();
}
