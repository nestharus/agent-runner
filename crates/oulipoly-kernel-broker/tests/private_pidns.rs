#![cfg(target_os = "linux")]
use oulipoly_kernel_broker::{
    Classification, PeerIdentity, PinnedProcess, RootRecord, RootRegistry, classify_peer,
};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const SCRIPT: &str = r#"
import os, socket, subprocess, sys
assert os.getpid() == 1
assert open('/proc/self/stat').read().split()[0] == '1'
assert 'NoNewPrivs:\t0' in open('/proc/self/status').read()
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1]); s.sendall(sys.argv[2].encode())
child = None
if sys.argv[2] == 'A':
    child = subprocess.Popen(['unshare', '--pid', '--fork', '--mount', '--mount-proc',
                              'python3', '-c', sys.argv[3], sys.argv[1], 'N'])
if s.recv(1) != b'X': sys.exit(41)
if child is not None and child.wait(timeout=5) != 0: sys.exit(42)
"#;

struct FixtureChild(Child);
impl Drop for FixtureChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn spawn(socket: &str, tag: &str) -> FixtureChild {
    let mut command = Command::new("unshare");
    command.args([
        "--user",
        "--map-root-user",
        "--pid",
        "--fork",
        "--mount",
        "--mount-proc",
        "python3",
        "-c",
        SCRIPT,
        socket,
        tag,
        SCRIPT,
    ]);
    command.process_group(0);
    FixtureChild(command.spawn().expect("spawn private namespace"))
}

fn accept(listener: &UnixListener) -> (u8, UnixStream, PinnedProcess) {
    let until = Instant::now() + Duration::from_secs(5);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(peer) => break peer,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) => panic!("private namespace peer did not connect: {error}"),
        }
    };
    let mut tag = [0];
    stream.read_exact(&mut tag).unwrap();
    let mut cred = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut length,
        )
    };
    assert_eq!(result, 0);
    let process = PinnedProcess::open(unsafe { cred.assume_init().pid }).unwrap();
    (tag[0], stream, process)
}

use std::os::fd::AsRawFd;

fn record(process: &PinnedProcess) -> RootRecord {
    RootRecord {
        version: 1,
        boot_id: process.boot_id.clone(),
        root_id: uuid::Uuid::new_v4().to_string(),
        owner_uid: unsafe { libc::getuid() },
        init_host_pid: process.host_pid,
        init_starttime_ticks: process.starttime_ticks,
        pidns_dev: process.pidns_dev,
        pidns_ino: process.pidns_ino,
    }
}

fn wait_dead(process: &PinnedProcess) {
    let until = Instant::now() + Duration::from_secs(5);
    while process.verify().is_ok() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        process.verify().is_err(),
        "private namespace child did not exit"
    );
}

#[test]
fn sibling_nested_classifier_and_durable_unknown_debt() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut a = spawn(socket.to_str().unwrap(), "A");
    let mut b = spawn(socket.to_str().unwrap(), "B");
    let mut peers = std::collections::HashMap::new();
    for _ in 0..3 {
        let (tag, socket, process) = accept(&listener);
        peers.insert(tag, (socket, process));
    }
    let host_ns = File::open("/proc/self/ns/pid").unwrap();
    let registry_error = RootRegistry::open(temp.path()).unwrap_err();
    // A socket in the registry directory is intentionally rejected. Separate
    // the durable registry from the fixture transport.
    assert_eq!(registry_error.kind(), std::io::ErrorKind::Other);
    let state = temp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let mut registry = RootRegistry::open(&state).unwrap();
    let root_a = record(&peers[&b'A'].1);
    let root_b = record(&peers[&b'B'].1);
    registry.insert(root_a.clone()).unwrap();
    registry.insert(root_b.clone()).unwrap();
    let classify = |tag| {
        let process = &peers[&tag].1;
        let peer = PeerIdentity {
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            process: PinnedProcess::open(process.host_pid).unwrap(),
        };
        classify_peer(&peer, &host_ns, &registry)
    };
    assert_eq!(
        classify(b'A'),
        Classification::Inside(root_a.root_id.clone())
    );
    assert_eq!(
        classify(b'N'),
        Classification::Inside(root_a.root_id.clone())
    );
    assert_eq!(
        classify(b'B'),
        Classification::Inside(root_b.root_id.clone())
    );
    let outside = PeerIdentity {
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        process: PinnedProcess::open(std::process::id() as i32).unwrap(),
    };
    assert_eq!(
        classify_peer(&outside, &host_ns, &registry),
        Classification::Outside
    );
    peers.get_mut(&b'N').unwrap().0.write_all(b"X").unwrap();
    wait_dead(&peers[&b'N'].1);
    peers.get_mut(&b'A').unwrap().0.write_all(b"X").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while a.0.try_wait().unwrap().is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(a.0.try_wait().unwrap().unwrap().success());
    wait_dead(&peers[&b'A'].1);
    let recovered = RootRegistry::open(&state).unwrap();
    assert_eq!(recovered.debt_records().len(), 1);
    assert_eq!(
        classify_peer(&outside, &host_ns, &recovered),
        Classification::Uncertain
    );
    peers.get_mut(&b'B').unwrap().0.write_all(b"X").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while b.0.try_wait().unwrap().is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(b.0.try_wait().unwrap().unwrap().success());
    wait_dead(&peers[&b'B'].1);
}
