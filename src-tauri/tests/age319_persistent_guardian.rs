//! Private user-namespace exercise of the production host-entry and completion
//! guardian path. The socket peer has namespace UID 0, never host privilege.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_kernel_broker::protocol;
use oulipoly_state::mailbox::MailboxDb;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    pid: i32,
    start: u64,
}

fn stamp(pid: i32) -> Option<Stamp> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(')')?.1;
    let fields: Vec<_> = tail.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    Some(Stamp {
        pid,
        start: fields.get(19)?.parse().ok()?,
    })
}

fn parent_pid(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn peer_pid(stream: &UnixStream) -> i32 {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    assert_eq!(
        unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        },
        0
    );
    assert_eq!(cred.uid, 0);
    cred.pid
}

#[derive(Default)]
struct Ledger {
    root: String,
    entry: Option<Stamp>,
    guardian: Option<Stamp>,
    domain: String,
    supervisor: String,
    phase: u8,
    reads: usize,
}

fn serve_one(mut stream: UnixStream, ledger: &mut Ledger, debt: &Path, mode: &str) {
    let peer = peer_pid(&stream);
    stream.write_all(&[23; 16]).unwrap();
    let mut head = [0u8; 17];
    if stream.read_exact(&mut head).is_err() {
        return;
    }
    let mut rest = vec![
        0;
        match head[0] {
            b'P' => 20,
            b'G' => 48,
            b'A' => 16,
            _ => 0,
        }
    ];
    if stream.read_exact(&mut rest).is_err() || head[1..] != [23; 16] {
        return;
    }
    let root = if rest.len() >= 16 {
        uuid::Uuid::from_slice(&rest[..16]).unwrap().to_string()
    } else {
        String::new()
    };
    let answer = match head[0] {
        b'E' if ledger.phase == 0 => {
            ledger.root = uuid::Uuid::new_v4().to_string();
            ledger.entry = stamp(peer);
            ledger.phase = 1;
            format!("reserved {}\n", ledger.root)
        }
        b'P' if mode != "refuse_prepare"
            && ledger.phase == 1
            && root == ledger.root
            && ledger.entry == stamp(peer) =>
        {
            let pid = i32::from_ne_bytes(rest[16..20].try_into().unwrap());
            if parent_pid(pid) == Some(peer) {
                ledger.guardian = stamp(pid);
                ledger.phase = 2;
                format!("prepared {root}\n")
            } else {
                "error guardian parent\n".into()
            }
        }
        b'G' if mode != "refuse_bind"
            && ledger.phase == 2
            && root == ledger.root
            && ledger.guardian == stamp(peer) =>
        {
            ledger.domain = uuid::Uuid::from_slice(&rest[16..32]).unwrap().to_string();
            ledger.supervisor = uuid::Uuid::from_slice(&rest[32..48]).unwrap().to_string();
            ledger.phase = 3;
            format!("bound {root} {} {}\n", ledger.domain, ledger.supervisor)
        }
        b'A' if ledger.phase == 3
            && root == ledger.root
            && ledger.entry == stamp(peer)
            && ledger
                .guardian
                .is_some_and(|guardian| stamp(guardian.pid) == Some(guardian)) =>
        {
            ledger.reads += 1;
            format!(
                "bound-entry {root} {} {} {}\n",
                ledger.domain,
                ledger.supervisor,
                ledger.guardian.unwrap().pid
            )
        }
        _ => "error denied\n".into(),
    };
    if ledger.phase != 0 {
        std::fs::write(
            debt,
            format!(
                "{} {} {} {}\n",
                ledger.root, ledger.phase, ledger.domain, ledger.supervisor
            ),
        )
        .unwrap();
        std::fs::File::open(debt).unwrap().sync_all().unwrap();
    }
    stream.write_all(answer.as_bytes()).unwrap();
}

fn broker(path: &Path, debt: PathBuf, mode: &str) -> Arc<Mutex<Ledger>> {
    let listener = UnixListener::bind(path).unwrap();
    let ledger = Arc::new(Mutex::new(Ledger::default()));
    let shared = Arc::clone(&ledger);
    let mode = mode.to_owned();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            serve_one(stream, &mut shared.lock().unwrap(), &debt, &mode);
        }
    });
    ledger
}

fn eventually(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < until, "private guardian fixture timed out");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn fixture(mode: &str) {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let mailbox_path = data.join("pid-identity.db");
    let mailbox = MailboxDb::open_completion_continuation_domain(&mailbox_path).unwrap();
    let domain = mailbox.completion_continuation_domain().unwrap().unwrap();
    drop(mailbox);
    // Domain and State initialization are separate authorized fixture setup.
    drop(oulipoly_state::StateDb::open(&data.join("state.db")).unwrap());
    let state_before = std::fs::read(data.join("state.db")).unwrap();
    let socket = temp.path().join("broker.sock");
    let debt = temp.path().join("debt.txt");
    let ledger = broker(&socket, debt.clone(), mode);
    let log = temp.path().join("entry.log");
    let mut entry = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(&log).unwrap()))
        .spawn()
        .unwrap();
    if mode.starts_with("refuse_") {
        eventually(|| entry.try_wait().unwrap().is_some());
        assert!(!entry.wait().unwrap().success());
        assert_eq!(std::fs::read(data.join("state.db")).unwrap(), state_before);
        assert!(
            MailboxDb::open(&mailbox_path)
                .unwrap()
                .completion_continuation_owner()
                .unwrap()
                .is_none()
        );
        assert_eq!(
            ledger.lock().unwrap().phase,
            if mode == "refuse_prepare" { 1 } else { 2 }
        );
        assert!(debt.exists());
        return;
    }
    eventually(|| {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("pinned guardian active")
    });
    let (root, supervisor, guardian) = {
        let state = ledger.lock().unwrap();
        assert_eq!(state.phase, 3);
        assert_eq!(state.reads, 2);
        assert_eq!(state.domain, domain);
        (
            state.root.clone(),
            state.supervisor.clone(),
            state.guardian.unwrap(),
        )
    };
    let owner = MailboxDb::open(&mailbox_path)
        .unwrap()
        .completion_continuation_owner()
        .unwrap()
        .unwrap();
    assert_eq!(owner.domain_id, domain);
    assert_eq!(owner.supervisor_authority_id, supervisor);
    assert_eq!(
        MailboxDb::open(&mailbox_path)
            .unwrap()
            .completion_owner_kernel_root_id(&owner.owner_generation)
            .unwrap(),
        Some(root.clone())
    );
    assert_eq!(owner.guardian_identity.pid, i64::from(guardian.pid));
    assert_eq!(stamp(guardian.pid), Some(guardian));
    assert_ne!(owner.driver_identity.pid, owner.guardian_identity.pid);
    assert_eq!(
        parent_pid(owner.driver_identity.pid as i32),
        Some(guardian.pid)
    );
    // The actual guardian replaces a dead recovery driver without minting a
    // second guardian, supervisor, or broker root incarnation.
    unsafe { libc::kill(owner.driver_identity.pid as i32, libc::SIGKILL) };
    eventually(|| {
        MailboxDb::open(&mailbox_path)
            .ok()
            .and_then(|db| db.completion_continuation_owner().ok().flatten())
            .is_some_and(|current| current.owner_generation != owner.owner_generation)
    });
    let replacement_db = MailboxDb::open(&mailbox_path).unwrap();
    let replacement = replacement_db
        .completion_continuation_owner()
        .unwrap()
        .unwrap();
    assert_eq!(replacement.guardian_identity, owner.guardian_identity);
    assert_eq!(replacement.supervisor_authority_id, supervisor);
    assert_eq!(
        replacement_db
            .completion_owner_kernel_root_id(&replacement.owner_generation)
            .unwrap(),
        Some(root.clone())
    );
    assert_ne!(replacement.driver_identity.pid, owner.driver_identity.pid);
    // A root UUID or sibling process cannot read the live binding or reserve a
    // second entry. The real broker's same tests cover its durable registry.
    assert!(
        protocol::read_entry_at(&socket, &root)
            .unwrap()
            .starts_with("error ")
    );
    assert!(
        protocol::bind_guardian_at(&socket, &root, &domain, &supervisor)
            .unwrap()
            .starts_with("error ")
    );
    assert!(
        protocol::request_at(&socket, protocol::Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    unsafe { libc::kill(guardian.pid, libc::SIGKILL) };
    eventually(|| stamp(guardian.pid).is_none());
    assert!(
        protocol::read_entry_at(&socket, &root)
            .unwrap()
            .starts_with("error ")
    );
    assert!(
        protocol::request_at(&socket, protocol::Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    assert!(debt.exists());
    stop(&mut entry);
}

#[test]
fn production_guardian_is_the_broker_pinned_owner_in_private_namespace() {
    if let Ok(mode) = std::env::var("AGE319_PRIVATE_GUARDIAN_MODE") {
        fixture(&mode);
        return;
    }
    for mode in ["refuse_prepare", "refuse_bind", "live"] {
        let output = Command::new("unshare")
            .arg("-Ur")
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "production_guardian_is_the_broker_pinned_owner_in_private_namespace",
                "--nocapture",
            ])
            .env("AGE319_PRIVATE_GUARDIAN_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "{mode}: private fixture did not execute: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
