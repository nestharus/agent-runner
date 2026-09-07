//! Synthetic pipe drainage and native JSONL paging through the production queue,
//! provider transport/source and observer scheduler/publication. No PTY is opened.
use super::super::outbound_observer::fixtures::ObserverFixture;
use super::*;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};

const BODY: &str = "synthetic delivered message";

struct Delivery {
    fixture: ObserverFixture,
    pane: MonitorPane,
    pending: PendingChildInput,
    line: InputLineState,
    read: File,
    write: File,
}

impl Delivery {
    fn new(mode: &str) -> Self {
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let mut pane = MonitorPane::new();
        pane.outbound.enqueue(BODY.into());
        Self {
            fixture: ObserverFixture::new(mode),
            pane,
            pending: PendingChildInput::new(),
            line: InputLineState::default(),
            read: unsafe { File::from_raw_fd(fds[0]) },
            write: unsafe { File::from_raw_fd(fds[1]) },
        }
    }

    fn pump(&mut self, control: bool) {
        pump_outbound_queue_from_worker(
            &mut self.pane,
            &mut self.pending,
            &mut self.line,
            &OutboundReleaseGate::default(),
            false,
            &self.fixture.worker,
            control,
        );
    }

    fn drain(&mut self, expected: &[u8]) {
        assert_eq!(self.pending.pending_len(), expected.len());
        flush_pending_child_input(self.write.as_raw_fd(), &mut self.pending).unwrap();
        assert!(self.pending.is_empty());
        let mut actual = vec![0; expected.len()];
        self.read.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }

    fn status(&self) -> OutboundStatus {
        self.pane.outbound.status(1).unwrap()
    }

    fn recover(&mut self) {
        self.pump(false);
        assert!(self.pane.observation_stop.is_some());
        self.pane.apply(MonitorCommand::RequestObservationRecovery);
        self.pane.apply(MonitorCommand::ConfirmAction);
        assert!(run_pending_observation_recovery(
            &mut self.pane,
            &self.fixture.worker
        ));
    }

    fn send(&mut self, recover_first: bool) {
        self.pump(false);
        assert!(self.fixture.tick());
        if recover_first {
            self.recover();
            assert_eq!(self.status(), OutboundStatus::Queued);
            assert!(self.pending.is_empty());
            self.fixture.set_mode("native-pages");
            assert!(self.fixture.tick());
        }
        self.pump(false);
        assert_eq!(self.status(), OutboundStatus::Sending);
        assert!(self.pane.outbound.observation_needed());
        // Body and submit have distinct drains. The anchor must remain held
        // across both even when the observer is scheduled between pumps.
        assert!(!self.fixture.tick());
        self.drain(BODY.as_bytes());
        self.pump(false);
        assert_eq!(self.status(), OutboundStatus::Sending);
        assert!(!self.fixture.tick());
        self.drain(b"\r");
        // Append may beat BOTH the Sent-state pump and the next observer read.
        self.fixture.append_native("user", BODY);
        self.pump(false);
        assert_eq!(self.status(), OutboundStatus::Sent);
        assert!(self.pending.is_empty());
        assert_eq!(self.pane.outbound.messages.len(), 1);
    }

    fn assert_paused_delivery(&mut self) {
        let result = self.fixture.worker.latest_result().unwrap();
        let cursor = self.fixture.cursor();
        let calls = self.fixture.calls().len();
        for _ in 0..32 {
            self.pump(true);
            assert!(!self.fixture.tick());
        }
        // An independently delayed UI (no pump at all) has the same bound.
        for _ in 0..32 {
            assert!(!self.fixture.tick());
        }
        assert_eq!(self.fixture.calls().len(), calls);
        assert_eq!(self.fixture.cursor(), cursor);
        assert_eq!(self.fixture.worker.latest_result().unwrap(), result);
        assert_eq!(self.status(), OutboundStatus::Sent);
        assert!(self.pending.is_empty());
    }
}

#[test]
fn single_message_append_before_observer_read_survives_send_drain_and_recovery() {
    for mode in [
        "native-pages",
        "session_turn_paging_paused",
        "session_turn_staging_capacity_exceeded",
    ] {
        let mut d = Delivery::new(mode);
        d.send(mode != "native-pages");
        assert!(d.fixture.tick());
        let calls = d.fixture.calls();
        assert_eq!(calls.last().unwrap()["params"]["after_token"], "0");
        assert_ne!(calls.last().unwrap()["params"]["start_mode"], "tail");
        d.pump(false);
        assert_eq!(d.status(), OutboundStatus::Consumed);
        assert!(d.pending.is_empty());
        assert!(d.pane.outbound.active.is_none());
        assert!(!d.fixture.tick());
    }
}

#[test]
fn incomplete_match_and_empty_completion_wait_for_control_consumer() {
    let mut d = Delivery::new("native-pages");
    d.send(false);
    d.fixture
        .append_native("compacted", "synthetic non-message record");
    assert!(d.fixture.tick());
    d.assert_paused_delivery();
    d.pump(false);
    assert_eq!(d.status(), OutboundStatus::Sent);
    let baseline = d
        .pane
        .outbound
        .message(1)
        .unwrap()
        .baseline
        .as_ref()
        .unwrap();
    assert_eq!(baseline.matching_turns, 1);
    assert!(d.fixture.tick());
    d.assert_paused_delivery();
    d.pump(false);
    assert_eq!(d.status(), OutboundStatus::Consumed);
    assert!(d.pending.is_empty());
}

#[test]
fn duplicate_matches_on_separate_pages_survive_paused_consumer() {
    let mut d = Delivery::new("native-pages");
    d.send(false);
    d.fixture
        .append_native("compacted", "synthetic intervening record");
    d.fixture.append_native("user", BODY);
    for _ in 0..3 {
        assert!(d.fixture.tick());
        d.assert_paused_delivery();
        d.pump(false);
    }
    assert_eq!(d.status(), OutboundStatus::Ambiguous);
    let message = d.pane.outbound.message(1).unwrap();
    assert_eq!(
        message.detail.as_deref(),
        Some("duplicate_matching_user_turns")
    );
    assert_eq!(message.baseline.as_ref().unwrap().matching_turns, 2);
    assert!(d.pending.is_empty());
}

#[test]
fn partial_match_survives_transient_then_fixed_stop_and_authorized_reentry() {
    let mut d = Delivery::new("native-pages");
    d.send(false);
    d.fixture
        .append_native("compacted", "synthetic trailing record");
    assert!(d.fixture.tick());
    d.pump(false);
    let cursor = d.fixture.cursor();
    d.fixture.set_mode("codex_rollout_read_failed");
    assert!(d.fixture.tick());
    d.pump(false);
    assert_eq!(d.fixture.cursor(), cursor);
    assert_eq!(d.status(), OutboundStatus::Sent);
    d.fixture.set_mode("session_turn_paging_paused");
    assert!(d.fixture.tick());
    d.pump(false);
    assert!(!d.fixture.tick());
    assert_eq!(d.fixture.cursor(), cursor);
    d.recover();
    assert_eq!(d.status(), OutboundStatus::Sent);
    assert_eq!(
        d.pane
            .outbound
            .message(1)
            .unwrap()
            .baseline
            .as_ref()
            .unwrap()
            .matching_turns,
        1
    );
    d.fixture.set_mode("native-pages");
    assert!(d.fixture.tick());
    d.pump(false);
    assert_eq!(d.status(), OutboundStatus::Consumed);
    assert!(d.pending.is_empty());
    let calls = d.fixture.calls();
    assert_eq!(
        calls[calls.len() - 2]["params"],
        calls[calls.len() - 1]["params"]
    );
}
