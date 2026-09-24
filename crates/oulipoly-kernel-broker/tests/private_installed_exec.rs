#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
//! Private userns fixture for the broker-owned fixed Runner entry. No host
//! installation, host-root sudo, or production State route is exercised.
use std::fs::{self, File};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn eventually(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < until, "private installed launch timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn launcher_command(launcher: &str, socket: &std::path::Path, generation: &str) -> Command {
    let mut command = Command::new(launcher);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1", generation);
    command
}

fn read_pty_until(master: i32, expected: &str) -> String {
    let until = Instant::now() + Duration::from_secs(20);
    let mut bytes = Vec::new();
    loop {
        let mut poll = libc::pollfd {
            fd: master,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 100) };
        if ready > 0 {
            let mut part = [0u8; 1024];
            let read = unsafe { libc::read(master, part.as_mut_ptr().cast(), part.len()) };
            if read > 0 {
                bytes.extend_from_slice(&part[..read as usize]);
            }
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if text.contains(expected) {
            return text;
        }
        assert!(Instant::now() < until, "PTY lacked {expected}: {text}");
    }
}

fn inner() {
    let original_runner = std::env::var("OULIPOLY_AGE319_RUNNER_IMAGE").unwrap();
    let image_dir = tempfile::tempdir_in(
        std::path::Path::new(&original_runner)
            .parent()
            .unwrap()
            .parent()
            .unwrap(),
    )
    .unwrap();
    let runner_image = image_dir.path().join("fixed-runner-setuid");
    fs::copy(&original_runner, &runner_image).unwrap();
    fs::set_permissions(&runner_image, fs::Permissions::from_mode(0o4755)).unwrap();
    let runner = runner_image.to_str().unwrap().to_owned();
    let launcher = env!("CARGO_BIN_EXE_oulipoly-installed-launcher");
    let broker_image = env!("CARGO_BIN_EXE_oulipoly-kernel-broker");
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    fs::create_dir(&state).unwrap();
    let socket = temp.path().join("broker.sock");
    let log = temp.path().join("broker.log");
    let generation = uuid::Uuid::new_v4().to_string();
    let mut broker = Command::new(broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1", &generation)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| socket.exists() || broker.try_wait().unwrap().is_some());
    assert!(
        socket.exists(),
        "broker: {}",
        fs::read_to_string(&log).unwrap()
    );
    let out = temp.path().join("help.out");
    let err = temp.path().join("help.err");
    let mut entry = launcher_command(launcher, &socket, &generation)
        .arg("--help")
        .stdout(Stdio::from(File::create(&out).unwrap()))
        .stderr(Stdio::from(File::create(&err).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| entry.try_wait().unwrap().is_some());
    let exit = entry.wait().unwrap();
    assert!(
        exit.success(),
        "launcher: {} broker: {}",
        fs::read_to_string(&err).unwrap(),
        fs::read_to_string(&log).unwrap()
    );
    assert!(fs::read_to_string(&out).unwrap().contains("Usage"));
    let records = fs::read_dir(state.join("private-launches"))
        .unwrap()
        .count();
    assert_eq!(records, 1);
    let offline = launcher_command(launcher, &socket, &generation)
        .args(["diagnostics", "--help"])
        .output()
        .unwrap();
    assert!(
        offline.status.success(),
        "{}",
        String::from_utf8_lossy(&offline.stderr)
    );
    assert!(String::from_utf8_lossy(&offline.stdout).contains("Usage"));
    let setuid = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-installed-probe-v1", "setuid"])
        .output()
        .unwrap();
    assert!(
        setuid.status.success(),
        "setuid: {} broker: {}",
        String::from_utf8_lossy(&setuid.stderr),
        fs::read_to_string(&log).unwrap()
    );
    assert!(
        String::from_utf8_lossy(&setuid.stdout)
            .contains("PRIVATE_SETUID_CHILD ruid=1 euid=0 nnp=0")
    );
    let missing = launcher_command(launcher, &temp.path().join("missing.sock"), &generation)
        .arg("--help")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let wrong_generation = launcher_command(launcher, &socket, &uuid::Uuid::new_v4().to_string())
        .arg("--help")
        .output()
        .unwrap();
    assert!(!wrong_generation.status.success());
    assert!(String::from_utf8_lossy(&wrong_generation.stderr).contains("generation mismatch"));
    let gui_link = temp.path().join("oulipoly-plane");
    std::os::unix::fs::symlink(launcher, &gui_link).unwrap();
    let gui = launcher_command(gui_link.to_str().unwrap(), &socket, &generation)
        .env("DISPLAY", ":77")
        .env("XDG_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(!gui.status.success());
    assert!(
        String::from_utf8_lossy(&gui.stderr)
            .contains("GUI needs installed display/session lifecycle")
    );
    let unsupported = launcher_command(launcher, &socket, &generation)
        .arg("__age319-private-unsupported-v1")
        .output()
        .unwrap();
    assert!(!unsupported.status.success());
    assert_eq!(
        fs::read_dir(state.join("private-launches"))
            .unwrap()
            .count(),
        3
    );

    // The launcher sends the slave as three SCM_RIGHTS descriptors. The
    // broker-owned child becomes the controlling terminal session leader.
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 37,
        ws_col: 91,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0
    );
    let master_file = unsafe { File::from_raw_fd(master) };
    let slave_file = unsafe { File::from_raw_fd(slave) };
    let cwd = temp.path().join("exact cwd");
    fs::create_dir(&cwd).unwrap();
    let mut tty = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-installed-probe-v1", "tty"])
        .env("DISPLAY", ":77")
        .current_dir(&cwd)
        .stdin(Stdio::from(slave_file.try_clone().unwrap()))
        .stdout(Stdio::from(slave_file.try_clone().unwrap()))
        .stderr(Stdio::from(slave_file.try_clone().unwrap()))
        .spawn()
        .unwrap();
    let ready = read_pty_until(master_file.as_raw_fd(), "PRIVATE_TTY_READY");
    assert!(ready.contains(&format!("cwd={}", cwd.display())), "{ready}");
    assert!(
        ready.contains("tty=true ctty=true rows=37 cols=91 display=:77"),
        "{ready}"
    );
    let resized = libc::winsize {
        ws_row: 39,
        ws_col: 88,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe { libc::ioctl(master_file.as_raw_fd(), libc::TIOCSWINSZ, &resized) },
        0
    );
    let input = b"\x03hello-from-pty\n";
    assert_eq!(
        unsafe { libc::write(master_file.as_raw_fd(), input.as_ptr().cast(), input.len()) },
        input.len() as isize
    );
    let result = read_pty_until(master_file.as_raw_fd(), "PRIVATE_TTY_RESULT");
    assert!(
        result.contains("input=hello-from-pty winch=true int=true"),
        "{result}"
    );
    eventually(|| tty.try_wait().unwrap().is_some());
    assert!(tty.wait().unwrap().success());
    drop(slave_file);
    drop(master_file);

    // A descendant loses environment, inherited FDs, session, and parent.
    // Its root PID1 remains live, so a second entrant is refused. The broker
    // then restarts and cancels only this recorded PID namespace.
    let marker = temp.path().join("ambient.marker");
    let ambient_out = temp.path().join("ambient.out");
    let ambient_err = temp.path().join("ambient.err");
    let mut ambient = launcher_command(launcher, &socket, &generation)
        .args([
            "__age319-private-installed-probe-v1",
            "ambient",
            marker.to_str().unwrap(),
        ])
        .stdout(Stdio::from(File::create(&ambient_out).unwrap()))
        .stderr(Stdio::from(File::create(&ambient_err).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| marker.exists() || ambient.try_wait().unwrap().is_some());
    assert!(
        marker.exists(),
        "ambient: {}",
        fs::read_to_string(&ambient_err).unwrap()
    );
    let grandchild_pid: i32 = fs::read_to_string(&marker)
        .unwrap()
        .split("host_pid=")
        .nth(1)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(grandchild_pid > 0);
    assert!(ambient.try_wait().unwrap().is_none());
    std::thread::sleep(Duration::from_secs(6));
    assert!(
        ambient.try_wait().unwrap().is_none(),
        "a live descendant must not meet a five-second workload cap"
    );
    let active: serde_json::Value = fs::read_dir(state.join("private-launches"))
        .unwrap()
        .map(|item| {
            serde_json::from_slice::<serde_json::Value>(&fs::read(item.unwrap().path()).unwrap())
                .unwrap()
        })
        .find(|record| record["terminal"].is_null())
        .unwrap();
    let request_id = active["request_id"].as_str().unwrap().to_owned();
    let init_pid = active["init"]["host_pid"].as_i64().unwrap() as i32;
    assert!(active["guardian"]["host_pid"].as_i64().unwrap() > 0);
    assert!(active["child"]["host_pid"].as_i64().unwrap() > 0);
    let second = launcher_command(launcher, &socket, &generation)
        .arg("--help")
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("duplicate or unsettled"));
    stop(&mut broker);
    let restart_log = temp.path().join("restart.log");
    let mut restarted = Command::new(broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1", &generation)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        restarted.try_wait().unwrap().is_some() || {
            launcher_command(launcher, &socket, &generation)
                .args(["__age319-private-status-v1", &request_id])
                .output()
                .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("running"))
        }
    });
    assert!(
        restarted.try_wait().unwrap().is_none(),
        "restart: {}",
        fs::read_to_string(&restart_log).unwrap()
    );
    // Lost launcher reply does not relinquish custody or permit another L.
    stop(&mut ambient);
    let status = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-status-v1", &request_id])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).contains("running"));
    let cancel = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-cancel-v1", &request_id])
        .output()
        .unwrap();
    assert!(
        cancel.status.success(),
        "{}",
        String::from_utf8_lossy(&cancel.stderr)
    );
    eventually(|| {
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                state
                    .join("private-launches")
                    .join(format!("{request_id}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        record["terminal"]
            .as_str()
            .is_some_and(|line| line.contains(" drained "))
    });
    assert!(!std::path::Path::new(&format!("/proc/{init_pid}")).exists());
    assert!(!std::path::Path::new(&format!("/proc/{grandchild_pid}")).exists());
    let settled = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-status-v1", &request_id])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&settled.stdout).contains(&format!("drained {request_id}")));
    let replay = launcher_command(launcher, &socket, &generation)
        .arg("--help")
        .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &request_id)
        .output()
        .unwrap();
    assert!(!replay.status.success());
    assert!(String::from_utf8_lossy(&replay.stderr).contains("duplicate or unsettled"));
    let sleep_out = temp.path().join("sleep.out");
    let mut sleeper = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-installed-probe-v1", "sleep"])
        .stdout(Stdio::from(File::create(&sleep_out).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        fs::read_to_string(&sleep_out)
            .unwrap()
            .contains("PRIVATE_SLEEP_READY")
    });
    let sleep_record: serde_json::Value = fs::read_dir(state.join("private-launches"))
        .unwrap()
        .map(|item| {
            serde_json::from_slice::<serde_json::Value>(&fs::read(item.unwrap().path()).unwrap())
                .unwrap()
        })
        .find(|record| record["terminal"].is_null())
        .unwrap();
    let sleep_id = sleep_record["request_id"].as_str().unwrap();
    let sleep_child = sleep_record["child"]["host_pid"].as_i64().unwrap() as i32;
    let sleep_cancel = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-cancel-v1", sleep_id])
        .output()
        .unwrap();
    assert!(sleep_cancel.status.success());
    eventually(|| sleeper.try_wait().unwrap().is_some());
    assert_eq!(sleeper.wait().unwrap().code(), Some(143));
    assert!(!std::path::Path::new(&format!("/proc/{sleep_child}")).exists());
    stop(&mut restarted);
    let wrong_log = temp.path().join("wrong-image.log");
    let mut wrong_broker = Command::new(broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1", &generation)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&wrong_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        wrong_broker.try_wait().unwrap().is_some() || {
            launcher_command(launcher, &socket, &generation)
                .arg("--help")
                .output()
                .is_ok_and(|output| {
                    String::from_utf8_lossy(&output.stderr).contains("image/generation mismatch")
                })
        }
    });
    assert!(
        wrong_broker.try_wait().unwrap().is_none(),
        "wrong broker: {}",
        fs::read_to_string(&wrong_log).unwrap()
    );
    stop(&mut wrong_broker);
}

#[test]
fn private_fixed_runner_help_drains() {
    if std::env::var_os("AGE319_PRIVATE_INSTALLED_INNER").is_some() {
        inner();
        return;
    }
    if std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE").is_none() {
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--map-auto", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "private_fixed_runner_help_drains", "--nocapture"])
        .env("AGE319_PRIVATE_INSTALLED_INNER", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
