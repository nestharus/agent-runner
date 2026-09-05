//! Control sockets and mailbox transactions run off the terminal input thread.
//! Only the relay writes the child PTY. Its submission state machine preserves the
//! paste/Enter boundary while continuing to route mouse events and render output.

use super::super::{
    CONTROL_IO_TIMEOUT, ForegroundOwnerState, INJECT_WAIT_LIMIT, PreparedControlPayload,
};
use super::{
    CONTROL_SUBMIT_DELAY, ChildOutputState, ControlSocket, InputLineState, PendingChildInput,
    Profile, TerminalParser, lock_or_recover, validate_control_input_ready,
};
use crate::executor::cli::pty_broker as broker;
use std::net::Shutdown;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Instant;

enum Event {
    Prepared,
    Started(Vec<u8>),
    Finished,
}

enum Command {
    Begin(Result<(), String>),
    Complete(Result<(), String>),
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Prepared,
    Beginning,
    Body(Instant),
    Delay(Instant),
    Delimiter(Instant),
    Settling,
}

#[derive(Clone)]
struct TraceSnapshot {
    foreground: ForegroundOwnerState,
    input: InputLineState,
    output: ChildOutputState,
}

struct Shared {
    shutdown: AtomicBool,
    // A duplicate lets shutdown interrupt a peer stalled halfway through a frame.
    peer: Mutex<Option<UnixStream>>,
    trace: Mutex<TraceSnapshot>,
    session_id: Arc<Mutex<Option<String>>>,
    invocation_uuid: String,
    trace_path: Option<PathBuf>,
    profile: Profile,
}

impl Shared {
    fn cancelled(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    fn stop(&self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(peer) = lock_or_recover(&self.peer).as_ref() {
            let _ = peer.shutdown(Shutdown::Both);
        }
    }

    fn trace_response(&self, ack: bool, message: &str) {
        let snapshot = lock_or_recover(&self.trace).clone();
        let session_id = lock_or_recover(&self.session_id).clone();
        let decision = if ack { "inject" } else { "skip" };
        let line = snapshot.input.trace_snapshot_for_decision(decision);
        let record = broker::notify_gate_trace_record(
            session_id.as_deref(),
            &self.invocation_uuid,
            snapshot.foreground,
            &line,
            &snapshot.output,
            decision,
            message,
        );
        if let Some(path) = self.trace_path.as_deref() {
            broker::append_notify_trace_record_at(path, &record);
        }
        if broker::trace_notify_enabled() {
            eprintln!("oulipoly_notify_trace {record}");
        }
    }
}

pub(super) struct ControlWorker {
    shared: Arc<Shared>,
    events: mpsc::Receiver<Event>,
    commands: mpsc::Sender<Command>,
    join: Option<JoinHandle<()>>,
    phase: Phase,
    control_started_at: Instant,
}

pub(super) struct ControlProgressIo<'a> {
    pub(super) master_fd: RawFd,
    pub(super) child_pid: Option<u32>,
    pub(super) parser: &'a TerminalParser,
    pub(super) input: &'a mut InputLineState,
    pub(super) output: &'a ChildOutputState,
    pub(super) pending: &'a mut PendingChildInput,
    pub(super) outbound_active: bool,
}

impl ControlWorker {
    pub(super) fn start(control: &ControlSocket, profile: Profile) -> Result<Self, String> {
        Self::start_with_trace_path(control, broker::notify_trace_path(), profile)
    }

    fn start_with_trace_path(
        control: &ControlSocket,
        trace_path: Option<PathBuf>,
        profile: Profile,
    ) -> Result<Self, String> {
        let listener = control
            .listener
            .try_clone()
            .map_err(|error| format!("Failed to clone PTY control listener: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Failed to configure PTY control listener: {error}"))?;
        let shared = Arc::new(Shared {
            shutdown: AtomicBool::new(false),
            peer: Mutex::new(None),
            trace: Mutex::new(TraceSnapshot {
                foreground: ForegroundOwnerState::Unknown,
                input: InputLineState::default(),
                output: ChildOutputState::default(),
            }),
            session_id: Arc::clone(&control.session_id),
            invocation_uuid: control.invocation_uuid.clone(),
            trace_path,
            profile,
        });
        let (events_tx, events) = mpsc::channel();
        let (commands, commands_rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let join = thread::Builder::new()
            .name("pty-broker-control".to_string())
            .spawn(move || run_worker(listener, worker_shared, events_tx, commands_rx))
            .map_err(|error| format!("Failed to start PTY control worker: {error}"))?;
        Ok(Self {
            shared,
            events,
            commands,
            join: Some(join),
            phase: Phase::Idle,
            control_started_at: control.child_started_at,
        })
    }

    /// Socket parsing and database preparation never reserve ordinary keyboard input.
    pub(super) fn owns_child_input(&self) -> bool {
        matches!(
            self.phase,
            Phase::Body(_) | Phase::Delay(_) | Phase::Delimiter(_)
        )
    }

    /// An outbound pseudo-editor paste must not race a granted control submission.
    pub(super) fn blocks_outbound(&self) -> bool {
        self.phase == Phase::Beginning || self.owns_child_input()
    }

    pub(super) fn progress(&mut self, io: &mut ControlProgressIo<'_>, now: Instant) {
        *lock_or_recover(&self.shared.trace) = TraceSnapshot {
            foreground: broker::foreground_owner_state(io.master_fd, io.child_pid),
            input: io.input.clone(),
            output: io.output.clone(),
        };
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Prepared => self.phase = Phase::Prepared,
                Event::Started(bytes) => {
                    broker::queue_control_injection(
                        io.pending,
                        &bytes,
                        io.parser.screen().bracketed_paste(),
                        false,
                    );
                    self.phase = Phase::Body(now + INJECT_WAIT_LIMIT);
                    if let Err(error) = broker::pty_delivery_test_fault("tui_body_drain") {
                        self.finish(Err(error));
                    }
                }
                Event::Finished => self.phase = Phase::Idle,
            }
        }
        match self.phase {
            Phase::Prepared if !io.outbound_active && io.pending.is_empty() => {
                let ready = validate_control_input_ready(
                    io.parser.screen().bracketed_paste(),
                    io.parser.screen().alternate_screen(),
                    now.saturating_duration_since(self.control_started_at),
                )
                .and_then(|()| broker::pty_delivery_test_fault("tui_pre_submission"));
                self.phase = if ready.is_ok() {
                    Phase::Beginning
                } else {
                    Phase::Settling
                };
                let _ = self.commands.send(Command::Begin(ready));
            }
            Phase::Body(_) if io.pending.is_empty() => {
                self.phase = Phase::Delay(now + CONTROL_SUBMIT_DELAY);
            }
            Phase::Delay(until) if now >= until => {
                io.pending.enqueue(b"\r");
                io.input.mark_submitted();
                self.phase = Phase::Delimiter(now + INJECT_WAIT_LIMIT);
                if let Err(error) = broker::pty_delivery_test_fault("tui_delimiter_drain") {
                    self.finish(Err(error));
                }
            }
            Phase::Delimiter(_) if io.pending.is_empty() => self.finish(Ok(())),
            Phase::Body(deadline) if now >= deadline => {
                self.finish(Err("control_submit_body_drain_timeout".to_string()));
            }
            Phase::Delimiter(deadline) if now >= deadline => {
                self.finish(Err("control_submit_delimiter_drain_timeout".to_string()));
            }
            _ => {}
        }
    }

    fn finish(&mut self, result: Result<(), String>) {
        self.phase = Phase::Settling;
        let _ = self.commands.send(Command::Complete(result));
    }
}

impl Drop for ControlWorker {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(join) = self.join.take() {
            // No socket writer or mailbox transaction outlives the child custody.
            let _ = join.join();
        }
    }
}

fn run_worker(
    listener: UnixListener,
    shared: Arc<Shared>,
    events: mpsc::Sender<Event>,
    commands: mpsc::Receiver<Command>,
) {
    while !shared.cancelled() {
        let mut descriptor = broker::poll_read_fd(listener.as_raw_fd());
        if broker::poll_fds(std::slice::from_mut(&mut descriptor), |error| {
            error.to_string()
        })
        .is_err()
        {
            return;
        }
        if !broker::readable(descriptor.revents) || shared.cancelled() {
            continue;
        }
        let Ok((mut peer, _)) = listener.accept() else {
            continue;
        };
        let response = configure_peer(&peer).and_then(|duplicate| {
            *lock_or_recover(&shared.peer) = Some(duplicate);
            if shared.cancelled() {
                return Err("control_worker_stopped".to_string());
            }
            process_peer(&mut peer, &shared, &events, &commands)
        });
        let (ack, message) = broker::control_response_parts(response);
        shared.trace_response(ack, &message);
        let _ = broker::write_control_response(&mut peer, ack, &message);
        *lock_or_recover(&shared.peer) = None;
        if events.send(Event::Finished).is_err() {
            return;
        }
    }
}

fn configure_peer(peer: &UnixStream) -> Result<UnixStream, String> {
    peer.set_read_timeout(Some(CONTROL_IO_TIMEOUT))
        .and_then(|()| peer.set_write_timeout(Some(CONTROL_IO_TIMEOUT)))
        .and_then(|()| peer.try_clone())
        .map_err(|error| format!("Failed to configure PTY control peer: {error}"))
}

fn process_peer(
    peer: &mut UnixStream,
    shared: &Shared,
    events: &mpsc::Sender<Event>,
    commands: &mpsc::Receiver<Command>,
) -> Result<broker::ControlPayloadOutcome, String> {
    broker::validate_peer_uid(peer)?;
    let session_id = lock_or_recover(&shared.session_id)
        .clone()
        .ok_or_else(|| "awaiting_session_identity".to_string())?;
    let bytes = {
        let _timing = shared.profile.measure("control.read");
        broker::read_control_request(peer)?
    };
    let mut payload = {
        let _timing = shared.profile.measure("control.prepare");
        broker::prepare_control_payload(bytes, Some((&session_id, &shared.invocation_uuid)))?
    };
    if payload.bytes.is_empty() {
        let _timing = shared.profile.measure("control.settle");
        return broker::settle_control_payload(&payload);
    }
    events
        .send(Event::Prepared)
        .map_err(|_| "control_relay_stopped".to_string())?;
    match next_command(shared, commands)? {
        Command::Begin(ready) => ready?,
        Command::Complete(_) => return Err("control_submission_not_started".to_string()),
    }
    {
        let _timing = shared.profile.measure("control.begin");
        broker::begin_control_payload_submission(&mut payload)?;
    }
    let submission = run_submission(&mut payload, shared, events, commands);
    match submission {
        Ok(()) => {
            let _timing = shared.profile.measure("control.settle");
            broker::settle_control_payload(&payload)
        }
        Err(_) if payload.submission_started => Ok(broker::submission_uncertain_outcome(&payload)),
        Err(error) => Err(error),
    }
}

fn run_submission(
    payload: &mut PreparedControlPayload,
    shared: &Shared,
    events: &mpsc::Sender<Event>,
    commands: &mpsc::Receiver<Command>,
) -> Result<(), String> {
    if shared.cancelled() {
        return Err("control_worker_stopped".to_string());
    }
    events
        .send(Event::Started(std::mem::take(&mut payload.bytes)))
        .map_err(|_| "control_relay_stopped".to_string())?;
    match next_command(shared, commands)? {
        Command::Complete(result) => result,
        Command::Begin(_) => Err("control_submission_already_started".to_string()),
    }
}

fn next_command(shared: &Shared, commands: &mpsc::Receiver<Command>) -> Result<Command, String> {
    loop {
        if shared.cancelled() {
            return Err("control_worker_stopped".to_string());
        }
        match commands.recv_timeout(std::time::Duration::from_millis(25)) {
            Ok(command) => return Ok(command),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("control_relay_stopped".to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{DataDirOverride, available_observation};
    use super::super::{
        InputRouter, MonitorPane, MouseRequest, OutboundReleaseGate, enqueue_routed_child_input,
        pump_outbound_queue_with_gate, release_deferred_child_input, route_real_input,
    };
    use super::*;
    use oulipoly_state::mailbox::MailboxDb;
    use std::io::{Read, Write};
    use std::time::Duration;

    struct Harness {
        worker: ControlWorker,
        parser: TerminalParser,
        input: InputLineState,
        output: ChildOutputState,
        pending: PendingChildInput,
        child_writer: UnixStream,
        child_reader: UnixStream,
        control: ControlSocket,
        _directory: tempfile::TempDir,
    }

    impl Harness {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("control.sock");
            let control = ControlSocket {
                listener: UnixListener::bind(&path).unwrap(),
                path,
                owned_dir: directory.path().to_path_buf(),
                session_id: Arc::new(Mutex::new(Some("session-a".to_string()))),
                invocation_uuid: "invocation-a".to_string(),
                child_started_at: Instant::now() - Duration::from_secs(20),
            };
            let worker = ControlWorker::start_with_trace_path(
                &control,
                Some(directory.path().join("trace.log")),
                Profile::default(),
            )
            .unwrap();
            let (child_writer, child_reader) = UnixStream::pair().unwrap();
            child_reader.set_nonblocking(true).unwrap();
            Self {
                worker,
                parser: TerminalParser::new(24, 80, 100),
                input: InputLineState::default(),
                output: ChildOutputState::default(),
                pending: PendingChildInput::new(),
                child_writer,
                child_reader,
                control,
                _directory: directory,
            }
        }

        fn step(&mut self, now: Instant) {
            self.worker.progress(
                &mut ControlProgressIo {
                    master_fd: self.child_writer.as_raw_fd(),
                    child_pid: None,
                    parser: &self.parser,
                    input: &mut self.input,
                    output: &self.output,
                    pending: &mut self.pending,
                    outbound_active: false,
                },
                now,
            );
        }

        fn until(&mut self, predicate: impl Fn(&Self) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !predicate(self) {
                assert!(
                    Instant::now() < deadline,
                    "control phase {:?}",
                    self.worker.phase
                );
                self.step(Instant::now());
                thread::sleep(Duration::from_millis(1));
            }
        }

        fn request(&self, body: &[u8]) -> UnixStream {
            let mut peer = UnixStream::connect(&self.control.path).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            broker::write_control_frame(&mut peer, broker::CONTROL_OP_INJECT, body).unwrap();
            peer
        }

        fn flush(&mut self) -> Vec<u8> {
            broker::flush_pending_child_input(self.child_writer.as_raw_fd(), &mut self.pending)
                .unwrap();
            let mut received = vec![0; 128 * 1024];
            let count = match self.child_reader.read(&mut received) {
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
                Err(error) => panic!("child read: {error}"),
            };
            received.truncate(count);
            received
        }

        fn route_key_and_wheel(&mut self, deferred: &mut PendingChildInput) -> i32 {
            let mut router = InputRouter::new();
            let pane = MonitorPane::new();
            let winsize = libc::winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let routed = route_real_input(
                b"a\x1b[<64;4;3M",
                &mut router,
                &pane,
                MouseRequest::disabled(),
                &winsize,
            );
            enqueue_routed_child_input(
                &mut self.input,
                &mut self.pending,
                deferred,
                &mut OutboundReleaseGate::default(),
                self.worker.owns_child_input(),
                &routed,
            );
            routed.top_scroll_lines
        }
    }

    #[test]
    fn partial_control_frame_leaves_input_and_wheel_responsive_and_shutdown_interrupts_read() {
        let mut harness = Harness::new();
        let mut peer = UnixStream::connect(&harness.control.path).unwrap();
        peer.write_all(b"OPT").unwrap();
        harness.until(|h| lock_or_recover(&h.worker.shared.peer).is_some());
        let started = Instant::now();
        let mut deferred = PendingChildInput::new();
        for _ in 0..20 {
            harness.step(Instant::now());
            assert_eq!(
                harness.route_key_and_wheel(&mut deferred),
                super::super::TOP_SCROLL_STEP
            );
            assert_eq!(harness.flush(), b"a");
        }
        assert!(
            deferred.is_empty(),
            "an incomplete peer must not reserve input"
        );
        assert!(started.elapsed() < Duration::from_millis(250));
        let shutdown = Instant::now();
        drop(harness);
        assert!(
            shutdown.elapsed() < Duration::from_millis(500),
            "shutdown must interrupt the incomplete frame"
        );
    }

    #[test]
    fn control_body_delay_and_backpressure_preserve_wheel_routing_and_keyboard_fifo() {
        let mut harness = Harness::new();
        harness.parser.process(b"\x1b[?2004h");
        let mut peer = harness.request(b"body");
        harness.until(|h| matches!(h.worker.phase, Phase::Body(_)));
        let mut pane = MonitorPane::new();
        pane.outbound.enqueue("later pseudo prompt".to_string());
        let started = Instant::now();
        let mut deferred = PendingChildInput::new();
        // Simulate a child that has not drained its body yet: wheel input must still
        // route, but keyboard bytes must never enter the middle of the paste.
        for _ in 0..20 {
            harness.step(Instant::now());
            assert_eq!(
                harness.route_key_and_wheel(&mut deferred),
                super::super::TOP_SCROLL_STEP
            );
        }
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(harness.flush(), b"\x1b[200~body\x1b[201~");
        harness.step(Instant::now());
        let Phase::Delay(until) = harness.worker.phase else {
            panic!("expected submit delay")
        };
        assert_eq!(
            harness.route_key_and_wheel(&mut deferred),
            super::super::TOP_SCROLL_STEP
        );
        harness.step(until - Duration::from_millis(1));
        assert!(
            harness.pending.is_empty(),
            "Enter cannot race the body commit"
        );
        harness.step(until);
        assert_eq!(harness.flush(), b"\r");
        harness.step(until);
        let response = broker::read_control_response(&mut peer).unwrap();
        assert!(response.ack, "{}", response.message);
        let observation = available_observation(1, []);
        // Counterfactual: with the same ready provider observation, pumping before
        // releasing deferred keys would start the later prompt and overtake them.
        let mut before_release_pane = pane.clone();
        let mut before_release_input = harness.input.clone();
        let mut before_release_pending = PendingChildInput::new();
        assert!(pump_outbound_queue_with_gate(
            &mut before_release_pane,
            &mut before_release_pending,
            &mut before_release_input,
            &OutboundReleaseGate::default(),
            true,
            Some(&observation),
            Instant::now(),
        ));
        assert!(before_release_pane.outbound.active.is_some());
        assert_eq!(
            before_release_pending.take_pending(),
            super::super::control_payload_bytes(b"later pseudo prompt", true),
        );
        release_deferred_child_input(
            harness.worker.owns_child_input(),
            &mut harness.input,
            &mut harness.pending,
            &mut deferred,
            &mut OutboundReleaseGate::default(),
        );
        pump_outbound_queue_with_gate(
            &mut pane,
            &mut harness.pending,
            &mut harness.input,
            &OutboundReleaseGate::default(),
            true,
            Some(&observation),
            Instant::now(),
        );
        assert!(
            pane.outbound.active.is_none(),
            "a later pseudo prompt cannot overtake deferred keys"
        );
        assert_eq!(harness.flush(), vec![b'a'; 21]);
    }

    #[test]
    fn control_drain_timeout_never_reports_delivery_ack() {
        let mut harness = Harness::new();
        let mut peer = harness.request(b"body");
        harness.until(|h| matches!(h.worker.phase, Phase::Body(_)));
        let Phase::Body(deadline) = harness.worker.phase else {
            unreachable!()
        };
        harness.step(deadline);
        let response = broker::read_control_response(&mut peer).unwrap();
        assert!(!response.ack);
        assert_eq!(response.message, "control_submit_body_drain_timeout");
        assert_eq!(
            harness.flush(),
            b"body",
            "timeout must not append Enter or retry the body"
        );
    }

    #[test]
    fn mailbox_control_acknowledges_only_after_delimiter_drain() {
        let _env_lock = crate::test_support::lock_env();
        let directory = tempfile::tempdir().unwrap();
        let _data_dir = DataDirOverride::install(directory.path());
        let attempt = "async-control-delimiter-ack";
        let path = broker::seed_test_mailbox_delivery(directory.path(), attempt);
        let mut harness = Harness::new();
        let mut peer = harness.request(format!("notify\n[OULIPOLY-DELIVERY {attempt}]").as_bytes());
        harness.until(|h| matches!(h.worker.phase, Phase::Body(_)));
        harness.flush();
        harness.step(Instant::now());
        let Phase::Delay(until) = harness.worker.phase else {
            panic!("expected delay")
        };
        harness.step(until);
        let db = MailboxDb::open(&path).unwrap();
        assert!(
            db.delivery_attempt_window(attempt)
                .unwrap()
                .unwrap()
                .acknowledged_at
                .is_none()
        );
        assert_eq!(harness.flush(), b"\r");
        harness.step(until);
        let response = broker::read_control_response(&mut peer).unwrap();
        assert!(response.ack);
        assert_eq!(response.message, broker::pty_delivery_ack_message(attempt));
        assert!(
            db.delivery_attempt_window(attempt)
                .unwrap()
                .unwrap()
                .acknowledged_at
                .is_some()
        );
    }

    #[test]
    fn mailbox_control_confirms_only_after_enter_drains_and_confirmation_failure_stays_uncertain() {
        let _env_lock = crate::test_support::lock_env();
        let directory = tempfile::tempdir().unwrap();
        let _data_dir = DataDirOverride::install(directory.path());
        let attempt = "async-control-confirmation-failure";
        let path = broker::seed_test_mailbox_delivery(directory.path(), attempt);
        let failure = rusqlite::Connection::open(&path).unwrap();
        failure.execute_batch(
            "CREATE TRIGGER fail_confirmation BEFORE UPDATE OF acknowledged_at ON mailbox_delivery_attempts
             BEGIN SELECT RAISE(FAIL, 'test confirmation failure'); END;",
        ).unwrap();
        let mut harness = Harness::new();
        let envelope = format!("notify\n[OULIPOLY-DELIVERY {attempt}]");
        let mut peer = harness.request(envelope.as_bytes());
        harness.until(|h| matches!(h.worker.phase, Phase::Body(_)));
        let db = MailboxDb::open(&path).unwrap();
        let window = db.delivery_attempt_window(attempt).unwrap().unwrap();
        assert!(window.submission_started_at.is_some());
        assert!(window.acknowledged_at.is_none());
        let body = harness.flush();
        assert!(String::from_utf8_lossy(&body).contains(attempt));
        harness.step(Instant::now());
        let Phase::Delay(until) = harness.worker.phase else {
            panic!("expected delay")
        };
        harness.step(until);
        assert!(
            db.delivery_attempt_window(attempt)
                .unwrap()
                .unwrap()
                .acknowledged_at
                .is_none()
        );
        assert_eq!(harness.flush(), b"\r");
        harness.step(until);
        let response = broker::read_control_response(&mut peer).unwrap();
        assert!(response.ack);
        assert_eq!(
            response.message,
            broker::pty_delivery_uncertain_message(attempt)
        );
        let window = db.delivery_attempt_window(attempt).unwrap().unwrap();
        assert!(window.acknowledged_at.is_none());
        assert!(window.resolved_at.is_none());
        assert_eq!(window.rows.len(), 1);
        failure
            .execute_batch("DROP TRIGGER fail_confirmation")
            .unwrap();
        let mut retry = harness.request(envelope.as_bytes());
        let response = broker::read_control_response(&mut retry).unwrap();
        assert_eq!(
            response.message,
            broker::pty_delivery_uncertain_message(attempt)
        );
        assert!(
            harness.flush().is_empty(),
            "uncertain delivery must never submit twice"
        );
    }

    #[test]
    fn mailbox_control_body_timeout_reports_uncertainty_without_retrying() {
        let _env_lock = crate::test_support::lock_env();
        let directory = tempfile::tempdir().unwrap();
        let _data_dir = DataDirOverride::install(directory.path());
        let attempt = "async-control-body-timeout";
        let path = broker::seed_test_mailbox_delivery(directory.path(), attempt);
        let mut harness = Harness::new();
        let envelope = format!("notify\n[OULIPOLY-DELIVERY {attempt}]");
        let mut peer = harness.request(envelope.as_bytes());
        harness.until(|h| matches!(h.worker.phase, Phase::Body(_)));
        let Phase::Body(deadline) = harness.worker.phase else {
            unreachable!()
        };
        harness.step(deadline);
        let response = broker::read_control_response(&mut peer).unwrap();
        assert!(response.ack);
        assert_eq!(
            response.message,
            broker::pty_delivery_uncertain_message(attempt)
        );
        let db = MailboxDb::open(&path).unwrap();
        let window = db.delivery_attempt_window(attempt).unwrap().unwrap();
        assert!(window.submission_started_at.is_some());
        assert!(window.acknowledged_at.is_none());
        let original = harness.flush();
        assert!(!original.ends_with(b"\r"));
        let mut retry = harness.request(envelope.as_bytes());
        assert_eq!(
            broker::read_control_response(&mut retry).unwrap().message,
            broker::pty_delivery_uncertain_message(attempt)
        );
        assert!(harness.flush().is_empty());
    }

    #[test]
    fn locked_mailbox_begin_does_not_reserve_keyboard_or_wheel_input() {
        let _env_lock = crate::test_support::lock_env();
        let directory = tempfile::tempdir().unwrap();
        let _data_dir = DataDirOverride::install(directory.path());
        let attempt = "async-control-busy-database";
        let path = broker::seed_test_mailbox_delivery(directory.path(), attempt);
        let mut harness = Harness::new();
        let _peer = harness.request(format!("notify\n[OULIPOLY-DELIVERY {attempt}]").as_bytes());
        assert!(matches!(
            harness
                .worker
                .events
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            Event::Prepared
        ));
        harness.worker.phase = Phase::Prepared;
        let blocker = rusqlite::Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        harness.step(Instant::now());
        assert_eq!(harness.worker.phase, Phase::Beginning);
        let started = Instant::now();
        let mut deferred = PendingChildInput::new();
        for _ in 0..20 {
            harness.step(Instant::now());
            assert_eq!(
                harness.route_key_and_wheel(&mut deferred),
                super::super::TOP_SCROLL_STEP
            );
            assert_eq!(harness.flush(), b"a");
        }
        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(deferred.is_empty());
        assert_eq!(harness.worker.phase, Phase::Beginning);
        blocker.execute_batch("ROLLBACK").unwrap();
        harness.until(|h| matches!(h.worker.phase, Phase::Body(_)));
    }
}
