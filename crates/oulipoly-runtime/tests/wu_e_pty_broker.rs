#![cfg(unix)]

//! ## Declared roles
//!
//! `orchestration`, `accessor`, `formatter`, `validator`
//!
//! Unix outer-PTY integration harness for the plain broker opt-out path.

use oulipoly_config::ProviderConfig;
use oulipoly_runtime::executor::cli::execute_interactive_with_result_and_model_identity;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HELPER_ENV: &str = "WU_E_PTY_HELPER";
const PROVIDER_SCRIPT_ENV: &str = "WU_E_PROVIDER_SCRIPT";
const RESULT_PATH_ENV: &str = "WU_E_RESULT_PATH";

#[test]
fn broker_child_sees_tty_relays_io_preserves_exit_and_restores_raw_mode() {
    let dir = tempfile::tempdir().unwrap();
    let provider = fixture_provider_script(dir.path());
    let result_path = dir.path().join("result.txt");
    let pty = OuterPty::open(33, 100);
    let before = terminal_attrs(pty.slave.as_raw_fd()).unwrap();

    let mut child = spawn_helper_under_pty(&pty, &provider, &result_path);
    let output = read_until(
        pty.master.as_raw_fd(),
        "SIZE:33 100",
        Duration::from_secs(5),
    );
    assert!(output.contains("TTY:yes"), "output was {output:?}");
    assert!(output.contains("SIZE:33 100"), "output was {output:?}");
    write_all_fd(pty.master.as_raw_fd(), b"hello broker\n").unwrap();
    let output = read_until(
        pty.master.as_raw_fd(),
        "ECHO:hello broker",
        Duration::from_secs(5),
    );
    assert!(
        output.contains("ECHO:hello broker"),
        "output was {output:?}"
    );

    let status = child.wait().unwrap();
    assert!(status.success(), "helper failed with {status:?}");
    let after = terminal_attrs(pty.slave.as_raw_fd()).unwrap();
    assert_termios_eq(&before, &after);
    assert_eq!(fs::read_to_string(result_path).unwrap(), "7\n");
}

#[test]
fn helper_runs_broker_session() {
    if std::env::var_os(HELPER_ENV).is_none() {
        return;
    }
    let provider_script = required_env_path(PROVIDER_SCRIPT_ENV);
    let result_path = required_env_path(RESULT_PATH_ENV);
    let provider = fixture_provider(&provider_script);
    let exit_code = execute_interactive_with_result_and_model_identity(
        &provider,
        None,
        None,
        None,
        Some("fixture-model"),
    )
    .unwrap()
    .exit_code;
    fs::write(result_path, format!("{exit_code}\n")).unwrap();
}

struct OuterPty {
    master: File,
    slave: File,
}

impl OuterPty {
    fn open(rows: u16, cols: u16) -> Self {
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut master_fd = -1;
        let mut slave_fd = -1;
        let rc = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize,
            )
        };
        assert_eq!(rc, 0, "openpty failed: {}", io::Error::last_os_error());
        Self {
            master: unsafe { File::from_raw_fd(master_fd) },
            slave: unsafe { File::from_raw_fd(slave_fd) },
        }
    }
}

fn required_env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap())
}

fn spawn_helper_under_pty(
    pty: &OuterPty,
    provider: &Path,
    result_path: &Path,
) -> std::process::Child {
    let stdin = pty.slave.try_clone().unwrap();
    let stdout = pty.slave.try_clone().unwrap();
    let stderr = pty.slave.try_clone().unwrap();
    let slave_fd = pty.slave.as_raw_fd();
    let master_fd = pty.master.as_raw_fd();
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.arg("--exact")
        .arg("helper_runs_broker_session")
        .arg("--nocapture")
        .env(HELPER_ENV, "1")
        .env("OULIPOLY_INTERACTIVE_TUI", "0")
        .env(PROVIDER_SCRIPT_ENV, provider)
        .env(RESULT_PATH_ENV, result_path)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        cmd.pre_exec(move || prepare_child_pty_session(slave_fd, master_fd));
    }
    cmd.spawn().unwrap()
}

fn prepare_child_pty_session(slave_fd: RawFd, master_fd: RawFd) -> io::Result<()> {
    if unsafe { libc::setsid() } == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::tcsetpgrp(slave_fd, libc::getpid()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if master_fd > 2 {
        unsafe { libc::close(master_fd) };
    }
    Ok(())
}

fn fixture_provider_script(dir: &Path) -> PathBuf {
    let path = dir.join("fixture-provider.sh");
    fs::write(
        &path,
        r#"#!/usr/bin/env bash
set -euo pipefail
test -t 0
test -t 1
test -t 2
printf 'TTY:yes\n'
printf 'SIZE:%s\n' "$(stty size)"
IFS= read -r line
printf 'ECHO:%s\n' "$line"
exit 7
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn fixture_provider(script: &Path) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: "fixture-provider".to_string(),
        command: script.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: Some(Vec::new()),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn read_until(fd: RawFd, needle: &str, timeout: Duration) -> String {
    let start = Instant::now();
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    while start.elapsed() < timeout {
        if !poll_readable(fd, Duration::from_millis(50)).unwrap() {
            continue;
        }
        let count = read_fd(fd, &mut buffer).unwrap();
        output.extend_from_slice(&buffer[..count]);
        if count == 0 || output_contains(&output, needle) {
            break;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn output_contains(output: &[u8], needle: &str) -> bool {
    output
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn poll_readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pollfd, 1, timeout.as_millis() as i32) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(pollfd.revents & libc::POLLIN != 0)
}

fn read_fd(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    let rc = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(rc as usize)
}

fn write_all_fd(fd: RawFd, bytes: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let rc = unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if rc == -1 {
            return Err(io::Error::last_os_error());
        }
        offset += rc as usize;
    }
    Ok(())
}

fn terminal_attrs(fd: RawFd) -> io::Result<libc::termios> {
    let mut attrs = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut attrs) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(attrs)
}

fn assert_termios_eq(left: &libc::termios, right: &libc::termios) {
    assert_eq!(left.c_iflag, right.c_iflag);
    assert_eq!(left.c_oflag, right.c_oflag);
    assert_eq!(left.c_cflag, right.c_cflag);
    assert_eq!(left.c_lflag, right.c_lflag);
    assert_eq!(left.c_cc, right.c_cc);
}
