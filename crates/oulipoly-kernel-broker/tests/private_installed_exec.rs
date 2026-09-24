#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
//! Private userns fixture for the broker-owned fixed Runner entry. No host
//! installation, host-root sudo, or production State route is exercised.
use std::fs::{self, File};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixListener};
use std::os::unix::process::CommandExt;
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
    let copied_launcher = temp.path().join("copied-launcher");
    fs::copy(launcher, &copied_launcher).unwrap();
    let copied_peer = launcher_command(copied_launcher.to_str().unwrap(), &socket, &generation)
        .arg("--help")
        .output()
        .unwrap();
    assert!(!copied_peer.status.success());
    assert!(String::from_utf8_lossy(&copied_peer.stderr).contains("image/generation mismatch"));
    let gui_link = temp.path().join("oulipoly-plane");
    std::os::unix::fs::symlink(launcher, &gui_link).unwrap();
    let gui = launcher_command(gui_link.to_str().unwrap(), &socket, &generation)
        .output()
        .unwrap();
    assert!(!gui.status.success());
    assert!(String::from_utf8_lossy(&gui.stderr).contains("GUI runtime directory missing"));
    let runtime = temp.path().join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let wayland = UnixListener::bind(runtime.join("wayland-0")).unwrap();
    let bus = UnixListener::bind(runtime.join("bus")).unwrap();
    let authority = runtime.join("Xauthority");
    fs::write(&authority, b"private-cookie").unwrap();
    let bus_address = format!("unix:path={}", runtime.join("bus").display());
    let gui_cwd = temp.path().join("gui cwd");
    fs::create_dir(&gui_cwd).unwrap();
    let gui_report = runtime.join("age319-private-gui-report");
    let good_gui = || {
        let mut command = launcher_command(gui_link.to_str().unwrap(), &socket, &generation);
        command
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("WAYLAND_DISPLAY", "wayland-0")
            .env("DBUS_SESSION_BUS_ADDRESS", &bus_address)
            .env("XAUTHORITY", &authority)
            .current_dir(&gui_cwd);
        command
    };
    let missing_display = good_gui()
        .env("WAYLAND_DISPLAY", "missing")
        .output()
        .unwrap();
    assert!(!missing_display.status.success());
    assert!(!gui_report.exists());
    let wrong_socket = runtime.join("wrong-owner");
    let owned_socket = UnixListener::bind(&wrong_socket).unwrap();
    let wrong_socket_c =
        std::ffi::CString::new(wrong_socket.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::chown(wrong_socket_c.as_ptr(), 1, 1) }, 0);
    let wrong_socket_result = good_gui()
        .env("WAYLAND_DISPLAY", "wrong-owner")
        .output()
        .unwrap();
    assert!(!wrong_socket_result.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_socket_result.stderr).contains("socket owner/type mismatch")
    );
    drop(owned_socket);
    let datagram = UnixDatagram::bind(runtime.join("not-stream")).unwrap();
    let child_error = good_gui()
        .env("WAYLAND_DISPLAY", "not-stream")
        .output()
        .unwrap();
    assert!(!child_error.status.success());
    assert!(String::from_utf8_lossy(&child_error.stderr).contains("PRIVATE_GUI_PROBE_GAP="));
    assert!(!gui_report.exists());
    drop(datagram);
    let wrong_runtime = temp.path().join("other-runtime");
    fs::create_dir(&wrong_runtime).unwrap();
    fs::set_permissions(&wrong_runtime, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        unsafe {
            libc::chown(
                std::ffi::CString::new(wrong_runtime.as_os_str().as_encoded_bytes())
                    .unwrap()
                    .as_ptr(),
                1,
                1,
            )
        },
        0
    );
    let wrong_uid = good_gui()
        .env("XDG_RUNTIME_DIR", &wrong_runtime)
        .output()
        .unwrap();
    assert!(!wrong_uid.status.success());
    assert!(String::from_utf8_lossy(&wrong_uid.stderr).contains("owner/mode mismatch"));
    let bad_dbus = good_gui()
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/missing/bus")
        .output()
        .unwrap();
    assert!(!bad_dbus.status.success());
    assert!(String::from_utf8_lossy(&bad_dbus.stderr).contains("outside runtime directory"));
    assert!(!gui_report.exists());
    // The dynamic loader can reopen closed fd numbers before Rust main. The
    // private mask carries the audited exec-boundary absence into capture;
    // the broker checks it again immediately before the fixed Runner exec.
    let mut no_stdio = good_gui();
    no_stdio
        .env("OULIPOLY_AGE319_PRIVATE_GUI_HOLD_V1", "1")
        .env("OULIPOLY_AGE319_PRIVATE_STDIO_MASK_V1", "000");
    let stdio_audit = File::create(temp.path().join("stdio-at-exec")).unwrap();
    let stdio_audit_fd = stdio_audit.as_raw_fd();
    unsafe {
        no_stdio.pre_exec(move || {
            for fd in 0..=2 {
                libc::close(fd);
            }
            let observed = [0, 1, 2].map(|fd| {
                if libc::fcntl(fd, libc::F_GETFD) >= 0 {
                    b'1'
                } else {
                    b'0'
                }
            });
            libc::write(stdio_audit_fd, observed.as_ptr().cast(), observed.len());
            Ok(())
        });
    }
    let mut gui_entry = no_stdio.spawn().unwrap();
    eventually(|| gui_report.exists() || gui_entry.try_wait().unwrap().is_some());
    let report = fs::read_to_string(&gui_report).unwrap();
    assert!(
        report.contains(&format!(
            "uid=0 euid=0 gid=0 egid=0 groups=0 cwd={}",
            gui_cwd.display()
        )),
        "{report}"
    );
    assert_eq!(fs::read(temp.path().join("stdio-at-exec")).unwrap(), b"000");
    assert!(report.contains("stdio_entry=000 nnp=0"), "{report}");
    let grandchild_pid: i32 = report
        .split("grandchild_host_pid=")
        .nth(1)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(grandchild_pid > 0);
    let gui_record: serde_json::Value = fs::read_dir(state.join("private-launches"))
        .unwrap()
        .map(|item| {
            serde_json::from_slice::<serde_json::Value>(&fs::read(item.unwrap().path()).unwrap())
                .unwrap()
        })
        .find(|record| record["terminal"].is_null())
        .unwrap();
    let gui_id = gui_record["request_id"].as_str().unwrap().to_owned();
    let gui_init = gui_record["init"]["host_pid"].as_i64().unwrap() as i32;
    let gui_child = gui_record["child"]["host_pid"].as_i64().unwrap() as i32;
    assert!(gui_child > 0 && gui_init > 0);
    std::thread::sleep(Duration::from_secs(6));
    assert!(gui_entry.try_wait().unwrap().is_none());
    let second_gui = good_gui().output().unwrap();
    assert!(!second_gui.status.success());
    assert!(String::from_utf8_lossy(&second_gui.stderr).contains("duplicate or unsettled"));
    stop(&mut broker);
    let gui_restart_log = temp.path().join("gui-restart.log");
    broker = Command::new(broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1", &generation)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&gui_restart_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        broker.try_wait().unwrap().is_some() || {
            launcher_command(launcher, &socket, &generation)
                .args(["__age319-private-status-v1", &gui_id])
                .output()
                .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("running"))
        }
    });
    assert!(
        broker.try_wait().unwrap().is_none(),
        "{}",
        fs::read_to_string(&gui_restart_log).unwrap()
    );
    stop(&mut gui_entry);
    let gui_running = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-status-v1", &gui_id])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&gui_running.stdout).contains("running"));
    let gui_cancel = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-cancel-v1", &gui_id])
        .output()
        .unwrap();
    assert!(gui_cancel.status.success());
    eventually(|| {
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                state
                    .join("private-launches")
                    .join(format!("{gui_id}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        record["terminal"]
            .as_str()
            .is_some_and(|line| line.contains(" drained "))
    });
    assert!(!std::path::Path::new(&format!("/proc/{gui_init}")).exists());
    assert!(!std::path::Path::new(&format!("/proc/{gui_child}")).exists());
    assert!(!std::path::Path::new(&format!("/proc/{grandchild_pid}")).exists());
    let gui_settled = launcher_command(launcher, &socket, &generation)
        .args(["__age319-private-status-v1", &gui_id])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&gui_settled.stdout).contains(&format!("drained {gui_id}")));
    let gui_replay = good_gui()
        .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &gui_id)
        .output()
        .unwrap();
    assert!(!gui_replay.status.success());
    assert!(String::from_utf8_lossy(&gui_replay.stderr).contains("duplicate or unsettled"));
    fs::remove_file(&gui_report).unwrap();
    let gui_present = good_gui().output().unwrap();
    assert!(
        gui_present.status.success(),
        "{}",
        String::from_utf8_lossy(&gui_present.stderr)
    );
    let report = fs::read_to_string(&gui_report).unwrap();
    assert!(report.contains("stdio_entry=111"), "{report}");
    // The mount is private to the unshare fixture. WSL may deny an overlay on
    // its display mount; in that case the X11 branch remains explicitly unproved.
    let private_mount = unsafe {
        libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_PRIVATE | libc::MS_REC,
            std::ptr::null(),
        )
    };
    let x11_mount = if private_mount == 0 {
        unsafe {
            libc::mount(
                c"tmpfs".as_ptr(),
                c"/tmp/.X11-unix".as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV,
                c"mode=1777".as_ptr().cast(),
            )
        }
    } else {
        -1
    };
    if x11_mount == 0 {
        let x11 = UnixListener::bind("/tmp/.X11-unix/X59993").unwrap();
        fs::remove_file(&gui_report).unwrap();
        let x11_gui = good_gui()
            .env_remove("WAYLAND_DISPLAY")
            .env("DISPLAY", ":59993")
            .output()
            .unwrap();
        assert!(
            x11_gui.status.success(),
            "{}",
            String::from_utf8_lossy(&x11_gui.stderr)
        );
        assert!(
            fs::read_to_string(&gui_report)
                .unwrap()
                .contains("PRIVATE_GUI_CONNECTED")
        );
        drop(x11);
    } else {
        eprintln!("private X11 mount unavailable; X11 socket access unproved");
    }
    drop((wayland, bus));
    let unsupported = launcher_command(launcher, &socket, &generation)
        .arg("__age319-private-unsupported-v1")
        .output()
        .unwrap();
    assert!(!unsupported.status.success());
    assert_eq!(
        fs::read_dir(state.join("private-launches"))
            .unwrap()
            .count(),
        if x11_mount == 0 { 7 } else { 6 }
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

    // A separate non-setuid fixed Runner image proves the GUI child runs as
    // the launcher's real/effective UID, while the broker remains UID 0.
    let plain_state = temp.path().join("plain-state");
    fs::create_dir(&plain_state).unwrap();
    let plain_launcher = temp.path().join("plain-launcher");
    fs::copy(launcher, &plain_launcher).unwrap();
    fs::set_permissions(&plain_launcher, fs::Permissions::from_mode(0o755)).unwrap();
    // The basename must be the installed GUI name for capture to classify it.
    let plain_gui_dir = temp.path().join("plain-gui");
    fs::create_dir(&plain_gui_dir).unwrap();
    let plain_gui_link = plain_gui_dir.join("oulipoly-plane");
    std::os::unix::fs::symlink(&plain_launcher, &plain_gui_link).unwrap();
    let plain_socket = temp.path().join("plain.sock");
    let plain_log = temp.path().join("plain.log");
    let plain_generation = uuid::Uuid::new_v4().to_string();
    let mut plain_broker = Command::new(broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &plain_socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &plain_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &original_runner)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1",
            &plain_launcher,
        )
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1",
            &plain_generation,
        )
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&plain_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| plain_socket.exists() || plain_broker.try_wait().unwrap().is_some());
    assert!(
        plain_socket.exists(),
        "{}",
        fs::read_to_string(&plain_log).unwrap()
    );
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o711)).unwrap();
    fs::set_permissions(&plain_socket, fs::Permissions::from_mode(0o777)).unwrap();
    let user_runtime = temp.path().join("user-runtime");
    fs::create_dir(&user_runtime).unwrap();
    fs::set_permissions(&user_runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let user_socket = user_runtime.join("wayland-user");
    let user_listener = UnixListener::bind(&user_socket).unwrap();
    for path in [&user_socket, &user_runtime] {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::chown(path.as_ptr(), 1, 1) }, 0);
    }
    fs::set_permissions(&plain_gui_dir, fs::Permissions::from_mode(0o755)).unwrap();
    let user_gui = launcher_command(
        plain_gui_link.to_str().unwrap(),
        &plain_socket,
        &plain_generation,
    )
    .uid(1)
    .gid(1)
    .env("XDG_RUNTIME_DIR", &user_runtime)
    .env("WAYLAND_DISPLAY", "wayland-user")
    .output()
    .unwrap();
    assert!(
        user_gui.status.success(),
        "launcher: {} broker: {}",
        String::from_utf8_lossy(&user_gui.stderr),
        fs::read_to_string(&plain_log).unwrap()
    );
    let user_report = fs::read_to_string(user_runtime.join("age319-private-gui-report")).unwrap();
    assert!(
        user_report.contains("uid=1 euid=1 gid=1 egid=1 groups=0"),
        "{user_report}"
    );
    assert!(
        user_report.contains("stdio_entry=111 nnp=0"),
        "{user_report}"
    );
    drop(user_listener);
    stop(&mut plain_broker);
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
