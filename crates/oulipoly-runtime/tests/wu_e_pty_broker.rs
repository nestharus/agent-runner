#![cfg(unix)]

//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `formatter`, `validator`
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
    if helper_env_missing() {
        return;
    }
    let provider_script = required_env_path(PROVIDER_SCRIPT_ENV);
    let result_path = required_env_path(RESULT_PATH_ENV);
    let provider = fixture_provider(&provider_script);
    let exit_code = run_broker_session(&provider);
    write_exit_code_result(&result_path, exit_code);
}

fn helper_env_missing() -> bool {
    std::env::var_os(HELPER_ENV).is_none()
}

fn required_env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap())
}

fn run_broker_session(provider: &ProviderConfig) -> i32 {
    execute_interactive_with_result_and_model_identity(
        provider,
        None,
        None,
        None,
        Some("fixture-model"),
    )
    .unwrap()
    .exit_code
}

fn write_exit_code_result(path: &Path, exit_code: i32) {
    fs::write(path, exit_code_result_text(exit_code)).unwrap();
}

fn exit_code_result_text(exit_code: i32) -> String {
    format!("{exit_code}\n")
}

struct OuterPty {
    master: File,
    slave: File,
}

impl OuterPty {
    fn open(rows: u16, cols: u16) -> Self {
        let winsize = pty_winsize(rows, cols);
        let (rc, master_fd, slave_fd) = openpty_fds(&winsize);
        assert_openpty_success(rc);
        outer_pty_from_fds(master_fd, slave_fd)
    }
}

fn pty_winsize(rows: u16, cols: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn openpty_fds(winsize: &libc::winsize) -> (i32, RawFd, RawFd) {
    let mut master_fd = -1;
    let mut slave_fd = -1;
    let rc = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null(),
            winsize,
        )
    };
    (rc, master_fd, slave_fd)
}

fn assert_openpty_success(rc: i32) {
    assert_eq!(rc, 0, "openpty failed: {}", io::Error::last_os_error());
}

fn outer_pty_from_fds(master_fd: RawFd, slave_fd: RawFd) -> OuterPty {
    OuterPty {
        master: unsafe { File::from_raw_fd(master_fd) },
        slave: unsafe { File::from_raw_fd(slave_fd) },
    }
}

fn spawn_helper_under_pty(
    pty: &OuterPty,
    provider: &Path,
    result_path: &Path,
) -> std::process::Child {
    let exe = current_test_exe();
    let (stdin, stdout, stderr) = cloned_slave_stdio(pty);
    let (slave_fd, master_fd) = pty_raw_fds(pty);
    let mut cmd = helper_command(exe, provider, result_path, stdin, stdout, stderr);
    install_helper_pre_exec(&mut cmd, slave_fd, master_fd);
    cmd.spawn().unwrap()
}

fn current_test_exe() -> PathBuf {
    std::env::current_exe().unwrap()
}

fn cloned_slave_stdio(pty: &OuterPty) -> (File, File, File) {
    (
        pty.slave.try_clone().unwrap(),
        pty.slave.try_clone().unwrap(),
        pty.slave.try_clone().unwrap(),
    )
}

fn pty_raw_fds(pty: &OuterPty) -> (RawFd, RawFd) {
    (pty.slave.as_raw_fd(), pty.master.as_raw_fd())
}

fn helper_command(
    exe: PathBuf,
    provider: &Path,
    result_path: &Path,
    stdin: File,
    stdout: File,
    stderr: File,
) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("--exact")
        .arg("helper_runs_broker_session")
        .arg("--nocapture")
        .env(HELPER_ENV, "1")
        // This test validates the PLAIN PTY broker relay specifically. The
        // split-pane observability TUI is the default for interactive sessions
        // with a controlling terminal, so opt out here to exercise the plain
        // path. The TUI path has its own end-to-end coverage in
        // `pty_broker::tui::tests::observed_relay_gives_child_a_tty_*`.
        .env("OULIPOLY_INTERACTIVE_TUI", "0")
        .env(PROVIDER_SCRIPT_ENV, provider)
        .env(RESULT_PATH_ENV, result_path)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    cmd
}

fn install_helper_pre_exec(cmd: &mut Command, slave_fd: RawFd, master_fd: RawFd) {
    unsafe {
        cmd.pre_exec(move || prepare_child_pty_session(slave_fd, master_fd));
    }
}

fn prepare_child_pty_session(slave_fd: RawFd, master_fd: RawFd) -> io::Result<()> {
    validate_setsid()?;
    validate_controlling_tty(slave_fd)?;
    validate_foreground_process_group(slave_fd)?;
    close_parent_master_fd(master_fd);
    Ok(())
}

fn validate_setsid() -> io::Result<()> {
    if unsafe { libc::setsid() } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn validate_controlling_tty(slave_fd: RawFd) -> io::Result<()> {
    if unsafe { libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn validate_foreground_process_group(slave_fd: RawFd) -> io::Result<()> {
    if unsafe { libc::tcsetpgrp(slave_fd, libc::getpid()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn close_parent_master_fd(master_fd: RawFd) {
    if master_fd > 2 {
        unsafe { libc::close(master_fd) };
    }
}

fn fixture_provider_script(dir: &Path) -> PathBuf {
    let path = dir.join("fixture-provider.sh");
    write_fixture_provider_script(&path);
    make_fixture_provider_script_executable(&path);
    path
}

fn fixture_provider_script_body() -> &'static str {
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
"#
}

fn write_fixture_provider_script(path: &Path) {
    fs::write(path, fixture_provider_script_body()).unwrap();
}

fn make_fixture_provider_script_executable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
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
    render_output(&read_until_bytes(fd, needle, timeout))
}

fn read_until_bytes(fd: RawFd, needle: &str, timeout: Duration) -> Vec<u8> {
    let start = Instant::now();
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    while start.elapsed() < timeout {
        if read_until_step(fd, needle, &mut output, &mut buffer) {
            break;
        }
    }
    output
}

fn read_until_step(fd: RawFd, needle: &str, output: &mut Vec<u8>, buffer: &mut [u8]) -> bool {
    if !poll_readable(fd, Duration::from_millis(50)).unwrap() {
        return false;
    }
    let n = read_fd(fd, buffer).unwrap();
    append_read_bytes(output, buffer, n);
    read_until_step_is_done(output, needle, n)
}

fn append_read_bytes(output: &mut Vec<u8>, buffer: &[u8], n: usize) {
    output.extend_from_slice(&buffer[..n]);
}

fn read_until_step_is_done(output: &[u8], needle: &str, n: usize) -> bool {
    read_reached_eof(n) || output_contains_needle_bytes(output, needle)
}

fn read_reached_eof(n: usize) -> bool {
    n == 0
}

fn output_contains_needle_bytes(output: &[u8], needle: &str) -> bool {
    output
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn render_output(output: &[u8]) -> String {
    String::from_utf8_lossy(output).into_owned()
}

fn poll_readable(fd: RawFd, timeout: Duration) -> io::Result<bool> {
    let pollfd = poll_fd(fd, timeout)?;
    Ok(pollfd_is_readable(&pollfd))
}

fn poll_fd(fd: RawFd, timeout: Duration) -> io::Result<libc::pollfd> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut pollfd, 1, timeout.as_millis() as i32) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(pollfd)
}

fn pollfd_is_readable(pollfd: &libc::pollfd) -> bool {
    pollfd.revents & libc::POLLIN != 0
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
        offset += write_fd(fd, &bytes[offset..])?;
    }
    Ok(())
}

fn write_fd(fd: RawFd, bytes: &[u8]) -> io::Result<usize> {
    let rc = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(rc as usize)
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
