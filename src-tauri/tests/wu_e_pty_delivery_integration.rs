#![cfg(unix)]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{MailboxDb, SessionRuntimeRunningUpdate};
use oulipoly_state::pid_identity::{
    PidIdentityDb, PidIdentityRecord, ProcessIdentity, read_live_process_identity,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const INVOCATION_A: &str = "11111111-1111-4111-8111-111111111111";
const LIVE_INVOCATION: &str = "22222222-2222-4222-8222-222222222222";
const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    runtime_dir: PathBuf,
    state_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

struct NotifyArtifacts {
    state_dir: PathBuf,
    meta: PathBuf,
    log: PathBuf,
    rc: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("xdg-config");
        let data_home = dir.path().join("xdg-data");
        let runtime_dir = dir.path().join("xdg-runtime");
        let state_home = dir.path().join("xdg-state");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&config_home).unwrap();
        fs::create_dir_all(&data_home).unwrap();
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::create_dir_all(&state_home).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&models_dir).unwrap();
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            runtime_dir,
            state_home,
            home_dir,
            app_config_dir,
            models_dir,
        }
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn conn(&self) -> Connection {
        let _ = StateDb::open(&self.state_path()).unwrap();
        Connection::open(self.state_path()).unwrap()
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir);
        cmd.env("XDG_STATE_HOME", &self.state_home);
        cmd.env("HOME", &self.home_dir);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }

    fn run_notify(&self, handle: &str, metadata: Value) -> Output {
        let artifacts = self.write_notify_artifacts(handle, metadata, 0);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("notify")
            .arg("agent-bash-complete")
            .arg("--caller-ppid")
            .arg(std::process::id().to_string())
            .arg("--handle")
            .arg(handle)
            .arg("--state-dir")
            .arg(&artifacts.state_dir)
            .arg("--meta")
            .arg(&artifacts.meta)
            .arg("--log")
            .arg(&artifacts.log)
            .arg("--rc")
            .arg(&artifacts.rc)
            .arg("--json");
        self.run(cmd)
    }

    fn base_repl_command(&self, model_name: &str, session_id: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("repl")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--resume")
            .arg(session_id)
            .arg(model_name);
        self.configure_env(&mut cmd);
        cmd.current_dir(self.dir.path());
        cmd
    }

    fn configure_env(&self, cmd: &mut Command) {
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("XDG_RUNTIME_DIR", &self.runtime_dir);
        cmd.env("XDG_STATE_HOME", &self.state_home);
        cmd.env("HOME", &self.home_dir);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
    }

    fn write_notify_artifacts(&self, handle: &str, metadata: Value, rc: i32) -> NotifyArtifacts {
        let state_dir = self.dir.path().join(format!("notify-{handle}"));
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc_path = state_dir.join("rc");
        fs::write(&meta, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();
        fs::write(&log, format!("log for {handle}\n")).unwrap();
        fs::write(&rc_path, format!("{rc}\n")).unwrap();
        NotifyArtifacts {
            state_dir,
            meta,
            log,
            rc: rc_path,
        }
    }

    fn record_owner_identity(&self, identity: &ProcessIdentity) {
        let sidecar = PidIdentityDb::open(&self.sidecar_path()).unwrap();
        sidecar
            .record_identity(PidIdentityRecord {
                identity,
                os_pgid: None,
                invocation_uuid: INVOCATION_A,
                session_id: Some(SESSION_A),
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture-model"),
                recorded_at: "2026-06-04T12:00:00Z",
            })
            .unwrap();
    }

    fn mark_live_pty_runtime(&self, identity: &ProcessIdentity, control_path: &Path) {
        let mut mailbox = MailboxDb::open(&self.sidecar_path()).unwrap();
        mailbox
            .mark_session_running(SessionRuntimeRunningUpdate {
                session_id: SESSION_A,
                mode: "pty_interactive",
                invocation_uuid: LIVE_INVOCATION,
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture-model"),
                identity,
                pty_control_path: Some(&path_string(control_path)),
                turn_start_max_mailbox_seq: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
    }

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn socket_path(&self, name: &str) -> PathBuf {
        let dir = self.runtime_dir.join("oulipoly-agent-runner/pty");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir.join(name)
    }

    fn write_interactive_model(&self, model_name: &str, provider_name: &str, script: &Path) {
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!(
                r#"[[providers]]
name = "{provider_name}"
args = []
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{provider_name}]
command = {}
args = []
interactive_args = []
prompt_mode = "arg"

[{provider_name}.resume]
kind = "flag"
flag = "--resume"
"#,
                toml_string(&path_string(script))
            ),
        )
        .unwrap();
    }

    fn seed_active_chain(&self, chain_id: &str, provider: &str, session_id: &str, model: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![chain_id, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![chain_id, provider, session_id],
        )
        .unwrap();
    }

    fn assert_default_user_paths_untouched(&self) {
        assert!(
            !self
                .home_dir
                .join(".local/share/oulipoly-agent-runner")
                .exists()
        );
        assert!(!self.home_dir.join(".config/oulipoly-agent-runner").exists());
    }
}

#[test]
fn notify_live_pty_ack_marks_delivered_and_skips_headless_wake() {
    let fixture = Fixture::new();
    let identity = current_identity();
    fixture.record_owner_identity(&identity);
    let socket = fixture.socket_path("ack.sock");
    let captured = Arc::new(Mutex::new(String::new()));
    let server = spawn_control_server(&socket, true, "ok", Arc::clone(&captured));
    fixture.mark_live_pty_runtime(&identity, &socket);

    let output = fixture.run_notify("h-live-ack", caller_chain(&identity));
    server.join().unwrap();

    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["status"], "enqueued");
    assert_eq!(value["pty_delivery"]["status"], "acked");
    assert_eq!(value["pty_delivery"]["delivered_seqs"], json!([1]));
    assert!(value["wake"].is_null());
    let payload = captured.lock().unwrap().clone();
    assert!(payload.contains("[OULIPOLY NOTIFICATIONS]"));
    assert!(payload.contains("handle: h-live-ack"));
    assert!(!payload.contains("log for h-live-ack"));

    let rows = fixture.mailbox().list_mailbox(SESSION_A, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_some());
    assert_eq!(
        rows[0].delivered_by_invocation_uuid.as_deref(),
        Some(LIVE_INVOCATION)
    );
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_live_pty_failure_leaves_pending_and_wake_busy() {
    let fixture = Fixture::new();
    let identity = current_identity();
    fixture.record_owner_identity(&identity);
    let socket = fixture.socket_path("unsafe.sock");
    let captured = Arc::new(Mutex::new(String::new()));
    let server = spawn_control_server(&socket, false, "unsafe_mid_line", Arc::clone(&captured));
    fixture.mark_live_pty_runtime(&identity, &socket);

    let output = fixture.run_notify("h-unsafe", caller_chain(&identity));
    server.join().unwrap();

    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["pty_delivery"]["status"], "unsafe_mid_line");
    assert_eq!(value["wake"]["status"], "busy");
    let rows = fixture.mailbox().list_mailbox(SESSION_A, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_none());
    assert!(captured.lock().unwrap().contains("h-unsafe"));
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn notify_stale_socket_cleans_runtime_and_does_not_report_busy() {
    let fixture = Fixture::new();
    let owner_identity = current_identity();
    fixture.record_owner_identity(&owner_identity);
    let stale_identity = ProcessIdentity {
        os_pid: 9_999_999,
        os_boot_id: "stale-boot".to_string(),
        os_pid_starttime_ticks: 1,
    };
    let stale_socket = fixture.socket_path("stale.sock");
    fs::write(&stale_socket, "stale").unwrap();
    fixture.mark_live_pty_runtime(&stale_identity, &stale_socket);

    let output = fixture.run_notify("h-stale", caller_chain(&owner_identity));

    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["pty_delivery"]["status"], "connect_error");
    assert_ne!(value["wake"]["status"], "busy");
    let runtime = fixture
        .mailbox()
        .session_runtime(SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.run_state, "idle");
    assert!(runtime.pty_control_path.is_none());
    assert!(!stale_socket.exists());
    fixture.assert_default_user_paths_untouched();
}

#[test]
fn fixture_interactive_session_agent_bash_completion_arrives_live() {
    let fixture = Fixture::new();
    let received_log = fixture.dir.path().join("received.log");
    let script = fixture_provider_waiting_for_notification(fixture.dir.path(), &received_log);
    fixture.write_interactive_model("fixture", "fixture-provider", &script);
    fixture.seed_active_chain(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "fixture-provider",
        SESSION_A,
        "fixture",
    );
    let pty = OuterPty::open(30, 100);
    let mut repl = spawn_repl_under_pty(&fixture, &pty, "fixture", SESSION_A);
    let startup = read_until(
        pty.master.as_raw_fd(),
        "READY_FOR_NOTIFY",
        Duration::from_secs(5),
    );
    assert!(
        startup.contains("READY_FOR_NOTIFY"),
        "startup was {startup:?}"
    );
    let invocation_uuid = wait_for_running_invocation(&fixture);
    let child_identity = wait_for_child_identity(&fixture, &invocation_uuid);

    let output = fixture.run_notify("h-e2e-live", caller_chain(&child_identity));
    assert_success(&output);
    let value = stdout_json(&output);
    assert_eq!(value["pty_delivery"]["status"], "acked");
    assert!(value["wake"].is_null());

    let output = read_until(pty.master.as_raw_fd(), "GOT_NOTIFY", Duration::from_secs(5));
    assert!(output.contains("GOT_NOTIFY"), "output was {output:?}");
    assert!(
        repl.try_wait().unwrap().is_none(),
        "interactive process should still be live when notification arrives"
    );
    let received = fs::read_to_string(&received_log).unwrap();
    assert!(received.contains("[OULIPOLY NOTIFICATIONS]"));
    assert!(received.contains("handle: h-e2e-live"));
    assert!(!received.contains("log for h-e2e-live"));
    assert!(repl.wait().unwrap().success());
    let runtime = fixture
        .mailbox()
        .session_runtime(SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.run_state, "idle");
    assert!(runtime.pty_control_path.is_none());
    fixture.assert_default_user_paths_untouched();
}

fn spawn_control_server(
    path: &Path,
    ack: bool,
    message: &'static str,
    captured: Arc<Mutex<String>>,
) -> thread::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let payload = read_inject_payload(&mut stream);
        *captured.lock().unwrap() = payload;
        write_response(&mut stream, ack, message);
    })
}

struct OuterPty {
    master: File,
    slave: File,
}

impl OuterPty {
    fn open(rows: u16, cols: u16) -> Self {
        let mut master_fd = -1;
        let mut slave_fd = -1;
        let winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
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

fn spawn_repl_under_pty(
    fixture: &Fixture,
    pty: &OuterPty,
    model_name: &str,
    session_id: &str,
) -> Child {
    let stdin = pty.slave.try_clone().unwrap();
    let stdout = pty.slave.try_clone().unwrap();
    let stderr = pty.slave.try_clone().unwrap();
    let slave_fd = pty.slave.as_raw_fd();
    let master_fd = pty.master.as_raw_fd();
    let mut cmd = fixture.base_repl_command(model_name, session_id);
    cmd.stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::tcsetpgrp(slave_fd, libc::getpid()) == -1 {
                return Err(io::Error::last_os_error());
            }
            if master_fd > 2 {
                libc::close(master_fd);
            }
            Ok(())
        });
    }
    cmd.spawn().unwrap()
}

fn fixture_provider_waiting_for_notification(dir: &Path, received_log: &Path) -> PathBuf {
    let path = dir.join("fixture-live-provider.sh");
    fs::write(
        &path,
        format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
: > {received}
test -t 0
test -t 1
test -t 2
printf 'READY_FOR_NOTIFY\n'
while IFS= read -r line; do
  printf '%s\n' "$line" >> {received}
  if [ "$line" = "[END OULIPOLY NOTIFICATIONS]" ]; then
    printf 'GOT_NOTIFY\n'
    sleep 1
    exit 0
  fi
done
"#,
            received = shell_single_quote(&path_string(received_log))
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

fn read_until(fd: RawFd, needle: &str, timeout: Duration) -> String {
    let start = Instant::now();
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    while start.elapsed() < timeout {
        if poll_readable(fd, Duration::from_millis(50)).unwrap() {
            let n = read_fd(fd, &mut buffer).unwrap();
            if n == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..n]);
            let rendered = String::from_utf8_lossy(&output);
            if rendered.contains(needle) {
                return rendered.into_owned();
            }
        }
    }
    String::from_utf8_lossy(&output).into_owned()
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
    Ok(rc > 0 && pollfd.revents & libc::POLLIN != 0)
}

fn read_fd(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    let rc = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if rc == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(rc as usize)
}

fn wait_for_running_invocation(fixture: &Fixture) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(db) = MailboxDb::open(&fixture.sidecar_path())
            && let Ok(Some(runtime)) = db.session_runtime(SESSION_A)
            && let Some(invocation_uuid) = runtime.running_invocation_uuid
        {
            return invocation_uuid;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for running invocation");
}

fn wait_for_child_identity(fixture: &Fixture, invocation_uuid: &str) -> ProcessIdentity {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(sidecar) = PidIdentityDb::open(&fixture.sidecar_path()) {
            let rows = sidecar.lookup_by_invocation_uuid(invocation_uuid).unwrap();
            if let Some(row) = rows.first() {
                return row.identity();
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for child identity for {invocation_uuid}");
}

fn read_inject_payload(stream: &mut UnixStream) -> String {
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(&header[..4], b"OPTY");
    assert_eq!(header[4], 1);
    assert_eq!(header[5], 1);
    let length = u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).unwrap();
    String::from_utf8(payload).unwrap()
}

fn write_response(stream: &mut UnixStream, ack: bool, message: &str) {
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(b"OPTY");
    header[4] = 1;
    header[5] = if ack { 0 } else { 1 };
    header[8..12].copy_from_slice(&(message.len() as u32).to_be_bytes());
    stream.write_all(&header).unwrap();
    stream.write_all(message.as_bytes()).unwrap();
}

fn caller_chain(identity: &ProcessIdentity) -> Value {
    json!({
        "caller_chain": [{
            "pid": identity.os_pid,
            "starttime_ticks": identity.os_pid_starttime_ticks,
            "boot_id": identity.os_boot_id,
        }]
    })
}

fn current_identity() -> ProcessIdentity {
    read_live_process_identity(i64::from(std::process::id()))
        .unwrap()
        .unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "failed to parse stdout JSON: {err}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
