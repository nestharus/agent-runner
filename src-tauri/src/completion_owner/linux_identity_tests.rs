//! Root round3 D1/D2: unknown observation must preserve inherited admission,
//! including the residual retirement sweep. These mount faults require the
//! explicitly selected private user/mount/PID/proc/DAC test environment.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct Overlay(std::ffi::CString);
impl Overlay {
    fn bind(source: &Path, target: &Path) -> Self {
        assert_eq!(
            std::env::var("AGE360_PRIVATE_IDENTITY_TEST").as_deref(),
            Ok("1")
        );
        let source = std::ffi::CString::new(source.as_os_str().as_encoded_bytes()).unwrap();
        let target = std::ffi::CString::new(target.as_os_str().as_encoded_bytes()).unwrap();
        let rc = unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        };
        assert_eq!(
            rc,
            0,
            "private proc overlay: {}",
            std::io::Error::last_os_error()
        );
        Self(target)
    }
}
impl Drop for Overlay {
    fn drop(&mut self) {
        assert_eq!(unsafe { libc::umount(self.0.as_ptr()) }, 0);
    }
}

#[test]
#[ignore = "requires explicitly selected masked private proc/mount/DAC environment"]
fn inherited_identity_uncertainty_retains_until_proved_expiry() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pid-identity.db");
    let db = MailboxDb::open(&path).unwrap();
    let mut child = std::process::Command::new("/bin/cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let id = identity(i64::from(child.id())).unwrap();
    db.retain_completion_context(&id).unwrap();
    let mut leases = ContextLeases::inherit(&path).unwrap();
    let proc_stat = PathBuf::from(format!("/proc/{}/stat", child.id()));
    let boot = Path::new("/proc/sys/kernel/random/boot_id");
    // Real reader results, not labels/injected expiry booleans. Invalid UTF8
    // is Err; denial and malformed stat/empty boot are the old optional None.
    for (name, target, bytes, mode, expect_err) in [
        (
            "stat-denied",
            proc_stat.as_path(),
            b"denied".as_slice(),
            0o000,
            false,
        ),
        (
            "stat-malformed",
            proc_stat.as_path(),
            b"not a stat".as_slice(),
            0o600,
            false,
        ),
        (
            "stat-read-error",
            proc_stat.as_path(),
            b"\xff".as_slice(),
            0o600,
            true,
        ),
        ("boot-empty", boot, b"".as_slice(), 0o600, false),
        ("boot-denied", boot, b"denied".as_slice(), 0o000, false),
        ("boot-read-error", boot, b"\xff".as_slice(), 0o600, true),
        (
            "boot-malformed",
            boot,
            b"not-a-boot-id".as_slice(),
            0o600,
            false,
        ),
    ] {
        let fixture = root.path().join(name);
        std::fs::write(&fixture, bytes).unwrap();
        std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(mode)).unwrap();
        let overlay = Overlay::bind(&fixture, target);
        let observed = read_live_process_identity(id.pid);
        if name == "boot-malformed" {
            assert!(observed.as_ref().unwrap().is_some());
        } else if expect_err {
            assert!(observed.is_err(), "{name}: {observed:?}");
        } else {
            assert_eq!(observed.as_ref().unwrap(), &None, "{name}");
        }
        assert!(child.try_wait().unwrap().is_none(), "fixture child exited");
        leases.release_disconnected(&path).unwrap();
        assert!(!leases.is_empty(), "{name}: inherited group erased");
        assert_eq!(
            db.completion_contexts().unwrap(),
            vec![id.clone()],
            "{name}"
        );
        assert!(
            retained_native_context(&path).unwrap(),
            "{name}: residual sweep retired"
        );
        assert_eq!(
            db.completion_contexts().unwrap(),
            vec![id.clone()],
            "{name}"
        );
        println!("{name}: actual reader={observed:?}; live child, group and residual row retained");
        drop(overlay);
    }
    assert_eq!(identity(id.pid).unwrap(), id);
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    assert!(context_leases::incarnation_expired(&id));
    leases.release_disconnected(&path).unwrap();
    assert!(leases.is_empty());
    assert!(db.completion_contexts().unwrap().is_empty());
    // Independently exercise the residual-only relationship after real exit.
    db.retain_completion_context(&id).unwrap();
    assert!(!retained_native_context(&path).unwrap());
    assert!(db.completion_contexts().unwrap().is_empty());
    println!("exact child waited: inherited and residual-only release both committed");
}

#[test]
fn readable_replacement_discharges_only_the_old_incarnation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pid-identity.db");
    let current = identity(i64::from(std::process::id())).unwrap();
    let mut previous = current.clone();
    previous.starttime_ticks += 1;
    let db = MailboxDb::open(&path).unwrap();
    db.retain_completion_context(&previous).unwrap();
    db.retain_completion_context(&current).unwrap();
    let mut inherited = ContextLeases::inherit(&path).unwrap();
    inherited.release_disconnected(&path).unwrap();
    assert_eq!(db.completion_contexts().unwrap(), vec![current.clone()]);
    db.retain_completion_context(&previous).unwrap();
    assert!(retained_native_context(&path).unwrap());
    assert_eq!(db.completion_contexts().unwrap(), vec![current]);
}

#[test]
fn actual_socket_peer_mismatch_precedes_refusal_interpretation() {
    let root = tempfile::tempdir().unwrap();
    let endpoint = root.path().join("peer.sock");
    // The child creates the listening socket, so SO_PEERCRED actually names a
    // different live process; its payload exactly matches expected identity.
    let mut expected = super::admission_tests::test_owner(&endpoint);
    expected.owner_generation = "actual-peer-test".into();
    let response =
        serde_json::to_string(&JoinRefusal::new(&expected, RefusalReason::QueueFull)).unwrap();
    let mut server = std::process::Command::new("/usr/bin/python3")
        .args(["-c", "import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.listen(); print('ready',flush=True); c,_=s.accept(); assert c.recv(6)==b'join!\\n'; c.sendall(sys.argv[2].encode()+b'\\n'); sys.stdin.read()", endpoint.to_str().unwrap(), &response])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    use std::io::BufRead;
    let mut ready = String::new();
    std::io::BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    let actual = identity(i64::from(server.id())).unwrap();
    assert_ne!(actual, expected.guardian_identity);
    let error = connect_context(&endpoint, &expected).unwrap_err();
    assert_eq!(error, "completion join peer conflict");
    assert!(server.try_wait().unwrap().is_none());
    drop(server.stdin.take());
    assert!(server.wait().unwrap().success());
    println!(
        "actual live peer {} differs from expected {}; rejected before negative category",
        actual.pid, expected.guardian_identity.pid
    );
}
