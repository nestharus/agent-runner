#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
//! Private physical readback surrogate. The Rust source PID1 reaps and writes
//! its terminal receipt; production source launch/consume remains closed.
use oulipoly_kernel_broker::RootRecord;
use oulipoly_kernel_broker::entry_registry::{EntryRecord, ProcessStamp};
use oulipoly_kernel_broker::identity::{PinnedProcess, observed_incarnation_gone};
use oulipoly_kernel_broker::source_physical::{
    SourceObservation, SourcePhysicalRegistry, install_source_pid1_cancel_handler,
    reap_source_pid1_with_pipes,
};
use oulipoly_state::completion_continuation::{ListenerIdentity, SourceProcessIdentity};
use oulipoly_state::mailbox::{BrokerSourceCandidate, BrokerSourceEffectGrant};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const JOINED: &str = r#"
import os, socket, sys
s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.sendall(b'J')
assert s.recv(1)==b'X'
"#;

const WORKER: &str = r#"
import os, socket, sys, time
socket_path,directory,grant=sys.argv[1:]
w=socket.socket(socket.AF_UNIX); w.connect(socket_path)
w.sendall(b'W'+os.getpid().to_bytes(4,'little'))
assert w.recv(1)==b'R'
os.write(1,b'worker-start\n'); os.write(2,b'worker-stderr\n')
if os.getenv('AGE319_PRIVATE_SOURCE_LARGE'):
    block=b'x'*65536
    for _ in range(1040): os.write(1,block)
if os.fork()==0:
    os.setsid(); time.sleep(6.5)
    os.write(1,b'adopted-after-five-seconds\n')
    os._exit(0)
os._exit(7)
"#;

const ROOT: &str = r#"
import os, socket, subprocess, sys, time
socket_path,directory,grant,joined,source_exe=sys.argv[1:]
assert os.getpid()==1
s=socket.socket(socket.AF_UNIX); s.connect(socket_path); s.sendall(b'R')
subprocess.Popen(['python3','-c',joined,socket_path])
env=os.environ.copy(); env['AGE319_PRIVATE_SOURCE_PID1']='1'
subprocess.Popen(['unshare','--pid','--fork','--mount','--mount-proc',source_exe,
                  '--exact','post_owner_source_readback_waits_for_adopted_descendant','--nocapture'],env=env)
while True:
    try: os.waitpid(-1,os.WNOHANG)
    except ChildProcessError: pass
    time.sleep(.05)
"#;

const GUARDIAN: &str = r#"
import os, socket, sys
path=sys.argv[1]
g=socket.socket(socket.AF_UNIX); g.connect(path); g.sendall(b'G')
if os.fork()==0:
    d=socket.socket(socket.AF_UNIX); d.connect(path); d.sendall(b'D')
    assert d.recv(1)==b'X'
    os._exit(0)
assert g.recv(1)==b'X'
os.waitpid(-1,0)
"#;

fn accept(listener: &UnixListener) -> (u8, UnixStream, PinnedProcess, Option<i32>) {
    let until = Instant::now() + Duration::from_secs(20);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(peer) => break peer,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("private source actor absent: {error}"),
        }
    };
    let mut tag = [0];
    stream.read_exact(&mut tag).unwrap();
    let local = if tag == [b'W'] {
        let mut bytes = [0; 4];
        stream.read_exact(&mut bytes).unwrap();
        Some(i32::from_le_bytes(bytes))
    } else {
        None
    };
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    assert_eq!(
        unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut length,
            )
        },
        0
    );
    let process = PinnedProcess::open(unsafe { credentials.assume_init().pid }).unwrap();
    (tag[0], stream, process, local)
}

fn dead(process: &PinnedProcess) -> bool {
    observed_incarnation_gone(
        process.host_pid,
        &process.boot_id,
        process.starttime_ticks,
        (process.pidns_dev, process.pidns_ino),
    )
    .unwrap()
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn source_pid1() {
    assert_eq!(unsafe { libc::getpid() }, 1);
    let socket = std::env::var("AGE319_SOURCE_SOCKET").unwrap();
    let directory = std::path::PathBuf::from(std::env::var("AGE319_SOURCE_DIRECTORY").unwrap());
    let grant = std::env::var("AGE319_SOURCE_GRANT").unwrap();
    let mut control = UnixStream::connect(&socket).unwrap();
    control.write_all(b"P").unwrap();
    install_source_pid1_cancel_handler().unwrap();
    let mut worker = Command::new("python3")
        .args(["-c", WORKER, &socket, directory.to_str().unwrap(), &grant])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    let record = loop {
        if let Ok(registry) = SourcePhysicalRegistry::open(&directory) {
            if let Some(record) = registry
                .records()
                .iter()
                .find(|r| r.grant.grant_id == grant)
            {
                break record.clone();
            }
        }
        assert!(Instant::now() < until, "held source record absent");
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout_file = fs::OpenOptions::new()
        .write(true)
        .open(directory.join(format!("{grant}.stdout")))
        .unwrap();
    let stderr_file = fs::OpenOptions::new()
        .write(true)
        .open(directory.join(format!("{grant}.stderr")))
        .unwrap();
    reap_source_pid1_with_pipes(
        &directory,
        &record,
        worker.stdout.take().unwrap(),
        worker.stderr.take().unwrap(),
        stdout_file,
        stderr_file,
    )
    .unwrap();
}

fn inner() {
    let cancel_mode = std::env::var_os("AGE319_PRIVATE_SOURCE_CANCEL").is_some();
    let large_mode = std::env::var_os("AGE319_PRIVATE_SOURCE_LARGE").is_some();
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("source-physical");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let orphan_dir = temp.path().join("orphaned-source");
    fs::create_dir(&orphan_dir).unwrap();
    fs::set_permissions(&orphan_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let orphan_id = uuid::Uuid::new_v4().to_string();
    SourcePhysicalRegistry::open(&orphan_dir)
        .unwrap()
        .prepare_outputs(&orphan_id)
        .unwrap();
    let orphaned = SourcePhysicalRegistry::open(&orphan_dir).unwrap();
    assert!(orphaned.has_debt());
    assert_eq!(orphaned.orphaned_grants(), &[orphan_id]);
    let grant_id = uuid::Uuid::new_v4().to_string();
    let mut registry = SourcePhysicalRegistry::open(&directory).unwrap();
    let (_stdout, _stderr) = registry.prepare_outputs(&grant_id).unwrap();
    let socket = temp.path().join("actors.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut guardian_child = Command::new("python3")
        .args(["-c", GUARDIAN, socket.to_str().unwrap()])
        .spawn()
        .unwrap();
    let mut root_child = Command::new("unshare")
        .args([
            "--pid",
            "--fork",
            "--mount",
            "--mount-proc",
            "python3",
            "-c",
            ROOT,
            socket.to_str().unwrap(),
            directory.to_str().unwrap(),
            &grant_id,
            JOINED,
            std::env::current_exe().unwrap().to_str().unwrap(),
        ])
        .env("AGE319_SOURCE_SOCKET", &socket)
        .env("AGE319_SOURCE_DIRECTORY", &directory)
        .env("AGE319_SOURCE_GRANT", &grant_id)
        .spawn()
        .unwrap();
    let mut actors = HashMap::new();
    for _ in 0..6 {
        let (tag, stream, process, local) = accept(&listener);
        assert!(actors.insert(tag, (stream, process, local)).is_none());
    }
    let root = &actors[&b'R'].1;
    let guardian = &actors[&b'G'].1;
    let driver = &actors[&b'D'].1;
    let joined = &actors[&b'J'].1;
    let pid1 = &actors[&b'P'].1;
    let worker = &actors[&b'W'].1;
    let root_id = uuid::Uuid::new_v4().to_string();
    let root_record = RootRecord {
        version: 1,
        boot_id: root.boot_id.clone(),
        root_id: root_id.clone(),
        owner_uid: 0,
        init_host_pid: root.host_pid,
        init_starttime_ticks: root.starttime_ticks,
        pidns_dev: root.pidns_dev,
        pidns_ino: root.pidns_ino,
    };
    let entry = EntryRecord {
        version: 1,
        root_id: root_id.clone(),
        owner_uid: 0,
        entry: ProcessStamp::from(guardian),
        prepared_guardian: Some(ProcessStamp::from(guardian)),
        domain_id: Some(uuid::Uuid::new_v4().to_string()),
        supervisor_authority_id: Some(uuid::Uuid::new_v4().to_string()),
        guardian: Some(ProcessStamp::from(guardian)),
        join_consumed: true,
        joined_child: Some(ProcessStamp::from(joined)),
        prepared_driver: Some(ProcessStamp::from(driver)),
    };
    let source_generation = uuid::Uuid::new_v4().to_string();
    let grant = BrokerSourceEffectGrant {
        grant_id: grant_id.clone(),
        source_generation,
        root_id,
        owner_generation: uuid::Uuid::new_v4().to_string(),
        driver_identity: SourceProcessIdentity {
            pid: i64::from(driver.host_pid),
            boot_id: driver.boot_id.clone(),
            starttime_ticks: driver.starttime_ticks as i64,
        },
        authority_ordinal: 1,
        candidate: BrokerSourceCandidate {
            registration_id: uuid::Uuid::new_v4().to_string(),
            registration_digest: "a".repeat(64),
            listener_revision: 1,
            listener: ListenerIdentity {
                listener_id: uuid::Uuid::new_v4().to_string(),
                session_id: "fixture".into(),
                owner_invocation_uuid: uuid::Uuid::new_v4().to_string(),
            },
        },
        phase: "consumed".into(),
        revision: 2,
    };
    let record = registry
        .insert_held(
            grant,
            &root_record,
            &entry,
            root,
            guardian,
            driver,
            pid1,
            worker,
            actors[&b'W'].2.unwrap(),
        )
        .unwrap();
    assert_eq!(record.grant.grant_id, grant_id);
    // A lost broker reply/restart reopens this record; no second held child
    // can be recorded for the same consumed grant.
    drop(registry);
    let mut registry = SourcePhysicalRegistry::open(&directory).unwrap();
    assert!(
        registry
            .insert_held(
                record.grant.clone(),
                &root_record,
                &entry,
                root,
                guardian,
                driver,
                pid1,
                worker,
                record.worker_local_pid
            )
            .is_err()
    );
    assert!(matches!(
        registry.observe(&grant_id).unwrap(),
        SourceObservation::Live { .. }
    ));
    actors[&b'W']
        .0
        .try_clone()
        .unwrap()
        .write_all(b"R")
        .unwrap();
    actors[&b'J']
        .0
        .try_clone()
        .unwrap()
        .write_all(b"X")
        .unwrap();
    actors[&b'D']
        .0
        .try_clone()
        .unwrap()
        .write_all(b"X")
        .unwrap();
    actors[&b'G']
        .0
        .try_clone()
        .unwrap()
        .write_all(b"X")
        .unwrap();
    guardian_child.wait().unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !dead(joined) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        dead(joined) && dead(guardian) && dead(driver),
        "original actors must be gone"
    );
    let until = Instant::now() + Duration::from_secs(10);
    while !dead(worker) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        dead(worker),
        "exact worker must have exited before adopted drain"
    );
    assert!(
        matches!(
            registry.observe(&grant_id).unwrap(),
            SourceObservation::Live { .. }
        ),
        "worker wait is not adopted-tree drain"
    );
    if cancel_mode {
        // Simulate a broker crash after the cancellation intent fsync and
        // before its signal reply. Reopen and reissue only to the stamped PID1.
        let marker = directory.join(format!("{grant_id}.cancel.json"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(marker)
            .unwrap();
        serde_json::to_writer(&mut file, &record.pid1).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
        fs::File::open(&directory).unwrap().sync_all().unwrap();
        drop(registry);
        registry = SourcePhysicalRegistry::open(&directory).unwrap();
        let results = registry.reconcile_cancellations();
        assert_eq!(results.len(), 1);
        results.into_iter().next().unwrap().1.unwrap();
        assert!(matches!(
            registry.observe(&grant_id).unwrap(),
            SourceObservation::Live {
                cancel_requested: true
            } | SourceObservation::DrainPending {
                cancel_requested: true
            } | SourceObservation::Drained {
                cancel_requested: true,
                ..
            }
        ));
    } else {
        std::thread::sleep(Duration::from_millis(5100));
        assert!(
            matches!(
                registry.observe(&grant_id).unwrap(),
                SourceObservation::Live { .. }
            ),
            "adopted descendant over five seconds must block terminal"
        );
    }
    let until = Instant::now() + Duration::from_secs(10);
    let drained = loop {
        let observation = registry.observe(&grant_id).unwrap();
        if let SourceObservation::Drained {
            worker_wait_status,
            stdout,
            stderr,
            cancel_requested,
        } = observation
        {
            break (worker_wait_status, stdout, stderr, cancel_requested);
        }
        assert!(
            Instant::now() < until,
            "source PID1 did not drain: {observation:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(drained.0, 7 << 8);
    let mut expected = Sha256::new();
    expected.update(b"worker-start\n");
    let mut expected_len = b"worker-start\n".len() as u64;
    if large_mode {
        let block = [b'x'; 65536];
        for _ in 0..1040 {
            expected.update(block);
            expected_len += block.len() as u64;
        }
    }
    if !cancel_mode {
        expected.update(b"adopted-after-five-seconds\n");
        expected_len += b"adopted-after-five-seconds\n".len() as u64;
    }
    assert_eq!(drained.1.byte_len, expected_len);
    assert_eq!(drained.1.sha256, format!("{:x}", expected.finalize()));
    assert_eq!(drained.2.byte_len, b"worker-stderr\n".len() as u64);
    assert_eq!(
        drained.2.sha256,
        format!("{:x}", Sha256::digest(b"worker-stderr\n"))
    );
    assert_eq!(drained.3, cancel_mode);
    let stdout_path = directory.join(format!("{grant_id}.stdout"));
    let mut start = [0; 13];
    fs::File::open(&stdout_path)
        .unwrap()
        .read_exact(&mut start)
        .unwrap();
    assert_eq!(&start, b"worker-start\n");
    assert!(dead(pid1));
    // A producer-reported disk failure cannot be mistaken for a successful
    // terminal even when otherwise valid output and receipt files exist.
    let incomplete_path = directory.join(format!("{grant_id}.incomplete.json"));
    let mut incomplete = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&incomplete_path)
        .unwrap();
    serde_json::to_writer(
        &mut incomplete,
        &oulipoly_kernel_broker::source_physical::SourceIncompleteReceipt {
            version: 1,
            grant_id: grant_id.clone(),
            stage: "stdout-pump".into(),
            stdout_bytes: Some(drained.1.byte_len),
            stderr_bytes: Some(drained.2.byte_len),
            io_error: "ENOSPC after captured bytes".into(),
        },
    )
    .unwrap();
    incomplete.write_all(b"\n").unwrap();
    incomplete.sync_all().unwrap();
    assert!(matches!(
        registry.observe(&grant_id).unwrap(),
        SourceObservation::Unknown {
            reason: "incomplete-capture",
            diagnostic: Some(_),
            ..
        }
    ));
    fs::remove_file(&incomplete_path).unwrap();
    // Post-terminal tampering with either output or the receipt loses the
    // positive observation, even if the same inode is changed in place.
    fs::OpenOptions::new()
        .append(true)
        .open(&stdout_path)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    assert!(matches!(
        registry.observe(&grant_id).unwrap(),
        SourceObservation::Unknown {
            reason: "incomplete-or-changed-output",
            ..
        }
    ));
    let terminal_path = directory.join(format!("{grant_id}.terminal.json"));
    let removed = directory.join(format!("{grant_id}.receipt-held"));
    fs::rename(&terminal_path, &removed).unwrap();
    assert!(matches!(
        registry.observe(&grant_id).unwrap(),
        SourceObservation::Unknown {
            reason: "missing-terminal-receipt",
            ..
        }
    ));
    stop(&mut root_child);
}

#[test]
fn post_owner_source_readback_waits_for_adopted_descendant() {
    if std::env::var_os("AGE319_PRIVATE_SOURCE_PID1").is_some() {
        source_pid1();
        return;
    }
    if std::env::var_os("AGE319_PRIVATE_SOURCE_INNER").is_some() {
        inner();
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "post_owner_source_readback_waits_for_adopted_descendant",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_SOURCE_INNER", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn post_owner_source_cancellation_drains_adopted_descendant() {
    if std::env::var_os("AGE319_PRIVATE_SOURCE_INNER").is_some() {
        inner();
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "post_owner_source_cancellation_drains_adopted_descendant",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_SOURCE_INNER", "1")
        .env("AGE319_PRIVATE_SOURCE_CANCEL", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn post_owner_source_capture_exceeds_old_cutoff() {
    if std::env::var_os("AGE319_PRIVATE_SOURCE_INNER").is_some() {
        inner();
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "post_owner_source_capture_exceeds_old_cutoff",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_SOURCE_INNER", "1")
        .env("AGE319_PRIVATE_SOURCE_LARGE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
