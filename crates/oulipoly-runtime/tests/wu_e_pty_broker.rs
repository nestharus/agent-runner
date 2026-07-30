#![cfg(unix)]

//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `formatter`, `validator`
//!
//! Unix outer-PTY integration harness for the plain broker opt-out path.

use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy};
use oulipoly_runtime::executor::cli::pty_broker::{
    inject_control_envelope, render_mailbox_notification_envelope,
};
use oulipoly_runtime::executor::cli::{
    ResumePayload, execute_interactive_with_result_and_model_config,
    execute_interactive_with_result_and_model_identity,
};
use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, EnqueueResult, MAILBOX_DELIVERY_UNCONFIRMED_ERROR, MailboxDb,
    MailboxRow,
};
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HELPER_ENV: &str = "WU_E_PTY_HELPER";
const PROVIDER_SCRIPT_ENV: &str = "WU_E_PROVIDER_SCRIPT";
const RESULT_PATH_ENV: &str = "WU_E_RESULT_PATH";
const OBSERVED_HELPER_ENV: &str = "WU_E_OBSERVED_HELPER";
const OBSERVER_ADAPTER_ENV: &str = "WU_E_OBSERVER_ADAPTER";
const OBSERVER_STATE_ENV: &str = "WU_E_OBSERVER_STATE";
const OBSERVER_EVENTS_ENV: &str = "WU_E_OBSERVER_EVENTS";
const OBSERVER_WORKING_DIR_ENV: &str = "WU_E_OBSERVER_WORKING_DIR";
const OBSERVER_SESSION_ID: &str = "session-overlay-proof";
const OBSERVER_INVOCATION_UUID: &str = "11111111-1111-4111-8111-111111111111";
const PREVIOUS_INVOCATION_UUID: &str = "22222222-2222-4222-8222-222222222222";
const MAILBOX_ATTEMPT_ID: &str = "mailbox-live-observation-attempt";

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
fn production_observed_relay_separates_idle_draft_from_queued_overlay() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("events.log");
    let state = dir.path().join("observer-state");
    fs::write(&state, "baseline").unwrap();
    let child_script = observed_child_script(dir.path(), &events, &state);
    let adapter = observer_adapter_script(dir.path(), &events, &state);
    let result_path = dir.path().join("observed-result.txt");
    let pty = OuterPty::open(33, 100);
    let before = terminal_attrs(pty.slave.as_raw_fd()).unwrap();
    let mut child = spawn_observed_helper_under_pty(
        &pty,
        &child_script,
        &adapter,
        &state,
        &events,
        dir.path(),
        &result_path,
    );

    let output = read_until_bytes(pty.master.as_raw_fd(), "OBS", Duration::from_secs(5));
    write_all_fd(pty.master.as_raw_fd(), b"first").unwrap();
    std::thread::sleep(Duration::from_millis(1_700));
    write_all_fd(pty.master.as_raw_fd(), b"\x1b[<0;1;32Msecond\r").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !fs::read_to_string(&events).unwrap().contains("child:"),
        "queued overlay reached the child before the ordinary draft was submitted"
    );
    write_all_fd(pty.master.as_raw_fd(), b"\x1b[<0;1;1M\r").unwrap();
    let output = read_until_bytes_with_prefix(
        pty.master.as_raw_fd(),
        output,
        "test result:",
        Duration::from_secs(10),
    );

    let status = child.wait().unwrap();
    assert!(status.success(), "observed helper failed with {status:?}");
    assert_eq!(fs::read_to_string(result_path).unwrap(), "0\n");
    assert_termios_eq(&before, &terminal_attrs(pty.slave.as_raw_fd()).unwrap());
    let event_text = fs::read_to_string(&events).unwrap();
    let events = event_text.lines().collect::<Vec<_>>();
    let child_lines = events
        .iter()
        .filter(|line| line.starts_with("child:"))
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(child_lines, vec!["child:first", "child:second"]);
    let observed_new = events
        .iter()
        .position(|line| *line == "observer:new-user-turn")
        .expect("adapter must observe the new canonical user turn");
    let child_second = events
        .iter()
        .position(|line| *line == "child:second")
        .expect("child second line");
    assert!(
        observed_new < child_second,
        "second body arrived before provider observation: {event_text}"
    );
    assert!(event_text.contains("session.read_turns"));
    assert!(event_text.contains("observer-account"));
    assert!(event_text.contains(OBSERVER_SESSION_ID));
    assert!(event_text.contains(&dir.path().display().to_string()));
    assert!(!rendered_screen_text(&output, 33, 100).contains("outbound:"));
}

#[test]
fn production_observed_relay_confirms_live_mailbox_marker_without_state_ingest() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("mailbox-events.log");
    let observer_state = dir.path().join("mailbox-observer-state");
    let data_root = dir.path().join("runner-data");
    let runtime_root = dir.path().join("runtime");
    fs::create_dir_all(&data_root).unwrap();
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(&observer_state, "baseline").unwrap();
    let child_script = observed_mailbox_child_script(dir.path(), &events, &observer_state);
    let adapter = observer_adapter_script(dir.path(), &events, &observer_state);
    let mailbox_path = data_root.join("pid-identity.db");
    let row = seed_mailbox_attempt(&mailbox_path, OBSERVER_INVOCATION_UUID, false);
    let result_path = dir.path().join("mailbox-observed-result.txt");
    let pty = OuterPty::open(33, 100);
    let mut child = spawn_mailbox_observed_helper_under_pty(
        &pty,
        &child_script,
        &adapter,
        &observer_state,
        &events,
        dir.path(),
        &data_root,
        &runtime_root,
        &result_path,
    );

    let output = read_until_bytes(
        pty.master.as_raw_fd(),
        "READY-MAILBOX",
        Duration::from_secs(5),
    );
    let control_path = wait_for_control_path(&mailbox_path);
    let envelope = render_mailbox_notification_envelope(&[row], 0, MAILBOX_ATTEMPT_ID);
    let response = inject_control_envelope(&control_path, &envelope).unwrap();
    assert!(response.ack, "{response:?}");
    let output = read_until_bytes_with_prefix(
        pty.master.as_raw_fd(),
        output,
        "GOT-MAILBOX",
        Duration::from_secs(5),
    );
    wait_for_mailbox_delivery(&mailbox_path);
    assert!(child.try_wait().unwrap().is_none());
    write_all_fd(pty.master.as_raw_fd(), b"exit\r").unwrap();

    let status = child.wait().unwrap();
    assert!(status.success(), "observed helper failed with {status:?}");
    assert_eq!(fs::read_to_string(result_path).unwrap(), "0\n");
    let event_text = fs::read_to_string(events).unwrap();
    assert!(event_text.contains("observer:mailbox-user-turn"));
    assert!(event_text.contains(MAILBOX_ATTEMPT_ID));
    assert!(!rendered_screen_text(&output, 33, 100).contains("outbound:"));
}

#[test]
fn production_observer_resolves_previous_pty_invocation_before_redelivery() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("restart-mailbox-events.log");
    let observer_state = dir.path().join("restart-mailbox-state");
    let data_root = dir.path().join("runner-data");
    let runtime_root = dir.path().join("runtime");
    fs::create_dir_all(&data_root).unwrap();
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        &observer_state,
        format!("[OULIPOLY-DELIVERY {MAILBOX_ATTEMPT_ID}]"),
    )
    .unwrap();
    let child_script = observed_mailbox_child_script(dir.path(), &events, &observer_state);
    let adapter = observer_adapter_script(dir.path(), &events, &observer_state);
    let mailbox_path = data_root.join("pid-identity.db");
    seed_mailbox_attempt(&mailbox_path, PREVIOUS_INVOCATION_UUID, true);
    let result_path = dir.path().join("restart-mailbox-result.txt");
    let pty = OuterPty::open(33, 100);
    let mut child = spawn_mailbox_observed_helper_under_pty(
        &pty,
        &child_script,
        &adapter,
        &observer_state,
        &events,
        dir.path(),
        &data_root,
        &runtime_root,
        &result_path,
    );

    read_until_bytes(
        pty.master.as_raw_fd(),
        "READY-MAILBOX",
        Duration::from_secs(5),
    );
    wait_for_mailbox_delivery(&mailbox_path);
    let delivered = MailboxDb::open(&mailbox_path)
        .unwrap()
        .list_mailbox(OBSERVER_SESSION_ID, true)
        .unwrap();
    assert_eq!(
        delivered[0].delivered_by_invocation_uuid.as_deref(),
        Some(PREVIOUS_INVOCATION_UUID)
    );
    assert!(child.try_wait().unwrap().is_none());
    write_all_fd(pty.master.as_raw_fd(), b"exit\r").unwrap();

    assert!(child.wait().unwrap().success());
    assert_eq!(fs::read_to_string(result_path).unwrap(), "0\n");
    let event_text = fs::read_to_string(events).unwrap();
    assert!(event_text.contains("observer:mailbox-user-turn"));
    assert!(!event_text.contains("child:[OULIPOLY NOTIFICATIONS]"));
}

#[test]
fn production_observer_releases_unobserved_previous_pty_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("restart-unobserved-mailbox-events.log");
    let observer_state = dir.path().join("restart-unobserved-mailbox-state");
    let data_root = dir.path().join("runner-data");
    let runtime_root = dir.path().join("runtime");
    fs::create_dir_all(&data_root).unwrap();
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(&observer_state, "baseline").unwrap();
    let child_script = observed_mailbox_child_script(dir.path(), &events, &observer_state);
    let adapter = observer_adapter_script(dir.path(), &events, &observer_state);
    let mailbox_path = data_root.join("pid-identity.db");
    seed_mailbox_attempt(&mailbox_path, PREVIOUS_INVOCATION_UUID, true);
    let result_path = dir.path().join("restart-unobserved-mailbox-result.txt");
    let pty = OuterPty::open(33, 100);
    let mut child = spawn_mailbox_observed_helper_under_pty(
        &pty,
        &child_script,
        &adapter,
        &observer_state,
        &events,
        dir.path(),
        &data_root,
        &runtime_root,
        &result_path,
    );

    read_until_bytes(
        pty.master.as_raw_fd(),
        "READY-MAILBOX",
        Duration::from_secs(5),
    );
    wait_for_unobserved_mailbox_attempt(&mailbox_path);
    assert!(child.try_wait().unwrap().is_none());
    write_all_fd(pty.master.as_raw_fd(), b"exit\r").unwrap();

    assert!(child.wait().unwrap().success());
    assert_eq!(fs::read_to_string(result_path).unwrap(), "0\n");
    let event_text = fs::read_to_string(events).unwrap();
    assert!(event_text.contains("session.read_turns"));
    assert!(!event_text.contains("child:[OULIPOLY NOTIFICATIONS]"));
}

#[test]
fn production_observed_relay_releases_unobserved_mailbox_attempt_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    let events = dir.path().join("unobserved-mailbox-events.log");
    let observer_state = dir.path().join("unobserved-mailbox-state");
    let data_root = dir.path().join("runner-data");
    let runtime_root = dir.path().join("runtime");
    fs::create_dir_all(&data_root).unwrap();
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(&observer_state, "baseline").unwrap();
    let child_script =
        observed_unconfirmed_mailbox_child_script(dir.path(), &events, &observer_state);
    let adapter = observer_adapter_script(dir.path(), &events, &observer_state);
    let mailbox_path = data_root.join("pid-identity.db");
    let row = seed_mailbox_attempt(&mailbox_path, OBSERVER_INVOCATION_UUID, false);
    let result_path = dir.path().join("unobserved-mailbox-result.txt");
    let pty = OuterPty::open(33, 100);
    let mut child = spawn_mailbox_observed_helper_under_pty(
        &pty,
        &child_script,
        &adapter,
        &observer_state,
        &events,
        dir.path(),
        &data_root,
        &runtime_root,
        &result_path,
    );

    read_until_bytes(
        pty.master.as_raw_fd(),
        "READY-MAILBOX",
        Duration::from_secs(5),
    );
    let control_path = wait_for_control_path(&mailbox_path);
    let envelope = render_mailbox_notification_envelope(&[row], 0, MAILBOX_ATTEMPT_ID);
    let response = inject_control_envelope(&control_path, &envelope).unwrap();
    assert!(response.ack, "{response:?}");
    read_until_bytes(
        pty.master.as_raw_fd(),
        "GOT-MAILBOX",
        Duration::from_secs(5),
    );
    wait_for_unobserved_mailbox_attempt(&mailbox_path);
    assert!(child.try_wait().unwrap().is_none());
    write_all_fd(pty.master.as_raw_fd(), b"exit\r").unwrap();

    let status = child.wait().unwrap();
    assert!(status.success(), "observed helper failed with {status:?}");
    assert_eq!(fs::read_to_string(result_path).unwrap(), "0\n");
    let event_text = fs::read_to_string(events).unwrap();
    assert!(event_text.contains("observer:later-user-turn"));
    assert!(!event_text.contains("observer:mailbox-user-turn"));
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

#[test]
fn helper_runs_production_observed_session() {
    if std::env::var_os(OBSERVED_HELPER_ENV).is_none() {
        return;
    }
    let child_script = required_env_path(PROVIDER_SCRIPT_ENV);
    let adapter = required_env_path(OBSERVER_ADAPTER_ENV);
    let working_dir = required_env_path(OBSERVER_WORKING_DIR_ENV);
    let result_path = required_env_path(RESULT_PATH_ENV);
    let mut provider = fixture_provider(&child_script);
    provider.name = "observer-account".to_string();
    let model = observed_model(&adapter);
    let registry = ProviderRegistry::from_model_configs(
        std::slice::from_ref(&model),
        ProviderRegistryOptions::default(),
    )
    .unwrap();
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };
    let invocation = format!(r#"{{"source":"test","id":"{OBSERVER_INVOCATION_UUID}"}}"#);
    let result = execute_interactive_with_result_and_model_config(
        &provider,
        Some(&working_dir),
        Some(&invocation),
        Some(ResumePayload {
            session_id: OBSERVER_SESSION_ID,
            strategy: &strategy,
        }),
        &model,
        Arc::new(registry),
    )
    .unwrap();
    write_exit_code_result(&result_path, result.exit_code);
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

#[allow(clippy::too_many_arguments)]
fn spawn_observed_helper_under_pty(
    pty: &OuterPty,
    child_script: &Path,
    adapter: &Path,
    state: &Path,
    events: &Path,
    working_dir: &Path,
    result_path: &Path,
) -> std::process::Child {
    let (stdin, stdout, stderr) = cloned_slave_stdio(pty);
    let (slave_fd, master_fd) = pty_raw_fds(pty);
    let mut cmd = Command::new(current_test_exe());
    cmd.arg("--exact")
        .arg("helper_runs_production_observed_session")
        .arg("--nocapture")
        .env(OBSERVED_HELPER_ENV, "1")
        .env(PROVIDER_SCRIPT_ENV, child_script)
        .env(OBSERVER_ADAPTER_ENV, adapter)
        .env(OBSERVER_STATE_ENV, state)
        .env(OBSERVER_EVENTS_ENV, events)
        .env(OBSERVER_WORKING_DIR_ENV, working_dir)
        .env(RESULT_PATH_ENV, result_path)
        .env("TERM", "xterm-256color")
        .env("OULIPOLY_INTERACTIVE_TUI", "1")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    install_helper_pre_exec(&mut cmd, slave_fd, master_fd);
    cmd.spawn().unwrap()
}

#[allow(clippy::too_many_arguments)]
fn spawn_mailbox_observed_helper_under_pty(
    pty: &OuterPty,
    child_script: &Path,
    adapter: &Path,
    state: &Path,
    events: &Path,
    working_dir: &Path,
    data_root: &Path,
    runtime_root: &Path,
    result_path: &Path,
) -> std::process::Child {
    let (stdin, stdout, stderr) = cloned_slave_stdio(pty);
    let (slave_fd, master_fd) = pty_raw_fds(pty);
    let mut cmd = Command::new(current_test_exe());
    cmd.arg("--exact")
        .arg("helper_runs_production_observed_session")
        .arg("--nocapture")
        .env(OBSERVED_HELPER_ENV, "1")
        .env(PROVIDER_SCRIPT_ENV, child_script)
        .env(OBSERVER_ADAPTER_ENV, adapter)
        .env(OBSERVER_STATE_ENV, state)
        .env(OBSERVER_EVENTS_ENV, events)
        .env(OBSERVER_WORKING_DIR_ENV, working_dir)
        .env(RESULT_PATH_ENV, result_path)
        .env("OULIPOLY_DATA_DIR", data_root)
        .env("XDG_RUNTIME_DIR", runtime_root)
        .env("TERM", "xterm-256color")
        .env("OULIPOLY_INTERACTIVE_TUI", "1")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
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

fn observed_model(adapter: &Path) -> ModelConfig {
    ModelConfig {
        name: "observer-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            "observer-account",
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some(adapter.display().to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
    }
}

fn observed_child_script(dir: &Path, events: &Path, state: &Path) -> PathBuf {
    let path = dir.join("observed-child.py");
    let body = format!(
        r#"#!/usr/bin/env python3
import pathlib
import sys
events = pathlib.Path({events:?})
state = pathlib.Path({state:?})
for line in sys.stdin:
    line = line.rstrip("\r\n")
    with events.open("a", encoding="utf-8") as handle:
        handle.write("child:" + line + "\n")
    if line == "first":
        state.write_text("new")
        print("READY-FOR-NEXT", flush=True)
    if line == "second":
        raise SystemExit(0)
raise SystemExit(9)
"#,
        events = events.display().to_string(),
        state = state.display().to_string(),
    );
    write_executable(&path, &body);
    path
}

fn observed_mailbox_child_script(dir: &Path, events: &Path, state: &Path) -> PathBuf {
    let path = dir.join("observed-mailbox-child.py");
    let body = format!(
        r#"#!/usr/bin/env python3
import pathlib
import sys
events = pathlib.Path({events:?})
state = pathlib.Path({state:?})
print("\x1b[?2004hREADY-MAILBOX", flush=True)
for line in sys.stdin:
    line = line.rstrip("\r\n")
    line = line.replace("\x1b[200~", "").replace("\x1b[201~", "")
    with events.open("a", encoding="utf-8") as handle:
        handle.write("child:" + line + "\n")
    if line.startswith("[OULIPOLY-DELIVERY "):
        state.write_text(line)
    if line == "[END OULIPOLY NOTIFICATIONS]":
        print("GOT-MAILBOX", flush=True)
    if line == "exit":
        raise SystemExit(0)
raise SystemExit(9)
"#,
        events = events.display().to_string(),
        state = state.display().to_string(),
    );
    write_executable(&path, &body);
    path
}

fn observed_unconfirmed_mailbox_child_script(dir: &Path, events: &Path, state: &Path) -> PathBuf {
    let path = dir.join("observed-unconfirmed-mailbox-child.py");
    let body = format!(
        r#"#!/usr/bin/env python3
import pathlib
import sys
events = pathlib.Path({events:?})
state = pathlib.Path({state:?})
print("\x1b[?2004hREADY-MAILBOX", flush=True)
for line in sys.stdin:
    line = line.rstrip("\r\n")
    line = line.replace("\x1b[200~", "").replace("\x1b[201~", "")
    with events.open("a", encoding="utf-8") as handle:
        handle.write("child:" + line + "\n")
    if line == "[END OULIPOLY NOTIFICATIONS]":
        state.write_text("later-without-marker")
        print("GOT-MAILBOX", flush=True)
    if line == "exit":
        raise SystemExit(0)
raise SystemExit(9)
"#,
        events = events.display().to_string(),
        state = state.display().to_string(),
    );
    write_executable(&path, &body);
    path
}

fn observer_adapter_script(dir: &Path, events: &Path, state: &Path) -> PathBuf {
    let path = dir.join("observer-adapter.py");
    let body = format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys
import time
CONTRACT = "oulipoly.provider/v1"
OBSERVER_SESSION_ID = {session:?}
events = pathlib.Path({events:?})
state = pathlib.Path({state:?})
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
with events.open("a", encoding="utf-8") as handle:
    handle.write("adapter:" + subcommand + ":" + json.dumps(request, sort_keys=True) + "\n")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "outer-pty-request"),
        "ok": True,
        "result": result,
    }}

if subcommand == "describe":
    print(json.dumps(envelope({{
        "provider_id": "observer-provider",
        "display_name": "Observer Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False, "policy": False, "quota": False, "session": True,
            "terminal": False, "rotation": False, "discovery": False,
            "settings": False, "setup_brain": False, "setup": False, "migration": False
        }}
    }})))
    raise SystemExit(0)

if subcommand != "session.read_turns":
    raise SystemExit(4)
time.sleep(0.2)
turns = [{{
    "session_id": OBSERVER_SESSION_ID,
    "turn_id": "old-equal-body",
    "timestamp": "2026-05-01T00:00:01Z",
    "role": "user",
    "body": [{{"type": "text", "text": "first"}}],
}}]
state_value = state.read_text().strip()
if state_value == "new":
    turns.append({{
        "session_id": OBSERVER_SESSION_ID,
        "turn_id": "new-exact-body",
        "timestamp": "2026-05-01T00:00:02Z",
        "role": "user",
        "body": [{{"type": "text", "text": "first"}}],
    }})
    with events.open("a", encoding="utf-8") as handle:
        handle.write("observer:new-user-turn\n")
elif state_value.startswith("[OULIPOLY-DELIVERY "):
    turns.append({{
        "session_id": OBSERVER_SESSION_ID,
        "turn_id": "mailbox-exact-marker",
        "timestamp": "2026-07-25T20:56:24Z",
        "role": "user",
        "body": [{{"type": "text", "text": state_value}}],
    }})
    with events.open("a", encoding="utf-8") as handle:
        handle.write("observer:mailbox-user-turn\n")
elif state_value == "later-without-marker":
    turns.append({{
        "session_id": OBSERVER_SESSION_ID,
        "turn_id": "later-ordinary-user-turn",
        "timestamp": "2099-07-25T20:57:25Z",
        "role": "user",
        "body": [{{"type": "text", "text": "ordinary later user turn"}}],
    }})
    with events.open("a", encoding="utf-8") as handle:
        handle.write("observer:later-user-turn\n")
print(json.dumps(envelope({{"turns": turns, "turn_count": len(turns), "complete": True}})))
"#,
        events = events.display().to_string(),
        state = state.display().to_string(),
        session = OBSERVER_SESSION_ID,
    );
    write_executable(&path, &body);
    path
}

fn seed_mailbox_attempt(
    mailbox_path: &Path,
    delivery_invocation_uuid: &str,
    acknowledged: bool,
) -> MailboxRow {
    let mut db = MailboxDb::open(mailbox_path).unwrap();
    let row = match db
        .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: OBSERVER_SESSION_ID,
            handle: "mailbox-live-observation-handle",
            payload_json: r#"{"schema_version":1,"kind":"agent_bash_complete"}"#,
            owner_invocation_uuid: Some(OBSERVER_INVOCATION_UUID),
            matched_os_pid: None,
            matched_os_boot_id: None,
            matched_os_pid_starttime_ticks: None,
            matched_chain_index: Some(0),
            state_dir: "/fixture/state",
            meta_path: "/fixture/meta.json",
            log_path: "/fixture/log",
            rc_path: "/fixture/rc",
            rc: 0,
        })
        .unwrap()
    {
        EnqueueResult::Inserted(row) => row,
        other => panic!("expected inserted mailbox row, got {other:?}"),
    };
    db.register_delivery_attempt(
        MAILBOX_ATTEMPT_ID,
        OBSERVER_SESSION_ID,
        delivery_invocation_uuid,
        &[row.seq],
        0,
    )
    .unwrap();
    if acknowledged {
        db.record_delivery_attempt_transport_ack(MAILBOX_ATTEMPT_ID)
            .unwrap();
    }
    row
}

fn wait_for_control_path(mailbox_path: &Path) -> PathBuf {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let db = MailboxDb::open(mailbox_path).unwrap();
        if let Some(path) = db
            .session_runtime(OBSERVER_SESSION_ID)
            .unwrap()
            .and_then(|runtime| runtime.pty_control_path)
        {
            return PathBuf::from(path);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for PTY control path")
}

fn wait_for_mailbox_delivery(mailbox_path: &Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let db = MailboxDb::open(mailbox_path).unwrap();
        let rows = db.list_mailbox(OBSERVER_SESSION_ID, true).unwrap();
        if rows.len() == 1 && rows[0].delivered_at.is_some() && rows[0].delivery_attempts == 1 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for provider-observed mailbox delivery")
}

fn wait_for_unobserved_mailbox_attempt(mailbox_path: &Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let db = MailboxDb::open(mailbox_path).unwrap();
        let rows = db.list_pending(OBSERVER_SESSION_ID).unwrap();
        let accepted = db
            .accepted_delivery_attempt_windows(OBSERVER_SESSION_ID)
            .unwrap();
        if rows.len() == 1
            && rows[0].delivery_attempts == 1
            && rows[0].delivery_error.as_deref() == Some(MAILBOX_DELIVERY_UNCONFIRMED_ERROR)
            && accepted.is_empty()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for unobserved mailbox attempt release")
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn read_until(fd: RawFd, needle: &str, timeout: Duration) -> String {
    render_output(&read_until_bytes(fd, needle, timeout))
}

fn read_until_bytes(fd: RawFd, needle: &str, timeout: Duration) -> Vec<u8> {
    read_until_bytes_with_prefix(fd, Vec::new(), needle, timeout)
}

fn read_until_bytes_with_prefix(
    fd: RawFd,
    mut output: Vec<u8>,
    needle: &str,
    timeout: Duration,
) -> Vec<u8> {
    let start = Instant::now();
    let mut buffer = [0_u8; 4096];
    while start.elapsed() < timeout {
        if read_until_step(fd, needle, &mut output, &mut buffer) {
            break;
        }
    }
    output
}

fn rendered_screen_text(output: &[u8], rows: u16, cols: u16) -> String {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(output);
    parser.screen().contents()
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
