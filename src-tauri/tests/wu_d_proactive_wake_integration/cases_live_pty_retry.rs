//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: live PTY mailbox retry regressions for wake-sweep recovery.

use crate::SESSION;
use crate::fake_cli::notify_command;
use crate::fixtures::Fixture;
use crate::liveness::{wait_for_file, wait_until};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_capture_notify_wake_busy, assert_no_wake_claim, assert_pending_handle_without_error,
    assert_pending_mailbox_count, assert_success, assert_xdg_isolated,
};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn live_pty_nack_pending_is_retried_by_sweep() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let control = spawn_control_socket(
        &fixture,
        "live-pty-retry.sock",
        vec![control_nack("unsafe_mid_line"), control_ack("ok")],
    );
    let identity = fixture.seed_live_pty_runtime(&control.path);
    fixture.record_identity(&identity);

    let output = notify_command(&fixture, "h-live-pty-retry", &identity)
        .output()
        .unwrap();
    assert_success(&output);
    let notify = notify_response(&output);
    assert_notify_pty_status(&notify, "unsafe_mid_line");
    assert_notify_pty_submitted(&notify, false);
    assert_capture_notify_wake_busy(&notify);
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_no_wake_claim(&fixture, SESSION);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    wait_until("two PTY control requests", || control.accepted_count() == 2);
    for _ in 0..2 {
        let output = fixture.run_mailbox_list(SESSION);
        assert_success(&output);
    }
    std::thread::sleep(Duration::from_millis(300));

    let payloads = control.payloads();
    assert_eq!(payloads.len(), 2, "control payloads: {payloads:?}");
    assert_payload_contains_handle(&payloads[0], "h-live-pty-retry");
    assert_payload_contains_handle(&payloads[1], "h-live-pty-retry");
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(rows[0].delivered_at.is_none(), "mailbox rows: {rows:?}");
    assert_eq!(rows[0].delivery_attempts, 0, "mailbox rows: {rows:?}");
    let accepted = fixture
        .mailbox()
        .accepted_delivery_attempt_windows(SESSION)
        .unwrap();
    assert_eq!(accepted.len(), 1, "accepted windows: {accepted:?}");
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn live_pty_acked_pending_is_submitted_once_across_repeated_sweeps() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let control = spawn_control_socket(
        &fixture,
        "live-pty-submit-once.sock",
        vec![control_ack("ok")],
    );
    let identity = fixture.seed_live_pty_runtime(&control.path);
    fixture.record_identity(&identity);

    let output = notify_command(&fixture, "h-live-pty-submit-once", &identity)
        .output()
        .unwrap();
    assert_success(&output);
    let notify = notify_response(&output);
    assert_notify_pty_status(&notify, "acked");
    assert_notify_pty_submitted(&notify, true);
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_no_wake_claim(&fixture, SESSION);

    for _ in 0..3 {
        let output = fixture.run_mailbox_list(SESSION);
        assert_success(&output);
    }

    assert_eq!(control.accepted_count(), 1);
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 1, "mailbox rows: {rows:?}");
    assert!(rows[0].delivered_at.is_none(), "mailbox rows: {rows:?}");
    assert_eq!(rows[0].delivery_attempts, 0, "mailbox rows: {rows:?}");
    let accepted = fixture
        .mailbox()
        .accepted_delivery_attempt_windows(SESSION)
        .unwrap();
    assert_eq!(accepted.len(), 1, "accepted windows: {accepted:?}");
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn live_pty_repeated_nack_keeps_pending_without_claim() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let control = spawn_control_socket(
        &fixture,
        "live-pty-repeat-nack.sock",
        vec![
            control_nack("unsafe_mid_line"),
            control_nack("unsafe_mid_line"),
        ],
    );
    let identity = fixture.seed_live_pty_runtime(&control.path);
    fixture.record_identity(&identity);

    let output = notify_command(&fixture, "h-live-pty-repeat-nack", &identity)
        .output()
        .unwrap();
    assert_success(&output);
    let notify = notify_response(&output);
    assert_notify_pty_status(&notify, "unsafe_mid_line");
    assert_notify_pty_submitted(&notify, false);
    assert_capture_notify_wake_busy(&notify);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    wait_until("second PTY control nack", || control.accepted_count() == 2);

    assert_pending_handle_without_error(&fixture, SESSION, "h-live-pty-repeat-nack");
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn foreground_owner_retries_live_pty_without_second_command_and_stops() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(
        r#"printf 'started\n' > "$WU_D_WORK_DIR/live-pty-owner-started"
sleep 10"#,
    );
    let control = spawn_control_socket(
        &fixture,
        "live-pty-owner-driver.sock",
        vec![
            control_nack("unsafe_mid_line"),
            control_ack("ok"),
            control_ack("leaked"),
        ],
    );
    let identity = fixture.seed_live_pty_runtime(&control.path);
    fixture.record_identity(&identity);

    let mut owner = fixture.spawn_agent("owner stays alive");
    wait_for_file(&fixture.work_dir.join("live-pty-owner-started"));

    let output = notify_command(&fixture, "h-live-pty-owner-driver", &identity)
        .output()
        .unwrap();
    assert_success(&output);
    let notify = notify_response(&output);
    assert_notify_pty_status(&notify, "unsafe_mid_line");
    assert_notify_pty_submitted(&notify, false);
    assert_capture_notify_wake_busy(&notify);

    wait_until("notify plus owner retry reached PTY control", || {
        control.accepted_count() == 2
    });
    assert_pending_mailbox_count(&fixture, SESSION, 1);

    stop_owner(&mut owner);
    fixture.seed_mailbox(SESSION, "h-after-owner-exit");
    std::thread::sleep(Duration::from_millis(3_500));

    assert_eq!(control.accepted_count(), 2);
    assert_pending_mailbox_count(&fixture, SESSION, 2);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn auto_wake_owner_does_not_host_live_pty_retry_driver() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(
        r#"printf 'started\n' > "$WU_D_WORK_DIR/auto-wake-owner-started"
sleep 4"#,
    );
    let control = spawn_control_socket(
        &fixture,
        "live-pty-auto-wake-no-driver.sock",
        vec![control_ack("unexpected")],
    );
    let identity = fixture.seed_live_pty_runtime(&control.path);
    fixture.record_identity(&identity);
    fixture.seed_mailbox(SESSION, "h-auto-wake-no-driver");

    let mut owner = fixture.spawn_agent_as_auto_wake("auto wake child");
    wait_for_file(&fixture.work_dir.join("auto-wake-owner-started"));
    std::thread::sleep(Duration::from_millis(3_500));
    stop_owner(&mut owner);

    assert_eq!(control.accepted_count(), 0);
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

struct ControlSocketScript {
    path: PathBuf,
    accepted: Arc<AtomicUsize>,
    payloads: Arc<Mutex<Vec<String>>>,
}

impl ControlSocketScript {
    fn accepted_count(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn payloads(&self) -> Vec<String> {
        self.payloads.lock().unwrap().clone()
    }
}

#[derive(Clone)]
struct ControlResponse {
    ack: bool,
    message: String,
}

fn spawn_control_socket(
    fixture: &Fixture,
    name: &str,
    responses: Vec<ControlResponse>,
) -> ControlSocketScript {
    let path = fixture.work_dir.join(name);
    let listener = UnixListener::bind(&path).unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let payloads = Arc::new(Mutex::new(Vec::new()));
    std::thread::spawn(control_socket_thread(
        listener,
        responses,
        Arc::clone(&accepted),
        Arc::clone(&payloads),
    ));
    ControlSocketScript {
        path,
        accepted,
        payloads,
    }
}

fn control_socket_thread(
    listener: UnixListener,
    responses: Vec<ControlResponse>,
    accepted: Arc<AtomicUsize>,
    payloads: Arc<Mutex<Vec<String>>>,
) -> impl FnOnce() + Send + 'static {
    move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let payload = read_control_payload(&mut stream);
            payloads.lock().unwrap().push(payload);
            write_control_response(&mut stream, &response);
            accepted.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn read_control_payload(stream: &mut UnixStream) -> String {
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

fn write_control_response(stream: &mut UnixStream, response: &ControlResponse) {
    let bytes = response.message.as_bytes();
    let mut header = [0_u8; 12];
    header[..4].copy_from_slice(b"OPTY");
    header[4] = 1;
    header[5] = if response.ack { 0 } else { 1 };
    header[8..12].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
    stream.write_all(&header).unwrap();
    stream.write_all(bytes).unwrap();
}

fn control_ack(message: &str) -> ControlResponse {
    ControlResponse {
        ack: true,
        message: message.to_string(),
    }
}

fn control_nack(message: &str) -> ControlResponse {
    ControlResponse {
        ack: false,
        message: message.to_string(),
    }
}

fn stop_owner(owner: &mut Child) {
    let _ = owner.kill();
    let _ = owner.wait();
}

fn notify_response(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_notify_pty_status(notify: &Value, status: &str) {
    assert_eq!(
        notify
            .get("pty_delivery")
            .and_then(|delivery| delivery.get("status"))
            .and_then(Value::as_str),
        Some(status),
        "notify response: {notify}"
    );
}

fn assert_notify_pty_submitted(notify: &Value, submitted: bool) {
    assert_eq!(
        notify
            .get("pty_delivery")
            .and_then(|delivery| delivery.get("submitted"))
            .and_then(Value::as_bool),
        Some(submitted),
        "notify response: {notify}"
    );
}

fn assert_payload_contains_handle(payload: &str, handle: &str) {
    assert!(payload.contains(&format!("handle: {handle}")), "{payload}");
}
