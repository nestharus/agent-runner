#![cfg(all(target_os = "linux", not(feature = "age319-private-broker-fixture")))]

use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_launch_ledger::InstalledLaunchLedger;
use oulipoly_kernel_broker::installed_pair::{self, InstalledPair};
use oulipoly_kernel_broker::protocol::{self, EntryRoute};
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TEST: &str = "disposable_fixed_path_driver_admission_and_public_refusal";
const STATE: &str = "/var/lib/oulipoly-kernel-broker";
const CHILD_ENV: &str = "AGE319_FIXED_PATH_CHILD";

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn bind(source: &Path, target: &Path) {
    // Preserve locked child mounts inherited by the user namespace (for
    // example WSL library mounts); a nonrecursive bind would detach them.
    let flags = libc::MS_BIND | if source.is_dir() { libc::MS_REC } else { 0 };
    let source = CString::new(source.as_os_str().as_bytes()).unwrap();
    let target = CString::new(target.as_os_str().as_bytes()).unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                flags,
                std::ptr::null(),
            )
        },
        0,
        "private bind mount {} -> {}: {}",
        source.to_string_lossy(),
        target.to_string_lossy(),
        std::io::Error::last_os_error()
    );
}

struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn disposable_fixed_path_driver_admission_and_public_refusal() {
    let Ok(runner_image) = std::env::var("AGE319_FEATURELESS_RUNNER_BIN") else {
        eprintln!("fixed-path oracle not selected: AGE319_FEATURELESS_RUNNER_BIN missing");
        return;
    };
    let bash_image = std::env::var("AGE319_FEATURELESS_BASH_BIN")
        .expect("selected fixed-path oracle requires the featureless Bash image");
    if std::env::var_os(CHILD_ENV).is_none() {
        // A single mapped UID establishes fixed-path behavior, not the actual
        // host non-root launcher / root Broker credential separation.
        let root = tempfile::Builder::new()
            .prefix("age319-fixed-path-")
            .tempdir()
            .unwrap();
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc", "--kill-child=KILL"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD_ENV, root.path())
            .status()
            .unwrap();
        assert!(status.success(), "fixed-path namespace oracle failed");
        return;
    }
    let uid_map = fs::read_to_string("/proc/self/uid_map").unwrap();
    let fields: Vec<_> = uid_map.split_ascii_whitespace().collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0], "0");
    assert_ne!(fields[1], "0", "never run this fixture as host root");
    assert_eq!(fields[2], "1");
    assert_eq!(unsafe { libc::getpid() }, 1);
    assert_eq!(unsafe { libc::geteuid() }, 0);
    // No write to a host fixed path: all fixed paths are created under this
    // temporary root before chroot. Mounts exist only in unshare's private ns.
    let root_name = std::env::var_os(CHILD_ENV).unwrap();
    let root = Path::new(&root_name);
    for directory in [
        "usr/local/libexec/oulipoly",
        "usr/lib",
        "usr/lib64",
        "proc",
        "dev",
        "tmp",
        "run/oulipoly-kernel-broker",
        "var/lib",
        "home",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    for (source, target) in [
        (runner_image.as_str(), installed_pair::RUNNER),
        (
            env!("CARGO_BIN_EXE_oulipoly-kernel-broker"),
            installed_pair::BROKER,
        ),
        (
            env!("CARGO_BIN_EXE_oulipoly-installed-launcher"),
            installed_pair::LAUNCHER,
        ),
        (bash_image.as_str(), installed_pair::BASH),
    ] {
        let target = root.join(target.trim_start_matches('/'));
        fs::copy(source, &target).unwrap();
        fs::set_permissions(target, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bind(Path::new("/usr/lib"), &root.join("usr/lib"));
    bind(Path::new("/usr/lib64"), &root.join("usr/lib64"));
    symlink("usr/lib", root.join("lib")).unwrap();
    symlink("usr/lib64", root.join("lib64")).unwrap();
    bind(Path::new("/proc"), &root.join("proc"));
    File::create(root.join("dev/null")).unwrap();
    bind(Path::new("/dev/null"), &root.join("dev/null"));
    let root_name = CString::new(root.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::chroot(root_name.as_ptr()) }, 0);
    std::env::set_current_dir("/").unwrap();
    // The parent removes the temporary root after this namespace has exited.

    let runner = Path::new(installed_pair::RUNNER);
    let broker_image = Path::new(installed_pair::BROKER);
    let launcher = Path::new(installed_pair::LAUNCHER);
    let bash = Path::new(installed_pair::BASH);
    let manifest = Path::new(installed_pair::MANIFEST);
    let pair = InstalledPair {
        schema: 2,
        version: env!("CARGO_PKG_VERSION").into(),
        generation: uuid::Uuid::new_v4().to_string(),
        runner_sha256: digest(runner),
        broker_sha256: digest(broker_image),
        launcher_sha256: Some(digest(launcher)),
        bash_sha256: Some(digest(bash)),
    };
    fs::write(manifest, serde_json::to_vec(&pair).unwrap()).unwrap();
    fs::set_permissions(manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let state = Path::new(STATE);
    let source = EmptyV30BootstrapIdentity::bootstrap_at(state).unwrap();
    FirstInstallActivation::activate_at(
        state,
        PairPaths {
            manifest,
            runner,
            broker: broker_image,
            launcher,
            bash,
        },
        true,
    )
    .unwrap();
    let socket = Path::new(protocol::INSTALLED_SOCKET);
    let mut broker = Reap(
        Command::new(broker_image)
            .env_clear()
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", runner)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", broker_image)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", bash)
            .stderr(Stdio::from(File::create("/tmp/broker.err").unwrap()))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while protocol::observe_entry_gate_at(socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
        assert!(
            broker.0.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string("/tmp/broker.err").unwrap()
        );
        assert!(
            Instant::now() < deadline,
            "fixed-path Broker startup timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    for (label, double) in [("markerless", false), ("double-marker", true)] {
        let mut direct = Command::new(runner);
        direct.arg("--help").env_clear();
        if double {
            direct
                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                .env("OULIPOLY_KERNEL_CHILD_JOIN_FD_V1", "3");
        }
        let output = direct.output().unwrap();
        println!(
            "{label} status={}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("fresh-only installed Runner requires one broker-owned entry gate")
        );
    }
    // An outside caller can spell the internal argv and inherit a socket, but
    // neither that socket number nor its same-UID peer is guardian authority.
    let (sender, receiver) = UnixStream::pair().unwrap();
    // EOF also makes an accidentally ungated dispatch finite; the assertion
    // below requires the refusal to come from installed ingress itself.
    drop(sender);
    let copied_fd = receiver.as_raw_fd();
    let mut spoof = Command::new(runner);
    spoof
        .args([
            "__completion-driver-v1",
            "unused",
            &copied_fd.to_string(),
            &uuid::Uuid::new_v4().to_string(),
        ])
        .env_clear();
    unsafe {
        spoof.pre_exec(move || {
            if libc::fcntl(copied_fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = spoof.output().unwrap();
    println!(
        "outside driver argv/socket status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("OULIPOLY_KERNEL_ENTRY_GAP="));
    let request = uuid::Uuid::new_v4().to_string();
    let mut launched = Reap(
        Command::new(launcher)
            .arg("--help")
            .env_clear()
            .env("HOME", "/home")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket)
            .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &request)
            .stdout(Stdio::from(File::create("/tmp/launch.out").unwrap()))
            .stderr(Stdio::from(File::create("/tmp/launch.err").unwrap()))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut observed_driver = None;
    let exit = loop {
        // Observe the re-exec itself, not just the entry Runner. Both markers
        // are absent: the real gate predicate is true solely from this path.
        if observed_driver.is_none() {
            for process in fs::read_dir("/proc").unwrap().flatten() {
                let Ok(pid) = process.file_name().to_string_lossy().parse::<i32>() else {
                    continue;
                };
                let Ok(cmdline) = fs::read(process.path().join("cmdline")) else {
                    continue;
                };
                if cmdline.split(|b| *b == 0).nth(1) != Some(b"__completion-driver-v1".as_slice()) {
                    continue;
                }
                let Ok(image) = fs::read_link(process.path().join("exe")) else {
                    continue;
                };
                assert_eq!(
                    image, runner,
                    "driver did not re-exec the fixed installed image"
                );
                let environment = fs::read(process.path().join("environ")).unwrap();
                assert!(!environment.split(|b| *b == 0).any(|v| {
                    v.starts_with(b"OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1=")
                        || v.starts_with(b"OULIPOLY_KERNEL_CHILD_JOIN_FD_V1=")
                }));
                println!(
                    "observed driver pid={pid}, exe={}, no host/child markers: needs_installed_entry_gate=true",
                    image.display()
                );
                observed_driver = Some(pid);
            }
        }
        if let Some(status) = launched.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "fixed-path launch timed out");
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = fs::read_to_string("/tmp/launch.out").unwrap();
    let stderr = fs::read_to_string("/tmp/launch.err").unwrap();
    let broker_stderr = fs::read_to_string("/tmp/broker.err").unwrap();
    println!(
        "owned fixed-path launch status={exit}\nstdout={stdout}\nstderr={stderr}\nbroker stderr={broker_stderr}"
    );
    assert!(exit.success(), "owned fixed-path launcher failed");
    assert!(
        observed_driver.is_some(),
        "actual fixed-path driver was not observed"
    );
    assert!(stdout.contains("Usage:"));
    assert!(!stderr.contains("OULIPOLY_KERNEL_ENTRY_GAP="));
    // custodian_entry prints these unprefixed driver results only after main's
    // installed gate has returned Ok and driver dispatch has actually run.
    // The help root can finish before the driver's next Broker read; retain
    // that late dead-peer result as evidence, not a restoration claim.
    assert!(
        stderr.lines().any(|line| matches!(
            line,
            "error dead peer"
                | "v30 repair boundary: no pending broker recipient; nothing to repair"
        )),
        "driver dispatch result was not observed: {stderr}"
    );
    let ledger =
        InstalledLaunchLedger::open(state, &pair.generation, &source.source_generation).unwrap();
    assert_eq!(
        ledger.read_terminal(&request).unwrap().unwrap().exit_code,
        0
    );
    let records: Vec<_> = fs::read_dir(state.join("entries"))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(records.len(), 1);
    let entry: serde_json::Value =
        serde_json::from_slice(&fs::read(records[0].path()).unwrap()).unwrap();
    assert!(
        entry["prepared_driver"].is_object(),
        "Broker did not seal the actual driver"
    );
    assert_eq!(
        entry["prepared_driver"]["host_pid"],
        observed_driver.unwrap()
    );
    println!("fixed-path terminal and sealed driver: {entry}");
}
