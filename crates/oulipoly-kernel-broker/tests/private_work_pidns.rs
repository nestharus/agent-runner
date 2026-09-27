#![cfg(target_os = "linux")]
//! The private user namespace makes this test unprivileged. It proves PID
//! namespace lineage and registry recovery, not host-root sudo/setuid.
use oulipoly_kernel_broker::{
    PeerIdentity, PinnedProcess, RootRecord, RootRegistry, Scope, WorkRegistry, classify_scope,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const INIT: &str = r#"
import os, socket, subprocess, sys
assert os.getpid() == 1
assert open('/proc/self/stat').read().split()[0] == '1'
assert 'NoNewPrivs:\t0' in open('/proc/self/status').read()
transport, tag, init, daemon = sys.argv[1:]
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(transport); s.sendall(tag.encode())
children = []
for nested in {'A': ['C', 'D', 'H'], 'B': ['E'], 'C': ['F']}.get(tag, []):
    children.append(subprocess.Popen(['unshare', '--pid', '--fork', '--mount', '--mount-proc',
                                      'python3', '-c', init, transport, nested, init, daemon]))
if tag == 'C':
    subprocess.run(['python3', '-c', daemon, transport], env={}, close_fds=True,
                   start_new_session=True, check=True)
if s.recv(1) != b'X': sys.exit(41)
for child in children:
    if child.wait(timeout=30) != 0: sys.exit(42)
while True:
    try: pid, _ = os.waitpid(-1, os.WNOHANG)
    except ChildProcessError: break
    if pid == 0: break
"#;

const DAEMON: &str = r#"
import os, socket, sys, time
if os.fork(): os._exit(0)
os.setsid()
if os.fork(): os._exit(0)
assert not any(name.startswith('OULIPOLY_') for name in os.environ)
os.environ.clear()
while os.getppid() != 1: time.sleep(.01)
assert 'NoNewPrivs:\t0' in open('/proc/self/status').read()
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1]); s.sendall(b'G')
if s.recv(1) != b'X': os._exit(43)
os._exit(0)
"#;

struct FixtureChild(Child);
impl Drop for FixtureChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
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
        INIT,
        socket,
        tag,
        INIT,
        DAEMON,
    ]);
    command.process_group(0);
    FixtureChild(command.spawn().expect("spawn private root namespace"))
}

fn accept(listener: &UnixListener) -> (u8, UnixStream, PinnedProcess) {
    let until = Instant::now() + Duration::from_secs(30);
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
    (tag[0], stream, process)
}

fn root_record(process: &PinnedProcess) -> RootRecord {
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

fn peer(process: &PinnedProcess) -> PeerIdentity {
    PeerIdentity {
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
        process: PinnedProcess::open(process.host_pid).unwrap(),
    }
}

fn wait_dead(process: &PinnedProcess) {
    let until = Instant::now() + Duration::from_secs(30);
    while process.verify().is_ok() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(process.verify().is_err(), "private process did not exit");
}

#[test]
fn exact_sibling_and_nested_work_survives_ambient_loss_and_restart_keeps_debt() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut a = spawn(socket.to_str().unwrap(), "A");
    let mut b = spawn(socket.to_str().unwrap(), "B");
    let mut peers: HashMap<u8, (UnixStream, PinnedProcess)> = HashMap::new();
    for _ in 0..8 {
        let (tag, stream, process) = accept(&listener);
        assert!(peers.insert(tag, (stream, process)).is_none());
    }
    let state = temp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let work_state = state.join("works");
    std::fs::create_dir(&work_state).unwrap();
    let mut roots = RootRegistry::open(&state).unwrap();
    let root_a = root_record(&peers[&b'A'].1);
    let root_b = root_record(&peers[&b'B'].1);
    roots.insert(root_a.clone()).unwrap();
    roots.insert(root_b.clone()).unwrap();
    let mut works = WorkRegistry::open(&work_state, &roots).unwrap();
    // Fixture-only grant IDs exercise persistence and sibling uniqueness;
    // this test does not authenticate a guardian or launch accepted work.
    let c_grant = uuid::Uuid::new_v4().to_string();
    let c = works
        .insert_prepared_granted(
            &roots,
            &root_a.root_id,
            "accepted-c",
            &c_grant,
            None,
            peers[&b'C'].1.host_pid,
        )
        .unwrap();
    assert!(
        works
            .insert_prepared_granted(
                &roots,
                &root_a.root_id,
                "reused-grant",
                &c_grant,
                None,
                peers[&b'D'].1.host_pid
            )
            .is_err()
    );
    assert!(
        works
            .insert_prepared(
                &roots,
                &root_b.root_id,
                "wrong-root",
                None,
                peers[&b'D'].1.host_pid
            )
            .is_err()
    );
    let d = works
        .insert_prepared(
            &roots,
            &root_a.root_id,
            "accepted-d",
            None,
            peers[&b'D'].1.host_pid,
        )
        .unwrap();
    let e = works
        .insert_prepared(
            &roots,
            &root_b.root_id,
            "accepted-e",
            None,
            peers[&b'E'].1.host_pid,
        )
        .unwrap();
    assert!(
        works
            .insert_prepared(
                &roots,
                &root_a.root_id,
                "wrong-parent",
                Some(&d.work_incarnation),
                peers[&b'F'].1.host_pid
            )
            .is_err()
    );
    let f = works
        .insert_prepared(
            &roots,
            &root_a.root_id,
            "accepted-f",
            Some(&c.work_incarnation),
            peers[&b'F'].1.host_pid,
        )
        .unwrap();
    let host = File::open("/proc/self/ns/pid").unwrap();
    for (tag, expected) in [
        (b'C', (&root_a.root_id, &c)),
        (b'G', (&root_a.root_id, &c)),
        (b'D', (&root_a.root_id, &d)),
        (b'E', (&root_b.root_id, &e)),
        (b'F', (&root_a.root_id, &f)),
    ] {
        assert_eq!(
            classify_scope(&peer(&peers[&tag].1), &host, &roots, &works),
            Scope::Work {
                root_id: expected.0.clone(),
                work_id: expected.1.work_id.clone(),
                work_incarnation: expected.1.work_incarnation.clone()
            }
        );
    }
    assert_eq!(
        classify_scope(&peer(&peers[&b'A'].1), &host, &roots, &works),
        Scope::Root(root_a.root_id.clone())
    );
    assert_eq!(
        classify_scope(&peer(&peers[&b'B'].1), &host, &roots, &works),
        Scope::Root(root_b.root_id.clone())
    );
    assert_eq!(
        classify_scope(
            &peer(&PinnedProcess::open(std::process::id() as i32).unwrap()),
            &host,
            &roots,
            &works
        ),
        Scope::Outside
    );

    // Reopening while both roots and all works remain live preserves exact
    // nested attribution, including the adopted peer in C.
    let live_roots = RootRegistry::open(&state).unwrap();
    let live_works = WorkRegistry::open(&work_state, &live_roots).unwrap();
    assert!(live_works.debt_records().is_empty());
    assert_eq!(
        live_works
            .live_works()
            .find(|work| work.record.work_id == "accepted-c")
            .unwrap()
            .record
            .accepted_grant_id
            .as_deref(),
        Some(c_grant.as_str())
    );
    assert_eq!(
        classify_scope(&peer(&peers[&b'G'].1), &host, &live_roots, &live_works),
        Scope::Work {
            root_id: root_a.root_id.clone(),
            work_id: c.work_id.clone(),
            work_incarnation: c.work_incarnation.clone(),
        }
    );

    // A persistence failure poisons the live registry immediately. It may
    // have left a partial file; a later request cannot reuse the old view.
    std::fs::set_permissions(&work_state, std::fs::Permissions::from_mode(0o500)).unwrap();
    assert!(
        works
            .insert_prepared(
                &roots,
                &root_a.root_id,
                "unpersisted-h",
                None,
                peers[&b'H'].1.host_pid
            )
            .is_err()
    );
    assert!(works.has_debt());
    assert_eq!(
        classify_scope(&peer(&peers[&b'D'].1), &host, &roots, &works),
        Scope::Uncertain
    );
    std::fs::set_permissions(&work_state, std::fs::Permissions::from_mode(0o700)).unwrap();

    // The double-forked G has no inherited environment or broker FD and its
    // immediate ancestors are gone; its namespace still identifies C exactly.
    for tag in [b'G', b'F', b'C'] {
        peers.get_mut(&tag).unwrap().0.write_all(b"X").unwrap();
        wait_dead(&peers[&tag].1);
    }
    let recovered_roots = RootRegistry::open(&state).unwrap();
    let recovered = WorkRegistry::open(&work_state, &recovered_roots).unwrap();
    assert_eq!(recovered.debt_records().len(), 2);
    assert_eq!(
        classify_scope(
            &peer(&PinnedProcess::open(std::process::id() as i32).unwrap()),
            &host,
            &recovered_roots,
            &recovered
        ),
        Scope::Uncertain
    );
    assert!(
        recovered_roots
            .live_roots()
            .any(|root| root.record.root_id == root_b.root_id)
    );

    for tag in [b'D', b'E', b'H', b'A', b'B'] {
        peers.get_mut(&tag).unwrap().0.write_all(b"X").unwrap();
        wait_dead(&peers[&tag].1);
    }
    assert!(a.0.wait().unwrap().success());
    assert!(b.0.wait().unwrap().success());
}
