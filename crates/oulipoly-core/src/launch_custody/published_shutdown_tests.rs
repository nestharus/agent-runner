//! Bounded fake workloads; all signals target this fixture's pinned descendants.
use super::*;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    root: PathBuf,
    custody: Arc<LaunchCustody>,
    child: Option<Child>,
    master: Option<OwnedFd>,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("shutdown-{}-{name}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let custody = Arc::new(LaunchCustody::start(root.join("proof")).unwrap());
        Self {
            root,
            custody,
            child: None,
            master: None,
        }
    }
    fn command(&mut self, script: &str, pty: bool) -> Command {
        let mut command = Command::new("/usr/bin/python3");
        command
            .args(["-c", script])
            .current_dir(&self.root)
            .env_clear();
        if pty {
            let mut master = -1;
            let mut slave = -1;
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                },
                0
            );
            self.master = Some(unsafe { OwnedFd::from_raw_fd(master) });
            let slave = unsafe { File::from_raw_fd(slave) };
            command
                .stdin(slave.try_clone().unwrap())
                .stdout(slave.try_clone().unwrap())
                .stderr(slave);
            unsafe {
                command.pre_exec(move || {
                    libc::close(master);
                    if libc::setsid() < 0
                        || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0
                        || libc::tcsetpgrp(0, libc::getpid()) < 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        } else {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
        }
        command
    }
    fn spawn(&mut self, script: &str, pty: bool) {
        let mut command = self.command(script, pty);
        self.custody.configure(&mut command).unwrap();
        self.child = Some(command.spawn().unwrap());
        drop(command);
        self.custody.seal();
    }
    fn published(&self) -> i32 {
        self.child.as_ref().unwrap().id() as i32
    }
    fn settle(&mut self) -> std::process::ExitStatus {
        let published = self.published();
        let status = until(|| self.child.as_mut().unwrap().try_wait().unwrap());
        until(|| self.custody.quiescent().then_some(()));
        assert_eq!(unsafe { libc::killpg(published, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        status
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                unsafe {
                    libc::killpg(child.id() as i32, libc::SIGKILL);
                }
            }
            let _ = child.wait();
        }
        self.master.take();
        self.custody.seal();
        // Preserve failed fixtures for evidence; finite workload backstops and
        // C-owned shutdown do not depend on deleting a directory.
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
fn until<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < end, "fixture deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn pid_file(root: &Path, name: &str) -> i32 {
    until(|| {
        std::fs::read_to_string(root.join(name))
            .ok()
            .and_then(|v| v.parse().ok())
    })
}
fn children(pid: i32) -> Vec<i32> {
    std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .unwrap()
        .split_whitespace()
        .map(|p| p.parse().unwrap())
        .collect()
}

#[test]
fn terminal_loss_before_workload_exists_is_not_consumed() {
    let mut fixture = Fixture::new("early-hangup");
    let mut command = fixture.command(
        "import pathlib,time; pathlib.Path('executed').touch(); time.sleep(4)",
        true,
    );
    let (mut observer, gate) = UnixStream::pair().unwrap();
    observer
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    super::linux::configure_impl(&fixture.custody, &mut command, Some(gate.as_raw_fd())).unwrap();
    let spawn = std::thread::spawn(move || {
        let result = command.spawn();
        drop(command);
        drop(gate);
        result
    });
    let mut packet = [0u8; 5];
    observer.read_exact(&mut packet).unwrap();
    assert_eq!(packet[0], b'R');
    let published = i32::from_ne_bytes(packet[1..].try_into().unwrap());
    let custodian = children(published)[0];
    assert_ne!(unsafe { libc::getpgid(custodian) }, published);
    assert!(children(custodian).is_empty());
    fixture.master.take();
    // Give the old handler an opportunity to consume HUP in an empty P group.
    std::thread::sleep(Duration::from_millis(100));
    assert!(children(custodian).is_empty());
    assert!(!fixture.custody.quiescent());
    observer.write_all(b"A").unwrap();
    fixture.child = Some(spawn.join().unwrap().unwrap());
    fixture.custody.seal();
    let status = fixture.settle();
    assert_eq!(status.signal(), Some(libc::SIGHUP));
    assert!(!fixture.root.join("executed").exists());
    eprintln!(
        "early PTY loss: C={custodian} before W; P={published} status={status:?} executable_never_started Q=true group=ESRCH"
    );
}

#[test]
fn resistant_shutdown_preserves_root_status_and_consumes_escaped_tree() {
    for graceful_root in [false, true] {
        let mut fixture = Fixture::new(if graceful_root {
            "exited-root"
        } else {
            "resistant-root"
        });
        let script = format!(
            r#"
import os,pathlib,signal,time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = os.fork()
if child == 0:
    os.setsid()
    pathlib.Path('escaped').write_text(str(os.getpid()))
    time.sleep(4)
    os._exit(92)
def term(sig, frame):
    pathlib.Path('trap').write_text('entered')
    time.sleep(.08)
    os._exit(37)
if {graceful_root}: signal.signal(signal.SIGTERM, term)
pathlib.Path('ready').write_text(str(os.getpid()))
time.sleep(4)
os._exit(93)
"#,
            graceful_root = if graceful_root { "True" } else { "False" }
        );
        fixture.spawn(&script, false);
        let workload = pid_file(&fixture.root, "ready");
        let escaped = pid_file(&fixture.root, "escaped");
        let published = fixture.published();
        assert_eq!(unsafe { libc::getpgid(workload) }, published);
        assert_eq!(unsafe { libc::getpgid(escaped) }, escaped);
        let start = Instant::now();
        assert_eq!(unsafe { libc::killpg(published, libc::SIGTERM) }, 0);
        std::thread::sleep(Duration::from_millis(40));
        assert!(
            fixture
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none()
        );
        assert!(!fixture.custody.quiescent());
        if graceful_root {
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("trap")).unwrap(),
                "entered"
            );
        }
        let status = fixture.settle();
        assert!(start.elapsed() >= TERMINATION_GRACE_PERIOD);
        assert!(start.elapsed() < Duration::from_secs(2));
        if graceful_root {
            assert_eq!(status.code(), Some(37));
        } else {
            assert_eq!(status.signal(), Some(libc::SIGKILL));
        }
        for pid in [workload, escaped] {
            assert!(!Path::new(&format!("/proc/{pid}")).exists());
        }
        eprintln!(
            "resistant graceful_root={graceful_root} W={workload} escaped={escaped} P={published} status={status:?} elapsed={:?} Q=true group=ESRCH",
            start.elapsed()
        );
    }
}

#[test]
fn interactive_interrupt_does_not_start_shutdown_grace() {
    let mut fixture = Fixture::new("interrupt");
    fixture.spawn(
        r#"
import os,pathlib,signal,time
signal.signal(signal.SIGINT, lambda *_: pathlib.Path('interrupt').touch())
pathlib.Path('ready').write_text(str(os.getpid()))
end=time.monotonic()+4
while not pathlib.Path('release').exists():
    if time.monotonic()>end: os._exit(92)
    time.sleep(.005)
os._exit(0)
"#,
        false,
    );
    pid_file(&fixture.root, "ready");
    assert_eq!(
        unsafe { libc::killpg(fixture.published(), libc::SIGINT) },
        0
    );
    until(|| fixture.root.join("interrupt").exists().then_some(()));
    std::thread::sleep(TERMINATION_GRACE_PERIOD * 2);
    assert!(
        fixture
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none()
    );
    assert!(!fixture.custody.quiescent());
    std::fs::write(fixture.root.join("release"), b"release").unwrap();
    assert_eq!(fixture.settle().code(), Some(0));
}

#[test]
fn orphaned_stopped_group_keeps_lost_custody_unknown() {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "launch_custody::published_shutdown_tests::orphan_observer",
            "--nocapture",
        ])
        .env_clear()
        .env("TMPDIR", std::env::temp_dir())
        .env("CUSTODY_ORPHAN_OBSERVER", "1")
        .spawn()
        .unwrap();
    let status = until(|| child.try_wait().unwrap());
    assert!(status.success());
}

#[test]
fn orphan_observer() {
    if std::env::var_os("CUSTODY_ORPHAN_OBSERVER").is_none() {
        return;
    }
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let mut fixture = Fixture::new("orphaned-stopped");
    fixture.spawn(
        r#"
import os,pathlib,signal,time
count=0
def hup(*_):
    global count
    count+=1
    pathlib.Path('hups').write_text(str(count))
signal.signal(signal.SIGHUP,hup)
pathlib.Path('ready').write_text(str(os.getpid()))
end=time.monotonic()+2
while not pathlib.Path('release').exists() and time.monotonic()<end: time.sleep(.005)
os._exit(37)
"#,
        true,
    );
    let workload = pid_file(&fixture.root, "ready");
    let published = fixture.published();
    let custodian = children(published)[0];
    assert_eq!(unsafe { libc::killpg(published, libc::SIGSTOP) }, 0);
    until(|| {
        let stopped = [published, workload].iter().all(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .unwrap()
                .split(") ")
                .nth(1)
                .unwrap()
                .starts_with('T')
        });
        stopped.then_some(())
    });
    assert_eq!(unsafe { libc::kill(custodian, libc::SIGKILL) }, 0);
    // The kernel orphan HUP/CONT resumes the group without our relaying HUP.
    let status = until(|| fixture.child.as_mut().unwrap().try_wait().unwrap());
    assert_eq!(status.code(), Some(125)); // C loss, explicitly not W status.
    let hups = pid_file(&fixture.root, "hups");
    assert!(hups >= 1);
    assert!(!fixture.custody.quiescent());
    std::fs::write(fixture.root.join("release"), b"release").unwrap();
    let mut status = 0;
    until(|| {
        let rc = unsafe { libc::waitpid(workload, &mut status, libc::WNOHANG) };
        assert!(rc >= 0);
        (rc == workload).then_some(())
    });
    assert_eq!(libc::WEXITSTATUS(status), 37);
    until(|| {
        let rc = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        (rc == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)).then_some(())
    });
    assert!(!fixture.custody.quiescent());
    assert_eq!(unsafe { libc::killpg(published, 0) }, -1);
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    eprintln!(
        "orphan stopped P={published} W={workload} C={custodian}: P=exit125 W=exit37 hups={hups} Q=false group=ESRCH; count is observed, not universal uniqueness"
    );
}

#[test]
fn ready_pty_hangup_preserves_trap_or_escalates_resistance() {
    for trapped in [true, false] {
        let mut fixture = Fixture::new(if trapped { "pty-trap" } else { "pty-resistant" });
        let script = format!(
            r#"
import os,pathlib,signal,time
count=0
def hup(*_):
    global count
    count+=1
    pathlib.Path('hups').write_text(str(count))
    time.sleep(.08)
    os._exit(37)
signal.signal(signal.SIGHUP, hup if {trapped} else signal.SIG_IGN)
pathlib.Path('ready').write_text(str(os.getpid()))
time.sleep(4)
os._exit(92)
"#,
            trapped = if trapped { "True" } else { "False" }
        );
        fixture.spawn(&script, true);
        let workload = pid_file(&fixture.root, "ready");
        let start = Instant::now();
        fixture.master.take();
        let status = fixture.settle();
        if trapped {
            assert_eq!(status.code(), Some(37));
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("hups")).unwrap(),
                "1"
            );
        } else {
            assert_eq!(status.signal(), Some(libc::SIGKILL));
            assert!(start.elapsed() >= TERMINATION_GRACE_PERIOD);
        }
        assert!(!Path::new(&format!("/proc/{workload}")).exists());
        eprintln!(
            "ready PTY loss trapped={trapped} status={status:?} W={workload} elapsed={:?} Q=true group=ESRCH",
            start.elapsed()
        );
    }
}
