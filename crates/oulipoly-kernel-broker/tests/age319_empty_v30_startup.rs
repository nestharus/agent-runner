#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_kernel_broker::protocol::{EntryRoute, observe_entry_gate_at};
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn root_mapped_broker_reopens_empty_bootstrap_on_restart() {
    if std::env::var_os("AGE319_EMPTY_STARTUP_CHILD").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("root_mapped_broker_reopens_empty_bootstrap_on_restart")
            .arg("--nocapture")
            .env("AGE319_EMPTY_STARTUP_CHILD", "1")
            .status()
            .expect("root-mapped user namespace required");
        assert!(status.success(), "root-mapped Broker startup child failed");
        return;
    }
    let private = tempfile::tempdir().unwrap();
    let root = private.path().join("broker");
    let socket = private.path().join("control.sock");
    let identity = EmptyV30BootstrapIdentity::bootstrap_at(&root).unwrap();
    for _ in 0..2 {
        let mut broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &root)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
                std::env::current_exe().unwrap(),
            )
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(route) = observe_entry_gate_at(&socket) {
                assert_eq!(route, EntryRoute::BrokerV30Closed);
                break;
            }
            if let Some(status) = broker.try_wait().unwrap() {
                panic!("Broker exited before State reopen: {status}");
            }
            assert!(Instant::now() < deadline, "Broker State reopen timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
        broker.kill().unwrap();
        broker.wait().unwrap();
        assert_eq!(
            EmptyV30BootstrapIdentity::readback_at(&root).unwrap(),
            identity
        );
    }
    std::fs::write(root.join("empty-v30-bootstrap-v1.json"), b"{}").unwrap();
    let failed = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &root)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
            std::env::current_exe().unwrap(),
        )
        .status()
        .unwrap();
    assert!(
        !failed.success(),
        "partial bootstrap identity admitted service"
    );
    std::fs::remove_file(root.join("empty-v30-bootstrap-v1.json")).unwrap();
    let missing = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &root)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
            std::env::current_exe().unwrap(),
        )
        .status()
        .unwrap();
    assert!(
        !missing.success(),
        "missing bootstrap identity admitted service"
    );
}
