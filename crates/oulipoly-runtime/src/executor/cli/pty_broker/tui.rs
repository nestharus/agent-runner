//! ## Declared roles
//!
//! Roles: orchestration, mapper, formatter, predicate, validator, accessor,
//! parser, filter.
//!
//! - orchestration: [`relay_until_exit_observed`] runs the split-pane event
//!   loop over named IO, sizing, refresh, control-injection, and render helpers;
//!   [`apply_routed_to_pane`]/[`MonitorPane::apply`] apply decoded monitor
//!   commands.
//! - mapper: [`top_pane_winsize`]/[`child_winsize`] map the real terminal size
//!   to the child PTY size; [`vt_color`]/[`vt_modifier`] map a `vt100` cell to
//!   `ratatui` style; [`render_screen_snapshot`] maps the virtual screen grid into the
//!   top-pane buffer; [`status_word`]/[`status_color`]/[`kind_word`]/
//!   [`liveness_glyph`] map snapshot enums to display tokens.
//! - formatter: [`render_frame`]/[`render_monitor`]/[`render_status_row`]/
//!   [`render_node_list`]/[`render_node_row`]/[`pad_to_width`] format the split
//!   layout (top virtual terminal + collapsible monitor pane).
//! - predicate: [`dimensions_sufficient`], sizing-change helpers, and injection
//!   safety helpers answer boolean readiness/safety questions.
//! - validator: input/control helpers classify terminal/control bytes before the
//!   orchestration helpers decide where they are applied.
//! - accessor: terminal, PTY, control socket, and snapshot helpers retrieve data
//!   without formatting or mutating parser state.
//! - parser: PTY output helpers feed bytes into the `vt100` parser.
//! - filter: routed-input helpers select the child-bound subset from a routed
//!   input batch.
//!
//! The TUI mode never writes child output straight to the real terminal: child
//! bytes feed a `vt100` parser that backs the top pane, while `ratatui` owns the
//! screen so the monitor pane is protected from provider escape codes.

use super::cancel::{
    CancelRequest, cancel_outcome_message, cancel_request_for_node, execute_cancel,
};
use super::outbound_observer::{
    ObservedUserTurn, OutboundObservation, OutboundObservationIdentity, OutboundObservationResult,
    OutboundObserverSource, OutboundObserverWorker,
};
use super::snapshot_worker::{MonitorSnapshotProvider, MonitorSnapshotWorker};
use super::transcript_view::project_transcript_tail;
use super::{
    ChildOutputState, ControlSocket, INJECT_WAIT_LIMIT, InputLineState, PendingChildInput,
    RELAY_BUFFER_BYTES, acknowledge_control_payload, flush_pending_child_input, is_pty_eof_error,
    poll_fds, poll_master_fd, poll_relay_fds, poll_single_fd, prepare_control_payload,
    pty_delivery_ack_message, queue_control_injection, read_control_request, read_fd, readable,
    send_signal_to_child_group, set_pty_winsize, terminal_winsize, validate_peer_uid, winsize_eq,
    writable, write_control_response,
};
use crate::observability::{
    InspectRef, LivenessStatus, MonitorDiagnostic, MonitorDiagnosticSeverity, MonitorNode,
    MonitorNodeId, MonitorNodeKind, MonitorSnapshot, MonitorStatus, ObservabilityRoot,
};
#[cfg(test)]
use crate::observability::{ObservabilitySnapshotPort, SnapshotLimits};
use base64::Engine as _;
#[cfg(test)]
use chrono::{DateTime, Utc};
#[cfg(test)]
use oulipoly_provider::client::CancellationToken;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, ExitStatus};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Rows reserved for the always-visible overlay at the bottom of the screen.
const COLLAPSED_MONITOR_ROWS: u16 = STATUS_ROW_ROWS + PSEUDO_INPUT_MIN_ROWS;
/// Rows reserved for the overlay/status bar.
const STATUS_ROW_ROWS: u16 = 1;
/// Minimum terminal rows for the split to be usable.
const MIN_TERMINAL_ROWS: u16 = 10;
/// Minimum terminal columns for the split to be usable.
const MIN_TERMINAL_COLS: u16 = 40;
/// Target render cadence while the monitor overlay owns focus.
pub(super) const FOREGROUND_RENDER_FPS: u64 = 60;
/// Target render cadence while the monitor overlay is collapsed/backgrounded.
pub(super) const BACKGROUND_RENDER_FPS: u64 = 10;
/// Bounded PTY reads folded into one relay iteration before publishing a frame.
const MAX_COALESCED_PTY_READS: usize = 8;

/// Bracketed-paste delimiters (DECSET 2004) the broker wraps an injected notification in
/// when the child has advertised the mode, so the body is treated as pasted content and
/// the trailing Enter submits it.
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const BRACKETED_PASTE_ENABLE: &[u8] = b"\x1b[?2004h";
const BRACKETED_PASTE_DISABLE: &[u8] = b"\x1b[?2004l";
/// Pause between writing an injected notification's body and the Enter that submits it.
/// An Ink-style child (Claude Code) commits a bracketed paste to its input buffer on a
/// later async render tick; a `\r` written back-to-back races ahead of that commit and is
/// dropped, leaving the notification unsent until the operator presses Enter themselves.
/// Waiting lets the paste commit land first so the Enter actually submits.
const CONTROL_SUBMIT_DELAY: Duration = Duration::from_millis(400);
/// Plain line-oriented providers may never advertise bracketed-paste mode. Keep
/// their startup delivery bounded without overriding a full-screen TUI that is
/// still initializing its input handler.
const CONTROL_PRIMARY_SCREEN_READY_FALLBACK: Duration = Duration::from_secs(10);
const MOUSE_PRESS_ENABLE: &[u8] = b"\x1b[?9h";
const MOUSE_PRESS_DISABLE: &[u8] = b"\x1b[?9l";
const MOUSE_PRESS_RELEASE_ENABLE: &[u8] = b"\x1b[?1000h";
const MOUSE_PRESS_RELEASE_DISABLE: &[u8] = b"\x1b[?1000l";
const MOUSE_BUTTON_MOTION_ENABLE: &[u8] = b"\x1b[?1002h";
const MOUSE_BUTTON_MOTION_DISABLE: &[u8] = b"\x1b[?1002l";
const MOUSE_ANY_MOTION_ENABLE: &[u8] = b"\x1b[?1003h";
const MOUSE_ANY_MOTION_DISABLE: &[u8] = b"\x1b[?1003l";
const MOUSE_UTF8_ENABLE: &[u8] = b"\x1b[?1005h";
const MOUSE_UTF8_DISABLE: &[u8] = b"\x1b[?1005l";
const MOUSE_SGR_ENABLE: &[u8] = b"\x1b[?1006h";
const MOUSE_SGR_DISABLE: &[u8] = b"\x1b[?1006l";
/// Kitty keyboard protocol push with disambiguate-escape-codes enabled.
const KITTY_KEYBOARD_DISAMBIGUATE_PUSH: &[u8] = b"\x1b[>1u";
/// Kitty keyboard protocol pop, restoring the prior keyboard enhancement flags.
const KITTY_KEYBOARD_POP: &[u8] = b"\x1b[<u";
/// xterm modifyOtherKeys level 2, used by Windows Terminal and xterm-family emulators.
const XTERM_MODIFY_OTHER_KEYS_LEVEL_2: &[u8] = b"\x1b[>4;2m";
/// xterm modifyOtherKeys reset to the default level.
const XTERM_MODIFY_OTHER_KEYS_RESET: &[u8] = b"\x1b[>4;0m";

/// Map the real terminal window size to the child PTY window size, reserving the
/// persistent overlay rows so the provider believes its terminal is the top pane.
pub(super) fn top_pane_winsize(full: &libc::winsize) -> libc::winsize {
    child_winsize(full, COLLAPSED_MONITOR_ROWS)
}

/// Child PTY window size for the given full terminal and reserved bottom rows.
fn child_winsize(full: &libc::winsize, bottom_rows: u16) -> libc::winsize {
    let rows = full.ws_row.saturating_sub(bottom_rows).max(1);
    libc::winsize {
        ws_row: rows,
        ws_col: full.ws_col.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Whether the terminal is large enough to host the split without crowding out
/// the interactive top pane.
pub(super) fn dimensions_sufficient(winsize: &libc::winsize) -> bool {
    winsize.ws_row >= MIN_TERMINAL_ROWS && winsize.ws_col >= MIN_TERMINAL_COLS
}

/// Which pane currently consumes keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RenderPriority {
    Background,
    Interactive,
}

impl RenderPriority {
    fn escalate(&mut self, priority: Self) {
        if priority > *self {
            *self = priority;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TypingProtection {
    active: bool,
}

impl TypingProtection {
    fn active() -> Self {
        Self { active: true }
    }

    fn inactive() -> Self {
        Self { active: false }
    }

    fn for_focus(focus: Focus) -> Self {
        if focus == Focus::Top {
            Self::active()
        } else {
            Self::inactive()
        }
    }

    fn top_min_rows(self) -> u16 {
        if self.active {
            INPUT_SAFE_TOP_PANE_MIN_ROWS
        } else {
            TOP_PANE_MIN_ROWS
        }
    }
}

fn typing_protection(focus: Focus, line_state: &InputLineState) -> TypingProtection {
    if focus == Focus::Top || !line_state.input_empty() {
        TypingProtection::active()
    } else {
        TypingProtection::inactive()
    }
}

/// A bottom-pane (monitor) command decoded from keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorCommand {
    SelectNext,
    SelectPrev,
    ToggleSelectIndex(usize),
    SelectFilter(MonitorFilterCategory),
    NextFilter,
    PrevFilter,
    ToggleTreeMode,
    Refresh,
    Collapse,
    ToggleList,
    ToggleInspect,
    RequestCancel,
    RequestOutboundRetry,
    RequestOutboundDiscard,
    ConfirmAction,
    AbortAction,
}

/// Outcome of routing a chunk of real-terminal input.
#[derive(Debug, Default, PartialEq, Eq)]
struct RoutedInput {
    forward: Vec<u8>,
    commands: Vec<MonitorCommand>,
    pseudo_input: Vec<PseudoInputAction>,
    pseudo_input_width: Option<u16>,
    /// The operator clicked into the monitor/input area (focus bottom).
    focus_bottom: bool,
    /// Net wheel scroll for the top pane's scrollback: positive moves toward older
    /// output, negative toward the live tail. Applied by the relay loop.
    top_scroll_lines: i32,
    /// Ordered top-pane drag-selection gestures (left button) the relay loop folds into
    /// the live selection state. Empty when the child owns the mouse.
    top_mouse: Vec<TopMouse>,
    /// A right-click in the top pane, with its 1-based pane-local (row, col). The relay
    /// loop copies+deselects if it lands on the selection, otherwise pastes.
    right_click: Option<(u16, u16)>,
    redraw: bool,
}

/// A left-button selection gesture in the interactive top pane, with 1-based terminal
/// coordinates localized to the top pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TopMouse {
    gesture: TopGesture,
    row: u16,
    col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopGesture {
    Press,
    Drag,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MouseRequest {
    mode: vt100::MouseProtocolMode,
    encoding: vt100::MouseProtocolEncoding,
}

impl MouseRequest {
    fn disabled() -> Self {
        Self {
            mode: vt100::MouseProtocolMode::None,
            encoding: vt100::MouseProtocolEncoding::Default,
        }
    }

    fn is_enabled(self) -> bool {
        self.mode != vt100::MouseProtocolMode::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MouseEvent {
    button: u16,
    col: u16,
    row: u16,
    released: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedMouseEvent {
    event: MouseEvent,
    consumed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MousePaneRoute {
    Top(MouseEvent),
    Bottom(MouseEvent),
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneAreas {
    top: Rect,
    bottom: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalMouseState {
    request: MouseRequest,
}

impl TerminalMouseState {
    fn new() -> Self {
        Self {
            request: MouseRequest::disabled(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomKey {
    Enter,
    Newline,
    Backspace,
    Delete,
    LeftArrow,
    RightArrow,
    UpArrow,
    DownArrow,
    MoveStart,
    MoveEnd,
    Clear,
    NextFilter,
    PrevFilter,
    TreeMode,
    Refresh,
    Collapse,
    Inspect,
    Cancel,
    RetryOutbound,
    DiscardOutbound,
    Confirm,
    Abort,
    Printable(char),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedBottomKey {
    key: BottomKey,
    consumed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomInputRoute {
    Command(MonitorCommand),
    PseudoInput(PseudoInputAction),
    Consume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PseudoInputAction {
    Insert(char),
    InsertNewline,
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveStart,
    MoveEnd,
    Clear,
    Submit,
}

enum InspectContentSource<'a> {
    AgentBashLog {
        path: &'a str,
        max_tail_bytes: usize,
    },
    SessionTranscript {
        path: &'a str,
        max_tail_bytes: usize,
        format_id: Option<&'a str>,
    },
    LastOutput(&'a str),
    None,
}

enum TailFileReadError {
    Open(io::Error),
    Seek,
    Read,
}

/// Routes real-terminal input by focus: in the top pane bytes pass through to the
/// child verbatim; in the bottom pane keys are decoded into monitor commands and
/// never reach the child.
#[derive(Debug)]
struct InputRouter {
    focus: Focus,
}

impl InputRouter {
    fn new() -> Self {
        Self { focus: Focus::Top }
    }

    /// Keyboard-only routing helper. Production input always flows through
    /// `route_mouse_aware_input` (which calls `route_next_input` per chunk) since the
    /// broker always captures the mouse; this convenience wrapper is used by the keyboard
    /// routing tests.
    #[cfg(test)]
    fn route_input(&mut self, bytes: &[u8]) -> RoutedInput {
        let mut routed = RoutedInput::default();
        let mut i = 0;
        while i < bytes.len() {
            i += self.route_next_input(&bytes[i..], &mut routed);
        }
        routed
    }

    fn route_next_input(&mut self, bytes: &[u8], routed: &mut RoutedInput) -> usize {
        match self.focus {
            Focus::Top => self.route_top_byte(bytes[0], routed),
            Focus::Bottom => self.route_bottom_key(bytes, routed),
        }
    }

    fn route_top_byte(&mut self, byte: u8, routed: &mut RoutedInput) -> usize {
        routed.forward.push(byte);
        1
    }

    fn focus_bottom(&mut self, routed: &mut RoutedInput) {
        self.focus = Focus::Bottom;
        routed.focus_bottom = true;
        routed.redraw = true;
    }

    fn route_bottom_key(&mut self, bytes: &[u8], routed: &mut RoutedInput) -> usize {
        let parsed = parse_bottom_key(bytes);
        trace_overlay_input_key(&bytes[..parsed.consumed.min(bytes.len())], parsed.key);
        let route = bottom_key_route(parsed.key);
        self.apply_bottom_route(route, parsed.consumed, routed)
    }

    fn apply_bottom_route(
        &mut self,
        route: BottomInputRoute,
        consumed: usize,
        routed: &mut RoutedInput,
    ) -> usize {
        match route {
            BottomInputRoute::Command(command) => apply_monitor_command(routed, command),
            BottomInputRoute::PseudoInput(action) => apply_pseudo_input_action(routed, action),
            BottomInputRoute::Consume => {}
        }
        consumed
    }

    fn focus_top(&mut self, routed: &mut RoutedInput) {
        self.focus = Focus::Top;
        routed.redraw = true;
    }
}

fn parse_bottom_key(bytes: &[u8]) -> ParsedBottomKey {
    if let Some(consumed) = ctrl_enter_sequence_len(bytes) {
        return ParsedBottomKey {
            key: BottomKey::Newline,
            consumed,
        };
    }
    match bytes {
        // Plain Enter submits. Most terminals send CR (0x0d) for Enter and LF (0x0a,
        // i.e. Ctrl+J) for Ctrl+Enter, so LF inserts a newline at the cursor (this also
        // makes a pasted raw '\n' insert rather than submit). Terminals that emit a
        // distinct CSI-u Ctrl+Enter are handled by ctrl_enter_sequence_len above.
        [b'\r', ..] => ParsedBottomKey {
            key: BottomKey::Enter,
            consumed: 1,
        },
        [b'\n', ..] => ParsedBottomKey {
            key: BottomKey::Newline,
            consumed: 1,
        },
        [0x1b, b'[', b'1', b'3', b'u', ..] => ParsedBottomKey {
            key: BottomKey::Enter,
            consumed: 5,
        },
        [0x7f, ..] | [0x08, ..] => ParsedBottomKey {
            key: BottomKey::Backspace,
            consumed: 1,
        },
        [0x1b, b'[', b'3', b'~', ..] => ParsedBottomKey {
            key: BottomKey::Delete,
            consumed: 4,
        },
        [0x1b, b'[', b'A', ..] => ParsedBottomKey {
            key: BottomKey::UpArrow,
            consumed: 3,
        },
        [0x1b, b'[', b'B', ..] => ParsedBottomKey {
            key: BottomKey::DownArrow,
            consumed: 3,
        },
        [0x1b, b'[', b'D', ..] => ParsedBottomKey {
            key: BottomKey::LeftArrow,
            consumed: 3,
        },
        [0x1b, b'[', b'C', ..] => ParsedBottomKey {
            key: BottomKey::RightArrow,
            consumed: 3,
        },
        [0x01, ..] => ParsedBottomKey {
            key: BottomKey::MoveStart,
            consumed: 1,
        },
        [0x05, ..] => ParsedBottomKey {
            key: BottomKey::MoveEnd,
            consumed: 1,
        },
        [0x15, ..] => ParsedBottomKey {
            key: BottomKey::Clear,
            consumed: 1,
        },
        [0x06, ..] => ParsedBottomKey {
            key: BottomKey::NextFilter,
            consumed: 1,
        },
        [0x1b, b'[', b'Z', ..] => ParsedBottomKey {
            key: BottomKey::PrevFilter,
            consumed: 3,
        },
        [0x14, ..] => ParsedBottomKey {
            key: BottomKey::TreeMode,
            consumed: 1,
        },
        [0x12, ..] => ParsedBottomKey {
            key: BottomKey::Refresh,
            consumed: 1,
        },
        [0x11, ..] => ParsedBottomKey {
            key: BottomKey::Collapse,
            consumed: 1,
        },
        [0x09, ..] => ParsedBottomKey {
            key: BottomKey::Inspect,
            consumed: 1,
        },
        [0x18, ..] => ParsedBottomKey {
            key: BottomKey::Cancel,
            consumed: 1,
        },
        [0x10, ..] => ParsedBottomKey {
            key: BottomKey::RetryOutbound,
            consumed: 1,
        },
        [0x04, ..] => ParsedBottomKey {
            key: BottomKey::DiscardOutbound,
            consumed: 1,
        },
        [0x19, ..] => ParsedBottomKey {
            key: BottomKey::Confirm,
            consumed: 1,
        },
        [0x0e, ..] => ParsedBottomKey {
            key: BottomKey::Abort,
            consumed: 1,
        },
        [byte, ..] if byte.is_ascii_graphic() || *byte == b' ' => ParsedBottomKey {
            key: BottomKey::Printable(*byte as char),
            consumed: 1,
        },
        _ => ParsedBottomKey {
            key: BottomKey::Unknown,
            consumed: 1,
        },
    }
}

fn ctrl_enter_sequence_len(bytes: &[u8]) -> Option<usize> {
    for sequence in [
        b"\x1b[13;5u".as_slice(),
        b"\x1b[13;5~".as_slice(),
        b"\x1b[13^".as_slice(),
        b"\x1b[27;5;13~".as_slice(),
    ] {
        if bytes.starts_with(sequence) {
            return Some(sequence.len());
        }
    }
    None
}

fn trace_overlay_input_key(bytes: &[u8], key: BottomKey) {
    let raw_hex = input_bytes_hex(bytes);
    super::append_overlay_input_trace_record(&raw_hex, overlay_input_classification(key));
}

fn overlay_input_classification(key: BottomKey) -> &'static str {
    match key {
        BottomKey::Newline => "ctrl_enter",
        BottomKey::Enter => "enter-submit",
        BottomKey::Printable(_) => "printable",
        _ => "other",
    }
}

fn input_bytes_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(HEX[(*byte >> 4) as usize] as char);
        value.push(HEX[(*byte & 0x0f) as usize] as char);
    }
    value
}

fn bottom_key_route(key: BottomKey) -> BottomInputRoute {
    match key {
        BottomKey::Enter => BottomInputRoute::PseudoInput(PseudoInputAction::Submit),
        BottomKey::Newline => BottomInputRoute::PseudoInput(PseudoInputAction::InsertNewline),
        BottomKey::Backspace => BottomInputRoute::PseudoInput(PseudoInputAction::Backspace),
        BottomKey::Delete => BottomInputRoute::PseudoInput(PseudoInputAction::Delete),
        BottomKey::LeftArrow => BottomInputRoute::PseudoInput(PseudoInputAction::MoveLeft),
        BottomKey::RightArrow => BottomInputRoute::PseudoInput(PseudoInputAction::MoveRight),
        BottomKey::UpArrow => BottomInputRoute::PseudoInput(PseudoInputAction::MoveUp),
        BottomKey::DownArrow => BottomInputRoute::PseudoInput(PseudoInputAction::MoveDown),
        BottomKey::MoveStart => BottomInputRoute::PseudoInput(PseudoInputAction::MoveStart),
        BottomKey::MoveEnd => BottomInputRoute::PseudoInput(PseudoInputAction::MoveEnd),
        BottomKey::Clear => BottomInputRoute::PseudoInput(PseudoInputAction::Clear),
        BottomKey::NextFilter => BottomInputRoute::Command(MonitorCommand::NextFilter),
        BottomKey::PrevFilter => BottomInputRoute::Command(MonitorCommand::PrevFilter),
        BottomKey::TreeMode => BottomInputRoute::Command(MonitorCommand::ToggleTreeMode),
        BottomKey::Refresh => BottomInputRoute::Command(MonitorCommand::Refresh),
        BottomKey::Collapse => BottomInputRoute::Command(MonitorCommand::Collapse),
        BottomKey::Inspect => BottomInputRoute::Command(MonitorCommand::ToggleInspect),
        BottomKey::Cancel => BottomInputRoute::Command(MonitorCommand::RequestCancel),
        BottomKey::RetryOutbound => BottomInputRoute::Command(MonitorCommand::RequestOutboundRetry),
        BottomKey::DiscardOutbound => {
            BottomInputRoute::Command(MonitorCommand::RequestOutboundDiscard)
        }
        BottomKey::Confirm => BottomInputRoute::Command(MonitorCommand::ConfirmAction),
        BottomKey::Abort => BottomInputRoute::Command(MonitorCommand::AbortAction),
        BottomKey::Printable(ch) => BottomInputRoute::PseudoInput(PseudoInputAction::Insert(ch)),
        BottomKey::Unknown => BottomInputRoute::Consume,
    }
}

fn apply_monitor_command(routed: &mut RoutedInput, command: MonitorCommand) {
    routed.commands.push(command);
    routed.redraw = true;
}

fn apply_pseudo_input_action(routed: &mut RoutedInput, action: PseudoInputAction) {
    routed.pseudo_input.push(action);
    routed.redraw = true;
}

fn mouse_request_from_screen(screen: &vt100::Screen) -> MouseRequest {
    MouseRequest {
        mode: screen.mouse_protocol_mode(),
        encoding: screen.mouse_protocol_encoding(),
    }
}

fn sync_terminal_mouse<W: Write>(
    writer: &mut W,
    state: &mut TerminalMouseState,
    next: MouseRequest,
) -> io::Result<()> {
    let bytes = terminal_mouse_delta(state.request, next);
    if !bytes.is_empty() {
        writer.write_all(&bytes)?;
        writer.flush()?;
    }
    state.request = next;
    Ok(())
}

fn terminal_mouse_delta(prev: MouseRequest, next: MouseRequest) -> Vec<u8> {
    let mut bytes = Vec::new();
    append_mouse_mode_disable(&mut bytes, prev.mode, next.mode);
    append_mouse_encoding_disable(&mut bytes, prev.encoding, next.encoding);
    append_mouse_encoding_enable(&mut bytes, prev.encoding, next.encoding);
    append_mouse_mode_enable(&mut bytes, prev.mode, next.mode);
    bytes
}

fn append_mouse_mode_disable(
    bytes: &mut Vec<u8>,
    prev: vt100::MouseProtocolMode,
    next: vt100::MouseProtocolMode,
) {
    if prev != next {
        bytes.extend_from_slice(mouse_mode_disable_sequence(prev));
    }
}

fn append_mouse_mode_enable(
    bytes: &mut Vec<u8>,
    prev: vt100::MouseProtocolMode,
    next: vt100::MouseProtocolMode,
) {
    if prev != next {
        bytes.extend_from_slice(mouse_mode_enable_sequence(next));
    }
}

fn append_mouse_encoding_disable(
    bytes: &mut Vec<u8>,
    prev: vt100::MouseProtocolEncoding,
    next: vt100::MouseProtocolEncoding,
) {
    if prev != next {
        bytes.extend_from_slice(mouse_encoding_disable_sequence(prev));
    }
}

fn append_mouse_encoding_enable(
    bytes: &mut Vec<u8>,
    prev: vt100::MouseProtocolEncoding,
    next: vt100::MouseProtocolEncoding,
) {
    if prev != next {
        bytes.extend_from_slice(mouse_encoding_enable_sequence(next));
    }
}

fn mouse_mode_enable_sequence(mode: vt100::MouseProtocolMode) -> &'static [u8] {
    match mode {
        vt100::MouseProtocolMode::None => b"",
        vt100::MouseProtocolMode::Press => MOUSE_PRESS_ENABLE,
        vt100::MouseProtocolMode::PressRelease => MOUSE_PRESS_RELEASE_ENABLE,
        vt100::MouseProtocolMode::ButtonMotion => MOUSE_BUTTON_MOTION_ENABLE,
        vt100::MouseProtocolMode::AnyMotion => MOUSE_ANY_MOTION_ENABLE,
    }
}

fn mouse_mode_disable_sequence(mode: vt100::MouseProtocolMode) -> &'static [u8] {
    match mode {
        vt100::MouseProtocolMode::None => b"",
        vt100::MouseProtocolMode::Press => MOUSE_PRESS_DISABLE,
        vt100::MouseProtocolMode::PressRelease => MOUSE_PRESS_RELEASE_DISABLE,
        vt100::MouseProtocolMode::ButtonMotion => MOUSE_BUTTON_MOTION_DISABLE,
        vt100::MouseProtocolMode::AnyMotion => MOUSE_ANY_MOTION_DISABLE,
    }
}

fn mouse_encoding_enable_sequence(encoding: vt100::MouseProtocolEncoding) -> &'static [u8] {
    match encoding {
        vt100::MouseProtocolEncoding::Default => b"",
        vt100::MouseProtocolEncoding::Utf8 => MOUSE_UTF8_ENABLE,
        vt100::MouseProtocolEncoding::Sgr => MOUSE_SGR_ENABLE,
    }
}

fn mouse_encoding_disable_sequence(encoding: vt100::MouseProtocolEncoding) -> &'static [u8] {
    match encoding {
        vt100::MouseProtocolEncoding::Default => b"",
        vt100::MouseProtocolEncoding::Utf8 => MOUSE_UTF8_DISABLE,
        vt100::MouseProtocolEncoding::Sgr => MOUSE_SGR_DISABLE,
    }
}

fn terminal_mouse_restore_sequence() -> Vec<u8> {
    [
        MOUSE_PRESS_DISABLE,
        MOUSE_PRESS_RELEASE_DISABLE,
        MOUSE_BUTTON_MOTION_DISABLE,
        MOUSE_ANY_MOTION_DISABLE,
        MOUSE_UTF8_DISABLE,
        MOUSE_SGR_DISABLE,
    ]
    .concat()
}

fn terminal_keyboard_enable_sequence() -> Vec<u8> {
    [
        KITTY_KEYBOARD_DISAMBIGUATE_PUSH,
        XTERM_MODIFY_OTHER_KEYS_LEVEL_2,
    ]
    .concat()
}

fn terminal_keyboard_restore_sequence() -> Vec<u8> {
    [KITTY_KEYBOARD_POP, XTERM_MODIFY_OTHER_KEYS_RESET].concat()
}

fn parse_mouse_event(bytes: &[u8]) -> Option<ParsedMouseEvent> {
    parse_sgr_mouse_event(bytes).or_else(|| parse_legacy_mouse_event(bytes))
}

fn parse_sgr_mouse_event(bytes: &[u8]) -> Option<ParsedMouseEvent> {
    if !bytes.starts_with(b"\x1b[<") {
        return None;
    }
    let terminator = sgr_mouse_terminator_index(bytes)?;
    let event = decode_sgr_mouse_event(&bytes[3..terminator], bytes[terminator])?;
    Some(ParsedMouseEvent {
        event,
        consumed: terminator + 1,
    })
}

fn sgr_mouse_terminator_index(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|byte| matches!(byte, b'M' | b'm'))
}

fn decode_sgr_mouse_event(fields: &[u8], terminator: u8) -> Option<MouseEvent> {
    let mut parts = fields.split(|byte| *byte == b';');
    let button = parse_mouse_number(parts.next()?)?;
    let col = parse_mouse_number(parts.next()?)?;
    let row = parse_mouse_number(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(MouseEvent {
        button,
        col,
        row,
        released: terminator == b'm',
    })
}

fn parse_mouse_number(bytes: &[u8]) -> Option<u16> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn parse_legacy_mouse_event(bytes: &[u8]) -> Option<ParsedMouseEvent> {
    if bytes.len() < 6 || !bytes.starts_with(b"\x1b[M") {
        return None;
    }
    let button = legacy_mouse_value(bytes[3])?;
    let col = legacy_mouse_value(bytes[4])?;
    let row = legacy_mouse_value(bytes[5])?;
    Some(ParsedMouseEvent {
        event: MouseEvent {
            button,
            col,
            row,
            released: legacy_button_is_release(button),
        },
        consumed: 6,
    })
}

fn legacy_mouse_value(byte: u8) -> Option<u16> {
    byte.checked_sub(32).map(u16::from)
}

fn legacy_button_is_release(button: u16) -> bool {
    button & 0b11 == 3 && !mouse_button_is_wheel(button)
}

fn encode_mouse_event(
    event: MouseEvent,
    encoding: vt100::MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    match encoding {
        vt100::MouseProtocolEncoding::Default => encode_default_mouse_event(event),
        vt100::MouseProtocolEncoding::Utf8 => encode_utf8_mouse_event(event),
        vt100::MouseProtocolEncoding::Sgr => Some(encode_sgr_mouse_event(event)),
    }
}

fn encode_sgr_mouse_event(event: MouseEvent) -> Vec<u8> {
    let terminator = if event.released { 'm' } else { 'M' };
    format!(
        "\x1b[<{};{};{}{}",
        event.button, event.col, event.row, terminator
    )
    .into_bytes()
}

fn encode_default_mouse_event(event: MouseEvent) -> Option<Vec<u8>> {
    let button = default_mouse_button(event);
    Some(vec![
        0x1b,
        b'[',
        b'M',
        encode_default_mouse_value(button)?,
        encode_default_mouse_value(event.col)?,
        encode_default_mouse_value(event.row)?,
    ])
}

fn encode_default_mouse_value(value: u16) -> Option<u8> {
    u8::try_from(value.checked_add(32)?).ok()
}

fn encode_utf8_mouse_event(event: MouseEvent) -> Option<Vec<u8>> {
    let mut bytes = b"\x1b[M".to_vec();
    append_utf8_mouse_value(&mut bytes, default_mouse_button(event))?;
    append_utf8_mouse_value(&mut bytes, event.col)?;
    append_utf8_mouse_value(&mut bytes, event.row)?;
    Some(bytes)
}

fn append_utf8_mouse_value(bytes: &mut Vec<u8>, value: u16) -> Option<()> {
    let codepoint = u32::from(value.checked_add(32)?);
    bytes.extend(char::from_u32(codepoint)?.to_string().as_bytes());
    Some(())
}

fn default_mouse_button(event: MouseEvent) -> u16 {
    if event.released && !mouse_button_is_wheel(event.button) {
        (event.button & !0b11) | 3
    } else {
        event.button
    }
}

fn mouse_button_is_wheel(button: u16) -> bool {
    button & 64 != 0
}

fn mouse_wheel_command(event: MouseEvent) -> Option<MonitorCommand> {
    if !mouse_button_is_wheel(event.button) {
        return None;
    }
    match event.button & 0b11 {
        0 => Some(MonitorCommand::SelectPrev),
        1 => Some(MonitorCommand::SelectNext),
        _ => None,
    }
}

/// Minimum mouse-reporting mode the broker drives on the real terminal so wheel events
/// reach the panes and click-drag selection works even when the child never requested
/// mouse input. `ButtonMotion` reports button press/release, the wheel, and motion *while
/// a button is held* (needed to extend a drag selection) without the noise of free motion.
const BROKER_WHEEL_CAPTURE_MODE: vt100::MouseProtocolMode = vt100::MouseProtocolMode::ButtonMotion;

fn mouse_mode_rank(mode: vt100::MouseProtocolMode) -> u8 {
    match mode {
        vt100::MouseProtocolMode::None => 0,
        vt100::MouseProtocolMode::Press => 1,
        vt100::MouseProtocolMode::PressRelease => 2,
        vt100::MouseProtocolMode::ButtonMotion => 3,
        vt100::MouseProtocolMode::AnyMotion => 4,
    }
}

fn stronger_mouse_mode(
    a: vt100::MouseProtocolMode,
    b: vt100::MouseProtocolMode,
) -> vt100::MouseProtocolMode {
    if mouse_mode_rank(a) >= mouse_mode_rank(b) {
        a
    } else {
        b
    }
}

/// The mouse request the broker drives on the *real* terminal — distinct from the
/// child's own request. The broker always captures wheel/click events (SGR encoding for
/// robustness, preferring a stronger child mode if the child set one) so the operator
/// can wheel through the top-pane scrollback and the monitor regardless of whether the
/// child enabled mouse mode. Without this the terminal's alternate-scroll mode turns the
/// wheel into ↑/↓ keystrokes that leak to the child as history navigation. Top-pane
/// events are still only forwarded to a child that requested mouse input (see
/// `route_top_mouse_event`); otherwise the wheel scrolls the broker's scrollback.
///
/// Tradeoff: claiming the mouse disables native click-drag text selection in the top
/// pane for the duration of the observed session (Shift+drag bypasses it in most
/// terminals).
fn effective_mouse_request(child: MouseRequest) -> MouseRequest {
    MouseRequest {
        mode: stronger_mouse_mode(child.mode, BROKER_WHEEL_CAPTURE_MODE),
        encoding: vt100::MouseProtocolEncoding::Sgr,
    }
}

/// Net top-pane scrollback movement for a wheel event: positive toward older output.
fn wheel_scroll_lines(event: MouseEvent) -> i32 {
    if !mouse_button_is_wheel(event.button) {
        return 0;
    }
    match event.button & 0b11 {
        0 => TOP_SCROLL_STEP,  // wheel up -> older output
        1 => -TOP_SCROLL_STEP, // wheel down -> toward the live tail
        _ => 0,
    }
}

/// Apply a signed scroll delta to the current top-pane scrollback offset, flooring at
/// the live tail (0). The upper bound is enforced by `Screen::set_scrollback`, which
/// clamps to the retained history.
fn apply_top_scroll(current: usize, delta: i32) -> usize {
    (current as i64 + i64::from(delta)).max(0) as usize
}

fn pane_areas(area: Rect, pane: &MonitorPane, protection: TypingProtection) -> PaneAreas {
    let bottom_rows = pane.bottom_rows(area.height, area.width, protection);
    let [top, bottom] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_rows)]).areas(area);
    PaneAreas { top, bottom }
}

fn pane_areas_for_winsize(
    winsize: &libc::winsize,
    pane: &MonitorPane,
    protection: TypingProtection,
) -> PaneAreas {
    pane_areas(
        Rect {
            x: 0,
            y: 0,
            width: winsize.ws_col,
            height: winsize.ws_row,
        },
        pane,
        protection,
    )
}

fn route_mouse_to_pane(event: MouseEvent, areas: PaneAreas) -> MousePaneRoute {
    if rect_contains_mouse(areas.top, event) {
        MousePaneRoute::Top(localize_mouse_event(event, areas.top))
    } else if rect_contains_mouse(areas.bottom, event) {
        MousePaneRoute::Bottom(event)
    } else {
        MousePaneRoute::Outside
    }
}

fn rect_contains_mouse(rect: Rect, event: MouseEvent) -> bool {
    event.col > rect.x
        && event.col <= rect.x.saturating_add(rect.width)
        && event.row > rect.y
        && event.row <= rect.y.saturating_add(rect.height)
}

fn localize_mouse_event(event: MouseEvent, rect: Rect) -> MouseEvent {
    MouseEvent {
        col: event.col.saturating_sub(rect.x),
        row: event.row.saturating_sub(rect.y),
        ..event
    }
}

/// Collapsed monitor refresh cadence (slow; just the summary row).
const COLLAPSED_REFRESH: Duration = Duration::from_millis(2000);
/// Expanded monitor refresh cadence (the operator is actively watching).
const EXPANDED_REFRESH: Duration = Duration::from_millis(500);
/// Local selected-detail refresh cadence. This is intentionally separate from
/// snapshot refreshes so live output tails update without polling every source.
const DETAIL_REFRESH: Duration = Duration::from_millis(250);
/// Expanded monitor target share of terminal height, and its floor.
const EXPANDED_MIN_ROWS: u16 = 9;
/// Rows always reserved for the interactive top pane.
const TOP_PANE_MIN_ROWS: u16 = 5;
/// Top-pane floor while real child input is active, preserving multi-line composers.
const INPUT_SAFE_TOP_PANE_MIN_ROWS: u16 = 14;
/// Scrollback rows retained for the interactive top pane so the operator can wheel
/// back through the child's output (the child runs in the broker's alt-screen, which
/// has no native scrollback of its own).
const TOP_PANE_SCROLLBACK_ROWS: usize = 10_000;
/// Rows moved per wheel notch when scrolling the top-pane scrollback.
const TOP_SCROLL_STEP: i32 = 3;
/// Minimum editable rows for the always-visible pseudo input lane.
const MIN_INPUT_ROWS: u16 = 1;
/// Maximum editable rows before the pseudo input scrolls.
const MAX_INPUT_ROWS: u16 = 10;
/// Width fallback for keyboard-only tests that bypass the full pane router.
const DEFAULT_PSEUDO_INPUT_WIDTH: u16 = 80;
/// Persistent help rows rendered beneath the pseudo input editor.
const PSEUDO_INPUT_HELP_ROWS: u16 = 0;
/// Minimum rows reserved by the pseudo input.
const PSEUDO_INPUT_MIN_ROWS: u16 = MIN_INPUT_ROWS + PSEUDO_INPUT_HELP_ROWS;
/// Broker-owned input messages larger than this fail before reaching the child.
const PSEUDO_INPUT_MAX_BYTES: usize = 64 * 1024;
/// Sent messages fail safe to ambiguous when no consumption proof appears in time.
const OUTBOUND_CONSUMPTION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
struct PseudoInputState {
    buffer: String,
    cursor: usize,
}

impl PseudoInputState {
    fn apply(&mut self, action: PseudoInputAction, width: u16) -> Option<String> {
        match action {
            PseudoInputAction::Insert(ch) => self.insert(ch),
            PseudoInputAction::InsertNewline => self.insert('\n'),
            PseudoInputAction::Backspace => self.backspace(),
            PseudoInputAction::Delete => self.delete(),
            PseudoInputAction::MoveLeft => self.move_left(),
            PseudoInputAction::MoveRight => self.move_right(),
            PseudoInputAction::MoveUp => self.move_up(width),
            PseudoInputAction::MoveDown => self.move_down(width),
            PseudoInputAction::MoveStart => self.move_start(),
            PseudoInputAction::MoveEnd => self.move_end(),
            PseudoInputAction::Clear => self.clear(),
            PseudoInputAction::Submit => return self.take_submission(),
        }
        None
    }

    fn insert(&mut self, ch: char) {
        self.buffer.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn backspace(&mut self) {
        let Some(prev) = previous_char_boundary(&self.buffer, self.cursor) else {
            return;
        };
        self.buffer.drain(prev..self.cursor);
        self.cursor = prev;
    }

    fn delete(&mut self) {
        let Some(next) = next_char_boundary(&self.buffer, self.cursor) else {
            return;
        };
        self.buffer.drain(self.cursor..next);
    }

    fn move_left(&mut self) {
        if let Some(prev) = previous_char_boundary(&self.buffer, self.cursor) {
            self.cursor = prev;
        }
    }

    fn move_right(&mut self) {
        if let Some(next) = next_char_boundary(&self.buffer, self.cursor) {
            self.cursor = next;
        }
    }

    fn move_up(&mut self, width: u16) {
        self.move_vertical(-1, width);
    }

    fn move_down(&mut self, width: u16) {
        self.move_vertical(1, width);
    }

    fn move_vertical(&mut self, delta: isize, width: u16) {
        let rows = pseudo_input_visual_rows(&self.buffer, width);
        let Some((current_row, desired_col)) =
            pseudo_input_cursor_visual_position(&self.buffer, self.cursor, width, &rows)
        else {
            return;
        };
        let target_row = if delta.is_negative() {
            let Some(previous) = current_row.checked_sub(delta.unsigned_abs()) else {
                self.cursor = 0;
                return;
            };
            previous
        } else {
            let next = current_row.saturating_add(delta as usize);
            if next >= rows.len() {
                self.cursor = self.buffer.len();
                return;
            }
            next
        };
        let target = rows[target_row];
        let target_len = target.end_col.saturating_sub(target.start_col);
        let target_col = target.start_col + desired_col.min(target_len);
        self.cursor = byte_index_for_line_col(&self.buffer, target.line_index, target_col);
    }

    fn move_start(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.buffer.len();
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    fn take_submission(&mut self) -> Option<String> {
        if self.buffer.trim().is_empty() {
            return None;
        }
        let body = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        Some(body)
    }

    #[cfg(test)]
    fn cursor_line_col(&self) -> (usize, usize) {
        pseudo_input_cursor_line_col(&self.buffer, self.cursor)
    }

    fn desired_rows(&self, width: u16) -> u16 {
        desired_pseudo_input_rows(&self.buffer, self.cursor, width)
    }
}

fn pseudo_input_cursor_line_col(buffer: &str, cursor: usize) -> (usize, usize) {
    let before = &buffer[..cursor];
    let line = before.bytes().filter(|byte| *byte == b'\n').count();
    let col = before
        .rsplit_once('\n')
        .map(|(_, tail)| tail.chars().count())
        .unwrap_or_else(|| before.chars().count());
    (line, col)
}

fn previous_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[..cursor]
        .char_indices()
        .last()
        .map(|(index, _)| index)
}

fn next_char_boundary(value: &str, cursor: usize) -> Option<usize> {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map(|(index, _)| cursor + index)
        .or_else(|| (cursor < value.len()).then_some(value.len()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundRecoveryAction {
    Retry,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingOutboundRecovery {
    message_id: u64,
    action: OutboundRecoveryAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundStatus {
    Queued,
    Sending,
    Sent,
    Consumed,
    Ambiguous,
    Retrying,
    Discarded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutboundBaseline {
    identity: OutboundObservationIdentity,
    generation: u64,
    turn_ids: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct OutboundMessage {
    id: u64,
    body: String,
    status: OutboundStatus,
    sent_at: Option<Instant>,
    baseline: Option<OutboundBaseline>,
    minimum_generation: u64,
    detail: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct OutboundQueue {
    next_id: u64,
    messages: Vec<OutboundMessage>,
    active: Option<ActiveOutboundSend>,
    minimum_generation: u64,
}

#[derive(Debug, Clone)]
struct ActiveOutboundSend {
    message_id: u64,
    phase: OutboundSendPhase,
    bracketed_paste: bool,
}

#[derive(Debug, Clone)]
enum OutboundSendPhase {
    Body,
    DelayUntil(Instant),
    Submit,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum OutboundReleaseState {
    #[default]
    Ready,
    // An Enter-like byte is only a parsed boundary after the child consumes it.
    UserBoundaryPendingWrite,
    AwaitingChildOutput,
}

#[derive(Debug, Default)]
struct OutboundReleaseGate {
    state: OutboundReleaseState,
    last_child_output_at: Option<Instant>,
}

impl OutboundReleaseGate {
    fn observe_user_input(&mut self, reached_line_boundary: bool) {
        if reached_line_boundary {
            self.state = OutboundReleaseState::UserBoundaryPendingWrite;
        }
    }

    fn observe_pending_write_drained(&mut self, pending_empty: bool) {
        if pending_empty && self.state == OutboundReleaseState::UserBoundaryPendingWrite {
            self.state = OutboundReleaseState::AwaitingChildOutput;
        }
    }

    fn awaiting_child_output(&self) -> bool {
        self.state == OutboundReleaseState::AwaitingChildOutput
    }

    fn observe_child_output(&mut self, acknowledges_boundary: bool, now: Instant) {
        self.last_child_output_at = Some(now);
        if acknowledges_boundary && self.awaiting_child_output() {
            self.state = OutboundReleaseState::Ready;
        }
    }

    fn blocking_detail(&self, now: Instant) -> Option<&'static str> {
        match self.state {
            OutboundReleaseState::UserBoundaryPendingWrite => Some("awaiting_user_boundary_write"),
            OutboundReleaseState::AwaitingChildOutput => Some("awaiting_user_boundary_output"),
            OutboundReleaseState::Ready
                if self.last_child_output_at.is_some_and(|last| {
                    now.saturating_duration_since(last) < super::INJECT_CHILD_OUTPUT_DEBOUNCE
                }) =>
            {
                Some("awaiting_child_output_quiescence")
            }
            OutboundReleaseState::Ready => None,
        }
    }
}

impl OutboundQueue {
    fn enqueue(&mut self, body: String) -> u64 {
        let id = self.next_message_id();
        self.messages.push(OutboundMessage {
            id,
            body,
            status: OutboundStatus::Queued,
            sent_at: None,
            baseline: None,
            minimum_generation: 0,
            detail: None,
        });
        id
    }

    fn next_message_id(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1).max(1);
        self.next_id
    }

    fn has_unresolved_blocker(&self) -> bool {
        self.messages.iter().any(|message| {
            matches!(
                message.status,
                OutboundStatus::Sending
                    | OutboundStatus::Sent
                    | OutboundStatus::Ambiguous
                    | OutboundStatus::Retrying
                    | OutboundStatus::Failed
            )
        })
    }

    fn next_sendable_id(&self) -> Option<u64> {
        self.messages
            .iter()
            .find(|message| message.status == OutboundStatus::Retrying)
            .or_else(|| {
                self.messages
                    .iter()
                    .find(|message| message.status == OutboundStatus::Queued)
            })
            .map(|message| message.id)
    }

    fn message(&self, id: u64) -> Option<&OutboundMessage> {
        self.messages.iter().find(|message| message.id == id)
    }

    fn message_mut(&mut self, id: u64) -> Option<&mut OutboundMessage> {
        self.messages.iter_mut().find(|message| message.id == id)
    }

    fn set_status(
        &mut self,
        id: u64,
        status: OutboundStatus,
        now: Instant,
        detail: Option<String>,
    ) -> bool {
        let Some(message) = self.message_mut(id) else {
            return false;
        };
        let changed = message.status != status || message.detail != detail;
        message.status = status;
        message.detail = detail;
        if status == OutboundStatus::Sent && message.sent_at.is_none() {
            message.sent_at = Some(now);
        }
        changed
    }

    fn mark_sending(&mut self, id: u64, baseline: OutboundBaseline) -> bool {
        let Some(message) = self.message_mut(id) else {
            return false;
        };
        message.status = OutboundStatus::Sending;
        message.baseline = Some(baseline);
        message.detail = None;
        true
    }

    fn observation_needed(&self) -> bool {
        self.messages.iter().any(|message| {
            matches!(
                message.status,
                OutboundStatus::Queued
                    | OutboundStatus::Sent
                    | OutboundStatus::Ambiguous
                    | OutboundStatus::Retrying
            )
        })
    }

    fn oldest_ambiguous_id(&self) -> Option<u64> {
        self.messages
            .iter()
            .find(|message| message.status == OutboundStatus::Ambiguous)
            .map(|message| message.id)
    }

    fn apply_recovery(
        &mut self,
        message_id: u64,
        action: OutboundRecoveryAction,
        generation_floor: u64,
    ) -> bool {
        if self.oldest_ambiguous_id() != Some(message_id) {
            return false;
        }
        let Some(message) = self.message_mut(message_id) else {
            return false;
        };
        match action {
            OutboundRecoveryAction::Retry => {
                message.status = OutboundStatus::Retrying;
                message.sent_at = None;
                message.baseline = None;
                message.minimum_generation = generation_floor;
                message.detail = Some("awaiting_retry_baseline".to_string());
            }
            OutboundRecoveryAction::Discard => {
                message.status = OutboundStatus::Discarded;
                message.detail = Some("operator_discarded".to_string());
                self.minimum_generation = self.minimum_generation.max(generation_floor);
            }
        }
        true
    }

    #[cfg(test)]
    fn status(&self, id: u64) -> Option<OutboundStatus> {
        self.message(id).map(|message| message.status)
    }
}

fn apply_outbound_observation(
    outbound: &mut OutboundQueue,
    result: &OutboundObservationResult,
    now: Instant,
) -> bool {
    let OutboundObservationResult::Available(observation) = result else {
        return false;
    };
    if !observation.complete {
        return false;
    }
    let mut dirty = false;
    let sent_ids: Vec<u64> = outbound
        .messages
        .iter()
        .filter(|message| {
            matches!(
                message.status,
                OutboundStatus::Sent | OutboundStatus::Ambiguous
            )
        })
        .map(|message| message.id)
        .collect();
    for id in sent_ids {
        dirty |= apply_outbound_observation_to_message(outbound, id, observation, now);
    }
    dirty
}

fn apply_outbound_observation_to_message(
    outbound: &mut OutboundQueue,
    id: u64,
    observation: &OutboundObservation,
    now: Instant,
) -> bool {
    let Some(message) = outbound.message(id).cloned() else {
        return false;
    };
    let Some(baseline) = message.baseline.as_ref() else {
        return false;
    };
    if baseline.identity != observation.identity || observation.generation < baseline.generation {
        return false;
    }
    let candidates = candidate_turns_after_baseline(observation, baseline);
    if candidates.is_empty() {
        return false;
    }
    let matches = exact_matching_turn_count(&message.body, candidates.iter().copied());
    match matches {
        1 => outbound.set_status(id, OutboundStatus::Consumed, now, None),
        0 => {
            let detail = if candidates.iter().any(|turn| turn.body.is_none()) {
                "new_user_turn_unmatchable"
            } else {
                "new_user_turn_did_not_match"
            };
            outbound.set_status(id, OutboundStatus::Ambiguous, now, Some(detail.to_string()))
        }
        _ => outbound.set_status(
            id,
            OutboundStatus::Ambiguous,
            now,
            Some("duplicate_matching_user_turns".to_string()),
        ),
    }
}

fn candidate_turns_after_baseline<'a>(
    observation: &'a OutboundObservation,
    baseline: &OutboundBaseline,
) -> Vec<&'a ObservedUserTurn> {
    observation
        .user_turns
        .iter()
        .filter(|turn| !baseline.turn_ids.contains(&turn.turn_id))
        .collect()
}

fn exact_matching_turn_count<'a>(
    body: &str,
    turns: impl Iterator<Item = &'a ObservedUserTurn>,
) -> usize {
    let wanted = normalize_message_body(body);
    turns
        .filter(|turn| {
            turn.body
                .as_deref()
                .is_some_and(|body| normalize_message_body(body) == wanted)
        })
        .count()
}

fn normalize_message_body(body: &str) -> String {
    body.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorViewMode {
    Flat,
    Tree,
}

impl MonitorViewMode {
    fn toggled(self) -> Self {
        match self {
            Self::Flat => Self::Tree,
            Self::Tree => Self::Flat,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorFilterCategory {
    Bash,
    Procs,
    Mailbox,
    All,
}

impl MonitorFilterCategory {
    const ALL: [Self; 4] = [Self::Bash, Self::Procs, Self::Mailbox, Self::All];

    fn label(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Procs => "procs",
            Self::Mailbox => "mailbox",
            Self::All => "all",
        }
    }

    fn next(self) -> Self {
        self.offset(1)
    }

    fn prev(self) -> Self {
        self.offset(Self::ALL.len() - 1)
    }

    fn offset(self, delta: usize) -> Self {
        let current = Self::ALL
            .iter()
            .position(|category| *category == self)
            .unwrap_or(0);
        Self::ALL[(current + delta) % Self::ALL.len()]
    }
}

/// Bottom-pane monitor state: collapse/expand, the latest read-only snapshot, and
/// the current selection. Holds no terminal or IO handles.
#[derive(Clone)]
struct MonitorPane {
    collapsed: bool,
    view_mode: MonitorViewMode,
    active_filter: MonitorFilterCategory,
    selected: usize,
    selected_node_id: Option<MonitorNodeId>,
    snapshot: Option<Arc<MonitorSnapshot>>,
    pseudo_input: PseudoInputState,
    outbound: OutboundQueue,
    /// Manual detail override for nodes without an InspectRef. Nodes with an
    /// InspectRef show detail automatically while selected in the expanded pane.
    inspecting: bool,
    closed_detail_node_id: Option<MonitorNodeId>,
    inspect: Vec<String>,
    last_inspect_refresh: Option<Instant>,
    /// The node id awaiting cancel confirmation, if the operator pressed `x`.
    pending_cancel: Option<MonitorNodeId>,
    /// A cancel request the operator confirmed, drained and executed by the loop.
    cancel_request: Option<CancelRequest>,
    /// The last cancel outcome message, surfaced to the operator.
    cancel_feedback: Option<String>,
    pending_outbound_recovery: Option<PendingOutboundRecovery>,
    outbound_recovery_request: Option<PendingOutboundRecovery>,
    outbound_recovery_feedback: Option<String>,
}

impl MonitorPane {
    fn new() -> Self {
        Self {
            collapsed: true,
            view_mode: MonitorViewMode::Flat,
            active_filter: MonitorFilterCategory::Bash,
            selected: 0,
            selected_node_id: None,
            snapshot: None,
            pseudo_input: PseudoInputState::default(),
            outbound: OutboundQueue::default(),
            inspecting: false,
            closed_detail_node_id: None,
            inspect: Vec::new(),
            last_inspect_refresh: None,
            pending_cancel: None,
            cancel_request: None,
            cancel_feedback: None,
            pending_outbound_recovery: None,
            outbound_recovery_request: None,
            outbound_recovery_feedback: None,
        }
    }

    /// Rows the monitor occupies at the bottom for the given full terminal height.
    fn bottom_rows(&self, full_rows: u16, full_cols: u16, protection: TypingProtection) -> u16 {
        let desired_input_rows = self.pseudo_input.desired_rows(full_cols);
        let desired_input_bottom = STATUS_ROW_ROWS + desired_input_rows;
        let protected_ceiling = full_rows.saturating_sub(protection.top_min_rows());
        let minimum_bottom = COLLAPSED_MONITOR_ROWS.min(full_rows);
        let ceiling = if protected_ceiling >= minimum_bottom {
            protected_ceiling
        } else {
            minimum_bottom
        };
        if self.collapsed {
            return desired_input_bottom.min(ceiling).min(full_rows);
        }
        let target = (u32::from(full_rows) * 35 / 100) as u16;
        target
            .max(EXPANDED_MIN_ROWS)
            .max(desired_input_bottom)
            .min(ceiling)
            .min(full_rows)
    }

    fn expand(&mut self) {
        self.collapsed = false;
        self.update_inspect();
    }

    fn refresh_interval(&self) -> Duration {
        if self.collapsed {
            COLLAPSED_REFRESH
        } else {
            EXPANDED_REFRESH
        }
    }

    fn adopt_snapshot(&mut self, snapshot: Option<Arc<MonitorSnapshot>>) -> bool {
        let Some(snapshot) = snapshot else {
            return false;
        };
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &snapshot))
        {
            return false;
        }
        self.store_snapshot(snapshot);
        true
    }

    fn store_snapshot(&mut self, snapshot: Arc<MonitorSnapshot>) {
        self.preserve_or_clamp_selection(&snapshot);
        self.snapshot = Some(snapshot);
        self.clamp_selection_to_filter();
        self.sync_selected_node_id();
        self.update_inspect();
    }

    fn node_count(&self) -> usize {
        self.snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.nodes.len())
    }

    fn preserve_or_clamp_selection(&mut self, snapshot: &MonitorSnapshot) {
        let current_id = self
            .selected_node_id
            .as_deref()
            .or_else(|| self.selected_node().map(|node| node.id.as_str()));
        self.selected = selection_index_for_snapshot(snapshot, current_id, self.selected);
    }

    fn select_next(&mut self) {
        let Some(next) = self.adjacent_selection(1) else {
            return;
        };
        self.selected = next;
        self.sync_selected_node_id();
    }

    fn select_prev(&mut self) {
        let Some(prev) = self.adjacent_selection(-1) else {
            return;
        };
        self.selected = prev;
        self.sync_selected_node_id();
    }

    fn adjacent_selection(&self, delta: isize) -> Option<usize> {
        let snapshot = self.snapshot.as_ref()?;
        let rows = self.projected_rows(snapshot);
        let current = selected_projected_position(&rows, self.selected).unwrap_or(0);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta as usize)
                .min(rows.len().saturating_sub(1))
        };
        rows.get(next).map(|row| row.index)
    }

    fn select_index(&mut self, index: usize) {
        if index < self.node_count() {
            let changed = self.selected != index;
            self.selected = index;
            if changed {
                self.closed_detail_node_id = None;
            }
            self.sync_selected_node_id();
        }
    }

    fn toggle_select_index(&mut self, index: usize) {
        if index >= self.node_count() {
            return;
        }
        if self.selected == index && self.detail_visible() {
            self.closed_detail_node_id = self.selected_node().map(node_id);
            self.inspecting = false;
            self.update_inspect();
            return;
        }
        self.closed_detail_node_id = None;
        self.select_index(index);
        self.update_inspect();
    }

    fn toggle_view_mode(&mut self) {
        self.view_mode = self.view_mode.toggled();
        self.clamp_selection_to_filter();
        self.sync_selected_node_id();
    }

    fn set_filter(&mut self, filter: MonitorFilterCategory) {
        if self.active_filter == filter {
            return;
        }
        self.active_filter = filter;
        self.closed_detail_node_id = None;
        self.clamp_selection_to_filter();
        self.sync_selected_node_id();
        self.update_inspect();
    }

    fn cycle_filter_next(&mut self) {
        self.set_filter(self.active_filter.next());
    }

    fn cycle_filter_prev(&mut self) {
        self.set_filter(self.active_filter.prev());
    }

    fn clamp_selection_to_filter(&mut self) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let rows = self.projected_rows(snapshot);
        if rows.is_empty() || rows.iter().any(|row| row.index == self.selected) {
            return;
        }
        self.selected = rows[0].index;
    }

    fn projected_rows(&self, snapshot: &MonitorSnapshot) -> Vec<ProjectedMonitorRow> {
        filtered_monitor_rows(snapshot, self.view_mode, self.active_filter)
    }

    /// Apply a decoded monitor command. Returns whether a forced refresh is wanted.
    fn apply(&mut self, command: MonitorCommand) -> bool {
        match command {
            MonitorCommand::SelectNext => {
                self.select_next();
                self.update_inspect();
                false
            }
            MonitorCommand::SelectPrev => {
                self.select_prev();
                self.update_inspect();
                false
            }
            MonitorCommand::ToggleSelectIndex(index) => {
                self.toggle_select_index(index);
                false
            }
            MonitorCommand::SelectFilter(filter) => {
                self.set_filter(filter);
                false
            }
            MonitorCommand::NextFilter => {
                self.cycle_filter_next();
                false
            }
            MonitorCommand::PrevFilter => {
                self.cycle_filter_prev();
                false
            }
            MonitorCommand::ToggleTreeMode => {
                self.toggle_view_mode();
                false
            }
            MonitorCommand::Refresh => true,
            MonitorCommand::Collapse => {
                self.collapse_list();
                false
            }
            MonitorCommand::ToggleList => {
                if self.collapsed {
                    self.expand();
                } else {
                    self.collapse_list();
                }
                false
            }
            MonitorCommand::ToggleInspect => {
                if self.detail_visible() {
                    self.closed_detail_node_id = self.selected_node().map(node_id);
                    self.inspecting = false;
                } else {
                    self.closed_detail_node_id = None;
                    self.inspecting = true;
                }
                self.update_inspect();
                false
            }
            MonitorCommand::RequestCancel => {
                self.request_cancel();
                false
            }
            MonitorCommand::RequestOutboundRetry => {
                self.request_outbound_recovery(OutboundRecoveryAction::Retry);
                false
            }
            MonitorCommand::RequestOutboundDiscard => {
                self.request_outbound_recovery(OutboundRecoveryAction::Discard);
                false
            }
            MonitorCommand::ConfirmAction => {
                self.confirm_action();
                false
            }
            MonitorCommand::AbortAction => {
                self.abort_action();
                false
            }
        }
    }

    fn apply_pseudo_input(&mut self, action: PseudoInputAction, width: u16) {
        if let Some(body) = self.pseudo_input.apply(action, width) {
            self.outbound.enqueue(body);
        }
    }

    fn collapse_list(&mut self) {
        self.collapsed = true;
        self.inspecting = false;
        self.inspect.clear();
        self.last_inspect_refresh = None;
        self.pending_cancel = None;
        self.pending_outbound_recovery = None;
    }

    fn selected_node(&self) -> Option<&MonitorNode> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.nodes.get(self.selected))
    }

    fn sync_selected_node_id(&mut self) {
        self.selected_node_id = self.selected_node().map(node_id);
    }

    /// Arm a cancel for the selected node, only when it exposes a cancel ref.
    fn request_cancel(&mut self) {
        self.cancel_feedback = None;
        self.pending_outbound_recovery = None;
        self.pending_cancel = self.selected_cancelable_node().map(node_id);
    }

    fn request_outbound_recovery(&mut self, action: OutboundRecoveryAction) {
        self.outbound_recovery_feedback = None;
        self.pending_cancel = None;
        self.pending_outbound_recovery = self
            .outbound
            .oldest_ambiguous_id()
            .map(|message_id| PendingOutboundRecovery { message_id, action });
    }

    /// The selected node, but only when it exposes a cancel ref.
    fn selected_cancelable_node(&self) -> Option<&MonitorNode> {
        self.selected_node().filter(|node| node_is_cancelable(node))
    }

    /// Confirm a previously armed cancel, producing a request for the loop to
    /// execute, but only if the same node is still selected.
    fn confirm_cancel(&mut self) {
        let Some(pending) = self.pending_cancel.take() else {
            return;
        };
        self.cancel_request = self
            .selected_node_with_id(&pending)
            .and_then(cancel_request_for_node);
    }

    fn confirm_action(&mut self) {
        if let Some(pending) = self.pending_outbound_recovery.take() {
            if self.outbound.oldest_ambiguous_id() == Some(pending.message_id) {
                self.outbound_recovery_request = Some(pending);
            } else {
                self.outbound_recovery_feedback = Some(format!(
                    "message #{} is no longer ambiguous",
                    pending.message_id
                ));
            }
            return;
        }
        self.confirm_cancel();
    }

    /// The selected node, but only when its id matches.
    fn selected_node_with_id(&self, id: &str) -> Option<&MonitorNode> {
        self.selected_node().filter(|node| node.id == id)
    }

    fn abort_action(&mut self) {
        self.pending_cancel = None;
        self.pending_outbound_recovery = None;
    }

    fn take_cancel_request(&mut self) -> Option<CancelRequest> {
        self.cancel_request.take()
    }

    fn take_outbound_recovery_request(&mut self) -> Option<PendingOutboundRecovery> {
        self.outbound_recovery_request.take()
    }

    fn record_outbound_recovery_feedback(&mut self, message: String) {
        self.outbound_recovery_feedback = Some(message);
        self.pending_outbound_recovery = None;
    }

    fn record_cancel_feedback(&mut self, message: String) {
        self.cancel_feedback = Some(message);
        self.pending_cancel = None;
    }

    /// Refresh the inspect buffer from the selected node's live source plus any
    /// diagnostics scoped to it, while the inspect pane is open; clear it otherwise.
    fn update_inspect(&mut self) {
        self.inspect = if self.detail_visible() {
            self.build_inspect_content()
        } else {
            Vec::new()
        };
        self.last_inspect_refresh = self.detail_refresh_active().then_some(Instant::now());
    }

    /// Refresh only the selected node's bounded detail source. This runs from the
    /// relay loop, not the render thread, and never requests a fresh snapshot.
    fn refresh_detail_if_due(&mut self, now: Instant) -> bool {
        if !self.detail_refresh_active() || !detail_refresh_due(self.last_inspect_refresh, now) {
            return false;
        }
        let previous = self.inspect.clone();
        self.inspect = self.build_inspect_content();
        self.last_inspect_refresh = Some(now);
        self.inspect != previous
    }

    fn detail_visible(&self) -> bool {
        !self.collapsed
            && !self.selected_detail_closed()
            && (self.inspecting || self.selected_node_has_inspect_ref())
    }

    fn selected_detail_closed(&self) -> bool {
        self.closed_detail_node_id
            .as_deref()
            .is_some_and(|id| self.selected_node_id.as_deref() == Some(id))
    }

    fn detail_refresh_active(&self) -> bool {
        self.detail_visible() && self.selected_node_has_inspect_ref()
    }

    fn selected_node_has_inspect_ref(&self) -> bool {
        self.selected_node().is_some_and(node_has_inspect_ref)
    }

    /// The selected node's inspect content followed by its scoped diagnostics.
    fn build_inspect_content(&self) -> Vec<String> {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Vec::new();
        };
        let Some(node) = snapshot.nodes.get(self.selected) else {
            return Vec::new();
        };
        let mut lines = read_inspect_content(node);
        lines.extend(node_diagnostic_lines(&node.id, &snapshot.diagnostics));
        lines
    }
}

/// Render the diagnostics scoped to a node as a labelled block; empty when none.
fn node_diagnostic_lines(node_id: &str, diagnostics: &[MonitorDiagnostic]) -> Vec<String> {
    let matching = diagnostics_for_node(node_id, diagnostics);
    if matching.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["— diagnostics —".to_string()];
    lines.extend(matching.into_iter().map(format_diagnostic_line));
    lines
}

fn diagnostics_for_node<'a>(
    node_id: &str,
    diagnostics: &'a [MonitorDiagnostic],
) -> Vec<&'a MonitorDiagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.node_id.as_deref() == Some(node_id))
        .collect()
}

fn format_diagnostic_line(diagnostic: &MonitorDiagnostic) -> String {
    format!(
        "{} {}: {}",
        diagnostic_severity_glyph(diagnostic.severity),
        diagnostic.code,
        diagnostic.message
    )
}

fn diagnostic_severity_glyph(severity: MonitorDiagnosticSeverity) -> &'static str {
    match severity {
        MonitorDiagnosticSeverity::Error => "✗",
        MonitorDiagnosticSeverity::Warning => "⚠",
        MonitorDiagnosticSeverity::Info => "ℹ",
    }
}

fn node_id(node: &MonitorNode) -> MonitorNodeId {
    node.id.clone()
}

fn node_has_inspect_ref(node: &MonitorNode) -> bool {
    node.inspect_ref.is_some()
}

fn selection_index_for_snapshot(
    snapshot: &MonitorSnapshot,
    selected_id: Option<&str>,
    fallback_index: usize,
) -> usize {
    selected_id
        .and_then(|id| node_index_by_id(snapshot, id))
        .unwrap_or_else(|| clamp_selection_index(snapshot_node_count(snapshot), fallback_index))
}

fn node_index_by_id(snapshot: &MonitorSnapshot, id: &str) -> Option<usize> {
    snapshot.nodes.iter().position(|node| node.id == id)
}

fn clamp_selection_index(len: usize, index: usize) -> usize {
    if len == 0 { 0 } else { index.min(len - 1) }
}

fn detail_refresh_due(last_refresh: Option<Instant>, now: Instant) -> bool {
    last_refresh.is_none_or(|last| now.duration_since(last) >= DETAIL_REFRESH)
}

fn node_is_cancelable(node: &MonitorNode) -> bool {
    node.cancel_ref.is_some()
}

/// Build the inspect-pane content for a node: an identity header plus its live
/// source — the bounded tail of an agent-bash log, or the node's last output.
fn read_inspect_content(node: &MonitorNode) -> Vec<String> {
    let mut lines = inspect_identity_lines(node);
    lines.extend(inspect_content_lines(inspect_content_source(node)));
    lines
}

fn inspect_identity_lines(node: &MonitorNode) -> Vec<String> {
    assemble_inspect_identity_lines(
        inspect_title_line(node),
        inspect_pid_values(node).map(format_inspect_pid_line),
        inspect_started_at(node).map(format_inspect_started_line),
    )
}

fn assemble_inspect_identity_lines(
    title: String,
    pid_line: Option<String>,
    started_line: Option<String>,
) -> Vec<String> {
    let mut lines = vec![title];
    lines.extend(pid_line);
    lines.extend(started_line);
    lines
}

fn inspect_title_line(node: &MonitorNode) -> String {
    format!(
        "{} [{}] {}",
        kind_word(node.kind),
        status_word(node.status),
        node.label
    )
}

fn inspect_pid_values(node: &MonitorNode) -> Option<(i64, Option<i64>)> {
    node.pid.map(|pid| (pid, node.pgid))
}

fn format_inspect_pid_line((pid, pgid): (i64, Option<i64>)) -> String {
    let pgid = pgid.map(format_inspect_pgid_suffix).unwrap_or_default();
    format!("pid={pid}{pgid}")
}

fn format_inspect_pgid_suffix(pgid: i64) -> String {
    format!(" pgid={pgid}")
}

fn inspect_started_at(node: &MonitorNode) -> Option<&str> {
    node.started_at.as_deref()
}

fn format_inspect_started_line(started: &str) -> String {
    format!("started {started}")
}

fn inspect_content_source(node: &MonitorNode) -> InspectContentSource<'_> {
    match node.inspect_ref.as_ref() {
        Some(InspectRef::AgentBashLog {
            path,
            max_tail_bytes,
        }) => InspectContentSource::AgentBashLog {
            path,
            max_tail_bytes: *max_tail_bytes,
        },
        Some(InspectRef::SessionTranscript {
            path,
            max_tail_bytes,
            format_id,
            ..
        }) => InspectContentSource::SessionTranscript {
            path,
            max_tail_bytes: *max_tail_bytes,
            format_id: format_id.as_deref(),
        },
        _ => inspect_excerpt_source(node),
    }
}

fn inspect_excerpt_source(node: &MonitorNode) -> InspectContentSource<'_> {
    node.last_output_excerpt
        .as_deref()
        .map(InspectContentSource::LastOutput)
        .unwrap_or(InspectContentSource::None)
}

fn inspect_content_lines(source: InspectContentSource<'_>) -> Vec<String> {
    match source {
        InspectContentSource::AgentBashLog {
            path,
            max_tail_bytes,
        } => inspect_log_content_lines(path, max_tail_bytes),
        InspectContentSource::SessionTranscript {
            path,
            max_tail_bytes,
            format_id,
        } => inspect_transcript_content_lines(path, max_tail_bytes, format_id),
        InspectContentSource::LastOutput(excerpt) => inspect_excerpt_content_lines(excerpt),
        InspectContentSource::None => Vec::new(),
    }
}

fn inspect_log_content_lines(path: &str, max_tail_bytes: usize) -> Vec<String> {
    let mut lines = vec![format_inspect_log_header(path)];
    lines.extend(tail_file(path, max_tail_bytes));
    lines
}

/// Project the transcript tail into readable conversation lines, falling back to
/// the raw tail when nothing projects (e.g. a single oversized message).
fn inspect_transcript_content_lines(
    path: &str,
    max_tail_bytes: usize,
    format_id: Option<&str>,
) -> Vec<String> {
    let raw = transcript_tail_lines(path, max_tail_bytes);
    let projected = transcript_projected_lines(format_id, &raw);
    format_transcript_inspect_lines(path, &projected, &raw)
}

fn transcript_projected_lines(format_id: Option<&str>, raw: &[String]) -> Vec<String> {
    match format_id {
        None => transcript_display_lines(raw),
        Some("provider-inspect-transcript-v1" | "canonical-transcript-v1") => Vec::new(),
        Some(_) => Vec::new(),
    }
}

fn transcript_tail_lines(path: &str, max_tail_bytes: usize) -> Vec<String> {
    tail_file(path, max_tail_bytes)
}

fn transcript_display_lines(raw: &[String]) -> Vec<String> {
    project_transcript_tail(raw)
}

fn format_transcript_inspect_lines(
    path: &str,
    projected: &[String],
    raw: &[String],
) -> Vec<String> {
    let mut lines = vec![format_inspect_transcript_header(path)];
    lines.extend(
        selected_transcript_display_lines(projected, raw)
            .iter()
            .cloned(),
    );
    lines
}

fn selected_transcript_display_lines<'a>(
    projected: &'a [String],
    raw: &'a [String],
) -> &'a [String] {
    if projected.is_empty() { raw } else { projected }
}

fn format_inspect_log_header(path: &str) -> String {
    format!("— log {path} —")
}

fn format_inspect_transcript_header(path: &str) -> String {
    format!("— session {path} —")
}

fn inspect_excerpt_content_lines(excerpt: &str) -> Vec<String> {
    vec![excerpt.to_string()]
}

/// Read the last `max_bytes` of a file as UTF-8-lossy lines (bounded; never reads
/// the whole file). Read errors degrade to a single explanatory line.
fn tail_file(path: &str, max_bytes: usize) -> Vec<String> {
    match read_tail_bytes(path, max_bytes) {
        Ok(buffer) => tail_bytes_to_lines(&buffer),
        Err(err) => vec![format_tail_file_read_error(err)],
    }
}

fn read_tail_bytes(path: &str, max_bytes: usize) -> Result<Vec<u8>, TailFileReadError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(TailFileReadError::Open)?;
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Err(TailFileReadError::Seek);
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return Err(TailFileReadError::Read);
    }
    Ok(buffer)
}

fn tail_bytes_to_lines(buffer: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(buffer)
        .lines()
        .map(str::to_string)
        .collect()
}

fn format_tail_file_read_error(err: TailFileReadError) -> String {
    match err {
        TailFileReadError::Open(err) => format!("(cannot read log: {err})"),
        TailFileReadError::Seek => "(log seek failed)".to_string(),
        TailFileReadError::Read => "(log read failed)".to_string(),
    }
}

fn snapshot_node_count(snapshot: &MonitorSnapshot) -> usize {
    snapshot.nodes.len()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectedMonitorRow {
    index: usize,
    prefix: String,
}

fn projected_monitor_rows(
    snapshot: &MonitorSnapshot,
    mode: MonitorViewMode,
) -> Vec<ProjectedMonitorRow> {
    match mode {
        MonitorViewMode::Flat => flat_monitor_rows(snapshot),
        MonitorViewMode::Tree => tree_monitor_rows(snapshot),
    }
}

fn filtered_monitor_rows(
    snapshot: &MonitorSnapshot,
    mode: MonitorViewMode,
    filter: MonitorFilterCategory,
) -> Vec<ProjectedMonitorRow> {
    projected_monitor_rows(snapshot, mode)
        .into_iter()
        .filter(|row| {
            snapshot
                .nodes
                .get(row.index)
                .is_some_and(|node| node_matches_filter(node, filter))
        })
        .collect()
}

fn node_matches_filter(node: &MonitorNode, filter: MonitorFilterCategory) -> bool {
    match filter {
        MonitorFilterCategory::Bash => {
            node.kind == MonitorNodeKind::AgentBashWorkload
                && matches!(
                    node.status,
                    MonitorStatus::Running | MonitorStatus::Cancelling
                )
        }
        MonitorFilterCategory::Procs => matches!(
            node.kind,
            MonitorNodeKind::Session
                | MonitorNodeKind::Invocation
                | MonitorNodeKind::ProviderProcess
        ),
        MonitorFilterCategory::Mailbox => matches!(
            node.kind,
            MonitorNodeKind::MailboxNotification | MonitorNodeKind::WakeClaim
        ),
        MonitorFilterCategory::All => true,
    }
}

fn flat_monitor_rows(snapshot: &MonitorSnapshot) -> Vec<ProjectedMonitorRow> {
    (0..snapshot.nodes.len())
        .map(|index| ProjectedMonitorRow {
            index,
            prefix: String::new(),
        })
        .collect()
}

fn tree_monitor_rows(snapshot: &MonitorSnapshot) -> Vec<ProjectedMonitorRow> {
    let index_by_id = monitor_index_by_id(snapshot);
    let children = monitor_children_by_parent(snapshot, &index_by_id);
    let roots = monitor_tree_roots(snapshot, &index_by_id);
    let mut rows = Vec::with_capacity(snapshot.nodes.len());
    let mut emitted = HashSet::new();
    for (position, root) in roots.iter().copied().enumerate() {
        push_tree_row(
            root,
            position + 1 == roots.len(),
            &[],
            &children,
            &mut emitted,
            &mut rows,
        );
    }
    for index in 0..snapshot.nodes.len() {
        if emitted.contains(&index) {
            continue;
        }
        push_tree_row(index, true, &[], &children, &mut emitted, &mut rows);
    }
    rows
}

fn monitor_index_by_id(snapshot: &MonitorSnapshot) -> HashMap<&str, usize> {
    let mut index_by_id = HashMap::new();
    for (index, node) in snapshot.nodes.iter().enumerate() {
        index_by_id.entry(node.id.as_str()).or_insert(index);
    }
    index_by_id
}

fn monitor_children_by_parent(
    snapshot: &MonitorSnapshot,
    index_by_id: &HashMap<&str, usize>,
) -> HashMap<usize, Vec<usize>> {
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    for (index, node) in snapshot.nodes.iter().enumerate() {
        let Some(parent) = node.parent_id.as_deref() else {
            continue;
        };
        if let Some(parent_index) = index_by_id.get(parent).copied() {
            children.entry(parent_index).or_default().push(index);
        }
    }
    children
}

fn monitor_tree_roots(
    snapshot: &MonitorSnapshot,
    index_by_id: &HashMap<&str, usize>,
) -> Vec<usize> {
    snapshot
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| monitor_node_is_root(node, index_by_id).then_some(index))
        .collect()
}

fn monitor_node_is_root(node: &MonitorNode, index_by_id: &HashMap<&str, usize>) -> bool {
    node.parent_id
        .as_deref()
        .is_none_or(|parent| !index_by_id.contains_key(parent))
}

fn push_tree_row(
    index: usize,
    is_last: bool,
    ancestor_last: &[bool],
    children: &HashMap<usize, Vec<usize>>,
    emitted: &mut HashSet<usize>,
    rows: &mut Vec<ProjectedMonitorRow>,
) {
    if !emitted.insert(index) {
        return;
    }
    rows.push(ProjectedMonitorRow {
        index,
        prefix: tree_prefix(ancestor_last, is_last),
    });
    let Some(child_indexes) = children.get(&index) else {
        return;
    };
    let mut child_ancestors = ancestor_last.to_vec();
    child_ancestors.push(is_last);
    for (position, child) in child_indexes.iter().copied().enumerate() {
        push_tree_row(
            child,
            position + 1 == child_indexes.len(),
            &child_ancestors,
            children,
            emitted,
            rows,
        );
    }
}

fn tree_prefix(ancestor_last: &[bool], is_last: bool) -> String {
    if ancestor_last.is_empty() {
        return String::new();
    }
    let mut prefix = String::new();
    for last in &ancestor_last[..ancestor_last.len() - 1] {
        prefix.push_str(if *last { "   " } else { "│  " });
    }
    prefix.push_str(if is_last { "└─ " } else { "├─ " });
    prefix
}

fn selected_projected_position(rows: &[ProjectedMonitorRow], selected: usize) -> Option<usize> {
    rows.iter().position(|row| row.index == selected)
}

/// Map a `vt100` colour to a `ratatui` colour.
fn vt_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(idx) => Color::Indexed(idx),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Map a `vt100` cell's attributes to a `ratatui` modifier set.
fn vt_modifier(cell: &vt100::Cell) -> Modifier {
    let mut modifier = Modifier::empty();
    if cell.bold() {
        modifier |= Modifier::BOLD;
    }
    if cell.italic() {
        modifier |= Modifier::ITALIC;
    }
    if cell.underline() {
        modifier |= Modifier::UNDERLINED;
    }
    if cell.inverse() {
        modifier |= Modifier::REVERSED;
    }
    modifier
}

/// Map a `vt100` cell (or a blank) to its rendered symbol and `ratatui` style.
fn vt_cell_render(cell: Option<&vt100::Cell>) -> (&str, Style) {
    match cell {
        Some(cell) => {
            let contents = cell.contents();
            let symbol = if contents.is_empty() { " " } else { contents };
            let style = Style::default()
                .fg(vt_color(cell.fgcolor()))
                .bg(vt_color(cell.bgcolor()))
                .add_modifier(vt_modifier(cell));
            (symbol, style)
        }
        None => (" ", Style::default()),
    }
}

#[derive(Clone)]
struct RenderCellSnapshot {
    symbol: String,
    style: Style,
}

#[derive(Clone)]
struct ScreenRenderSnapshot {
    rows: u16,
    cols: u16,
    cells: Arc<[RenderCellSnapshot]>,
    scrollback: usize,
    hide_cursor: bool,
    cursor_position: (u16, u16),
}

impl ScreenRenderSnapshot {
    fn from_screen(screen: &vt100::Screen) -> Self {
        let (rows, cols) = screen.size();
        let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(cols));
        for row in 0..rows {
            for col in 0..cols {
                cells.push(render_cell_snapshot(screen.cell(row, col)));
            }
        }
        Self {
            rows,
            cols,
            cells: Arc::from(cells.into_boxed_slice()),
            scrollback: screen.scrollback(),
            hide_cursor: screen.hide_cursor(),
            cursor_position: screen.cursor_position(),
        }
    }

    fn cell(&self, row: u16, col: u16) -> Option<&RenderCellSnapshot> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let index = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.cells.get(index)
    }
}

fn render_cell_snapshot(cell: Option<&vt100::Cell>) -> RenderCellSnapshot {
    let (symbol, style) = vt_cell_render(cell);
    RenderCellSnapshot {
        symbol: symbol.to_string(),
        style,
    }
}

#[derive(Clone)]
struct RenderSnapshot {
    screen: ScreenRenderSnapshot,
    focus: Focus,
    pane: MonitorPane,
    selection: Option<SelectionSpan>,
    mouse_request: MouseRequest,
    bracketed_paste: bool,
    typing_protection: TypingProtection,
    priority: RenderPriority,
}

impl RenderSnapshot {
    #[cfg(test)]
    fn capture(
        screen: &vt100::Screen,
        focus: Focus,
        pane: &MonitorPane,
        selection: Option<SelectionSpan>,
    ) -> Self {
        Self::capture_with_typing_protection(
            screen,
            focus,
            pane,
            selection,
            TypingProtection::for_focus(focus),
        )
    }

    #[cfg(test)]
    fn capture_with_typing_protection(
        screen: &vt100::Screen,
        focus: Focus,
        pane: &MonitorPane,
        selection: Option<SelectionSpan>,
        typing_protection: TypingProtection,
    ) -> Self {
        Self::capture_with_typing_protection_and_priority(
            screen,
            focus,
            pane,
            selection,
            typing_protection,
            RenderPriority::Background,
        )
    }

    fn capture_with_typing_protection_and_priority(
        screen: &vt100::Screen,
        focus: Focus,
        pane: &MonitorPane,
        selection: Option<SelectionSpan>,
        typing_protection: TypingProtection,
        priority: RenderPriority,
    ) -> Self {
        Self {
            screen: ScreenRenderSnapshot::from_screen(screen),
            focus,
            pane: pane.clone(),
            selection,
            mouse_request: effective_mouse_request(mouse_request_from_screen(screen)),
            bracketed_paste: focus == Focus::Top && screen.bracketed_paste(),
            typing_protection,
            priority,
        }
    }
}

/// Render the virtual terminal screen snapshot into the top-pane buffer cells.
fn render_screen_snapshot(
    buf: &mut Buffer,
    area: Rect,
    screen: &ScreenRenderSnapshot,
    selection: Option<SelectionSpan>,
) {
    for row in 0..area.height {
        for col in 0..area.width {
            let (symbol, mut style) = screen
                .cell(row, col)
                .map(|cell| (cell.symbol.as_str(), cell.style))
                .unwrap_or((" ", Style::default()));
            if selection.is_some_and(|span| cell_in_selection(span, row, col)) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            buf.set_string(area.x + col, area.y + row, symbol, style);
        }
    }
}

/// Split the screen into the interactive top pane and the monitor, and render both.
#[cfg(test)]
fn render_frame(
    frame: &mut ratatui::Frame,
    screen: &vt100::Screen,
    focus: Focus,
    pane: &MonitorPane,
    selection: Option<SelectionSpan>,
) {
    let snapshot = RenderSnapshot::capture(screen, focus, pane, selection);
    render_snapshot_frame(frame, &snapshot);
}

#[cfg(test)]
fn render_frame_with_typing_protection(
    frame: &mut ratatui::Frame,
    screen: &vt100::Screen,
    focus: Focus,
    pane: &MonitorPane,
    selection: Option<SelectionSpan>,
    typing_protection: TypingProtection,
) {
    let snapshot = RenderSnapshot::capture_with_typing_protection(
        screen,
        focus,
        pane,
        selection,
        typing_protection,
    );
    render_snapshot_frame(frame, &snapshot);
}

fn render_snapshot_frame(frame: &mut ratatui::Frame, snapshot: &RenderSnapshot) {
    let area = frame.area();
    let screen = &snapshot.screen;
    let bottom_rows =
        snapshot
            .pane
            .bottom_rows(area.height, area.width, snapshot.typing_protection);
    let overlay_constrained = overlay_constrained_for_typing(
        &snapshot.pane,
        area.height,
        area.width,
        snapshot.typing_protection,
    );
    let [top, bottom] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_rows)]).areas(area);
    render_screen_snapshot(frame.buffer_mut(), top, screen, snapshot.selection);
    render_scrollback_indicator(frame.buffer_mut(), top, screen.scrollback);
    render_monitor(
        frame.buffer_mut(),
        bottom,
        &snapshot.pane,
        snapshot.focus,
        overlay_constrained,
    );
    // Suppress the child cursor while scrolled back or selecting — it belongs to the live
    // tail, not the history/selection the operator is reading.
    if snapshot.focus == Focus::Top
        && !screen.hide_cursor
        && screen.scrollback == 0
        && snapshot.selection.is_none()
    {
        let (crow, ccol) = screen.cursor_position;
        if crow < top.height && ccol < top.width {
            frame.set_cursor_position(Position::new(top.x + ccol, top.y + crow));
        }
    }
}

fn overlay_constrained_for_typing(
    pane: &MonitorPane,
    full_rows: u16,
    full_cols: u16,
    protection: TypingProtection,
) -> bool {
    !pane.collapsed
        && protection.active
        && pane.bottom_rows(full_rows, full_cols, protection)
            < pane.bottom_rows(full_rows, full_cols, TypingProtection::inactive())
}

/// Badge in the top-right of the interactive pane while the operator is scrolled back,
/// so a frozen (non-live) view is unmistakable.
fn render_scrollback_indicator(buf: &mut Buffer, area: Rect, scrollback: usize) {
    if scrollback == 0 || area.width == 0 || area.height == 0 {
        return;
    }
    let label = format!(" SCROLLBACK -{scrollback} (type to resume) ");
    let width = (label.chars().count() as u16).min(area.width);
    let x = area.x + area.width - width;
    buf.set_string(
        x,
        area.y,
        label,
        Style::default()
            .fg(Color::Black)
            .bg(Color::Indexed(214))
            .add_modifier(Modifier::BOLD),
    );
}

/// Render the monitor: the always-visible pseudo input sits above the status row; the
/// node list is the collapsible portion below it.
fn render_monitor(
    buf: &mut Buffer,
    area: Rect,
    pane: &MonitorPane,
    focus: Focus,
    overlay_constrained: bool,
) {
    if area.height == 0 {
        return;
    }
    let layout = expanded_bottom_layout(area, pane);
    render_pseudo_input(buf, layout.input, pane, focus);
    render_status_row(buf, layout.status, pane, focus, overlay_constrained);
    if !pane.collapsed {
        render_monitor_body(buf, layout.content, pane);
    }
}

#[derive(Debug, Clone, Copy)]
struct ExpandedBottomLayout {
    status: Rect,
    content: Rect,
    input: Rect,
}

fn expanded_bottom_layout(area: Rect, pane: &MonitorPane) -> ExpandedBottomLayout {
    let status_rows = STATUS_ROW_ROWS.min(area.height);
    let input_rows = pane
        .pseudo_input
        .desired_rows(area.width)
        .min(area.height.saturating_sub(status_rows));
    let content_rows = area.height.saturating_sub(input_rows + status_rows);
    ExpandedBottomLayout {
        input: Rect {
            height: input_rows,
            ..area
        },
        status: Rect {
            y: area.y + input_rows,
            height: status_rows,
            ..area
        },
        content: Rect {
            y: area.y + input_rows + status_rows,
            height: content_rows,
            ..area
        },
    }
}

fn render_monitor_body(buf: &mut Buffer, body: Rect, pane: &MonitorPane) {
    if body.height == 0 {
        return;
    }
    if pane.detail_visible() && body.height >= 4 {
        let list_height = (body.height / 2).max(2);
        let list_area = Rect {
            height: list_height,
            ..body
        };
        let inspect_area = Rect {
            y: body.y + list_height,
            height: body.height - list_height,
            ..body
        };
        render_node_list(buf, list_area, pane);
        render_inspect_pane(buf, inspect_area, pane);
    } else {
        render_node_list(buf, body, pane);
    }
}

fn render_pseudo_input(buf: &mut Buffer, area: Rect, pane: &MonitorPane, focus: Focus) {
    if area.height == 0 {
        return;
    }
    let style = if focus == Focus::Bottom {
        Style::default().fg(Color::White).bg(Color::Indexed(235))
    } else {
        Style::default().fg(Color::Gray).bg(Color::Indexed(235))
    };
    let editor_rows = area.height;
    let display = pseudo_input_display_rows(
        &pane.pseudo_input.buffer,
        pane.pseudo_input.cursor,
        area.width,
    );
    let start =
        pseudo_input_scroll_start(display.cursor_row, display.rows.len(), editor_rows as usize);
    for (line_index, line) in format_pseudo_input_rows_from_display(&display, start, editor_rows)
        .into_iter()
        .enumerate()
    {
        buf.set_string(
            area.x,
            area.y + line_index as u16,
            pad_to_width(line, area.width),
            style,
        );
    }
    if focus == Focus::Bottom
        && area.width > 0
        && display.cursor_row >= start
        && display.cursor_row < start + editor_rows as usize
    {
        let cursor_x = area.x + (display.cursor_col as u16).min(area.width - 1);
        let cursor_y = area.y + (display.cursor_row - start) as u16;
        buf[(cursor_x, cursor_y)].set_style(style.add_modifier(Modifier::REVERSED));
    }
}

#[cfg(test)]
fn format_pseudo_input_rows(pane: &MonitorPane, visible_rows: u16, width: u16) -> Vec<String> {
    if visible_rows == 0 {
        return Vec::new();
    }
    let display =
        pseudo_input_display_rows(&pane.pseudo_input.buffer, pane.pseudo_input.cursor, width);
    let start = pseudo_input_scroll_start(
        display.cursor_row,
        display.rows.len(),
        visible_rows as usize,
    );
    format_pseudo_input_rows_from_display(&display, start, visible_rows)
}

fn format_pseudo_input_rows_from_display(
    display: &PseudoInputDisplayRows,
    start: usize,
    visible_rows: u16,
) -> Vec<String> {
    let end = (start + visible_rows as usize).min(display.rows.len());
    let has_above = start > 0;
    let has_below = end < display.rows.len();
    let mut rows: Vec<String> = display
        .rows
        .iter()
        .skip(start)
        .take(end - start)
        .cloned()
        .collect();
    if has_above && let Some(first) = rows.first_mut() {
        first.push_str(" ↑");
    }
    if has_below && let Some(last) = rows.last_mut() {
        last.push_str(" ↓");
    }
    rows.resize(visible_rows as usize, String::new());
    rows
}

fn desired_pseudo_input_rows(buffer: &str, cursor: usize, width: u16) -> u16 {
    desired_pseudo_input_editor_rows(buffer, cursor, width) + PSEUDO_INPUT_HELP_ROWS
}

fn desired_pseudo_input_editor_rows(buffer: &str, cursor: usize, width: u16) -> u16 {
    let count = pseudo_input_display_rows(buffer, cursor, width)
        .rows
        .len()
        .min(u16::MAX as usize) as u16;
    count.clamp(MIN_INPUT_ROWS, MAX_INPUT_ROWS)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PseudoInputDisplayRows {
    rows: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
}

fn pseudo_input_display_rows(buffer: &str, cursor: usize, width: u16) -> PseudoInputDisplayRows {
    let cursor = pseudo_input_cursor_line_col(buffer, cursor);
    let mut rows = Vec::new();
    let mut cursor_row = 0;
    let mut cursor_col = 0;
    for (line_index, line) in pseudo_input_logical_lines(buffer).into_iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        let mut segment_index = 0;
        loop {
            let row_index = rows.len();
            let prefix = pseudo_input_row_prefix(row_index, line_index, segment_index, cursor);
            let wrap_width = pseudo_input_wrap_width(width, &prefix);
            let end = if chars.is_empty() {
                0
            } else {
                (start + wrap_width).min(chars.len())
            };
            let is_last = end >= chars.len();
            let segment: String = chars[start..end].iter().collect();
            let cursor_at_full_line_end = !chars.is_empty()
                && is_last
                && cursor.0 == line_index
                && cursor.1 == end
                && cursor.1 == chars.len()
                && segment.chars().count() >= wrap_width;
            if pseudo_input_cursor_in_segment(cursor, line_index, start, end, is_last)
                && !cursor_at_full_line_end
            {
                cursor_col = cursor.1.saturating_sub(start).min(segment.chars().count());
                cursor_row = row_index;
            }
            rows.push(format!("{prefix}{segment}"));
            if cursor_at_full_line_end {
                cursor_col = 0;
                cursor_row = rows.len();
                rows.push(String::new());
            }
            if chars.is_empty() || is_last {
                break;
            }
            start = end;
            segment_index += 1;
        }
    }
    PseudoInputDisplayRows {
        rows,
        cursor_row,
        cursor_col,
    }
}

fn pseudo_input_logical_lines(buffer: &str) -> Vec<&str> {
    if buffer.is_empty() {
        vec![""]
    } else {
        buffer.split('\n').collect()
    }
}

fn pseudo_input_row_prefix(
    _row_index: usize,
    _line_index: usize,
    _segment_index: usize,
    _cursor: (usize, usize),
) -> String {
    String::new()
}

fn pseudo_input_wrap_width(width: u16, _prefix: &str) -> usize {
    (width as usize).max(1)
}

fn pseudo_input_cursor_in_segment(
    cursor: (usize, usize),
    line_index: usize,
    start: usize,
    end: usize,
    is_last: bool,
) -> bool {
    if cursor.0 != line_index || cursor.1 < start {
        return false;
    }
    if is_last {
        cursor.1 <= end
    } else {
        cursor.1 < end
    }
}

fn pseudo_input_scroll_start(cursor_line: usize, len: usize, visible_rows: usize) -> usize {
    if visible_rows == 0 || len <= visible_rows {
        0
    } else {
        cursor_line
            .saturating_sub(visible_rows - 1)
            .min(len - visible_rows)
    }
}

fn byte_index_for_char_col(value: &str, col: usize) -> usize {
    value
        .char_indices()
        .nth(col)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PseudoInputLogicalLineSpan {
    start_byte: usize,
    byte_len: usize,
    char_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PseudoInputVisualRow {
    line_index: usize,
    start_col: usize,
    end_col: usize,
}

fn pseudo_input_logical_line_spans(buffer: &str) -> Vec<PseudoInputLogicalLineSpan> {
    if buffer.is_empty() {
        return vec![PseudoInputLogicalLineSpan {
            start_byte: 0,
            byte_len: 0,
            char_len: 0,
        }];
    }
    let mut spans = Vec::new();
    let mut start_byte = 0;
    for line in buffer.split('\n') {
        spans.push(PseudoInputLogicalLineSpan {
            start_byte,
            byte_len: line.len(),
            char_len: line.chars().count(),
        });
        start_byte = start_byte.saturating_add(line.len()).saturating_add(1);
    }
    spans
}

fn pseudo_input_visual_rows(buffer: &str, width: u16) -> Vec<PseudoInputVisualRow> {
    let wrap_width = pseudo_input_wrap_width(width, "");
    let mut rows = Vec::new();
    for (line_index, line) in pseudo_input_logical_line_spans(buffer)
        .into_iter()
        .enumerate()
    {
        if line.char_len == 0 {
            rows.push(PseudoInputVisualRow {
                line_index,
                start_col: 0,
                end_col: 0,
            });
            continue;
        }
        let mut start_col = 0;
        while start_col < line.char_len {
            let end_col = (start_col + wrap_width).min(line.char_len);
            rows.push(PseudoInputVisualRow {
                line_index,
                start_col,
                end_col,
            });
            start_col = end_col;
        }
    }
    rows
}

fn pseudo_input_cursor_visual_position(
    buffer: &str,
    cursor: usize,
    width: u16,
    rows: &[PseudoInputVisualRow],
) -> Option<(usize, usize)> {
    let cursor = pseudo_input_cursor_line_col(buffer, cursor);
    let wrap_width = pseudo_input_wrap_width(width, "");
    for (index, row) in rows.iter().enumerate() {
        if row.line_index != cursor.0 || cursor.1 < row.start_col {
            continue;
        }
        let is_last_for_line = rows
            .get(index + 1)
            .is_none_or(|next| next.line_index != row.line_index);
        if cursor.1 < row.end_col || (is_last_for_line && cursor.1 <= row.end_col) {
            let col = cursor.1.saturating_sub(row.start_col).min(wrap_width);
            return Some((index, col));
        }
    }
    None
}

fn byte_index_for_line_col(buffer: &str, line_index: usize, col: usize) -> usize {
    let spans = pseudo_input_logical_line_spans(buffer);
    let Some(span) = spans.get(line_index) else {
        return buffer.len();
    };
    let line = &buffer[span.start_byte..span.start_byte + span.byte_len];
    span.start_byte + byte_index_for_char_col(line, col.min(span.char_len))
}

/// Render the inspect sub-pane: a header bar plus the selected node's bounded
/// live output (tail-anchored so the most recent lines stay visible).
fn render_inspect_pane(buf: &mut Buffer, area: Rect, pane: &MonitorPane) {
    if area.height == 0 {
        return;
    }
    buf.set_string(
        area.x,
        area.y,
        pad_to_width(" detail — selected live output ".to_string(), area.width),
        Style::default().fg(Color::Black).bg(Color::Indexed(244)),
    );
    let body = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    for (line_index, line) in visible_inspect_rows(pane, body.height).enumerate() {
        buf.set_string(
            body.x,
            body.y + line_index as u16,
            pad_to_width(line.clone(), body.width),
            Style::default().fg(Color::Gray),
        );
    }
}

fn visible_inspect_rows(pane: &MonitorPane, body_height: u16) -> impl Iterator<Item = &String> {
    let rows = body_height as usize;
    let skip = pane.inspect.len().saturating_sub(rows);
    pane.inspect.iter().skip(skip).take(rows)
}

/// The single status row (the whole monitor when collapsed; the header when open).
fn render_status_row(
    buf: &mut Buffer,
    area: Rect,
    pane: &MonitorPane,
    focus: Focus,
    overlay_constrained: bool,
) {
    let hint = status_hint(pane, focus, overlay_constrained);
    let summary = format!(
        " OBS  {} · {}",
        monitor_summary_text(pane),
        view_mode_word(pane.view_mode),
    );
    let label = if hint.is_empty() {
        pad_to_width(summary, area.width)
    } else {
        pad_to_width(format!("{summary}  —  {hint}"), area.width)
    };
    let style = Style::default()
        .fg(Color::White)
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD);
    buf.set_string(area.x, area.y, label, style);
}

/// The status-row hint, reflecting focus and any armed/last cancel state.
fn status_hint(pane: &MonitorPane, focus: Focus, overlay_constrained: bool) -> String {
    match focus {
        Focus::Top if overlay_constrained => {
            "input viewport protected · click input/bar".to_string()
        }
        Focus::Top => "click input/bar".to_string(),
        Focus::Bottom => bottom_status_hint(pane, overlay_constrained),
    }
}

fn bottom_status_hint(pane: &MonitorPane, overlay_constrained: bool) -> String {
    let protected_suffix = overlay_constrained.then_some("input viewport protected");
    if let Some(pending) = pane.pending_outbound_recovery {
        let action = match pending.action {
            OutboundRecoveryAction::Retry => {
                "retry may duplicate a prompt that was consumed but not observed"
            }
            OutboundRecoveryAction::Discard => "discard may drop a prompt that was never consumed",
        };
        let suffix = protected_suffix
            .map(|suffix| format!(" · {suffix}"))
            .unwrap_or_default();
        return format!(
            "confirm {action} for #{}: Ctrl+Y = confirm · Ctrl+N = abort{suffix}",
            pending.message_id
        );
    }
    if pane.pending_cancel.is_some() {
        let protected_suffix = protected_suffix
            .map(|suffix| format!(" · {suffix}"))
            .unwrap_or_default();
        return format!("confirm cancel: y = SIGTERM · n = abort{protected_suffix}");
    }
    if let Some(message_id) = pane.outbound.oldest_ambiguous_id() {
        let suffix = protected_suffix
            .map(|suffix| format!(" · {suffix}"))
            .unwrap_or_default();
        return format!("#{message_id} ambiguous: Ctrl+P = retry · Ctrl+D = discard{suffix}");
    }
    match pane
        .outbound_recovery_feedback
        .as_deref()
        .or(pane.cancel_feedback.as_deref())
    {
        Some(feedback) => match protected_suffix {
            Some(suffix) => format!("{feedback} · {suffix}"),
            None => feedback.to_string(),
        },
        None => protected_suffix.unwrap_or_default().to_string(),
    }
}

fn view_mode_word(mode: MonitorViewMode) -> &'static str {
    match mode {
        MonitorViewMode::Flat => "flat",
        MonitorViewMode::Tree => "tree",
    }
}

/// One-line summary of the latest snapshot for the status row.
fn monitor_summary_text(pane: &MonitorPane) -> String {
    match pane.snapshot.as_ref() {
        None => "starting…".to_string(),
        Some(snapshot) => format!(
            "{} · {} proc · {} bash running · {} mailbox pending · {} diag",
            status_word(snapshot.summary.status),
            snapshot.summary.running_nodes,
            snapshot.summary.running_agent_bash_count,
            snapshot.summary.pending_mailbox_count,
            snapshot.summary.diagnostics_count,
        ),
    }
}

/// Render the bounded, selection-following node list when expanded.
fn render_node_list(buf: &mut Buffer, area: Rect, pane: &MonitorPane) {
    if area.height == 0 {
        return;
    }
    render_filter_tabs(buf, filter_tabs_area(area), pane.active_filter);
    let rows_area = node_rows_area(area);
    if rows_area.height == 0 {
        return;
    }
    let Some(snapshot) = pane.snapshot.as_ref() else {
        return;
    };
    let projected = pane.projected_rows(snapshot);
    if projected.is_empty() {
        buf.set_string(
            rows_area.x,
            rows_area.y,
            pad_to_width(
                format!(" (no {} items)", pane.active_filter.label()),
                rows_area.width,
            ),
            Style::default().fg(Color::DarkGray),
        );
        return;
    }
    for VisibleNodeRow {
        index,
        y,
        node,
        prefix,
    } in visible_node_rows(pane, snapshot, rows_area, &projected)
    {
        let row = Rect {
            y,
            height: 1,
            ..rows_area
        };
        render_node_row(buf, row, node, &prefix, index == pane.selected);
    }
}

fn render_filter_tabs(buf: &mut Buffer, area: Rect, active: MonitorFilterCategory) {
    if area.height == 0 {
        return;
    }
    let labels = MonitorFilterCategory::ALL.map(|category| {
        if category == active {
            format!("[{}]", category.label())
        } else {
            format!(" {} ", category.label())
        }
    });
    let text = format!(" filters: {} ", labels.join(" "));
    buf.set_string(
        area.x,
        area.y,
        pad_to_width(text, area.width),
        Style::default().fg(Color::Black).bg(Color::Indexed(250)),
    );
}

fn filter_tabs_area(area: Rect) -> Rect {
    Rect {
        height: 1.min(area.height),
        ..area
    }
}

fn node_rows_area(area: Rect) -> Rect {
    Rect {
        y: area.y + 1.min(area.height),
        height: area.height.saturating_sub(1),
        ..area
    }
}

struct VisibleNodeRow<'a> {
    index: usize,
    y: u16,
    node: &'a MonitorNode,
    prefix: String,
}

fn visible_node_rows<'a>(
    pane: &'a MonitorPane,
    snapshot: &'a MonitorSnapshot,
    area: Rect,
    projected: &[ProjectedMonitorRow],
) -> Vec<VisibleNodeRow<'a>> {
    let rows = area.height as usize;
    let selected_position = selected_projected_position(projected, pane.selected).unwrap_or(0);
    let offset = scroll_offset(selected_position, projected.len(), rows);
    projected
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows)
        .filter_map(|(position, row)| visible_node_row(area.y, offset, position, row, snapshot))
        .collect()
}

fn visible_node_row<'a>(
    area_y: u16,
    offset: usize,
    position: usize,
    row: &ProjectedMonitorRow,
    snapshot: &'a MonitorSnapshot,
) -> Option<VisibleNodeRow<'a>> {
    let node = snapshot.nodes.get(row.index)?;
    Some(VisibleNodeRow {
        index: row.index,
        y: area_y + (position - offset) as u16,
        node,
        prefix: row.prefix.clone(),
    })
}

/// Keep the selected node visible within `rows` visible lines.
fn scroll_offset(selected: usize, len: usize, rows: usize) -> usize {
    if rows == 0 || len <= rows {
        return 0;
    }
    let max_offset = len - rows;
    selected.saturating_sub(rows - 1).min(max_offset)
}

/// Render a single node row: `kind [status]<anomaly> pid=… <label>`, with
/// completed-OK nodes dimmed and the selected row highlighted.
fn render_node_row(buf: &mut Buffer, area: Rect, node: &MonitorNode, prefix: &str, selected: bool) {
    let marker = if selected { '>' } else { ' ' };
    let pid = node
        .pid
        .map(|pid| format!(" pid={pid}"))
        .unwrap_or_default();
    let text = pad_to_width(
        format!(
            "{marker} {prefix}{} [{}]{}{} {}",
            kind_word(node.kind),
            status_word(node.status),
            liveness_glyph(node.liveness),
            pid,
            node.label
        ),
        area.width,
    );
    buf.set_string(area.x, area.y, text, node_row_style(node.status, selected));
}

/// A finished, successful node is de-emphasized so the live view is not
/// dominated by completed work.
fn is_completed_ok(status: MonitorStatus) -> bool {
    matches!(status, MonitorStatus::Succeeded | MonitorStatus::Delivered)
}

/// Row style: status colour, dimmed when completed-OK, reversed when selected.
fn node_row_style(status: MonitorStatus, selected: bool) -> Style {
    let mut style = if is_completed_ok(status) {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(status_color(status))
    };
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    style
}

/// Pad or truncate a label to `width` characters (char-boundary safe; the label
/// may contain multi-byte glyphs such as `·`/`—`/`…`).
fn pad_to_width(label: String, width: u16) -> String {
    let width = width as usize;
    let mut out: String = label.chars().take(width).collect();
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

fn status_word(status: MonitorStatus) -> &'static str {
    match status {
        MonitorStatus::Running => "running",
        MonitorStatus::Idle => "idle",
        MonitorStatus::Pending => "pending",
        MonitorStatus::Delivered => "delivered",
        MonitorStatus::Succeeded => "done",
        MonitorStatus::Failed => "failed",
        MonitorStatus::Error => "error",
        MonitorStatus::Stale => "stale",
        MonitorStatus::Unknown => "unknown",
        MonitorStatus::Cancelling => "cancelling",
        MonitorStatus::Cancelled => "cancelled",
    }
}

fn status_color(status: MonitorStatus) -> Color {
    match status {
        MonitorStatus::Running => Color::Green,
        MonitorStatus::Pending | MonitorStatus::Cancelling => Color::Yellow,
        MonitorStatus::Failed | MonitorStatus::Error | MonitorStatus::Stale => Color::Red,
        MonitorStatus::Succeeded | MonitorStatus::Delivered => Color::Cyan,
        _ => Color::Gray,
    }
}

fn kind_word(kind: MonitorNodeKind) -> &'static str {
    match kind {
        MonitorNodeKind::Session => "session",
        MonitorNodeKind::Invocation => "invocation",
        MonitorNodeKind::ProviderProcess => "process",
        MonitorNodeKind::AgentBashWorkload => "bash",
        MonitorNodeKind::MailboxNotification => "mailbox",
        MonitorNodeKind::WakeClaim => "wake",
    }
}

/// A liveness flag is shown only for genuine anomalies; the status word already
/// conveys the normal cases (a `running` node is live; a `done`/`stale` node's
/// process has exited), so `VerifiedLive`/`Dead`/`Unknown` add no glyph.
fn liveness_glyph(liveness: LivenessStatus) -> &'static str {
    match liveness {
        LivenessStatus::UnverifiedLive => " ~live",
        LivenessStatus::PidReused => " reused",
        _ => "",
    }
}

/// Guard that enters the alternate screen and restores the primary screen on drop.
struct AltScreenGuard {
    writer: File,
    mouse: TerminalMouseState,
    bracketed_paste: bool,
}

impl AltScreenGuard {
    fn enter(mut writer: File) -> Result<Self, String> {
        execute!(writer, EnterAlternateScreen)
            .map_err(|err| format!("Failed to enter alternate screen: {err}"))?;
        let mut guard = Self {
            writer,
            mouse: TerminalMouseState::new(),
            bracketed_paste: false,
        };
        guard.enable_keyboard_protocol()?;
        Ok(guard)
    }

    fn enable_keyboard_protocol(&mut self) -> Result<(), String> {
        self.writer
            .write_all(&terminal_keyboard_enable_sequence())
            .and_then(|()| self.writer.flush())
            .map_err(format_tui_keyboard_sync_error)
    }

    fn sync_mouse(&mut self, request: MouseRequest) -> Result<(), String> {
        sync_terminal_mouse(&mut self.writer, &mut self.mouse, request)
            .map_err(format_tui_mouse_sync_error)
    }

    fn sync_bracketed_paste(&mut self, enabled: bool) -> Result<(), String> {
        if self.bracketed_paste == enabled {
            return Ok(());
        }
        let sequence = if enabled {
            BRACKETED_PASTE_ENABLE
        } else {
            BRACKETED_PASTE_DISABLE
        };
        self.writer
            .write_all(sequence)
            .and_then(|()| self.writer.flush())
            .map_err(format_tui_bracketed_paste_sync_error)?;
        self.bracketed_paste = enabled;
        Ok(())
    }

    /// Copy `text` to the host's system clipboard via the OSC 52 escape, so the operator
    /// can paste a drag selection elsewhere even though the broker has claimed the mouse.
    fn copy_to_clipboard(&mut self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let payload = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let sequence = format!("\x1b]52;c;{payload}\x07");
        self.writer
            .write_all(sequence.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|err| format!("Failed to write clipboard selection: {err}"))
    }
}

impl Drop for AltScreenGuard {
    fn drop(&mut self) {
        let _ = self.writer.write_all(BRACKETED_PASTE_DISABLE);
        let _ = self.writer.write_all(&terminal_keyboard_restore_sequence());
        let _ = self.writer.write_all(&terminal_mouse_restore_sequence());
        let _ = execute!(self.writer, LeaveAlternateScreen);
    }
}

fn format_tui_keyboard_sync_error(err: io::Error) -> String {
    format!("Failed to sync terminal keyboard protocol: {err}")
}

fn format_tui_mouse_sync_error(err: io::Error) -> String {
    format!("Failed to sync terminal mouse mode: {err}")
}

fn format_tui_bracketed_paste_sync_error(err: io::Error) -> String {
    format!("Failed to sync terminal bracketed-paste mode: {err}")
}

fn clone_terminal_writer(writer: &File) -> io::Result<File> {
    writer.try_clone()
}

fn format_tui_terminal_clone_error(err: io::Error) -> String {
    format!("Failed to clone terminal for TUI: {err}")
}

fn new_tui_terminal(writer: File) -> io::Result<Terminal<CrosstermBackend<File>>> {
    Terminal::new(CrosstermBackend::new(writer))
}

fn format_tui_terminal_init_error(err: io::Error) -> String {
    format!("Failed to initialize TUI terminal: {err}")
}

fn try_wait_child(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    child.try_wait()
}

fn format_interactive_child_poll_error(err: io::Error) -> String {
    format!("Failed to poll interactive child: {err}")
}

#[derive(Default)]
struct RenderState {
    snapshot: Option<Arc<RenderSnapshot>>,
    generation: u64,
}

struct PublishedRenderSnapshot {
    snapshot: Arc<RenderSnapshot>,
    generation: u64,
}

#[derive(Default)]
struct RenderShared {
    state: Mutex<RenderState>,
    clipboard: Mutex<Vec<String>>,
    shutdown: AtomicBool,
    error: Mutex<Option<String>>,
    wake: Condvar,
    #[cfg(test)]
    frame_count: AtomicUsize,
}

impl RenderShared {
    fn publish(&self, snapshot: RenderSnapshot) {
        let mut state = lock_or_recover(&self.state);
        state.generation = state.generation.wrapping_add(1);
        state.snapshot = Some(Arc::new(snapshot));
        self.wake.notify_one();
    }

    fn latest_snapshot(&self) -> Option<Arc<RenderSnapshot>> {
        lock_or_recover(&self.state).snapshot.clone()
    }

    fn latest_published(&self) -> Option<PublishedRenderSnapshot> {
        let state = lock_or_recover(&self.state);
        state
            .snapshot
            .as_ref()
            .map(|snapshot| PublishedRenderSnapshot {
                snapshot: Arc::clone(snapshot),
                generation: state.generation,
            })
    }

    fn queue_clipboard_copy(&self, text: String) {
        lock_or_recover(&self.clipboard).push(text);
        self.wake.notify_one();
    }

    fn drain_clipboard_copies(&self) -> Vec<String> {
        std::mem::take(&mut *lock_or_recover(&self.clipboard))
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }

    fn shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn set_error(&self, message: String) {
        let mut error = lock_or_recover(&self.error);
        if error.is_none() {
            *error = Some(message);
        }
        self.request_shutdown();
    }

    fn check_error(&self) -> Result<(), String> {
        match lock_or_recover(&self.error).as_ref() {
            Some(message) => Err(message.clone()),
            None => Ok(()),
        }
    }

    fn wait_until(&self, deadline: Instant) {
        if self.shutdown_requested() {
            return;
        }
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let guard = lock_or_recover(&self.state);
        match self.wake.wait_timeout(guard, deadline - now) {
            Ok((guard, _)) => drop(guard),
            Err(poisoned) => drop(poisoned.into_inner()),
        }
    }

    #[cfg(test)]
    fn record_frame(&self) {
        self.frame_count.fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn frame_count(&self) -> usize {
        self.frame_count.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
struct RenderPublisher {
    shared: Arc<RenderShared>,
}

impl RenderPublisher {
    fn publish(&self, snapshot: RenderSnapshot) {
        self.shared.publish(snapshot);
    }

    fn copy_to_clipboard(&self, text: String) {
        if !text.is_empty() {
            self.shared.queue_clipboard_copy(text);
        }
    }

    fn check_error(&self) -> Result<(), String> {
        self.shared.check_error()
    }

    #[cfg(test)]
    fn frame_count(&self) -> usize {
        self.shared.frame_count()
    }
}

struct RenderThread {
    shared: Arc<RenderShared>,
    join: Option<JoinHandle<Result<(), String>>>,
}

impl RenderThread {
    fn start(writer: File) -> Result<Self, String> {
        let shared = Arc::new(RenderShared::default());
        let thread_shared = Arc::clone(&shared);
        let (ready_tx, ready_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("pty-broker-render".to_string())
            .spawn(move || {
                let result = render_thread_entry(writer, Arc::clone(&thread_shared), ready_tx);
                if let Err(message) = &result {
                    thread_shared.set_error(message.clone());
                }
                result
            })
            .map_err(format_render_thread_spawn_error)?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shared,
                join: Some(join),
            }),
            Ok(Err(message)) => {
                shared.request_shutdown();
                let _ = join.join();
                Err(message)
            }
            Err(err) => {
                shared.request_shutdown();
                let _ = join.join();
                Err(format_render_thread_ready_error(err))
            }
        }
    }

    fn publisher(&self) -> RenderPublisher {
        RenderPublisher {
            shared: Arc::clone(&self.shared),
        }
    }

    fn shutdown_and_join(mut self) -> Result<(), String> {
        self.shared.request_shutdown();
        if let Some(join) = self.join.take() {
            join.join().map_err(|_| render_thread_panic_error())??;
        }
        self.shared.check_error()
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        self.shared.request_shutdown();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn render_thread_entry(
    writer: File,
    shared: Arc<RenderShared>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let alt_writer = match clone_terminal_writer(&writer).map_err(format_tui_terminal_clone_error) {
        Ok(writer) => writer,
        Err(message) => return report_render_ready_error(ready, message),
    };
    let mut alt = match AltScreenGuard::enter(alt_writer) {
        Ok(alt) => alt,
        Err(message) => return report_render_ready_error(ready, message),
    };
    let mut terminal = match new_tui_terminal(writer).map_err(format_tui_terminal_init_error) {
        Ok(terminal) => terminal,
        Err(message) => return report_render_ready_error(ready, message),
    };
    let _ = ready.send(Ok(()));
    run_render_loop(&shared, &mut terminal, &mut alt)
}

fn report_render_ready_error(
    ready: mpsc::Sender<Result<(), String>>,
    message: String,
) -> Result<(), String> {
    let _ = ready.send(Err(message.clone()));
    Err(message)
}

fn run_render_loop(
    shared: &RenderShared,
    terminal: &mut Terminal<CrosstermBackend<File>>,
    alt: &mut AltScreenGuard,
) -> Result<(), String> {
    let mut next_frame = Instant::now();
    let mut last_frame_at = None;
    let mut last_rendered_generation = 0;
    loop {
        if shared.shutdown_requested() {
            render_latest_snapshot(shared, terminal, alt)?;
            return Ok(());
        }
        if let Some(published) = shared.latest_published()
            && published.generation != last_rendered_generation
        {
            next_frame =
                unrendered_publish_deadline(last_frame_at, &published.snapshot, Instant::now());
        }
        let now = Instant::now();
        if now < next_frame {
            shared.wait_until(next_frame);
            continue;
        }
        let Some(published) = shared.latest_published() else {
            next_frame = Instant::now() + render_frame_interval(BACKGROUND_RENDER_FPS);
            continue;
        };
        render_snapshot(shared, terminal, alt, &published.snapshot)?;
        let rendered_at = Instant::now();
        last_frame_at = Some(rendered_at);
        last_rendered_generation = published.generation;
        next_frame = rendered_at + snapshot_frame_interval(&published.snapshot);
    }
}

fn unrendered_publish_deadline(
    last_frame_at: Option<Instant>,
    snapshot: &RenderSnapshot,
    now: Instant,
) -> Instant {
    last_frame_at.map_or(now, |last| last + snapshot_frame_interval(snapshot))
}

fn render_latest_snapshot(
    shared: &RenderShared,
    terminal: &mut Terminal<CrosstermBackend<File>>,
    alt: &mut AltScreenGuard,
) -> Result<(), String> {
    if let Some(snapshot) = shared.latest_snapshot() {
        render_snapshot(shared, terminal, alt, &snapshot)?;
    }
    Ok(())
}

fn render_snapshot(
    shared: &RenderShared,
    terminal: &mut Terminal<CrosstermBackend<File>>,
    alt: &mut AltScreenGuard,
    snapshot: &RenderSnapshot,
) -> Result<(), String> {
    for text in shared.drain_clipboard_copies() {
        alt.copy_to_clipboard(&text)?;
    }
    alt.sync_bracketed_paste(snapshot.bracketed_paste)?;
    alt.sync_mouse(snapshot.mouse_request)?;
    draw_snapshot(terminal, snapshot)?;
    #[cfg(test)]
    shared.record_frame();
    Ok(())
}

fn snapshot_frame_interval(snapshot: &RenderSnapshot) -> Duration {
    render_frame_interval(snapshot_render_fps(snapshot))
}

fn snapshot_render_fps(snapshot: &RenderSnapshot) -> u64 {
    if snapshot_is_foreground(snapshot) {
        FOREGROUND_RENDER_FPS
    } else {
        BACKGROUND_RENDER_FPS
    }
}

fn snapshot_is_foreground(snapshot: &RenderSnapshot) -> bool {
    snapshot.priority == RenderPriority::Interactive
        || (snapshot.focus == Focus::Bottom && !snapshot.pane.collapsed)
}

fn render_frame_interval(fps: u64) -> Duration {
    Duration::from_nanos(1_000_000_000 / fps.max(1))
}

fn format_render_thread_spawn_error(err: io::Error) -> String {
    format!("Failed to spawn TUI render thread: {err}")
}

fn format_render_thread_ready_error(err: mpsc::RecvError) -> String {
    format!("TUI render thread exited before initialization: {err}")
}

fn render_thread_panic_error() -> String {
    "TUI render thread panicked".to_string()
}

/// Run the split-pane relay until the child exits, returning its exit status.
#[allow(clippy::too_many_arguments)]
pub(super) fn relay_until_exit_observed(
    input_fd: RawFd,
    writer: File,
    master: &File,
    control: Option<&ControlSocket>,
    child: &mut Child,
    monitor: MonitorSnapshotProvider,
    root: ObservabilityRoot,
    outbound_source: OutboundObserverSource,
) -> Result<ExitStatus, String> {
    let real_fd = input_fd;
    let master_fd = master.as_raw_fd();
    let renderer = RenderThread::start(writer)?;
    let publisher = renderer.publisher();

    let mut pane = MonitorPane::new();
    let snapshot_worker = MonitorSnapshotWorker::start(monitor, root, pane.refresh_interval())?;
    let outbound_worker = OutboundObserverWorker::start(outbound_source)?;
    let initial = child_pane_winsize(real_fd, &pane, TypingProtection::for_focus(Focus::Top));
    let mut parser = vt100::Parser::new(initial.ws_row, initial.ws_col, TOP_PANE_SCROLLBACK_ROWS);
    let mut top_scrollback: usize = 0;
    let mut selection: Option<TopSelection> = None;
    let mut clipboard = String::new();
    let mut router = InputRouter::new();
    let mut line_state = InputLineState::default();
    let mut child_output_state = ChildOutputState::default();
    let mut applied: Option<(libc::winsize, u16)> = None;
    let mut buffer = vec![0_u8; RELAY_BUFFER_BYTES];
    let mut pending_child_input = PendingChildInput::new();
    let mut deferred_child_input = PendingChildInput::new();
    let mut outbound_release_gate = OutboundReleaseGate::default();
    let mut status = None;
    publish_render_snapshot(
        &publisher,
        &parser,
        router.focus,
        &pane,
        None,
        typing_protection(router.focus, &line_state),
        RenderPriority::Background,
    );

    while status.is_none() {
        publisher.check_error()?;
        let mut dirty = false;
        let mut priority = RenderPriority::Background;
        mark_render_dirty(
            &mut dirty,
            &mut priority,
            pane.adopt_snapshot(snapshot_worker.latest_snapshot()),
            RenderPriority::Background,
        );
        mark_render_dirty(
            &mut dirty,
            &mut priority,
            pane.refresh_detail_if_due(Instant::now()),
            RenderPriority::Background,
        );
        release_deferred_child_input(
            pane.outbound.active.is_some(),
            &mut line_state,
            &mut pending_child_input,
            &mut deferred_child_input,
            &mut outbound_release_gate,
        );
        let mut protection = typing_protection(router.focus, &line_state);
        mark_render_dirty(
            &mut dirty,
            &mut priority,
            apply_sizing(
                real_fd,
                master_fd,
                child.id(),
                &pane,
                protection,
                &mut parser,
                &mut applied,
            ),
            RenderPriority::Background,
        );
        mark_render_dirty(
            &mut dirty,
            &mut priority,
            pump_outbound_queue_from_worker(
                &mut pane,
                &mut pending_child_input,
                &mut line_state,
                &outbound_release_gate,
                parser.screen().bracketed_paste(),
                &outbound_worker,
            ),
            RenderPriority::Interactive,
        );
        let release_gate_was_awaiting_output = outbound_release_gate.awaiting_child_output();
        let ready = poll_relay_fds(
            real_fd,
            master_fd,
            control.map(ControlSocket::fd),
            !pending_child_input.is_empty(),
        )?;
        if ready.pty_writable {
            flush_pending_child_input(master_fd, &mut pending_child_input)?;
            outbound_release_gate.observe_pending_write_drained(pending_child_input.is_empty());
            mark_render_dirty(
                &mut dirty,
                &mut priority,
                pump_outbound_queue_from_worker(
                    &mut pane,
                    &mut pending_child_input,
                    &mut line_state,
                    &outbound_release_gate,
                    parser.screen().bracketed_paste(),
                    &outbound_worker,
                ),
                RenderPriority::Interactive,
            );
        }
        if ready.real_input {
            let mut input_io = RealInputForwardIo {
                real_fd,
                router: &mut router,
                pane: &pane,
                mouse_request: mouse_request_from_screen(parser.screen()),
                line_state: &mut line_state,
                pending_child_input: &mut pending_child_input,
                deferred_child_input: &mut deferred_child_input,
                outbound_release_gate: &mut outbound_release_gate,
                buffer: &mut buffer,
            };
            let mut routed = forward_real_input(&mut input_io)?;
            let scroll_lines = routed.top_scroll_lines;
            // Sending keystrokes to the child snaps the view back to the live tail, like
            // a terminal jumps to the prompt when you start typing.
            let typed_to_child = !routed.forward.is_empty();
            let right_click = routed.right_click;
            let gestures = std::mem::take(&mut routed.top_mouse);
            let input_priority = routed_input_render_priority(
                &routed,
                typed_to_child,
                scroll_lines,
                !gestures.is_empty(),
                right_click.is_some(),
            );
            mark_render_dirty(
                &mut dirty,
                &mut priority,
                apply_routed_to_pane(&mut pane, routed, &snapshot_worker, &outbound_worker),
                input_priority,
            );
            mark_render_dirty(
                &mut dirty,
                &mut priority,
                pump_outbound_queue_from_worker(
                    &mut pane,
                    &mut pending_child_input,
                    &mut line_state,
                    &outbound_release_gate,
                    parser.screen().bracketed_paste(),
                    &outbound_worker,
                ),
                RenderPriority::Interactive,
            );
            if typed_to_child {
                // Typing snaps to the live tail and drops the selection highlight.
                selection = None;
                if top_scrollback != 0 {
                    top_scrollback = 0;
                }
                mark_render_dirty(&mut dirty, &mut priority, true, RenderPriority::Interactive);
            } else if scroll_lines != 0 {
                // Keep the selection — its highlight follows the content as we scroll.
                top_scrollback = apply_top_scroll(top_scrollback, scroll_lines);
                mark_render_dirty(&mut dirty, &mut priority, true, RenderPriority::Interactive);
            }
            if !gestures.is_empty() {
                mark_render_dirty(
                    &mut dirty,
                    &mut priority,
                    apply_selection_gestures(
                        &mut selection,
                        &gestures,
                        parser.screen(),
                        &publisher,
                        top_scrollback,
                        &mut clipboard,
                    )?,
                    RenderPriority::Interactive,
                );
            }
            if let Some(click) = right_click {
                let mut io = MouseActionIo {
                    clipboard: &mut clipboard,
                    renderer: &publisher,
                    line_state: &mut line_state,
                    pending_child_input: &mut pending_child_input,
                    deferred_child_input: &mut deferred_child_input,
                    outbound_release_gate: &mut outbound_release_gate,
                    outbound_active: pane.outbound.active.is_some(),
                };
                mark_render_dirty(
                    &mut dirty,
                    &mut priority,
                    handle_top_right_click(
                        &mut selection,
                        click,
                        parser.screen(),
                        top_scrollback,
                        &mut io,
                    )?,
                    RenderPriority::Interactive,
                );
            }
        }
        if ready.pty_output && pump_pty_output_burst(master_fd, &mut parser, &mut buffer)? {
            child_output_state.observe_child_output();
            outbound_release_gate
                .observe_child_output(release_gate_was_awaiting_output, Instant::now());
            mark_render_dirty(&mut dirty, &mut priority, true, RenderPriority::Interactive);
        }
        if ready.control
            && let Some(control) = control
        {
            let mut control_io = ControlInjectionIo {
                master_fd,
                parser: &mut parser,
                line_state: &mut line_state,
                child_output_state: &mut child_output_state,
                pending_child_input: &mut pending_child_input,
                outbound_release_gate: &mut outbound_release_gate,
                buffer: &mut buffer,
                child_pid: Some(child.id()),
            };
            let _ = service_control(control, &mut control_io);
            mark_render_dirty(&mut dirty, &mut priority, true, RenderPriority::Interactive);
        }
        // Re-assert the scrollback view each frame (clamped to retained history) so it
        // survives child output and resizes; reading it back keeps our offset honest.
        protection = typing_protection(router.focus, &line_state);
        mark_render_dirty(
            &mut dirty,
            &mut priority,
            apply_sizing(
                real_fd,
                master_fd,
                child.id(),
                &pane,
                protection,
                &mut parser,
                &mut applied,
            ),
            RenderPriority::Background,
        );
        parser.screen_mut().set_scrollback(top_scrollback);
        top_scrollback = parser.screen().scrollback();
        if dirty {
            publish_render_snapshot(
                &publisher,
                &parser,
                router.focus,
                &pane,
                current_render_selection(&selection, top_scrollback, parser.screen().size().0),
                protection,
                priority,
            );
        }
        publisher.check_error()?;
        status = try_wait_child(child).map_err(format_interactive_child_poll_error)?;
    }

    drain_pty_output(master_fd, &mut parser, &mut buffer)?;
    parser.screen_mut().set_scrollback(top_scrollback);
    let _ = pane.adopt_snapshot(snapshot_worker.latest_snapshot());
    publish_render_snapshot(
        &publisher,
        &parser,
        router.focus,
        &pane,
        None,
        typing_protection(router.focus, &line_state),
        RenderPriority::Background,
    );
    renderer.shutdown_and_join()?;
    snapshot_worker.shutdown_and_join()?;
    outbound_worker.shutdown_and_join()?;
    Ok(status.expect("status checked above"))
}

/// Initial child PTY size for the (collapsed) monitor at the current terminal size.
fn child_pane_winsize(
    real_fd: RawFd,
    pane: &MonitorPane,
    protection: TypingProtection,
) -> libc::winsize {
    let full = terminal_winsize_with_fallback(read_terminal_winsize(real_fd));
    child_winsize_for_pane(&full, pane, protection)
}

fn terminal_winsize_with_fallback(winsize: Option<libc::winsize>) -> libc::winsize {
    winsize.unwrap_or_else(minimum_terminal_winsize)
}

fn minimum_terminal_winsize() -> libc::winsize {
    libc::winsize {
        ws_row: MIN_TERMINAL_ROWS,
        ws_col: MIN_TERMINAL_COLS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn child_winsize_for_pane(
    full: &libc::winsize,
    pane: &MonitorPane,
    protection: TypingProtection,
) -> libc::winsize {
    child_winsize(full, pane.bottom_rows(full.ws_row, full.ws_col, protection))
}

/// On a change to the terminal size OR the monitor's reserved rows, resize the
/// virtual terminal and the child PTY (top-pane sized) and notify the child group.
fn apply_sizing(
    real_fd: RawFd,
    master_fd: RawFd,
    child_pid: u32,
    pane: &MonitorPane,
    protection: TypingProtection,
    parser: &mut vt100::Parser,
    applied: &mut Option<(libc::winsize, u16)>,
) -> bool {
    let Some(full) = read_terminal_winsize(real_fd) else {
        return false;
    };
    let bottom = pane.bottom_rows(full.ws_row, full.ws_col, protection);
    if !sizing_update_needed(applied, &full, bottom) {
        return false;
    }
    let child = child_winsize(&full, bottom);
    resize_virtual_terminal(parser, &child);
    apply_child_pty_winsize(master_fd, child_pid, &child);
    record_applied_sizing(applied, full, bottom);
    true
}

fn sizing_update_needed(
    applied: &Option<(libc::winsize, u16)>,
    full: &libc::winsize,
    bottom: u16,
) -> bool {
    !sizing_already_applied(applied, full, bottom)
}

fn read_terminal_winsize(real_fd: RawFd) -> Option<libc::winsize> {
    terminal_winsize(real_fd).ok()
}

fn sizing_already_applied(
    applied: &Option<(libc::winsize, u16)>,
    full: &libc::winsize,
    bottom: u16,
) -> bool {
    applied.as_ref().is_some_and(|(prev_full, prev_bottom)| {
        winsize_eq(prev_full, full) && *prev_bottom == bottom
    })
}

fn resize_virtual_terminal(parser: &mut vt100::Parser, child: &libc::winsize) {
    parser.screen_mut().set_size(child.ws_row, child.ws_col);
}

fn apply_child_pty_winsize(master_fd: RawFd, child_pid: u32, child: &libc::winsize) {
    if set_pty_winsize(master_fd, child).is_ok() {
        send_signal_to_child_group(child_pid, libc::SIGWINCH);
    }
}

fn record_applied_sizing(
    applied: &mut Option<(libc::winsize, u16)>,
    full: libc::winsize,
    bottom: u16,
) {
    *applied = Some((full, bottom));
}

/// Read real-terminal input, route it by focus, and forward top-pane bytes to the
/// child. Returns the routing outcome for the caller to apply to the monitor.
struct RealInputForwardIo<'a> {
    real_fd: RawFd,
    router: &'a mut InputRouter,
    pane: &'a MonitorPane,
    mouse_request: MouseRequest,
    line_state: &'a mut InputLineState,
    pending_child_input: &'a mut PendingChildInput,
    deferred_child_input: &'a mut PendingChildInput,
    outbound_release_gate: &'a mut OutboundReleaseGate,
    buffer: &'a mut [u8],
}

fn forward_real_input(io: &mut RealInputForwardIo<'_>) -> Result<RoutedInput, String> {
    match read_real_input(io.real_fd, io.buffer) {
        Ok(0) => Ok(RoutedInput::default()),
        Ok(n) => {
            let full = terminal_winsize_with_fallback(read_terminal_winsize(io.real_fd));
            let protection = typing_protection(io.router.focus, io.line_state);
            let routed = route_real_input_with_protection(
                &io.buffer[..n],
                io.router,
                io.pane,
                io.mouse_request,
                &full,
                protection,
            );
            enqueue_routed_child_input(
                io.line_state,
                io.pending_child_input,
                io.deferred_child_input,
                io.outbound_release_gate,
                io.pane.outbound.active.is_some(),
                &routed,
            );
            Ok(routed)
        }
        Err(err) => Err(format_user_terminal_input_read_error(err)),
    }
}

#[cfg(test)]
fn route_real_input(
    bytes: &[u8],
    router: &mut InputRouter,
    pane: &MonitorPane,
    child_mouse: MouseRequest,
    winsize: &libc::winsize,
) -> RoutedInput {
    let protection = TypingProtection::for_focus(router.focus);
    route_real_input_with_protection(bytes, router, pane, child_mouse, winsize, protection)
}

fn route_real_input_with_protection(
    bytes: &[u8],
    router: &mut InputRouter,
    pane: &MonitorPane,
    child_mouse: MouseRequest,
    winsize: &libc::winsize,
    protection: TypingProtection,
) -> RoutedInput {
    // The broker always captures the wheel on the real terminal (see
    // `effective_mouse_request`), so always parse mouse events: top-pane wheel scrolls
    // the scrollback, bottom-pane wheel scrolls the monitor, and non-wheel mouse is
    // forwarded only to a child that requested mouse input.
    route_mouse_aware_input(bytes, router, pane, child_mouse, winsize, protection)
}

fn route_mouse_aware_input(
    bytes: &[u8],
    router: &mut InputRouter,
    pane: &MonitorPane,
    mouse_request: MouseRequest,
    winsize: &libc::winsize,
    protection: TypingProtection,
) -> RoutedInput {
    let areas = pane_areas_for_winsize(winsize, pane, protection);
    let mut routed = RoutedInput {
        pseudo_input_width: Some(expanded_bottom_layout(areas.bottom, pane).input.width),
        ..Default::default()
    };
    let mut i = 0;
    while i < bytes.len() {
        if let Some(parsed) = parse_mouse_event(&bytes[i..]) {
            route_mouse_event(
                parsed.event,
                areas,
                router,
                pane,
                mouse_request,
                &mut routed,
            );
            i += parsed.consumed;
        } else {
            i += router.route_next_input(&bytes[i..], &mut routed);
        }
    }
    routed
}

fn route_mouse_event(
    event: MouseEvent,
    areas: PaneAreas,
    router: &mut InputRouter,
    pane: &MonitorPane,
    mouse_request: MouseRequest,
    routed: &mut RoutedInput,
) {
    match route_mouse_to_pane(event, areas) {
        MousePaneRoute::Top(local) => route_top_mouse_event(local, mouse_request, router, routed),
        MousePaneRoute::Bottom(bottom) => {
            route_bottom_mouse_event(bottom, areas.bottom, pane, router, routed)
        }
        MousePaneRoute::Outside => {}
    }
}

fn route_top_mouse_event(
    event: MouseEvent,
    child_mouse: MouseRequest,
    router: &mut InputRouter,
    routed: &mut RoutedInput,
) {
    if is_primary_press(event) && router.focus == Focus::Bottom {
        router.focus_top(routed);
    }
    // A child that requested mouse owns its own events (including the wheel, e.g. a
    // full-screen TUI agent): forward them in the child's encoding.
    if child_mouse.is_enabled() {
        if let Some(bytes) = encode_mouse_event(event, child_mouse.encoding) {
            routed.forward.extend(bytes);
        }
        return;
    }
    // Child has no mouse mode: the wheel scrolls the broker's top-pane scrollback, the
    // left button drives drag selection, and everything else is swallowed rather than
    // injected as input the child never asked for.
    if mouse_button_is_wheel(event.button) {
        routed.top_scroll_lines += wheel_scroll_lines(event);
    } else if let Some(gesture) = top_selection_gesture(event) {
        routed.top_mouse.push(TopMouse {
            gesture,
            row: event.row,
            col: event.col,
        });
    } else if is_right_press(event) {
        routed.right_click = Some((event.row, event.col));
    }
}

fn mouse_button_is_motion(button: u16) -> bool {
    button & 32 != 0
}

/// A right-button press (base button 2, not a release or motion) — the broker's paste
/// trigger, mirroring the right-click-to-paste convention of many terminals.
fn is_right_press(event: MouseEvent) -> bool {
    event.button & 0b11 == 2 && !event.released && !mouse_button_is_motion(event.button)
}

fn is_primary_press(event: MouseEvent) -> bool {
    event.button & 0b11 == 0
        && !event.released
        && !mouse_button_is_motion(event.button)
        && !mouse_button_is_wheel(event.button)
}

/// Classify a non-wheel top-pane mouse event as a left-button selection gesture, or
/// `None` for buttons/events the selection machine ignores.
fn top_selection_gesture(event: MouseEvent) -> Option<TopGesture> {
    let is_left = event.button & 0b11 == 0;
    if !is_left {
        return None;
    }
    if event.released {
        Some(TopGesture::Release)
    } else if mouse_button_is_motion(event.button) {
        Some(TopGesture::Drag)
    } else {
        Some(TopGesture::Press)
    }
}

/// Live drag selection in the interactive top pane. Coordinates are 1-based terminal
/// coordinates localized to the pane (the renderer's space plus one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TopSelection {
    anchor: (u16, u16),
    head: (u16, u16),
    active: bool,
    dragged: bool,
    /// The scrollback offset when the selection began, so the highlight can follow the
    /// content (rather than the screen position) as the operator wheels through history.
    scrollback_at: usize,
}

impl TopSelection {
    fn begin(row: u16, col: u16, scrollback_at: usize) -> Self {
        Self {
            anchor: (row, col),
            head: (row, col),
            active: true,
            dragged: false,
            scrollback_at,
        }
    }
}

/// Normalized selection range in 0-based cell coordinates (start <= end, row-major).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectionSpan {
    start: (u16, u16),
    end: (u16, u16),
}

fn selection_span(selection: &TopSelection) -> SelectionSpan {
    let anchor = (
        selection.anchor.0.saturating_sub(1),
        selection.anchor.1.saturating_sub(1),
    );
    let head = (
        selection.head.0.saturating_sub(1),
        selection.head.1.saturating_sub(1),
    );
    let (start, end) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    SelectionSpan { start, end }
}

fn cell_in_selection(span: SelectionSpan, row: u16, col: u16) -> bool {
    (row, col) >= span.start && (row, col) <= span.end
}

/// Column range of a selection on a given row, clamped to the screen width.
fn selection_row_cols(span: SelectionSpan, row: u16, cols: u16) -> (u16, u16) {
    let last = cols.saturating_sub(1);
    let first = if row == span.start.0 { span.start.1 } else { 0 };
    let final_col = if row == span.end.0 { span.end.1 } else { last };
    (first.min(last), final_col.min(last))
}

/// Extract the selected text from the (already scroll-positioned) screen, preserving
/// inner spacing and trimming trailing whitespace per line.
fn extract_selection_text(screen: &vt100::Screen, span: SelectionSpan) -> String {
    let (rows, cols) = screen.size();
    if rows == 0 || cols == 0 {
        return String::new();
    }
    let row_end = span.end.0.min(rows - 1);
    let mut lines: Vec<String> = Vec::new();
    for row in span.start.0..=row_end {
        let (first, last) = selection_row_cols(span, row, cols);
        let mut line = String::new();
        for col in first..=last {
            let contents = screen.cell(row, col).map(vt100::Cell::contents);
            match contents {
                Some(text) if !text.is_empty() => line.push_str(text),
                _ => line.push(' '),
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

/// Fold an ordered batch of left-button gestures into the live selection, copying to the
/// system clipboard (OSC 52) when a drag completes. Returns whether a redraw is needed.
enum ReleaseOutcome {
    Copy(SelectionSpan),
    Clear,
    None,
}

fn apply_selection_gestures(
    selection: &mut Option<TopSelection>,
    gestures: &[TopMouse],
    screen: &vt100::Screen,
    renderer: &RenderPublisher,
    scrollback_at: usize,
    clipboard: &mut String,
) -> Result<bool, String> {
    let mut dirty = false;
    for gesture in gestures {
        match gesture.gesture {
            TopGesture::Press => {
                *selection = Some(TopSelection::begin(gesture.row, gesture.col, scrollback_at));
                dirty = true;
            }
            TopGesture::Drag => {
                if let Some(active) = selection.as_mut().filter(|sel| sel.active) {
                    active.head = (gesture.row, gesture.col);
                    active.dragged = true;
                    dirty = true;
                }
            }
            TopGesture::Release => {
                // Resolve the outcome while holding the &mut, then act on it after the
                // borrow ends (so we can clear `selection` or write to the terminal).
                let outcome = match selection.as_mut() {
                    Some(active) if active.active => {
                        active.active = false;
                        dirty = true;
                        // A real drag (the head actually moved off the anchor) copies; a
                        // bare click — including a one-pixel jitter that never left the
                        // cell — just clears.
                        if active.dragged && active.head != active.anchor {
                            ReleaseOutcome::Copy(selection_span(active))
                        } else {
                            ReleaseOutcome::Clear
                        }
                    }
                    _ => ReleaseOutcome::None,
                };
                match outcome {
                    ReleaseOutcome::Copy(span) => {
                        let text = extract_selection_text(screen, span);
                        if text.is_empty() {
                            *selection = None;
                        } else {
                            *clipboard = text.clone();
                            renderer.copy_to_clipboard(text);
                        }
                    }
                    ReleaseOutcome::Clear => *selection = None,
                    ReleaseOutcome::None => {}
                }
            }
        }
    }
    Ok(dirty)
}

/// Map a stored selection onto the currently-visible rows, shifting by how far the view
/// has scrolled since the selection was made so the highlight tracks the content. Returns
/// `None` when the selection has scrolled entirely off-screen.
fn visible_selection_span(
    selection: &TopSelection,
    current_scrollback: usize,
    height: u16,
) -> Option<SelectionSpan> {
    if height == 0 {
        return None;
    }
    let base = selection_span(selection);
    let delta = current_scrollback as i64 - selection.scrollback_at as i64;
    let start_row = base.start.0 as i64 + delta;
    let end_row = base.end.0 as i64 + delta;
    let max_row = i64::from(height) - 1;
    if end_row < 0 || start_row > max_row {
        return None;
    }
    // A row clamped at the top starts at column 0; one clamped at the bottom runs to the
    // line end, since those are interior rows of a selection extending off-screen.
    let start_col = if start_row < 0 { 0 } else { base.start.1 };
    let end_col = if end_row > max_row {
        u16::MAX
    } else {
        base.end.1
    };
    Some(SelectionSpan {
        start: (start_row.max(0) as u16, start_col),
        end: (end_row.min(max_row) as u16, end_col),
    })
}

/// Mutable IO handles the relay loop lends to top-pane mouse-action helpers.
struct MouseActionIo<'a> {
    clipboard: &'a mut String,
    renderer: &'a RenderPublisher,
    line_state: &'a mut InputLineState,
    pending_child_input: &'a mut PendingChildInput,
    deferred_child_input: &'a mut PendingChildInput,
    outbound_release_gate: &'a mut OutboundReleaseGate,
    outbound_active: bool,
}

/// Handle a top-pane right-click: if it lands on the current selection, copy that
/// selection and deselect; otherwise paste the broker clipboard into the child. Returns
/// whether a redraw is needed. `click` is 1-based, pane-local.
fn handle_top_right_click(
    selection: &mut Option<TopSelection>,
    click: (u16, u16),
    screen: &vt100::Screen,
    top_scrollback: usize,
    io: &mut MouseActionIo<'_>,
) -> Result<bool, String> {
    let height = screen.size().0;
    let cell = (click.0.saturating_sub(1), click.1.saturating_sub(1));
    let visible = selection
        .as_ref()
        .and_then(|sel| visible_selection_span(sel, top_scrollback, height));
    let on_selection = visible.is_some_and(|span| cell_in_selection(span, cell.0, cell.1));
    if on_selection {
        if let Some(span) = visible {
            let text = extract_selection_text(screen, span);
            if !text.is_empty() {
                *io.clipboard = text.clone();
                io.renderer.copy_to_clipboard(text);
            }
        }
        *selection = None;
        Ok(true)
    } else {
        inject_clipboard_paste(
            io.line_state,
            io.pending_child_input,
            io.deferred_child_input,
            io.outbound_release_gate,
            io.outbound_active,
            io.clipboard,
        );
        Ok(false)
    }
}

/// Inject the broker clipboard into the child as a bracketed paste, so the child treats
/// it as pasted data rather than typed commands (no accidental command execution).
fn inject_clipboard_paste(
    line_state: &mut InputLineState,
    pending_child_input: &mut PendingChildInput,
    deferred_child_input: &mut PendingChildInput,
    outbound_release_gate: &mut OutboundReleaseGate,
    outbound_active: bool,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let bytes = wrap_real_terminal_paste(text.as_bytes());
    enqueue_user_child_input(
        line_state,
        pending_child_input,
        deferred_child_input,
        outbound_release_gate,
        outbound_active,
        &bytes,
    );
}

fn route_bottom_mouse_event(
    event: MouseEvent,
    bottom: Rect,
    pane: &MonitorPane,
    router: &mut InputRouter,
    routed: &mut RoutedInput,
) {
    if let Some(command) = mouse_wheel_command(event) {
        apply_monitor_command(routed, command);
    } else if is_primary_press(event) {
        router.focus_bottom(routed);
        if bottom_status_row_contains(bottom, pane, event) {
            apply_monitor_command(routed, MonitorCommand::ToggleList);
            return;
        }
        if let Some(filter) = bottom_filter_tab_at(event, bottom, pane) {
            apply_monitor_command(routed, MonitorCommand::SelectFilter(filter));
            return;
        }
        if let Some(index) = bottom_visible_row_index(event, bottom, pane) {
            apply_monitor_command(routed, MonitorCommand::ToggleSelectIndex(index));
        }
    }
}

fn bottom_status_row_contains(bottom: Rect, pane: &MonitorPane, event: MouseEvent) -> bool {
    rect_contains_mouse(expanded_bottom_layout(bottom, pane).status, event)
}

fn bottom_filter_tab_at(
    event: MouseEvent,
    bottom: Rect,
    pane: &MonitorPane,
) -> Option<MonitorFilterCategory> {
    let tabs = bottom_filter_tabs_area(bottom, pane)?;
    if !rect_contains_mouse(tabs, event) {
        return None;
    }
    filter_category_at_column(event.col.saturating_sub(tabs.x + 1))
}

fn bottom_filter_tabs_area(bottom: Rect, pane: &MonitorPane) -> Option<Rect> {
    if pane.collapsed {
        return None;
    }
    let content = expanded_bottom_layout(bottom, pane).content;
    (content.height > 0).then_some(filter_tabs_area(content))
}

fn filter_category_at_column(col: u16) -> Option<MonitorFilterCategory> {
    let mut offset = " filters: ".chars().count() as u16;
    for category in MonitorFilterCategory::ALL {
        let width = filter_tab_label_width(category);
        if col >= offset && col < offset.saturating_add(width) {
            return Some(category);
        }
        offset = offset.saturating_add(width + 1);
    }
    None
}

fn filter_tab_label_width(category: MonitorFilterCategory) -> u16 {
    category.label().chars().count() as u16 + 2
}

fn bottom_visible_row_index(event: MouseEvent, bottom: Rect, pane: &MonitorPane) -> Option<usize> {
    let list = bottom_list_area(bottom, pane)?;
    if !rect_contains_mouse(list, event) {
        return None;
    }
    let snapshot = pane.snapshot.as_ref()?;
    let row = usize::from(event.row.saturating_sub(list.y.saturating_add(1)));
    let projected = pane.projected_rows(snapshot);
    let selected_position = selected_projected_position(&projected, pane.selected).unwrap_or(0);
    let offset = scroll_offset(selected_position, projected.len(), list.height as usize);
    projected.get(offset + row).map(|row| row.index)
}

fn bottom_list_area(bottom: Rect, pane: &MonitorPane) -> Option<Rect> {
    if pane.collapsed || bottom.height <= 1 {
        return None;
    }
    let body = expanded_bottom_layout(bottom, pane).content;
    if body.height == 0 {
        return None;
    }
    if pane.detail_visible() && body.height >= 4 {
        Some(node_rows_area(Rect {
            height: (body.height / 2).max(2),
            ..body
        }))
    } else {
        Some(node_rows_area(body))
    }
}

fn read_real_input(real_fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    read_fd(real_fd, buffer)
}

fn format_user_terminal_input_read_error(err: io::Error) -> String {
    format!("Failed to read user terminal input: {err}")
}

fn routed_child_input(routed: &RoutedInput) -> Option<&[u8]> {
    if routed.forward.is_empty() {
        None
    } else {
        Some(&routed.forward)
    }
}

fn enqueue_routed_child_input(
    line_state: &mut InputLineState,
    pending_child_input: &mut PendingChildInput,
    deferred_child_input: &mut PendingChildInput,
    outbound_release_gate: &mut OutboundReleaseGate,
    outbound_active: bool,
    routed: &RoutedInput,
) {
    let Some(bytes) = routed_child_input(routed) else {
        return;
    };
    let child_bytes = child_input_for_real_read(bytes);
    enqueue_user_child_input(
        line_state,
        pending_child_input,
        deferred_child_input,
        outbound_release_gate,
        outbound_active,
        &child_bytes,
    );
}

fn enqueue_user_child_input(
    line_state: &mut InputLineState,
    pending_child_input: &mut PendingChildInput,
    deferred_child_input: &mut PendingChildInput,
    outbound_release_gate: &mut OutboundReleaseGate,
    outbound_active: bool,
    bytes: &[u8],
) {
    if outbound_active || !deferred_child_input.is_empty() {
        deferred_child_input.enqueue(bytes);
        return;
    }
    let reached_line_boundary = line_state.observe_user_input(bytes);
    outbound_release_gate.observe_user_input(reached_line_boundary);
    pending_child_input.enqueue(bytes);
}

fn release_deferred_child_input(
    outbound_active: bool,
    line_state: &mut InputLineState,
    pending_child_input: &mut PendingChildInput,
    deferred_child_input: &mut PendingChildInput,
    outbound_release_gate: &mut OutboundReleaseGate,
) {
    if outbound_active || !pending_child_input.is_empty() || deferred_child_input.is_empty() {
        return;
    }
    let bytes = deferred_child_input.take_pending();
    let reached_line_boundary = line_state.observe_user_input(&bytes);
    outbound_release_gate.observe_user_input(reached_line_boundary);
    pending_child_input.enqueue(&bytes);
}

/// Apply routed monitor effects (expand/select/refresh/collapse) to the pane.
/// Returns whether a redraw is needed.
fn apply_routed_to_pane(
    pane: &mut MonitorPane,
    routed: RoutedInput,
    snapshot_worker: &MonitorSnapshotWorker,
    outbound_worker: &OutboundObserverWorker,
) -> bool {
    apply_routed_pseudo_input(
        pane,
        &routed.pseudo_input,
        routed
            .pseudo_input_width
            .unwrap_or(DEFAULT_PSEUDO_INPUT_WIDTH),
    );
    let force_refresh = apply_routed_commands(pane, &routed.commands);
    let cancelled = run_pending_cancel(pane);
    let recovered = run_pending_outbound_recovery(pane, outbound_worker);
    snapshot_worker.set_interval(pane.refresh_interval());
    if pane_refresh_required(force_refresh, cancelled || recovered) {
        snapshot_worker.request_refresh();
    }
    routed.redraw
}

fn mark_render_dirty(
    dirty: &mut bool,
    priority: &mut RenderPriority,
    changed: bool,
    changed_priority: RenderPriority,
) {
    if changed {
        *dirty = true;
        priority.escalate(changed_priority);
    }
}

fn routed_input_render_priority(
    routed: &RoutedInput,
    typed_to_child: bool,
    scroll_lines: i32,
    selection_gestures: bool,
    right_click: bool,
) -> RenderPriority {
    if typed_to_child
        || scroll_lines != 0
        || selection_gestures
        || right_click
        || routed.focus_bottom
        || routed.redraw
        || !routed.pseudo_input.is_empty()
        || routed.commands.iter().any(monitor_command_is_interactive)
    {
        RenderPriority::Interactive
    } else {
        RenderPriority::Background
    }
}

fn monitor_command_is_interactive(command: &MonitorCommand) -> bool {
    matches!(
        command,
        MonitorCommand::SelectNext
            | MonitorCommand::SelectPrev
            | MonitorCommand::ToggleSelectIndex(_)
            | MonitorCommand::SelectFilter(_)
            | MonitorCommand::NextFilter
            | MonitorCommand::PrevFilter
            | MonitorCommand::ToggleTreeMode
            | MonitorCommand::Refresh
            | MonitorCommand::Collapse
            | MonitorCommand::ToggleList
            | MonitorCommand::ToggleInspect
            | MonitorCommand::RequestCancel
            | MonitorCommand::RequestOutboundRetry
            | MonitorCommand::RequestOutboundDiscard
            | MonitorCommand::ConfirmAction
            | MonitorCommand::AbortAction
    )
}

fn apply_routed_pseudo_input(pane: &mut MonitorPane, actions: &[PseudoInputAction], width: u16) {
    for action in actions {
        pane.apply_pseudo_input(*action, width);
    }
}

fn apply_routed_commands(pane: &mut MonitorPane, commands: &[MonitorCommand]) -> bool {
    commands
        .iter()
        .fold(false, |force, command| pane.apply(*command) || force)
}

fn pane_refresh_required(force_refresh: bool, cancelled: bool) -> bool {
    force_refresh || cancelled
}

/// Execute a confirmed cancel request, if any, recording operator feedback.
/// Returns whether a cancel ran (so the caller forces a snapshot refresh).
fn run_pending_cancel(pane: &mut MonitorPane) -> bool {
    let Some(request) = pane.take_cancel_request() else {
        return false;
    };
    let outcome = execute_cancel(&request);
    pane.record_cancel_feedback(cancel_outcome_message(&outcome));
    true
}

fn run_pending_outbound_recovery(pane: &mut MonitorPane, worker: &OutboundObserverWorker) -> bool {
    let Some(request) = pane.take_outbound_recovery_request() else {
        return false;
    };
    let generation_floor = worker.request_fresh_generation();
    let applied =
        pane.outbound
            .apply_recovery(request.message_id, request.action, generation_floor);
    let action = match request.action {
        OutboundRecoveryAction::Retry => "retrying",
        OutboundRecoveryAction::Discard => "discarded",
    };
    let feedback = if applied {
        format!("message #{} {action}", request.message_id)
    } else {
        format!("message #{} is no longer ambiguous", request.message_id)
    };
    pane.record_outbound_recovery_feedback(feedback);
    applied
}

/// Read child PTY output into the virtual terminal. Returns whether new output
/// arrived (requiring a redraw).
fn pump_pty_output(
    master_fd: RawFd,
    parser: &mut vt100::Parser,
    buffer: &mut [u8],
) -> Result<bool, String> {
    let output = read_pty_output(master_fd, buffer).map_err(format_pty_output_read_error)?;
    Ok(process_pty_output(parser, buffer, output))
}

fn pump_pty_output_burst(
    master_fd: RawFd,
    parser: &mut vt100::Parser,
    buffer: &mut [u8],
) -> Result<bool, String> {
    let mut saw_output = false;
    for read_index in 0..MAX_COALESCED_PTY_READS {
        if read_index != 0 && !pty_output_ready_now(master_fd)? {
            break;
        }
        if !pump_pty_output(master_fd, parser, buffer)? {
            break;
        }
        saw_output = true;
    }
    Ok(saw_output)
}

fn pty_output_ready_now(master_fd: RawFd) -> Result<bool, String> {
    let mut pollfd = poll_master_fd(master_fd, false);
    let rc = unsafe { libc::poll(std::slice::from_mut(&mut pollfd).as_mut_ptr(), 1, 0) };
    if rc >= 0 {
        return Ok(readable(pollfd.revents));
    }
    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EINTR) {
        return Ok(false);
    }
    Err(format_pty_output_coalesce_poll_error(err))
}

fn format_pty_output_coalesce_poll_error(err: io::Error) -> String {
    format!("Failed to poll PTY output for coalescing: {err}")
}

/// Drain any buffered child output into the virtual terminal after exit.
fn drain_pty_output(
    master_fd: RawFd,
    parser: &mut vt100::Parser,
    buffer: &mut [u8],
) -> Result<(), String> {
    while poll_single_fd(master_fd)? {
        let output = read_pty_output(master_fd, buffer).map_err(format_pty_output_drain_error)?;
        if !process_pty_output(parser, buffer, output) {
            return Ok(());
        }
    }
    Ok(())
}

enum PtyOutput {
    Empty,
    Bytes(usize),
}

fn read_pty_output(master_fd: RawFd, buffer: &mut [u8]) -> io::Result<PtyOutput> {
    match read_fd(master_fd, buffer) {
        Ok(n) => Ok(pty_output_from_len(n)),
        Err(err) if pty_read_error_is_eof(&err) => Ok(PtyOutput::Empty),
        Err(err) => Err(err),
    }
}

fn pty_output_from_len(len: usize) -> PtyOutput {
    if len == 0 {
        PtyOutput::Empty
    } else {
        PtyOutput::Bytes(len)
    }
}

fn pty_read_error_is_eof(err: &io::Error) -> bool {
    is_pty_eof_error(err)
}

fn process_pty_output(parser: &mut vt100::Parser, buffer: &[u8], output: PtyOutput) -> bool {
    if !pty_output_has_bytes(&output) {
        return false;
    }
    process_pty_bytes(parser, pty_output_bytes(buffer, &output));
    true
}

fn pty_output_has_bytes(output: &PtyOutput) -> bool {
    matches!(output, PtyOutput::Bytes(_))
}

fn pty_output_bytes<'a>(buffer: &'a [u8], output: &PtyOutput) -> &'a [u8] {
    match output {
        PtyOutput::Empty => &buffer[..0],
        PtyOutput::Bytes(len) => &buffer[..*len],
    }
}

fn process_pty_bytes(parser: &mut vt100::Parser, bytes: &[u8]) {
    parser.process(bytes);
}

fn format_pty_output_read_error(err: io::Error) -> String {
    format!("Failed to read PTY output: {err}")
}

fn format_pty_output_drain_error(err: io::Error) -> String {
    format!("Failed to drain PTY output: {err}")
}

fn current_render_selection(
    selection: &Option<TopSelection>,
    top_scrollback: usize,
    screen_height: u16,
) -> Option<SelectionSpan> {
    selection
        .as_ref()
        .and_then(|sel| visible_selection_span(sel, top_scrollback, screen_height))
}

fn publish_render_snapshot(
    publisher: &RenderPublisher,
    parser: &vt100::Parser,
    focus: Focus,
    pane: &MonitorPane,
    selection: Option<SelectionSpan>,
    typing_protection: TypingProtection,
    priority: RenderPriority,
) {
    publisher.publish(RenderSnapshot::capture_with_typing_protection_and_priority(
        parser.screen(),
        focus,
        pane,
        selection,
        typing_protection,
        priority,
    ));
}

fn pump_outbound_queue_with_gate(
    pane: &mut MonitorPane,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    release_gate: &OutboundReleaseGate,
    bracketed_paste: bool,
    observation: Option<&OutboundObservationResult>,
    now: Instant,
) -> bool {
    let mut dirty = observation.is_some_and(|observation| {
        apply_outbound_observation(&mut pane.outbound, observation, now)
    });
    dirty |= mark_outbound_timeouts(&mut pane.outbound, now);
    dirty |= advance_active_outbound_send(&mut pane.outbound, pending_child_input, line_state, now);
    dirty |= start_next_outbound_message(
        &mut pane.outbound,
        pending_child_input,
        line_state,
        release_gate,
        bracketed_paste,
        observation,
        now,
    );
    dirty
}

#[cfg(test)]
fn pump_outbound_queue(
    pane: &mut MonitorPane,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    bracketed_paste: bool,
    observation: Option<&OutboundObservationResult>,
    now: Instant,
) -> bool {
    pump_outbound_queue_with_gate(
        pane,
        pending_child_input,
        line_state,
        &OutboundReleaseGate::default(),
        bracketed_paste,
        observation,
        now,
    )
}

fn pump_outbound_queue_from_worker(
    pane: &mut MonitorPane,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    release_gate: &OutboundReleaseGate,
    bracketed_paste: bool,
    worker: &OutboundObserverWorker,
) -> bool {
    if let Some(generation_floor) = worker.set_demand(pane.outbound.observation_needed()) {
        pane.outbound.minimum_generation = pane.outbound.minimum_generation.max(generation_floor);
    }
    let latest = worker.latest_result();
    let now = Instant::now();
    let dirty = pump_outbound_queue_with_gate(
        pane,
        pending_child_input,
        line_state,
        release_gate,
        bracketed_paste,
        latest.as_deref(),
        now,
    );
    let _ = worker.set_demand(pane.outbound.observation_needed());
    dirty
}

fn mark_outbound_timeouts(outbound: &mut OutboundQueue, now: Instant) -> bool {
    let mut dirty = false;
    let timed_out: Vec<u64> = outbound
        .messages
        .iter()
        .filter(|message| outbound_message_timed_out(message, now))
        .map(|message| message.id)
        .collect();
    for id in timed_out {
        dirty |= outbound.set_status(
            id,
            OutboundStatus::Ambiguous,
            now,
            Some("consumption_timeout".to_string()),
        );
    }
    dirty
}

fn outbound_message_timed_out(message: &OutboundMessage, now: Instant) -> bool {
    message.status == OutboundStatus::Sent
        && message
            .sent_at
            .is_some_and(|sent| now.duration_since(sent) >= OUTBOUND_CONSUMPTION_TIMEOUT)
}

fn advance_active_outbound_send(
    outbound: &mut OutboundQueue,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    now: Instant,
) -> bool {
    if pending_child_input.pending_len() != 0 {
        return false;
    }
    let Some(active) = outbound.active.clone() else {
        return false;
    };
    match active.phase {
        OutboundSendPhase::Body => {
            advance_outbound_after_body(outbound, pending_child_input, line_state, active, now)
        }
        OutboundSendPhase::DelayUntil(deadline) => advance_outbound_after_delay(
            outbound,
            pending_child_input,
            line_state,
            active,
            deadline,
            now,
        ),
        OutboundSendPhase::Submit => finish_outbound_send(outbound, active.message_id, now),
    }
}

fn advance_outbound_after_body(
    outbound: &mut OutboundQueue,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    mut active: ActiveOutboundSend,
    now: Instant,
) -> bool {
    if active.bracketed_paste {
        active.phase = OutboundSendPhase::DelayUntil(now + CONTROL_SUBMIT_DELAY);
        outbound.active = Some(active);
        false
    } else {
        queue_outbound_submit_delimiter(pending_child_input, line_state);
        active.phase = OutboundSendPhase::Submit;
        outbound.active = Some(active);
        false
    }
}

fn advance_outbound_after_delay(
    outbound: &mut OutboundQueue,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    mut active: ActiveOutboundSend,
    deadline: Instant,
    now: Instant,
) -> bool {
    if now < deadline {
        return false;
    }
    queue_outbound_submit_delimiter(pending_child_input, line_state);
    active.phase = OutboundSendPhase::Submit;
    outbound.active = Some(active);
    false
}

fn finish_outbound_send(outbound: &mut OutboundQueue, id: u64, now: Instant) -> bool {
    outbound.active = None;
    outbound.set_status(id, OutboundStatus::Sent, now, None)
}

fn start_next_outbound_message(
    outbound: &mut OutboundQueue,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    release_gate: &OutboundReleaseGate,
    bracketed_paste: bool,
    observation: Option<&OutboundObservationResult>,
    now: Instant,
) -> bool {
    if outbound.active.is_some() || !pending_child_input.is_empty() {
        return false;
    }
    let Some(id) = outbound.next_sendable_id() else {
        return false;
    };
    if !line_state.input_empty() {
        return outbound.set_status(
            id,
            outbound
                .message(id)
                .map(|message| message.status)
                .unwrap_or(OutboundStatus::Queued),
            now,
            Some("awaiting_line_boundary".to_string()),
        );
    }
    if let Some(detail) = release_gate.blocking_detail(now) {
        return outbound.set_status(
            id,
            outbound
                .message(id)
                .map(|message| message.status)
                .unwrap_or(OutboundStatus::Queued),
            now,
            Some(detail.to_string()),
        );
    }
    if outbound.has_unresolved_blocker()
        && outbound.messages.iter().any(|message| {
            message.id != id
                && matches!(
                    message.status,
                    OutboundStatus::Sending
                        | OutboundStatus::Sent
                        | OutboundStatus::Ambiguous
                        | OutboundStatus::Retrying
                        | OutboundStatus::Failed
                )
        })
    {
        return false;
    }
    let Some(body) = outbound.message(id).map(|message| message.body.clone()) else {
        return false;
    };
    if body.len() > PSEUDO_INPUT_MAX_BYTES {
        return outbound.set_status(
            id,
            OutboundStatus::Failed,
            now,
            Some("oversize_message".to_string()),
        );
    }
    let minimum_generation = outbound
        .message(id)
        .map(|message| message.minimum_generation)
        .unwrap_or_default()
        .max(outbound.minimum_generation);
    let baseline = match observation_baseline(observation, minimum_generation) {
        Ok(baseline) => baseline,
        Err(detail) => {
            return outbound.set_status(
                id,
                outbound
                    .message(id)
                    .map(|message| message.status)
                    .unwrap_or(OutboundStatus::Queued),
                now,
                Some(detail),
            );
        }
    };
    let child_bytes = control_payload_bytes(body.as_bytes(), bracketed_paste);
    line_state.observe_user_input(&child_bytes);
    pending_child_input.enqueue(&child_bytes);
    outbound.mark_sending(id, baseline);
    outbound.active = Some(ActiveOutboundSend {
        message_id: id,
        phase: OutboundSendPhase::Body,
        bracketed_paste,
    });
    true
}

fn observation_baseline(
    result: Option<&OutboundObservationResult>,
    minimum_generation: u64,
) -> Result<OutboundBaseline, String> {
    match result {
        None => Err("awaiting_outbound_observation".to_string()),
        Some(OutboundObservationResult::Unavailable { detail, .. }) => Err(detail.clone()),
        Some(OutboundObservationResult::Failed { detail, .. }) => {
            Err(format!("outbound_observation_failed:{detail}"))
        }
        Some(OutboundObservationResult::Available(observation))
            if observation.generation < minimum_generation =>
        {
            Err("awaiting_fresh_observation".to_string())
        }
        Some(OutboundObservationResult::Available(observation)) if !observation.complete => {
            Err("awaiting_complete_observation".to_string())
        }
        Some(OutboundObservationResult::Available(observation)) => Ok(OutboundBaseline {
            identity: observation.identity.clone(),
            generation: observation.generation,
            turn_ids: observation.turn_ids.clone(),
        }),
    }
}

fn queue_outbound_submit_delimiter(
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
) {
    pending_child_input.enqueue(b"\r");
    line_state.mark_submitted();
}

/// Render one frame to the real terminal.
fn draw_snapshot(
    terminal: &mut Terminal<CrosstermBackend<File>>,
    snapshot: &RenderSnapshot,
) -> Result<(), String> {
    terminal
        .draw(|frame| render_snapshot_frame(frame, snapshot))
        .map(|_| ())
        .map_err(|err| format!("Failed to render TUI frame: {err}"))
}

struct ControlInjectionIo<'a> {
    master_fd: RawFd,
    parser: &'a mut vt100::Parser,
    line_state: &'a mut InputLineState,
    child_output_state: &'a mut ChildOutputState,
    pending_child_input: &'a mut PendingChildInput,
    outbound_release_gate: &'a mut OutboundReleaseGate,
    buffer: &'a mut [u8],
    child_pid: Option<u32>,
}

/// Inject a control-socket notification immediately; the agent harness queues it.
fn service_control(control: &ControlSocket, io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let mut stream = accept_control_stream(control).map_err(format_control_accept_error)?;
    let response = inject_control_payload(&mut stream, io, control);
    let (ack, message) = control_response_message(response);
    super::trace_notify_gate_decision(
        control,
        io.master_fd,
        io.child_pid,
        io.line_state,
        io.child_output_state,
        if ack { "inject" } else { "skip" },
        &message,
    );
    write_tui_control_response(&mut stream, ack, &message)
        .map_err(format_control_response_write_error)
}

fn accept_control_stream(control: &ControlSocket) -> io::Result<UnixStream> {
    control.listener.accept().map(|(stream, _)| stream)
}

fn format_control_accept_error(err: io::Error) -> String {
    format!("Failed to accept PTY control connection: {err}")
}

fn control_response_message(response: Result<Option<String>, String>) -> (bool, String) {
    match response {
        Ok(Some(delivery_nonce)) => (true, pty_delivery_ack_message(&delivery_nonce)),
        Ok(None) => (true, "ok".to_string()),
        Err(message) => (false, message),
    }
}

fn write_tui_control_response(stream: &mut UnixStream, ack: bool, message: &str) -> io::Result<()> {
    write_control_response(stream, ack, message)
}

fn format_control_response_write_error(err: io::Error) -> String {
    format!("Failed to write PTY control response: {err}")
}

fn inject_control_payload(
    stream: &mut UnixStream,
    io: &mut ControlInjectionIo<'_>,
    control: &ControlSocket,
) -> Result<Option<String>, String> {
    validate_control_peer(stream)?;
    let session_id = control
        .session_id()
        .ok_or_else(|| "awaiting_session_identity".to_string())?;
    let payload = prepare_control_payload(
        read_tui_control_payload(stream)?,
        Some((&session_id, control.invocation_uuid())),
    )?;
    if payload.bytes.is_empty() {
        acknowledge_control_payload(&payload)?;
        return Ok(payload.delivery_attempt_id);
    }
    validate_control_input_ready(
        io.parser.screen().bracketed_paste(),
        io.parser.screen().alternate_screen(),
        control.age(),
    )?;
    let bracketed_paste = io.parser.screen().bracketed_paste();
    submit_control_payload(io, &payload.bytes, bracketed_paste)?;
    acknowledge_control_payload(&payload)?;
    Ok(payload.delivery_attempt_id)
}

fn validate_control_input_ready(
    bracketed_paste: bool,
    alternate_screen: bool,
    control_age: Duration,
) -> Result<(), String> {
    if bracketed_paste
        || (!alternate_screen && control_age >= CONTROL_PRIMARY_SCREEN_READY_FALLBACK)
    {
        Ok(())
    } else {
        Err("unsafe_provider_starting".to_string())
    }
}

fn validate_control_peer(stream: &UnixStream) -> Result<(), String> {
    validate_peer_uid(stream)
}

fn read_tui_control_payload(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    read_control_request(stream)
}

fn submit_control_payload(
    io: &mut ControlInjectionIo<'_>,
    payload: &[u8],
    bracketed_paste: bool,
) -> Result<(), String> {
    queue_control_injection(io.pending_child_input, payload, bracketed_paste, false);
    drain_control_payload(io, ControlSubmitDrainPhase::Body)?;
    // Let the child commit the body to its input buffer before Enter. Ink-style TUIs may
    // batch a raw control write as a paste even before they advertise bracketed-paste mode.
    std::thread::sleep(CONTROL_SUBMIT_DELAY);
    io.pending_child_input.enqueue(b"\r");
    io.line_state.mark_submitted();
    drain_control_payload(io, ControlSubmitDrainPhase::Delimiter)
}

#[derive(Clone, Copy)]
enum ControlSubmitDrainPhase {
    Body,
    Delimiter,
}

impl ControlSubmitDrainPhase {
    fn token(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Delimiter => "delimiter",
        }
    }
}

fn drain_control_payload(
    io: &mut ControlInjectionIo<'_>,
    phase: ControlSubmitDrainPhase,
) -> Result<(), String> {
    let start = Instant::now();
    while !io.pending_child_input.is_empty() {
        if start.elapsed() >= INJECT_WAIT_LIMIT {
            return Err(format!("control_submit_{}_drain_timeout", phase.token()));
        }
        let ready = poll_control_submit_pty(io.master_fd)
            .map_err(|err| format!("control_submit_{}_drain_failed:{err}", phase.token()))?;
        if ready.pty_writable {
            flush_pending_child_input(io.master_fd, io.pending_child_input)
                .map_err(|err| format!("control_submit_{}_drain_failed:{err}", phase.token()))?;
        }
        if ready.pty_output {
            if pump_pty_output(io.master_fd, io.parser, io.buffer)
                .map_err(|err| format!("control_submit_{}_drain_failed:{err}", phase.token()))?
            {
                io.child_output_state.observe_child_output();
                io.outbound_release_gate
                    .observe_child_output(false, Instant::now());
            } else {
                return Err(format!("control_submit_{}_pty_closed", phase.token()));
            }
        }
    }
    Ok(())
}

struct ControlSubmitReady {
    pty_output: bool,
    pty_writable: bool,
}

fn poll_control_submit_pty(master_fd: RawFd) -> Result<ControlSubmitReady, String> {
    let mut pollfd = poll_master_fd(master_fd, true);
    poll_fds(
        std::slice::from_mut(&mut pollfd),
        format_control_submit_poll_error,
    )?;
    Ok(ControlSubmitReady {
        pty_output: readable(pollfd.revents),
        pty_writable: writable(pollfd.revents),
    })
}

fn format_control_submit_poll_error(err: io::Error) -> String {
    format!("Failed to poll PTY before control submit: {err}")
}

/// The bytes to inject for a control payload. When the child advertised bracketed-paste
/// mode (DECSET 2004) the (multi-line) body is wrapped in paste markers so an Ink-style
/// TUI treats it as pasted content and the trailing `\r` as a distinct
/// Enter keypress that submits it; without the markers the child batches the whole burst
/// as one paste and absorbs the submit, leaving the notification unsent in the input box.
pub(super) fn control_payload_bytes(payload: &[u8], bracketed_paste: bool) -> Vec<u8> {
    if !bracketed_paste {
        return payload.to_vec();
    }
    let mut bytes =
        Vec::with_capacity(BRACKETED_PASTE_START.len() + payload.len() + BRACKETED_PASTE_END.len());
    bytes.extend_from_slice(BRACKETED_PASTE_START);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(BRACKETED_PASTE_END);
    bytes
}

fn wrap_real_terminal_paste(child_bytes: &[u8]) -> Vec<u8> {
    control_payload_bytes(child_bytes, true)
}

fn child_input_for_real_read(forward: &[u8]) -> Vec<u8> {
    forward.to_vec()
}

#[cfg(test)]
mod tests {
    use super::super::{PtyPair, configure_child_pty};
    use super::*;
    use ratatui::backend::TestBackend;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::FromRawFd;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    fn row_text(buf: &Buffer, area_y: u16, width: u16) -> String {
        (0..width)
            .map(|x| buf[(x, area_y)].symbol().to_string())
            .collect()
    }

    fn screen_row_text(screen: &vt100::Screen, row: u16, width: u16) -> String {
        (0..width)
            .map(|col| {
                screen
                    .cell(row, col)
                    .map(vt100::Cell::contents)
                    .unwrap_or(" ")
                    .to_string()
            })
            .collect()
    }

    fn routed_priority(routed: &RoutedInput) -> RenderPriority {
        routed_input_render_priority(
            routed,
            !routed.forward.is_empty(),
            routed.top_scroll_lines,
            !routed.top_mouse.is_empty(),
            routed.right_click.is_some(),
        )
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    #[test]
    fn top_pane_reserves_the_persistent_overlay_rows() {
        let full = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let top = top_pane_winsize(&full);
        assert_eq!(top.ws_row, 24 - COLLAPSED_MONITOR_ROWS);
        assert_eq!(top.ws_col, 80);
    }

    #[test]
    fn top_pane_never_collapses_below_one_row() {
        let full = libc::winsize {
            ws_row: 1,
            ws_col: 1,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let top = top_pane_winsize(&full);
        assert_eq!(top.ws_row, 1);
        assert_eq!(top.ws_col, 1);
    }

    #[test]
    fn dimensions_gate_rejects_tiny_terminals() {
        let tiny = libc::winsize {
            ws_row: 8,
            ws_col: 30,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let ok = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert!(!dimensions_sufficient(&tiny));
        assert!(dimensions_sufficient(&ok));
    }

    #[test]
    fn top_focus_forwards_ctrl_o_instead_of_toggling_overlay_focus() {
        let mut router = InputRouter::new();
        let routed = router.route_input(&[b'a', 0x0f, b'b']);
        assert_eq!(routed.forward, vec![b'a', 0x0f, b'b']);
        assert!(!routed.redraw);
        assert_eq!(router.focus, Focus::Top);
    }

    #[test]
    fn bottom_focus_edits_draft_and_ctrl_o_is_not_a_focus_command() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        assert_eq!(router.focus, Focus::Bottom);

        let routed = router.route_input(b"jjkk");
        assert!(routed.forward.is_empty());
        assert_eq!(
            routed.pseudo_input,
            vec![
                PseudoInputAction::Insert('j'),
                PseudoInputAction::Insert('j'),
                PseudoInputAction::Insert('k'),
                PseudoInputAction::Insert('k'),
            ]
        );
        assert_eq!(router.focus, Focus::Bottom);

        let routed = router.route_input(&[0x0f]);
        assert!(routed.forward.is_empty());
        assert!(routed.commands.is_empty());
        assert!(routed.pseudo_input.is_empty());
        assert_eq!(router.focus, Focus::Bottom);
    }

    #[test]
    fn terminal_keyboard_protocol_sequences_are_ordered() {
        assert_eq!(
            terminal_keyboard_enable_sequence().as_slice(),
            b"\x1b[>1u\x1b[>4;2m"
        );
        assert_eq!(
            terminal_keyboard_restore_sequence().as_slice(),
            b"\x1b[<u\x1b[>4;0m"
        );
    }

    #[test]
    fn alt_screen_guard_enables_and_restores_keyboard_protocols_on_drop() {
        let file = tempfile::tempfile().unwrap();
        let mut reader = file.try_clone().unwrap();

        {
            let _guard = AltScreenGuard::enter(file).unwrap();
        }

        let mut written = Vec::new();
        reader.seek(SeekFrom::Start(0)).unwrap();
        reader.read_to_end(&mut written).unwrap();

        let enable = terminal_keyboard_enable_sequence();
        let restore = terminal_keyboard_restore_sequence();
        let enable_pos = find_bytes(&written, &enable).expect("keyboard enable sequence");
        let restore_pos = find_bytes(&written, &restore).expect("keyboard restore sequence");
        let leave_pos = find_bytes(&written, b"\x1b[?1049l").expect("leave alternate screen");
        assert!(enable_pos < restore_pos, "restore should follow enable");
        assert!(
            restore_pos < leave_pos,
            "restore should precede alternate-screen exit"
        );
    }

    #[test]
    fn child_bracketed_paste_mode_is_mirrored_only_for_agent_input() {
        let mut parser = vt100::Parser::new(10, 20, 0);
        parser.process(b"\x1b[?2004h");
        let pane = MonitorPane::new();

        let agent_input = RenderSnapshot::capture(parser.screen(), Focus::Top, &pane, None);
        let overlay_input = RenderSnapshot::capture(parser.screen(), Focus::Bottom, &pane, None);

        assert!(agent_input.bracketed_paste);
        assert!(!overlay_input.bracketed_paste);
    }

    #[test]
    fn alt_screen_guard_mirrors_and_restores_bracketed_paste_mode() {
        let file = tempfile::tempfile().unwrap();
        let mut reader = file.try_clone().unwrap();

        {
            let mut guard = AltScreenGuard::enter(file).unwrap();
            guard.sync_bracketed_paste(true).unwrap();
            guard.sync_bracketed_paste(true).unwrap();
        }

        let mut written = Vec::new();
        reader.seek(SeekFrom::Start(0)).unwrap();
        reader.read_to_end(&mut written).unwrap();

        let enable_pos =
            find_bytes(&written, BRACKETED_PASTE_ENABLE).expect("bracketed-paste enable sequence");
        let disable_pos = find_bytes(&written, BRACKETED_PASTE_DISABLE)
            .expect("bracketed-paste disable sequence");
        let leave_pos = find_bytes(&written, b"\x1b[?1049l").expect("leave alternate screen");
        assert_eq!(
            written
                .windows(BRACKETED_PASTE_ENABLE.len())
                .filter(|window| *window == BRACKETED_PASTE_ENABLE)
                .count(),
            1,
            "an unchanged mode must not be written again"
        );
        assert!(enable_pos < disable_pos, "restore should follow enable");
        assert!(
            disable_pos < leave_pos,
            "restore should precede alternate-screen exit"
        );
    }

    #[test]
    fn vt100_mouse_mode_mirrors_to_terminal_decsets_and_restore() {
        let mut parser = vt100::Parser::new(10, 20, 0);
        parser.process(b"\x1b[?1006h\x1b[?1002h");
        let enabled = mouse_request_from_screen(parser.screen());
        assert_eq!(enabled.mode, vt100::MouseProtocolMode::ButtonMotion);
        assert_eq!(enabled.encoding, vt100::MouseProtocolEncoding::Sgr);

        let mut state = TerminalMouseState::new();
        let mut written = Vec::new();
        sync_terminal_mouse(&mut written, &mut state, enabled).unwrap();
        assert_eq!(written, b"\x1b[?1006h\x1b[?1002h");

        written.clear();
        sync_terminal_mouse(&mut written, &mut state, MouseRequest::disabled()).unwrap();
        assert_eq!(written, b"\x1b[?1002l\x1b[?1006l");

        let restore = terminal_mouse_restore_sequence();
        assert!(
            restore
                .windows(MOUSE_PRESS_DISABLE.len())
                .any(|w| w == MOUSE_PRESS_DISABLE)
        );
        assert!(
            restore
                .windows(MOUSE_SGR_DISABLE.len())
                .any(|w| w == MOUSE_SGR_DISABLE)
        );
    }

    #[test]
    fn mouse_event_parser_decodes_sgr_and_legacy_sequences() {
        let sgr = parse_mouse_event(b"\x1b[<65;10;5Mrest").expect("sgr mouse");
        assert_eq!(sgr.consumed, 11);
        assert_eq!(
            sgr.event,
            MouseEvent {
                button: 65,
                col: 10,
                row: 5,
                released: false,
            }
        );

        let legacy_bytes = [0x1b, b'[', b'M', 32 + 64, 32 + 4, 32 + 3, b'x'];
        let legacy = parse_mouse_event(&legacy_bytes).expect("legacy mouse");
        assert_eq!(legacy.consumed, 6);
        assert_eq!(
            legacy.event,
            MouseEvent {
                button: 64,
                col: 4,
                row: 3,
                released: false,
            }
        );
    }

    #[test]
    fn mouse_routing_forwards_top_events_and_consumes_bottom_wheel() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let request = MouseRequest {
            mode: vt100::MouseProtocolMode::ButtonMotion,
            encoding: vt100::MouseProtocolEncoding::Sgr,
        };

        let routed = route_real_input(b"\x1b[<64;4;3M", &mut router, &pane, request, &winsize);
        assert_eq!(routed.forward, b"\x1b[<64;4;3M".to_vec());
        assert!(routed.commands.is_empty());

        let routed = route_real_input(b"\x1b[<65;4;10M", &mut router, &pane, request, &winsize);
        assert!(routed.forward.is_empty());
        assert_eq!(routed.commands, vec![MonitorCommand::SelectNext]);
    }

    #[test]
    fn bottom_status_row_primary_click_toggles_list() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let status = expanded_bottom_layout(areas.bottom, &pane).status;
        let bytes = sgr_press(0, 4, status.y + 1);

        let routed = route_real_input(
            &bytes,
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );

        assert_eq!(router.focus, Focus::Bottom);
        assert!(routed.focus_bottom);
        assert!(routed.redraw);
        assert!(routed.forward.is_empty());
        assert_eq!(routed.commands, vec![MonitorCommand::ToggleList]);
    }

    #[test]
    fn top_pane_primary_click_restores_top_focus_from_bottom() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let mut pane = MonitorPane::new();
        pane.expand();
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(
            b"\x1b[<0;4;3M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );

        assert_eq!(router.focus, Focus::Top);
        assert!(routed.redraw);
        assert!(routed.forward.is_empty());
        assert_eq!(
            routed.top_mouse,
            vec![TopMouse {
                gesture: TopGesture::Press,
                row: 3,
                col: 4,
            }]
        );
    }

    #[test]
    fn top_pane_primary_click_restores_focus_and_forwards_when_child_mouse_enabled() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let mut pane = MonitorPane::new();
        pane.expand();
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let child = MouseRequest {
            mode: vt100::MouseProtocolMode::ButtonMotion,
            encoding: vt100::MouseProtocolEncoding::Sgr,
        };

        let routed = route_real_input(b"\x1b[<0;4;3M", &mut router, &pane, child, &winsize);

        assert_eq!(router.focus, Focus::Top);
        assert_eq!(routed.forward, b"\x1b[<0;4;3M".to_vec());
        assert!(routed.top_mouse.is_empty());
        assert!(routed.commands.is_empty());
    }

    #[test]
    fn bottom_visible_row_primary_click_selects_row_after_scroll_offset() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let pane = pane_with(snapshot_with_nodes(12), false, 9);
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let list = bottom_list_area(areas.bottom, &pane).expect("list area");
        let bytes = sgr_press(0, list.x + 1, list.y + 2);
        let snapshot = pane.snapshot.as_ref().expect("snapshot");
        let projected = pane.projected_rows(snapshot);
        let selected_position = selected_projected_position(&projected, pane.selected).unwrap_or(0);
        let offset = scroll_offset(selected_position, projected.len(), list.height as usize);
        let expected_index = projected[offset + 1].index;

        let routed = route_real_input(
            &bytes,
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );

        assert_eq!(router.focus, Focus::Bottom);
        assert!(routed.focus_bottom);
        assert_eq!(
            routed.commands,
            vec![MonitorCommand::ToggleSelectIndex(expected_index)]
        );
        let mut applied = pane.clone();
        applied.apply(routed.commands[0]);
        assert_eq!(applied.selected, expected_index);
    }

    #[test]
    fn bottom_non_row_primary_click_does_not_select_or_forward() {
        let mut router = InputRouter::new();
        let pane = pane_with(snapshot_with_nodes(1), false, 0);
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let child = MouseRequest {
            mode: vt100::MouseProtocolMode::ButtonMotion,
            encoding: vt100::MouseProtocolEncoding::Sgr,
        };

        let areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let content = expanded_bottom_layout(areas.bottom, &pane).content;
        let bytes = sgr_press(
            0,
            content.x + 1,
            content.y + content.height.saturating_sub(1),
        );

        let routed = route_real_input(&bytes, &mut router, &pane, child, &winsize);

        assert_eq!(router.focus, Focus::Bottom);
        assert!(routed.focus_bottom);
        assert!(routed.commands.is_empty());
        assert!(routed.forward.is_empty());
    }

    #[test]
    fn mouse_routing_preserves_keyboard_behavior_when_child_mouse_is_disabled() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let routed = route_real_input(
            b"abc",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        assert_eq!(routed.forward, b"abc".to_vec());
    }

    #[test]
    fn mouse_wheel_scrolls_monitor_when_expanded_even_if_child_mouse_disabled() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let mut pane = MonitorPane::new();
        pane.expand();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // Wheel-down (button 65) over the bottom monitor pane while the child has no
        // mouse mode: the broker captures it and scrolls the selection.
        let routed = route_real_input(
            b"\x1b[<65;4;8M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        assert!(routed.forward.is_empty());
        assert_eq!(routed.commands, vec![MonitorCommand::SelectNext]);
    }

    #[test]
    fn top_pane_wheel_scrolls_scrollback_when_child_mouse_disabled() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // Wheel-up over the top pane with no child mouse mode: scroll the top-pane
        // scrollback toward older output, without forwarding anything to the child or
        // emitting a monitor command.
        let up = route_real_input(
            b"\x1b[<64;4;3M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        assert!(up.forward.is_empty());
        assert!(up.commands.is_empty());
        assert_eq!(up.top_scroll_lines, TOP_SCROLL_STEP);
        // Wheel-down moves back toward the live tail.
        let down = route_real_input(
            b"\x1b[<65;4;3M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        assert_eq!(down.top_scroll_lines, -TOP_SCROLL_STEP);
    }

    #[test]
    fn broker_always_captures_wheel_regardless_of_child_mouse() {
        // Child mouse off -> broker still captures with at least PressRelease + SGR so the
        // terminal's alternate-scroll mode never turns the wheel into history keystrokes.
        let effective = effective_mouse_request(MouseRequest::disabled());
        assert!(effective.is_enabled());
        assert_eq!(effective.mode, vt100::MouseProtocolMode::ButtonMotion);
        assert_eq!(effective.encoding, vt100::MouseProtocolEncoding::Sgr);
        // A stronger child mode is preserved (so a child's own drag/motion still works).
        let child = MouseRequest {
            mode: vt100::MouseProtocolMode::AnyMotion,
            encoding: vt100::MouseProtocolEncoding::Sgr,
        };
        assert_eq!(
            effective_mouse_request(child).mode,
            vt100::MouseProtocolMode::AnyMotion
        );
    }

    #[test]
    fn top_pane_wheel_forwards_to_child_that_requested_mouse() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let child = MouseRequest {
            mode: vt100::MouseProtocolMode::ButtonMotion,
            encoding: vt100::MouseProtocolEncoding::Sgr,
        };
        // A child in mouse mode owns the wheel: forward it, never steal it for scrollback.
        let routed = route_real_input(b"\x1b[<64;4;3M", &mut router, &pane, child, &winsize);
        assert_eq!(routed.forward, b"\x1b[<64;4;3M".to_vec());
        assert_eq!(routed.top_scroll_lines, 0);
    }

    #[test]
    fn apply_top_scroll_floors_at_live_tail() {
        assert_eq!(apply_top_scroll(0, -TOP_SCROLL_STEP), 0);
        assert_eq!(
            apply_top_scroll(2, TOP_SCROLL_STEP),
            2 + TOP_SCROLL_STEP as usize
        );
        assert_eq!(apply_top_scroll(1, -5), 0);
    }

    #[test]
    fn control_payload_bytes_wraps_only_for_bracketed_paste() {
        // No mode 2004: inject the body verbatim (then a `\r` submit follows).
        assert_eq!(
            control_payload_bytes(b"line1\nline2", false),
            b"line1\nline2".to_vec()
        );
        // Mode 2004 advertised: wrap as a real paste so the trailing Enter submits.
        assert_eq!(
            control_payload_bytes(b"line1\nline2", true),
            b"\x1b[200~line1\nline2\x1b[201~".to_vec()
        );
    }

    #[test]
    fn wrap_real_terminal_paste_adds_one_bracketed_batch() {
        assert_eq!(
            wrap_real_terminal_paste(b"abc"),
            [BRACKETED_PASTE_START, b"abc", BRACKETED_PASTE_END].concat()
        );
    }

    #[test]
    fn real_terminal_input_forwards_byte_for_byte() {
        let burst = vec![b'a'; RELAY_BUFFER_BYTES];
        assert_eq!(child_input_for_real_read(&burst), burst);

        assert_eq!(child_input_for_real_read(b"x"), b"x".to_vec());

        assert!(child_input_for_real_read(&[]).is_empty());
    }

    #[test]
    fn real_terminal_bracketed_paste_markers_are_preserved() {
        let paste = b"\x1b[200~abc\x1b[201~";
        assert_eq!(child_input_for_real_read(paste), paste.to_vec());
    }

    #[test]
    fn control_submit_drains_body_then_final_delimiter_before_returning() {
        for bracketed_paste in [false, true] {
            let (mut read_end, write_end) = pipe_files();
            let mut parser = vt100::Parser::new(10, 20, 0);
            let mut line_state = InputLineState::default();
            let mut child_output_state = ChildOutputState::default();
            let mut pending_child_input = PendingChildInput::new();
            let mut outbound_release_gate = OutboundReleaseGate::default();
            let mut buffer = vec![0_u8; RELAY_BUFFER_BYTES];
            let mut io = ControlInjectionIo {
                master_fd: write_end.as_raw_fd(),
                parser: &mut parser,
                line_state: &mut line_state,
                child_output_state: &mut child_output_state,
                pending_child_input: &mut pending_child_input,
                outbound_release_gate: &mut outbound_release_gate,
                buffer: &mut buffer,
                child_pid: None,
            };

            let started = Instant::now();
            submit_control_payload(&mut io, b"body", bracketed_paste).expect("submit payload");

            assert!(started.elapsed() >= CONTROL_SUBMIT_DELAY);
            assert!(pending_child_input.is_empty());
            let expected = [
                control_payload_bytes(b"body", bracketed_paste).as_slice(),
                b"\r",
            ]
            .concat();
            let mut received = vec![0_u8; expected.len()];
            read_end
                .read_exact(&mut received)
                .expect("body and delimiter should drain before submit returns");
            assert_eq!(received, expected);
        }

        let (_read_end, _write_end) = pipe_files();
        let mut parser = vt100::Parser::new(10, 20, 0);
        let mut line_state = InputLineState::default();
        let mut child_output_state = ChildOutputState::default();
        let mut pending_child_input = PendingChildInput::new();
        pending_child_input.enqueue(b"\r");
        let mut outbound_release_gate = OutboundReleaseGate::default();
        let mut buffer = vec![0_u8; RELAY_BUFFER_BYTES];
        let mut io = ControlInjectionIo {
            master_fd: -1,
            parser: &mut parser,
            line_state: &mut line_state,
            child_output_state: &mut child_output_state,
            pending_child_input: &mut pending_child_input,
            outbound_release_gate: &mut outbound_release_gate,
            buffer: &mut buffer,
            child_pid: None,
        };
        let error = drain_control_payload(&mut io, ControlSubmitDrainPhase::Delimiter)
            .expect_err("a delimiter drain failure must not report success");
        assert!(error.starts_with("control_submit_delimiter_"), "{error}");
    }

    #[test]
    fn control_input_waits_for_harness_readiness_without_terminal_idle_checks() {
        assert_eq!(
            validate_control_input_ready(false, true, Duration::from_secs(30)),
            Err("unsafe_provider_starting".to_string())
        );
        assert_eq!(
            validate_control_input_ready(true, true, Duration::ZERO),
            Ok(())
        );
        assert_eq!(
            validate_control_input_ready(false, false, CONTROL_PRIMARY_SCREEN_READY_FALLBACK,),
            Ok(())
        );
    }

    #[test]
    fn pseudo_input_draft_bytes_do_not_forward_before_enter() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let mut pane = MonitorPane::new();
        let routed = router.route_input(b"draft only");

        assert!(routed.forward.is_empty());
        apply_routed_pseudo_input(&mut pane, &routed.pseudo_input, DEFAULT_PSEUDO_INPUT_WIDTH);

        assert_eq!(pane.pseudo_input.buffer, "draft only");
        assert!(pane.outbound.messages.is_empty());
    }

    #[test]
    fn enter_queues_message_clears_draft_and_scheduler_sends_once() {
        let now = Instant::now();
        let mut pane = MonitorPane::new();
        for action in [
            PseudoInputAction::Insert('h'),
            PseudoInputAction::Insert('i'),
            PseudoInputAction::Submit,
        ] {
            pane.apply_pseudo_input(action, DEFAULT_PSEUDO_INPUT_WIDTH);
        }
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let baseline = available_observation(1, []);

        assert_eq!(pane.pseudo_input.buffer, "");
        assert_eq!(pane.outbound.messages.len(), 1);
        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sending));
        let pending_after_start = pending.pending_len();
        assert_eq!(pending_after_start, 2);

        assert!(!pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        ));
        assert_eq!(
            pending.pending_len(),
            pending_after_start,
            "no duplicate send"
        );
    }

    #[test]
    fn newly_activated_queue_rejects_an_observation_from_the_previous_demand_cycle() {
        let now = Instant::now();
        let mut pane = queued_pane("hello", now);
        pane.outbound.minimum_generation = 2;
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();

        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&available_observation(1, [])),
            now,
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Queued));
        assert!(pending.is_empty());
        assert_eq!(
            pane.outbound
                .message(1)
                .and_then(|message| message.detail.as_deref()),
            Some("awaiting_fresh_observation")
        );

        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&available_observation(2, [])),
            now,
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sending));
        assert_eq!(pending.pending_len(), "hello".len());
    }

    #[test]
    fn queued_message_waits_for_complete_available_baseline_without_writing() {
        let now = Instant::now();
        for observation in [
            None,
            Some(OutboundObservationResult::Available(Box::new(
                OutboundObservation {
                    identity: outbound_identity(),
                    generation: 1,
                    complete: false,
                    turn_count: 1,
                    turn_ids: ["old".to_string()].into_iter().collect(),
                    user_turns: vec![ObservedUserTurn {
                        turn_id: "old".to_string(),
                        timestamp: DateTime::parse_from_rfc3339("2026-05-01T00:00:01Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        body: Some("hello".to_string()),
                    }],
                },
            ))),
            Some(OutboundObservationResult::Unavailable {
                generation: 1,
                detail: "observation_identity_mismatch".to_string(),
            }),
            Some(OutboundObservationResult::Failed {
                generation: 1,
                detail: "host_timeout".to_string(),
            }),
        ] {
            let mut pane = queued_pane("hello", now);
            let mut pending = PendingChildInput::new();
            let mut line_state = InputLineState::default();
            pump_outbound_queue(
                &mut pane,
                &mut pending,
                &mut line_state,
                false,
                observation.as_ref(),
                now,
            );
            assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Queued));
            assert!(pending.is_empty());
        }
    }

    #[test]
    fn queued_message_waits_for_parsed_line_boundary_even_after_idle() {
        let now = Instant::now();
        let mut pane = queued_pane("overlay", now);
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"ordinary draft");
        line_state.last_user_input_at = Some(
            Instant::now() - Duration::from_millis(super::super::USER_INPUT_IDLE_INJECT_MS + 1),
        );

        assert!(
            line_state.user_input_idle(),
            "generic idle tracking remains"
        );
        assert!(!line_state.input_empty(), "draft has no submit boundary");
        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&available_observation(1, [])),
            now,
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Queued));
        assert!(pending.is_empty());
        assert_eq!(
            pane.outbound
                .message(1)
                .and_then(|message| message.detail.as_deref()),
            Some("awaiting_line_boundary")
        );

        line_state.mark_submitted();
        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&available_observation(1, [])),
            now,
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sending));
        assert_eq!(pending.pending_len(), "overlay".len());
    }

    #[test]
    fn queued_message_waits_for_post_submit_child_output_boundary() {
        let now = Instant::now();
        let (mut read_end, write_end) = pipe_files();
        let mut pane = queued_pane("overlay", now);
        let mut pending = PendingChildInput::new();
        let mut deferred = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let mut release_gate = OutboundReleaseGate::default();
        let baseline = available_observation(1, []);

        line_state.observe_user_input(b"ordinary draft");
        let submit = RoutedInput {
            forward: b"\r".to_vec(),
            ..Default::default()
        };
        enqueue_routed_child_input(
            &mut line_state,
            &mut pending,
            &mut deferred,
            &mut release_gate,
            false,
            &submit,
        );
        assert!(line_state.input_empty());
        assert_eq!(pending.pending_len(), 1);

        assert!(!pump_outbound_queue_with_gate(
            &mut pane,
            &mut pending,
            &mut line_state,
            &release_gate,
            false,
            Some(&baseline),
            now,
        ));
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        release_gate.observe_pending_write_drained(pending.is_empty());
        assert_pipe_bytes(&mut read_end, b"\r");

        assert!(pump_outbound_queue_with_gate(
            &mut pane,
            &mut pending,
            &mut line_state,
            &release_gate,
            false,
            Some(&baseline),
            now,
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Queued));
        assert_eq!(
            pane.outbound
                .message(1)
                .and_then(|message| message.detail.as_deref()),
            Some("awaiting_user_boundary_output")
        );

        release_gate.observe_child_output(true, now);
        assert!(pump_outbound_queue_with_gate(
            &mut pane,
            &mut pending,
            &mut line_state,
            &release_gate,
            false,
            Some(&baseline),
            now,
        ));
        assert_eq!(
            pane.outbound
                .message(1)
                .and_then(|message| message.detail.as_deref()),
            Some("awaiting_child_output_quiescence")
        );
        assert!(pending.is_empty());

        assert!(pump_outbound_queue_with_gate(
            &mut pane,
            &mut pending,
            &mut line_state,
            &release_gate,
            false,
            Some(&baseline),
            now + super::super::INJECT_CHILD_OUTPUT_DEBOUNCE + Duration::from_millis(1),
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sending));
        assert_eq!(pending.pending_len(), "overlay".len());
    }

    #[test]
    fn queued_message_transitions_sending_to_sent_after_body_and_enter_drain() {
        let now = Instant::now();
        let (mut read_end, write_end) = pipe_files();
        let mut pane = queued_pane("hello", now);
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let baseline = available_observation(1, []);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"hello");
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"\r");

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        );
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sent));
    }

    #[test]
    fn bracketed_paste_send_reuses_body_delay_before_submit_delimiter() {
        let now = Instant::now();
        let (mut read_end, write_end) = pipe_files();
        let mut pane = queued_pane("hello", now);
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let baseline = available_observation(1, []);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            Some(&baseline),
            now,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, &control_payload_bytes(b"hello", true));
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            Some(&baseline),
            now,
        );
        assert_eq!(pending.pending_len(), 0, "submit waits for paste delay");

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            Some(&baseline),
            now + CONTROL_SUBMIT_DELAY,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"\r");
    }

    #[test]
    fn active_bracketed_send_defers_real_input_until_submit_drains() {
        let now = Instant::now();
        let (mut read_end, write_end) = pipe_files();
        let mut pane = queued_pane("overlay", now);
        let mut pending = PendingChildInput::new();
        let mut deferred = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let mut outbound_release_gate = OutboundReleaseGate::default();
        let baseline = available_observation(1, []);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            Some(&baseline),
            now,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, &control_payload_bytes(b"overlay", true));
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            Some(&baseline),
            now,
        );

        let routed = RoutedInput {
            forward: b"ordinary".to_vec(),
            ..Default::default()
        };
        enqueue_routed_child_input(
            &mut line_state,
            &mut pending,
            &mut deferred,
            &mut outbound_release_gate,
            pane.outbound.active.is_some(),
            &routed,
        );
        assert!(pending.is_empty());
        assert_eq!(deferred.pending_len(), "ordinary".len());

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            Some(&baseline),
            now + CONTROL_SUBMIT_DELAY,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"\r");
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            Some(&baseline),
            now + CONTROL_SUBMIT_DELAY,
        );
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sent));

        release_deferred_child_input(
            pane.outbound.active.is_some(),
            &mut line_state,
            &mut pending,
            &mut deferred,
            &mut outbound_release_gate,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"ordinary");
    }

    #[test]
    fn sent_message_becomes_consumed_on_exact_post_baseline_user_turn_match() {
        let now = Instant::now();
        let mut pane = sent_pane("hello", ["old"], now);
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let observed = available_observation(2, [("old", Some("hello")), ("new", Some("hello"))]);

        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&observed),
            now,
        ));

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Consumed));
    }

    #[test]
    fn duplicate_or_transformed_turns_mark_sent_message_ambiguous() {
        let now = Instant::now();
        let mut duplicate = sent_pane("hello", ["old"], now);
        assert!(apply_outbound_observation(
            &mut duplicate.outbound,
            &available_observation(
                2,
                [
                    ("old", Some("hello")),
                    ("new-1", Some("hello")),
                    ("new-2", Some("hello")),
                ],
            ),
            now,
        ));
        assert_eq!(
            duplicate.outbound.status(1),
            Some(OutboundStatus::Ambiguous)
        );

        let mut transformed = sent_pane("hello", ["old"], now);
        assert!(apply_outbound_observation(
            &mut transformed.outbound,
            &available_observation(2, [("old", Some("hello")), ("new", Some("HELLO"))],),
            now,
        ));
        assert_eq!(
            transformed.outbound.status(1),
            Some(OutboundStatus::Ambiguous)
        );
        assert!(apply_outbound_observation(
            &mut transformed.outbound,
            &available_observation(
                3,
                [
                    ("old", Some("hello")),
                    ("new", Some("HELLO")),
                    ("late", Some("hello")),
                ],
            ),
            now,
        ));
        assert_eq!(
            transformed.outbound.status(1),
            Some(OutboundStatus::Consumed),
            "late exact evidence is evaluated against the original baseline"
        );
    }

    #[test]
    fn unavailable_turn_source_times_out_sent_message_to_ambiguous() {
        let now = Instant::now();
        let mut pane = sent_pane(
            "hello",
            [],
            now - OUTBOUND_CONSUMPTION_TIMEOUT - Duration::from_millis(1),
        );
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let unavailable = OutboundObservationResult::Unavailable {
            generation: 2,
            detail: "session_turn_source_unavailable".to_string(),
        };

        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&unavailable),
            now,
        ));

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Ambiguous));
    }

    #[test]
    fn production_confirmation_timeout_does_not_permanently_block_later_messages() {
        let now = Instant::now();
        let mut pane = sent_pane("first", [], now);
        pane.outbound.enqueue("second".to_string());
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let baseline = available_observation(1, []);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now + OUTBOUND_CONSUMPTION_TIMEOUT,
        );

        assert_eq!(
            (pane.outbound.status(1), pane.outbound.status(2)),
            (
                Some(OutboundStatus::Ambiguous),
                Some(OutboundStatus::Queued)
            ),
            "timeout remains fail-closed and does not forward the later message"
        );
        assert!(pending.is_empty(), "timeout must enqueue no second bytes");

        assert!(
            pane.outbound
                .apply_recovery(1, OutboundRecoveryAction::Discard, 2,)
        );
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now + OUTBOUND_CONSUMPTION_TIMEOUT,
        );
        assert!(!pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now + OUTBOUND_CONSUMPTION_TIMEOUT,
        ));
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Discarded));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Queued));
        assert!(
            pending.is_empty(),
            "pre-discard read cannot cross the barrier"
        );

        let fresh = available_observation(2, []);
        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&fresh),
            now + OUTBOUND_CONSUMPTION_TIMEOUT,
        ));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Sending));
        assert_eq!(pending.pending_len(), "second".len());
    }

    #[test]
    fn confirmed_retry_waits_for_fresh_baseline_and_stays_ahead_of_later_items() {
        let now = Instant::now();
        let mut pane = sent_pane("first", [], now);
        pane.outbound.enqueue("second".to_string());
        pane.outbound.set_status(
            1,
            OutboundStatus::Ambiguous,
            now,
            Some("consumption_timeout".to_string()),
        );
        assert!(
            pane.outbound
                .apply_recovery(1, OutboundRecoveryAction::Retry, 2,)
        );
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&available_observation(1, [])),
            now,
        );
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Retrying));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Queued));
        assert!(pending.is_empty());

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&available_observation(2, [("possibly-old", Some("first"))])),
            now,
        );
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sending));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Queued));
        assert_eq!(pending.pending_len(), "first".len());
    }

    #[test]
    fn single_flight_blocks_later_message_until_first_is_consumed() {
        let now = Instant::now();
        let mut pane = MonitorPane::new();
        pane.outbound.enqueue("first".to_string());
        pane.outbound.enqueue("second".to_string());
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let baseline = available_observation(1, [("old", Some("first"))]);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        );
        drain_pending_without_pipe(&mut pending);
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        );
        drain_pending_without_pipe(&mut pending);
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&baseline),
            now,
        );

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sent));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Queued));
        assert!(pending.is_empty());

        let consumed = available_observation(2, [("old", Some("first")), ("new", Some("first"))]);
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            Some(&consumed),
            now,
        );

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Consumed));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Sending));
    }

    fn queued_pane(body: &str, _now: Instant) -> MonitorPane {
        let mut pane = MonitorPane::new();
        pane.outbound.enqueue(body.to_string());
        pane
    }

    fn sent_pane<const N: usize>(
        body: &str,
        baseline_ids: [&str; N],
        sent_at: Instant,
    ) -> MonitorPane {
        let mut pane = queued_pane(body, sent_at);
        pane.outbound.mark_sending(
            1,
            OutboundBaseline {
                identity: outbound_identity(),
                generation: 1,
                turn_ids: baseline_ids.into_iter().map(str::to_string).collect(),
            },
        );
        pane.outbound
            .set_status(1, OutboundStatus::Sent, sent_at, None);
        pane
    }

    fn outbound_identity() -> OutboundObservationIdentity {
        OutboundObservationIdentity {
            invocation_uuid: "11111111-1111-4111-8111-111111111111".to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-account".to_string(),
            provider_instance_id: Some("fixture-instance".to_string()),
            settings_id: "fixture-settings".to_string(),
            provider_session_id: "fixture-session".to_string(),
            effective_cwd: Some(std::path::PathBuf::from("/fixture")),
        }
    }

    fn available_observation<const N: usize>(
        generation: u64,
        turns: [(&str, Option<&str>); N],
    ) -> OutboundObservationResult {
        let turn_ids = turns
            .iter()
            .map(|(turn_id, _)| (*turn_id).to_string())
            .collect();
        OutboundObservationResult::Available(Box::new(OutboundObservation {
            identity: outbound_identity(),
            generation,
            complete: true,
            turn_count: turns.len() as u64,
            turn_ids,
            user_turns: turns
                .into_iter()
                .map(|(turn_id, body)| ObservedUserTurn {
                    turn_id: turn_id.to_string(),
                    timestamp: DateTime::parse_from_rfc3339("2026-05-01T00:00:01Z")
                        .unwrap()
                        .with_timezone(&Utc),
                    body: body.map(str::to_string),
                })
                .collect(),
        }))
    }

    fn assert_pipe_bytes(read_end: &mut File, expected: &[u8]) {
        let mut received = vec![0_u8; expected.len()];
        read_end.read_exact(&mut received).unwrap();
        assert_eq!(received, expected);
    }

    fn drain_pending_without_pipe(pending: &mut PendingChildInput) {
        let (read_end, write_end) = pipe_files();
        set_nonblocking(read_end.as_raw_fd());
        set_nonblocking(write_end.as_raw_fd());
        let mut received = Vec::new();
        while !pending.is_empty() {
            pending.flush_some(write_end.as_raw_fd()).unwrap();
            drain_available(read_end.as_raw_fd(), &mut received).unwrap();
        }
    }

    fn drain_available(read_fd: RawFd, output: &mut Vec<u8>) -> io::Result<usize> {
        let mut total = 0;
        let mut buffer = [0_u8; 8192];
        loop {
            let rc = unsafe { libc::read(read_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if rc > 0 {
                let n = rc as usize;
                output.extend_from_slice(&buffer[..n]);
                total += n;
                continue;
            }
            if rc == 0 {
                return Ok(total);
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(code) if code == libc::EINTR => continue,
                Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => {
                    return Ok(total);
                }
                _ => return Err(err),
            }
        }
    }

    fn mouse(button: u16, col: u16, row: u16, released: bool) -> MouseEvent {
        MouseEvent {
            button,
            col,
            row,
            released,
        }
    }

    fn sgr_press(button: u16, col: u16, row: u16) -> Vec<u8> {
        encode_sgr_mouse_event(mouse(button, col, row, false))
    }

    #[test]
    fn top_selection_gesture_classifies_left_button() {
        // Left press (button 0, 'M'), left drag (motion bit 32), left release ('m').
        assert_eq!(
            top_selection_gesture(mouse(0, 3, 2, false)),
            Some(TopGesture::Press)
        );
        assert_eq!(
            top_selection_gesture(mouse(32, 6, 4, false)),
            Some(TopGesture::Drag)
        );
        assert_eq!(
            top_selection_gesture(mouse(0, 6, 4, true)),
            Some(TopGesture::Release)
        );
        // Right button (base bits != left) is ignored by the selection machine.
        assert_eq!(top_selection_gesture(mouse(2, 6, 4, false)), None);
    }

    #[test]
    fn top_left_drag_emits_selection_gestures_when_child_mouse_disabled() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // Left press in the top pane, then a drag: both become selection gestures, nothing
        // is forwarded to the child or turned into scroll/monitor commands.
        let routed = route_real_input(
            b"\x1b[<0;3;2M\x1b[<32;7;2M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        assert!(routed.forward.is_empty());
        assert_eq!(routed.top_scroll_lines, 0);
        assert_eq!(
            routed.top_mouse,
            vec![
                TopMouse {
                    gesture: TopGesture::Press,
                    row: 2,
                    col: 3
                },
                TopMouse {
                    gesture: TopGesture::Drag,
                    row: 2,
                    col: 7
                },
            ]
        );
    }

    #[test]
    fn selection_span_normalizes_to_zero_based_reading_order() {
        // Anchor below/right of head -> span is reordered and converted to 0-based cells.
        let sel = TopSelection {
            anchor: (3, 5),
            head: (2, 2),
            active: false,
            dragged: true,
            scrollback_at: 0,
        };
        let span = selection_span(&sel);
        assert_eq!(span.start, (1, 1));
        assert_eq!(span.end, (2, 4));
        assert!(cell_in_selection(span, 1, 1));
        assert!(cell_in_selection(span, 1, 9)); // mid first row extends to line end
        assert!(cell_in_selection(span, 2, 4));
        assert!(!cell_in_selection(span, 2, 5)); // past the end column on the last row
        assert!(!cell_in_selection(span, 0, 9)); // before the first row
    }

    #[test]
    fn is_right_press_only_for_right_button_down() {
        assert!(is_right_press(mouse(2, 5, 3, false)));
        assert!(!is_right_press(mouse(2, 5, 3, true))); // release, not press
        assert!(!is_right_press(mouse(0, 5, 3, false))); // left button
        assert!(!is_right_press(mouse(34, 5, 3, false))); // right + motion (drag)
    }

    #[test]
    fn right_click_records_pane_local_position_when_child_mouse_disabled() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let routed = route_real_input(
            b"\x1b[<2;5;3M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        assert_eq!(routed.right_click, Some((3, 5)));
        assert!(routed.forward.is_empty());
        assert!(routed.top_mouse.is_empty());
    }

    #[test]
    fn visible_selection_span_follows_scroll_and_drops_offscreen() {
        let sel = TopSelection {
            anchor: (3, 2), // 1-based -> base rows 1..=4 (0-based)
            head: (5, 6),
            active: false,
            dragged: true,
            scrollback_at: 0,
        };
        // No scroll: visible at the base rows.
        let here = visible_selection_span(&sel, 0, 10).expect("visible");
        assert_eq!(here.start.0, 2);
        assert_eq!(here.end.0, 4);
        // Scrolled up by 3: the highlight moves down with the content.
        let scrolled = visible_selection_span(&sel, 3, 10).expect("shifted");
        assert_eq!(scrolled.start.0, 5);
        assert_eq!(scrolled.end.0, 7);
        // Scrolled far enough that the whole selection is below the viewport.
        assert!(visible_selection_span(&sel, 20, 10).is_none());
    }

    #[test]
    fn extract_selection_text_reads_screen_range_and_trims() {
        let mut parser = vt100::Parser::new(4, 20, 0);
        parser.process(b"hello\r\nworld wide");
        let span = SelectionSpan {
            start: (0, 0),
            end: (1, 4),
        };
        assert_eq!(
            extract_selection_text(parser.screen(), span),
            "hello\nworld"
        );
    }

    #[test]
    fn mouse_encoder_targets_child_requested_encoding() {
        let event = MouseEvent {
            button: 64,
            col: 4,
            row: 3,
            released: false,
        };
        assert_eq!(
            encode_mouse_event(event, vt100::MouseProtocolEncoding::Sgr).unwrap(),
            b"\x1b[<64;4;3M".to_vec()
        );
        assert_eq!(
            encode_mouse_event(event, vt100::MouseProtocolEncoding::Default).unwrap(),
            vec![0x1b, b'[', b'M', 96, 36, 35]
        );
        assert_eq!(
            encode_mouse_event(event, vt100::MouseProtocolEncoding::Utf8).unwrap(),
            vec![0x1b, b'[', b'M', 96, 36, 35]
        );
    }

    #[test]
    fn vt_color_maps_default_indexed_and_rgb() {
        assert_eq!(vt_color(vt100::Color::Default), Color::Reset);
        assert_eq!(vt_color(vt100::Color::Idx(5)), Color::Indexed(5));
        assert_eq!(vt_color(vt100::Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
    }

    #[test]
    fn frame_renders_child_output_in_top_and_monitor_in_bottom() {
        let backend = TestBackend::new(20, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        // top pane is 5 rows (10 - persistent overlay rows), 20 cols.
        let mut parser = vt100::Parser::new(5, 20, 0);
        parser.process(b"hello world");
        let screen_owner = parser;
        let screen = screen_owner.screen();
        let pane = MonitorPane::new();
        terminal
            .draw(|frame| render_frame(frame, screen, Focus::Top, &pane, None))
            .unwrap();

        let buf = terminal.backend().buffer();
        let top_row = row_text(buf, 0, 20);
        assert!(top_row.starts_with("hello world"), "top row: {top_row:?}");
        let text = screen_text(buf, 10, 20);
        assert!(text.contains("OBS"), "screen: {text:?}");
    }

    #[test]
    fn render_cadence_tracks_overlay_focus() {
        let parser = vt100::Parser::new(5, 80, 0);
        let background =
            RenderSnapshot::capture(parser.screen(), Focus::Top, &MonitorPane::new(), None);
        assert_eq!(snapshot_render_fps(&background), BACKGROUND_RENDER_FPS);

        let mut pane = MonitorPane::new();
        pane.expand();
        let foreground = RenderSnapshot::capture(parser.screen(), Focus::Bottom, &pane, None);
        assert_eq!(snapshot_render_fps(&foreground), FOREGROUND_RENDER_FPS);
    }

    #[test]
    fn top_focus_recent_input_is_interactive_render_priority() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(b"a", &mut router, &pane, MouseRequest::disabled(), &winsize);

        assert_eq!(routed_priority(&routed), RenderPriority::Interactive);
    }

    #[test]
    fn top_scrollback_change_is_interactive_render_priority() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(
            b"\x1b[<64;4;3M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );

        assert_eq!(routed.top_scroll_lines, TOP_SCROLL_STEP);
        assert_eq!(routed_priority(&routed), RenderPriority::Interactive);
    }

    #[test]
    fn lower_pane_pseudo_input_move_is_interactive_render_priority() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;

        let routed = router.route_input(&[0x1b, b'[', b'B']);

        assert_eq!(routed.pseudo_input, vec![PseudoInputAction::MoveDown]);
        assert_eq!(routed_priority(&routed), RenderPriority::Interactive);
    }

    #[test]
    fn child_output_priority_reaches_foreground_without_bottom_focus() {
        let parser = vt100::Parser::new(5, 80, 0);
        let snapshot = RenderSnapshot::capture_with_typing_protection_and_priority(
            parser.screen(),
            Focus::Top,
            &MonitorPane::new(),
            None,
            TypingProtection::for_focus(Focus::Top),
            RenderPriority::Interactive,
        );

        assert_eq!(snapshot_render_fps(&snapshot), FOREGROUND_RENDER_FPS);
    }

    #[test]
    fn interactive_publish_deadline_preempts_stale_background_wait() {
        let parser = vt100::Parser::new(5, 80, 0);
        let pane = MonitorPane::new();
        let background = RenderSnapshot::capture_with_typing_protection_and_priority(
            parser.screen(),
            Focus::Top,
            &pane,
            None,
            TypingProtection::for_focus(Focus::Top),
            RenderPriority::Background,
        );
        let interactive = RenderSnapshot::capture_with_typing_protection_and_priority(
            parser.screen(),
            Focus::Top,
            &pane,
            None,
            TypingProtection::for_focus(Focus::Top),
            RenderPriority::Interactive,
        );
        let last_frame = Instant::now();
        let now = last_frame + Duration::from_millis(5);

        let background_deadline = unrendered_publish_deadline(Some(last_frame), &background, now);
        let interactive_deadline = unrendered_publish_deadline(Some(last_frame), &interactive, now);

        assert_eq!(
            background_deadline.duration_since(last_frame),
            render_frame_interval(BACKGROUND_RENDER_FPS)
        );
        assert_eq!(
            interactive_deadline.duration_since(last_frame),
            render_frame_interval(FOREGROUND_RENDER_FPS)
        );
        assert!(interactive_deadline < background_deadline);
    }

    #[test]
    fn background_publish_deadline_remains_throttled() {
        let parser = vt100::Parser::new(5, 80, 0);
        let snapshot =
            RenderSnapshot::capture(parser.screen(), Focus::Top, &MonitorPane::new(), None);
        let last_frame = Instant::now();
        let now = last_frame + Duration::from_millis(5);

        let deadline = unrendered_publish_deadline(Some(last_frame), &snapshot, now);

        assert_eq!(
            deadline.duration_since(last_frame),
            render_frame_interval(BACKGROUND_RENDER_FPS)
        );
    }

    #[test]
    fn child_output_burst_coalescing_preserves_final_screen_state() {
        let (read_end, mut write_end) = pipe_files();
        write_end.write_all(b"abcdef").unwrap();
        let mut parser = vt100::Parser::new(1, 20, 0);
        let mut buffer = [0_u8; 2];

        assert!(pump_pty_output_burst(read_end.as_raw_fd(), &mut parser, &mut buffer).unwrap());

        assert_eq!(screen_row_text(parser.screen(), 0, 6), "abcdef");
    }

    #[test]
    fn top_wheel_burst_coalesces_to_final_scroll_delta() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(
            b"\x1b[<64;4;3M\x1b[<64;4;3M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );

        assert_eq!(routed.top_scroll_lines, TOP_SCROLL_STEP * 2);
        assert_eq!(routed_priority(&routed), RenderPriority::Interactive);
    }

    #[test]
    fn render_thread_keeps_drawing_while_processing_side_is_busy() {
        let outer = open_outer_pty(24, 80);
        make_raw(outer.slave.as_raw_fd());
        let mut drain_master = outer.master.try_clone().expect("clone drain master");
        set_nonblocking(drain_master.as_raw_fd());
        let done = Arc::new(AtomicBool::new(false));
        let done_reader = Arc::clone(&done);
        let reader = thread::spawn(move || {
            let mut buf = [0_u8; 8192];
            while !done_reader.load(Ordering::SeqCst) {
                match drain_master.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        let renderer = RenderThread::start(outer.slave.try_clone().expect("clone render writer"))
            .expect("start renderer");
        let publisher = renderer.publisher();
        let mut pane = MonitorPane::new();
        pane.expand();
        let mut parser = vt100::Parser::new(10, 80, 0);
        parser.process(b"responsive input echo");
        publish_render_snapshot(
            &publisher,
            &parser,
            Focus::Bottom,
            &pane,
            None,
            TypingProtection::for_focus(Focus::Bottom),
            RenderPriority::Interactive,
        );

        let before = publisher.frame_count();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(350) {
            std::hint::spin_loop();
        }
        let frames = publisher.frame_count().saturating_sub(before);

        renderer.shutdown_and_join().expect("renderer shutdown");
        done.store(true, Ordering::SeqCst);
        reader.join().expect("reader thread");
        assert!(
            frames >= 3,
            "render thread should keep drawing while processing is busy; frames={frames}"
        );
    }

    struct OuterPty {
        master: File,
        slave: File,
    }

    fn open_outer_pty(rows: u16, cols: u16) -> OuterPty {
        let mut master_fd = -1;
        let mut slave_fd = -1;
        let ws = libc::winsize {
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
                &ws,
            )
        };
        assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());
        OuterPty {
            master: unsafe { File::from_raw_fd(master_fd) },
            slave: unsafe { File::from_raw_fd(slave_fd) },
        }
    }

    fn tty_termios(fd: RawFd) -> libc::termios {
        let mut attrs = unsafe { std::mem::zeroed::<libc::termios>() };
        unsafe { libc::tcgetattr(fd, &mut attrs) };
        attrs
    }

    fn make_raw(fd: RawFd) {
        let mut attrs = tty_termios(fd);
        unsafe { libc::cfmakeraw(&mut attrs) };
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &attrs) };
    }

    fn set_nonblocking(fd: RawFd) {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    }

    fn pipe_files() -> (File, File) {
        let mut fds = [0; 2];
        let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "pipe failed: {}", std::io::Error::last_os_error());
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    /// Strip ANSI control sequences so the rendered terminal stream can be
    /// substring-checked for painted text.
    fn strip_ansi(bytes: &[u8]) -> String {
        ansi_visible_tokens(bytes).into_iter().collect()
    }

    fn ansi_visible_tokens(bytes: &[u8]) -> Vec<char> {
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            let token = next_ansi_token(bytes, i);
            out.extend(ansi_token_chars(&token));
            i = token.next_index;
        }
        out.chars().collect()
    }

    struct AnsiToken {
        visible: Option<char>,
        next_index: usize,
    }

    fn next_ansi_token(bytes: &[u8], index: usize) -> AnsiToken {
        match bytes[index] {
            0x1b => ansi_escape_token(bytes, index),
            b'\r' | b'\n' => visible_ansi_token(' ', index + 1),
            b if printable_ascii(b) => visible_ansi_token(b as char, index + 1),
            _ => hidden_ansi_token(index + 1),
        }
    }

    fn ansi_escape_token(bytes: &[u8], index: usize) -> AnsiToken {
        let next = match bytes.get(index + 1) {
            Some(b'[') => csi_escape_end(bytes, index + 2),
            Some(b']') => osc_escape_end(bytes, index + 2),
            Some(_) => index + 2,
            None => index + 1,
        };
        hidden_ansi_token(next)
    }

    fn csi_escape_end(bytes: &[u8], mut index: usize) -> usize {
        while index < bytes.len() && !(0x40..=0x7e).contains(&bytes[index]) {
            index += 1;
        }
        index.saturating_add(1)
    }

    fn osc_escape_end(bytes: &[u8], mut index: usize) -> usize {
        while index < bytes.len() && bytes[index] != 0x07 && bytes[index] != 0x1b {
            index += 1;
        }
        if bytes.get(index) == Some(&0x1b) {
            index += 1;
        }
        index.saturating_add(1)
    }

    fn printable_ascii(byte: u8) -> bool {
        (0x20..0x7f).contains(&byte)
    }

    fn visible_ansi_token(visible: char, next_index: usize) -> AnsiToken {
        AnsiToken {
            visible: Some(visible),
            next_index,
        }
    }

    fn hidden_ansi_token(next_index: usize) -> AnsiToken {
        AnsiToken {
            visible: None,
            next_index,
        }
    }

    fn ansi_token_chars(token: &AnsiToken) -> impl Iterator<Item = char> {
        token.visible.into_iter()
    }

    // Outer-PTY end-to-end proof: the relay gives the child a real terminal,
    // forwards typed input through the mux to the child, propagates the child's
    // exit status, and paints the collapsed monitor row to the real terminal.
    #[test]
    fn observed_relay_gives_child_a_tty_forwards_input_and_renders_monitor() {
        let outer = open_outer_pty(24, 80);
        make_raw(outer.slave.as_raw_fd());
        let full = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let termios = tty_termios(outer.slave.as_raw_fd());
        let pty = PtyPair::open(&top_pane_winsize(&full), &termios).expect("inner pty");

        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(
            r#"[ -t 0 ] || exit 7; IFS= read -r -t 5 line || exit 6; [ "$line" = "ping" ] && exit 42 || exit 8"#,
        );
        configure_child_pty(&mut cmd, &pty).expect("configure child pty");
        let child = cmd.spawn().expect("spawn child");
        drop(pty.slave);

        let writer = outer.slave.try_clone().expect("clone writer");
        let input_fd = outer.slave.as_raw_fd();
        let master = pty.master;
        let done = Arc::new(AtomicBool::new(false));
        let done_relay = Arc::clone(&done);
        let monitor = Box::new(FakeMonitor::new(empty_snapshot()));
        let root = ObservabilityRoot::default();
        let relay = thread::spawn(move || {
            let mut child = child;
            let result = relay_until_exit_observed(
                input_fd,
                writer,
                &master,
                None,
                &mut child,
                monitor,
                root,
                OutboundObserverSource::Unavailable("test_observer_unavailable".to_string()),
            );
            done_relay.store(true, Ordering::SeqCst);
            result
        });

        // Read rendered frames continuously (so the PTY buffer never blocks the
        // relay) and inject the line once the child has had time to reach `read`.
        set_nonblocking(outer.master.as_raw_fd());
        let mut rendered = Vec::new();
        let mut buf = [0_u8; 8192];
        let start = Instant::now();
        let mut injected = false;
        loop {
            if let Ok(n) = (&outer.master).read(&mut buf)
                && n > 0
            {
                rendered.extend_from_slice(&buf[..n]);
            }
            if !injected && start.elapsed() >= Duration::from_millis(200) {
                (&outer.master).write_all(b"ping\n").expect("write input");
                injected = true;
            }
            if done.load(Ordering::SeqCst) {
                while let Ok(n) = (&outer.master).read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    rendered.extend_from_slice(&buf[..n]);
                }
                break;
            }
            if start.elapsed() > Duration::from_secs(10) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }

        let status = relay
            .join()
            .expect("relay thread panicked")
            .expect("relay error");
        assert_eq!(
            status.code(),
            Some(42),
            "child must observe a tty (-t 0) and receive the forwarded line through the mux"
        );
        let stripped = strip_ansi(&rendered);
        assert!(
            stripped.contains("OBS"),
            "collapsed monitor row should be painted to the real terminal; stripped: {stripped:?}"
        );
    }

    #[test]
    fn observed_relay_forwards_input_while_snapshot_provider_is_slow() {
        let outer = open_outer_pty(24, 80);
        make_raw(outer.slave.as_raw_fd());
        let full = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let termios = tty_termios(outer.slave.as_raw_fd());
        let pty = PtyPair::open(&top_pane_winsize(&full), &termios).expect("inner pty");
        let observation_dir = tempfile::tempdir().expect("observation tempdir");
        let observation_path = observation_dir.path().join("input-observed");

        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(
            r#"[ -t 0 ] || exit 7; IFS= read -r -t 5 line || exit 6; [ "$line" = "ping" ] || exit 8; printf observed > "$OBSERVED_INPUT_PATH"; exit 42"#,
        );
        cmd.env("OBSERVED_INPUT_PATH", &observation_path);
        configure_child_pty(&mut cmd, &pty).expect("configure child pty");
        let child = cmd.spawn().expect("spawn child");
        drop(pty.slave);

        let writer = outer.slave.try_clone().expect("clone writer");
        let input_fd = outer.slave.as_raw_fd();
        let master = pty.master;
        let (snapshot_entered_tx, snapshot_entered_rx) = mpsc::sync_channel(1);
        let snapshot_release = Arc::new((Mutex::new(false), Condvar::new()));
        let monitor = Box::new(BlockingMonitor::new(
            empty_snapshot(),
            snapshot_entered_tx,
            Arc::clone(&snapshot_release),
        ));
        let root = ObservabilityRoot::default();
        let (relay_done_tx, relay_done_rx) = mpsc::sync_channel(1);
        let relay = thread::spawn(move || {
            let mut child = child;
            let result = relay_until_exit_observed(
                input_fd,
                writer,
                &master,
                None,
                &mut child,
                monitor,
                root,
                OutboundObserverSource::Unavailable("test_observer_unavailable".to_string()),
            );
            let _ = relay_done_tx.send(result);
        });

        set_nonblocking(outer.master.as_raw_fd());
        let mut buf = [0_u8; 8192];
        snapshot_entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("snapshot worker should enter the blocking provider");
        (&outer.master).write_all(b"ping\n").expect("write input");
        let start = Instant::now();
        while !observation_path.exists() && start.elapsed() < Duration::from_secs(2) {
            let _ = (&outer.master).read(&mut buf);
            thread::sleep(Duration::from_millis(5));
        }
        let input_observed_while_snapshot_blocked = observation_path.exists();

        let relay_result = relay_done_rx.recv_timeout(Duration::from_millis(500));
        let settled_before_snapshot_release = relay_result.is_ok();

        let (released, wake) = &*snapshot_release;
        *released.lock().expect("snapshot release lock") = true;
        wake.notify_all();

        let result = relay_result.unwrap_or_else(|_| {
            relay_done_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("relay should settle after snapshot release")
        });
        relay.join().expect("relay thread panicked");
        let status = result.expect("relay error");
        assert!(
            input_observed_while_snapshot_blocked,
            "relay must forward input without waiting for the blocked snapshot provider"
        );
        assert!(
            settled_before_snapshot_release,
            "child settlement must not wait for best-effort snapshot observation"
        );
        assert_eq!(
            status.code(),
            Some(42),
            "child read timeout proves input was forwarded without waiting for the slow snapshot"
        );
    }

    // The child PTY is resized to the TOP pane (persistent overlay rows reserved) on terminal
    // resize, and the virtual terminal grid is resized to match.
    #[test]
    fn resize_sizes_child_pty_and_virtual_terminal_to_top_pane() {
        let outer = open_outer_pty(30, 100);
        let initial = libc::winsize {
            ws_row: 10,
            ws_col: 10,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let termios = tty_termios(outer.slave.as_raw_fd());
        let pty = PtyPair::open(&initial, &termios).expect("inner pty");
        let mut parser = vt100::Parser::new(10, 10, 0);
        let mut applied = None;

        // Collapsed: child PTY reserves the persistent overlay rows (30 -> 25).
        let pane = MonitorPane::new();
        let dirty = apply_sizing(
            outer.slave.as_raw_fd(),
            pty.master.as_raw_fd(),
            std::process::id(),
            &pane,
            TypingProtection::inactive(),
            &mut parser,
            &mut applied,
        );
        assert!(dirty, "first observation of a size is a change");
        assert_eq!(
            parser.screen().size(),
            (30 - COLLAPSED_MONITOR_ROWS, 100),
            "collapsed virtual terminal"
        );
        let mut ws = unsafe { std::mem::zeroed::<libc::winsize>() };
        let rc = unsafe { libc::ioctl(pty.master.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
        assert_eq!(rc, 0);
        assert_eq!(
            (ws.ws_row, ws.ws_col),
            (30 - COLLAPSED_MONITOR_ROWS, 100),
            "collapsed child PTY"
        );

        // Expanding the monitor shrinks the top pane and the child PTY.
        let mut expanded = MonitorPane::new();
        expanded.expand();
        let protection = TypingProtection::inactive();
        let bottom = expanded.bottom_rows(30, 100, protection);
        let top_rows = 30 - bottom;
        let dirty = apply_sizing(
            outer.slave.as_raw_fd(),
            pty.master.as_raw_fd(),
            std::process::id(),
            &expanded,
            protection,
            &mut parser,
            &mut applied,
        );
        assert!(dirty, "expanding reserves more rows");
        assert_eq!(
            parser.screen().size(),
            (top_rows, 100),
            "expanded virtual terminal"
        );
        let rc = unsafe { libc::ioctl(pty.master.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
        assert_eq!(rc, 0);
        assert_eq!(
            (ws.ws_row, ws.ws_col),
            (top_rows, 100),
            "expanded child PTY"
        );
    }

    // ---- WU-3 monitor pane fixtures ----

    fn empty_snapshot() -> MonitorSnapshot {
        MonitorSnapshot {
            generated_at: std::time::SystemTime::UNIX_EPOCH,
            root_invocation_uuid: None,
            active_session_id: None,
            summary: snapshot_summary(MonitorStatus::Idle, 0, 0, 0, 0),
            nodes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn snapshot_summary(
        status: MonitorStatus,
        running: usize,
        bash: usize,
        pending: usize,
        diag: usize,
    ) -> crate::observability::MonitorSummary {
        crate::observability::MonitorSummary {
            status,
            total_nodes: 0,
            invocation_nodes: 0,
            running_nodes: running,
            pending_mailbox_count: pending,
            running_agent_bash_count: bash,
            diagnostics_count: diag,
        }
    }

    fn node(id: &str, kind: MonitorNodeKind, status: MonitorStatus, label: &str) -> MonitorNode {
        MonitorNode {
            id: id.to_string(),
            parent_id: None,
            kind,
            label: label.to_string(),
            status,
            pid: None,
            pgid: None,
            liveness: LivenessStatus::NotApplicable,
            started_at: None,
            updated_at: None,
            completed_at: None,
            last_output_excerpt: None,
            inspect_ref: None,
            cancel_ref: None,
            wake: None,
            mailbox: None,
        }
    }

    fn parented(mut node: MonitorNode, parent_id: &str) -> MonitorNode {
        node.parent_id = Some(parent_id.to_string());
        node
    }

    fn snapshot_with_nodes(count: usize) -> MonitorSnapshot {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = (0..count)
            .map(|index| {
                node(
                    &format!("node:{index}"),
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    &format!("node {index}"),
                )
            })
            .collect();
        snapshot
    }

    fn projected_ids(snapshot: &MonitorSnapshot, mode: MonitorViewMode) -> Vec<String> {
        projected_monitor_rows(snapshot, mode)
            .into_iter()
            .map(|row| snapshot.nodes[row.index].id.clone())
            .collect()
    }

    fn projected_labels(snapshot: &MonitorSnapshot, mode: MonitorViewMode) -> Vec<String> {
        projected_monitor_rows(snapshot, mode)
            .into_iter()
            .map(|row| format!("{}{}", row.prefix, snapshot.nodes[row.index].label))
            .collect()
    }

    fn filtered_labels(pane: &MonitorPane, snapshot: &MonitorSnapshot) -> Vec<String> {
        pane.projected_rows(snapshot)
            .into_iter()
            .map(|row| snapshot.nodes[row.index].label.clone())
            .collect()
    }

    fn filtered_labels_for_pane(pane: &MonitorPane) -> Vec<String> {
        let snapshot = pane.snapshot.as_ref().expect("snapshot");
        filtered_labels(pane, snapshot)
    }

    struct FakeMonitor {
        snapshot: MonitorSnapshot,
    }

    impl FakeMonitor {
        fn new(snapshot: MonitorSnapshot) -> Self {
            Self { snapshot }
        }
    }

    impl ObservabilitySnapshotPort for FakeMonitor {
        fn snapshot_with_cancel(
            &self,
            _root: &ObservabilityRoot,
            _limits: SnapshotLimits,
            _cancellation: &CancellationToken,
        ) -> MonitorSnapshot {
            self.snapshot.clone()
        }
    }

    struct BlockingMonitor {
        snapshot: MonitorSnapshot,
        entered: mpsc::SyncSender<()>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl BlockingMonitor {
        fn new(
            snapshot: MonitorSnapshot,
            entered: mpsc::SyncSender<()>,
            release: Arc<(Mutex<bool>, Condvar)>,
        ) -> Self {
            Self {
                snapshot,
                entered,
                release,
            }
        }

        fn wait_for_release(&self, cancellation: &CancellationToken) -> MonitorSnapshot {
            let _ = self.entered.try_send(());
            let (released, wake) = &*self.release;
            let mut guard = released.lock().expect("snapshot release lock");
            while !*guard && !cancellation.is_cancelled() {
                guard = wake
                    .wait_timeout(guard, Duration::from_millis(5))
                    .expect("snapshot release wait")
                    .0;
            }
            self.snapshot.clone()
        }
    }

    impl ObservabilitySnapshotPort for BlockingMonitor {
        fn snapshot_with_cancel(
            &self,
            _root: &ObservabilityRoot,
            _limits: SnapshotLimits,
            cancellation: &CancellationToken,
        ) -> MonitorSnapshot {
            self.wait_for_release(cancellation)
        }
    }

    fn pane_with(snapshot: MonitorSnapshot, collapsed: bool, selected: usize) -> MonitorPane {
        let selected_node_id = snapshot.nodes.get(selected).map(node_id);
        MonitorPane {
            collapsed,
            view_mode: MonitorViewMode::Flat,
            active_filter: MonitorFilterCategory::All,
            selected,
            selected_node_id,
            snapshot: Some(Arc::new(snapshot)),
            pseudo_input: PseudoInputState::default(),
            outbound: OutboundQueue::default(),
            inspecting: false,
            closed_detail_node_id: None,
            inspect: Vec::new(),
            last_inspect_refresh: None,
            pending_cancel: None,
            cancel_request: None,
            cancel_feedback: None,
            pending_outbound_recovery: None,
            outbound_recovery_request: None,
            outbound_recovery_feedback: None,
        }
    }

    fn screen_text(buf: &Buffer, height: u16, width: u16) -> String {
        (0..height)
            .map(|y| row_text(buf, y, width))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn inspect_lines_for_transcript(
        path: &str,
        format_id: Option<&str>,
        source_id: Option<&str>,
    ) -> Vec<String> {
        let mut inspected = node(
            "session:inspect",
            MonitorNodeKind::Session,
            MonitorStatus::Running,
            "inspect session",
        );
        inspected.inspect_ref = Some(InspectRef::SessionTranscript {
            path: path.to_string(),
            max_tail_bytes: 4096,
            format_id: format_id.map(str::to_string),
            source_id: source_id.map(str::to_string),
        });
        inspect_content_lines(inspect_content_source(&inspected))
    }

    fn write_inspect_transcript_fixture(contents: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inspect-session.jsonl");
        std::fs::write(&path, format!("{contents}\n")).unwrap();
        (dir, path.display().to_string())
    }

    fn write_inspect_log_fixture(contents: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-bash.log");
        std::fs::write(&path, contents).unwrap();
        (dir, path.display().to_string())
    }

    fn node_with_log(id: &str, label: &str, path: &str) -> MonitorNode {
        let mut node = node(
            id,
            MonitorNodeKind::AgentBashWorkload,
            MonitorStatus::Running,
            label,
        );
        node.inspect_ref = Some(InspectRef::AgentBashLog {
            path: path.to_string(),
            max_tail_bytes: 4096,
        });
        node
    }

    fn native_projectable_line(sentinel: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{sentinel}"}}]}}}}"#
        )
    }

    #[test]
    fn collapsed_reserves_persistent_input_rows_expanded_reserves_a_bounded_share() {
        let collapsed = MonitorPane::new();
        assert_eq!(
            collapsed.bottom_rows(40, 80, TypingProtection::inactive()),
            COLLAPSED_MONITOR_ROWS
        );
        let mut expanded = MonitorPane::new();
        expanded.expand();
        let rows = expanded.bottom_rows(40, 80, TypingProtection::inactive());
        assert!(rows >= EXPANDED_MIN_ROWS, "rows={rows}");
        assert!(rows <= 40 - TOP_PANE_MIN_ROWS, "rows={rows}");
    }

    #[test]
    fn input_safe_floor_reduces_expanded_bottom_rows_across_terminal_sizes() {
        let mut pane = MonitorPane::new();
        pane.expand();
        for full_rows in [15, 20, 40] {
            let bottom = pane.bottom_rows(full_rows, 80, TypingProtection::active());
            let top = full_rows.saturating_sub(bottom);
            assert!(
                top >= INPUT_SAFE_TOP_PANE_MIN_ROWS || bottom == COLLAPSED_MONITOR_ROWS,
                "full_rows={full_rows} top={top} bottom={bottom}"
            );
        }
        assert_eq!(
            pane.bottom_rows(15, 80, TypingProtection::active()),
            COLLAPSED_MONITOR_ROWS
        );
        assert!(pane.bottom_rows(20, 80, TypingProtection::active()) < EXPANDED_MIN_ROWS);
        assert_eq!(pane.bottom_rows(40, 80, TypingProtection::active()), 14);
    }

    #[test]
    fn top_focus_applies_input_safe_floor_when_overlay_is_expanded() {
        let full = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut pane = MonitorPane::new();
        pane.expand();
        let mut line_state = InputLineState::default();
        let protection = typing_protection(Focus::Top, &line_state);
        let child = child_winsize_for_pane(&full, &pane, protection);
        assert!(protection.active);
        assert_eq!(child.ws_row, INPUT_SAFE_TOP_PANE_MIN_ROWS);

        line_state.observe_user_input(b"\r");
        let still_top = typing_protection(Focus::Top, &line_state);
        assert!(
            still_top.active,
            "top focus keeps the real child composer protected"
        );
    }

    #[test]
    fn mid_line_state_applies_input_safe_floor_when_focus_is_bottom() {
        let full = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut pane = MonitorPane::new();
        pane.expand();
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"draft");

        let protection = typing_protection(Focus::Bottom, &line_state);
        let child = child_winsize_for_pane(&full, &pane, protection);

        assert!(protection.active);
        assert_eq!(child.ws_row, INPUT_SAFE_TOP_PANE_MIN_ROWS);
    }

    #[test]
    fn safe_line_boundary_restores_normal_expanded_sizing() {
        let full = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut pane = MonitorPane::new();
        pane.expand();
        let mut line_state = InputLineState::default();
        line_state.observe_user_input(b"draft");
        let protected =
            child_winsize_for_pane(&full, &pane, typing_protection(Focus::Bottom, &line_state));

        line_state.observe_user_input(b"\r");
        let normal =
            child_winsize_for_pane(&full, &pane, typing_protection(Focus::Bottom, &line_state));

        assert_eq!(protected.ws_row, INPUT_SAFE_TOP_PANE_MIN_ROWS);
        assert_eq!(normal.ws_row, 20 - EXPANDED_MIN_ROWS);
    }

    #[test]
    fn constrained_overlay_status_mentions_input_viewport_protection() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut pane = MonitorPane::new();
        pane.expand();
        let parser = vt100::Parser::new(INPUT_SAFE_TOP_PANE_MIN_ROWS, 80, 0);
        terminal
            .draw(|frame| {
                render_frame_with_typing_protection(
                    frame,
                    parser.screen(),
                    Focus::Top,
                    &pane,
                    None,
                    TypingProtection::active(),
                )
            })
            .unwrap();

        let text = screen_text(terminal.backend().buffer(), 20, 80);
        assert!(text.contains("input viewport protected"), "{text}");
    }

    #[test]
    fn constrained_overlay_status_preserves_cancel_confirmation() {
        let mut pane = MonitorPane::new();
        pane.pending_cancel = Some("p".to_string());

        let hint = status_hint(&pane, Focus::Bottom, true);

        assert!(hint.starts_with("confirm cancel: y = SIGTERM · n = abort"));
        assert!(hint.contains("input viewport protected"), "{hint}");
    }

    #[test]
    fn synthetic_composer_render_stays_inside_protected_top_pane() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut pane = MonitorPane::new();
        pane.expand();
        let mut parser = vt100::Parser::new(INPUT_SAFE_TOP_PANE_MIN_ROWS, 80, 0);
        parser.process(b"\x1b[12;1Hcomposer line one\x1b[13;1Hcomposer line two\x1b[14;19H");

        terminal
            .draw(|frame| {
                render_frame_with_typing_protection(
                    frame,
                    parser.screen(),
                    Focus::Top,
                    &pane,
                    None,
                    TypingProtection::active(),
                )
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        assert!(row_text(buf, 11, 80).contains("composer line one"));
        assert!(row_text(buf, 12, 80).contains("composer line two"));
        for y in INPUT_SAFE_TOP_PANE_MIN_ROWS..20 {
            let row = row_text(buf, y, 80);
            assert!(!row.contains("composer line"), "row {y}: {row:?}");
        }
        let cursor = terminal.backend().cursor_position();
        assert!(
            cursor.y < INPUT_SAFE_TOP_PANE_MIN_ROWS,
            "cursor should stay in top pane: {cursor:?}"
        );
    }

    #[test]
    fn collapsed_status_row_summarizes_snapshot() {
        let backend = TestBackend::new(80, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.summary = snapshot_summary(MonitorStatus::Running, 2, 1, 3, 0);
        let pane = pane_with(snapshot, true, 0);
        let parser = vt100::Parser::new(5, 80, 0);
        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Top, &pane, None))
            .unwrap();
        let text = screen_text(terminal.backend().buffer(), 6, 80);
        assert!(text.contains("OBS"), "status screen: {text:?}");
        assert!(text.contains("running"), "status screen: {text:?}");
        assert!(
            text.contains("3 mailbox pending"),
            "status screen: {text:?}"
        );
    }

    #[test]
    fn ctrl_t_routes_to_tree_mode_toggle_without_forwarding() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;

        let routed = router.route_input(&[0x14]);

        assert!(routed.forward.is_empty());
        assert_eq!(routed.commands, vec![MonitorCommand::ToggleTreeMode]);
    }

    #[test]
    fn tree_projection_orders_session_invocation_process_and_bash_children() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node(
                "session:s",
                MonitorNodeKind::Session,
                MonitorStatus::Running,
                "session s",
            ),
            parented(
                node(
                    "invocation:i",
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    "invocation i",
                ),
                "session:s",
            ),
            parented(
                node(
                    "process:i:10",
                    MonitorNodeKind::ProviderProcess,
                    MonitorStatus::Running,
                    "provider pid 10",
                ),
                "invocation:i",
            ),
            parented(
                node(
                    "agent-bash:h",
                    MonitorNodeKind::AgentBashWorkload,
                    MonitorStatus::Running,
                    "agent-bash cargo test",
                ),
                "process:i:10",
            ),
        ];

        assert_eq!(
            projected_ids(&snapshot, MonitorViewMode::Tree),
            vec!["session:s", "invocation:i", "process:i:10", "agent-bash:h"]
        );
        assert_eq!(
            projected_labels(&snapshot, MonitorViewMode::Tree),
            vec![
                "session s",
                "└─ invocation i",
                "   └─ provider pid 10",
                "      └─ agent-bash cargo test",
            ]
        );
    }

    #[test]
    fn tree_projection_roots_orphans_with_missing_parents() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![parented(
            node(
                "agent-bash:orphan",
                MonitorNodeKind::AgentBashWorkload,
                MonitorStatus::Running,
                "orphan bash",
            ),
            "missing:parent",
        )];

        assert_eq!(
            projected_labels(&snapshot, MonitorViewMode::Tree),
            vec!["orphan bash"]
        );
    }

    #[test]
    fn tree_projection_guards_cycles_without_recursing_forever() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            parented(
                node(
                    "invocation:a",
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    "a",
                ),
                "invocation:b",
            ),
            parented(
                node(
                    "invocation:b",
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    "b",
                ),
                "invocation:a",
            ),
        ];

        assert_eq!(
            projected_ids(&snapshot, MonitorViewMode::Tree),
            vec!["invocation:a", "invocation:b"]
        );
        assert_eq!(
            projected_labels(&snapshot, MonitorViewMode::Tree),
            vec!["a", "└─ b"]
        );
    }

    #[test]
    fn pseudo_input_actions_apply_to_draft_and_queue() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let mut pane = pane_with(empty_snapshot(), false, 0);
        let routed = router.route_input(b"hi\x1b[D!\r");

        apply_routed_pseudo_input(&mut pane, &routed.pseudo_input, DEFAULT_PSEUDO_INPUT_WIDTH);

        assert_eq!(pane.pseudo_input.buffer, "");
        assert_eq!(pane.outbound.messages.len(), 1);
        assert_eq!(pane.outbound.messages[0].body, "h!i");
        assert_eq!(pane.outbound.messages[0].status, OutboundStatus::Queued);
    }

    #[test]
    fn ctrl_enter_inserts_newline_and_plain_enter_queues_multiline_draft() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let mut pane = pane_with(empty_snapshot(), false, 0);
        let routed = router.route_input(b"line1\x1b[13;5uline2\r");

        apply_routed_pseudo_input(&mut pane, &routed.pseudo_input, DEFAULT_PSEUDO_INPUT_WIDTH);

        assert!(routed.forward.is_empty());
        assert_eq!(pane.pseudo_input.buffer, "");
        assert_eq!(pane.outbound.messages.len(), 1);
        assert_eq!(pane.outbound.messages[0].body, "line1\nline2");
    }

    #[test]
    fn common_ctrl_enter_sequences_route_to_newline_not_submit() {
        for sequence in [
            b"\x1b[13;5u".as_slice(),
            b"\x1b[13;5~".as_slice(),
            b"\x1b[13^".as_slice(),
            b"\x1b[27;5;13~".as_slice(),
        ] {
            let mut router = InputRouter::new();
            router.focus = Focus::Bottom;

            let routed = router.route_input(sequence);

            assert_eq!(routed.pseudo_input, vec![PseudoInputAction::InsertNewline]);
            assert!(routed.commands.is_empty());
        }
    }

    #[test]
    fn plain_enter_sequences_route_to_submit_not_newline() {
        for sequence in [b"\r".as_slice(), b"\x1b[13u".as_slice()] {
            let mut router = InputRouter::new();
            router.focus = Focus::Bottom;

            let routed = router.route_input(sequence);

            assert_eq!(routed.pseudo_input, vec![PseudoInputAction::Submit]);
            assert!(routed.commands.is_empty());
        }
    }

    #[test]
    fn line_feed_routes_to_newline_for_ctrl_enter() {
        // Terminals commonly send LF (0x0a, Ctrl+J) for Ctrl+Enter while plain Enter
        // sends CR (0x0d); LF must insert a newline at the cursor, not submit.
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;

        let routed = router.route_input(b"\n");

        assert_eq!(routed.pseudo_input, vec![PseudoInputAction::InsertNewline]);
        assert!(routed.commands.is_empty());
    }

    #[test]
    fn multiline_draft_renders_across_input_rows() {
        let mut pane = pane_with(empty_snapshot(), true, 0);
        pane.pseudo_input.buffer = "alpha\nbravo".to_string();
        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();
        let rows = format_pseudo_input_rows(&pane, 2, 80);

        assert!(rows[0].contains("alpha"), "{rows:?}");
        assert!(rows[1].contains("bravo"), "{rows:?}");
        assert!(!rows.iter().any(|row| row.contains('▌')), "{rows:?}");
        assert!(!rows.iter().any(|row| row.contains("input[")), "{rows:?}");
        assert!(!rows.iter().any(|row| row.starts_with('>')), "{rows:?}");
    }

    #[test]
    fn pseudo_input_cursor_styles_cell_without_shifting_text() {
        let mut pane = pane_with(empty_snapshot(), true, 0);
        pane.pseudo_input.buffer = "abc".to_string();
        pane.pseudo_input.cursor = 1;
        let area = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 1,
        };
        let mut buf = Buffer::empty(area);

        render_pseudo_input(&mut buf, area, &pane, Focus::Bottom);

        assert_eq!(row_text(&buf, 0, 4), "abc ");
        assert_eq!(buf[(1, 0)].symbol(), "b");
        assert!(buf[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(0, 0)].modifier.contains(Modifier::REVERSED));

        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();
        let mut end_buf = Buffer::empty(area);
        render_pseudo_input(&mut end_buf, area, &pane, Focus::Bottom);

        assert_eq!(row_text(&end_buf, 0, 4), "abc ");
        assert_eq!(end_buf[(3, 0)].symbol(), " ");
        assert!(end_buf[(3, 0)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn insert_newline_splits_at_cursor() {
        let mut input = PseudoInputState {
            buffer: "abcd".to_string(),
            cursor: 2,
        };

        input.apply(PseudoInputAction::InsertNewline, DEFAULT_PSEUDO_INPUT_WIDTH);

        assert_eq!(input.buffer, "ab\ncd");
        assert_eq!(input.cursor_line_col(), (1, 0));
    }

    #[test]
    fn up_down_move_cursor_between_input_lines() {
        let mut input = PseudoInputState {
            buffer: "alpha\nbravo".to_string(),
            cursor: byte_index_for_line_col("alpha\nbravo", 1, 3),
        };

        input.apply(PseudoInputAction::MoveUp, DEFAULT_PSEUDO_INPUT_WIDTH);
        assert_eq!(input.cursor_line_col(), (0, 3));

        input.apply(PseudoInputAction::MoveDown, DEFAULT_PSEUDO_INPUT_WIDTH);
        assert_eq!(input.cursor_line_col(), (1, 3));

        input.cursor = byte_index_for_line_col(&input.buffer, 0, 2);
        input.apply(PseudoInputAction::MoveUp, DEFAULT_PSEUDO_INPUT_WIDTH);
        assert_eq!(input.cursor, 0);

        input.cursor = byte_index_for_line_col(&input.buffer, 1, 2);
        input.apply(PseudoInputAction::MoveDown, DEFAULT_PSEUDO_INPUT_WIDTH);
        assert_eq!(input.cursor, input.buffer.len());
    }

    #[test]
    fn pseudo_input_height_grows_caps_and_shrinks() {
        let mut input = PseudoInputState::default();
        assert_eq!(input.desired_rows(80), MIN_INPUT_ROWS);

        input.buffer = "one\ntwo\nthree".to_string();
        input.cursor = input.buffer.len();
        assert_eq!(
            desired_pseudo_input_editor_rows(&input.buffer, input.cursor, 80),
            3
        );
        assert_eq!(input.desired_rows(80), 3);

        input.buffer = (0..12)
            .map(|index| format!("line{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        input.cursor = input.buffer.len();
        assert_eq!(
            desired_pseudo_input_editor_rows(&input.buffer, input.cursor, 80),
            MAX_INPUT_ROWS
        );

        input.clear();
        assert_eq!(input.desired_rows(80), MIN_INPUT_ROWS);
    }

    #[test]
    fn long_single_line_counts_soft_wrapped_visual_rows() {
        let buffer = "abcdefghijklmnopqrstuvwxyz";

        assert!(
            desired_pseudo_input_editor_rows(buffer, buffer.len(), 24) > MIN_INPUT_ROWS,
            "long pasted lines should grow the input even without explicit newlines"
        );
    }

    #[test]
    fn pseudo_input_scroll_window_follows_cursor_visual_row() {
        let mut pane = pane_with(empty_snapshot(), true, 0);
        pane.pseudo_input.buffer = (0..12)
            .map(|index| format!("line{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();

        let bottom_rows = format_pseudo_input_rows(&pane, MAX_INPUT_ROWS, 80);

        assert!(bottom_rows[0].contains("line2"), "{bottom_rows:?}");
        assert!(bottom_rows[0].contains('↑'), "{bottom_rows:?}");
        assert!(bottom_rows[9].contains("line11"), "{bottom_rows:?}");
        assert!(
            !bottom_rows.iter().any(|row| row.contains('▌')),
            "{bottom_rows:?}"
        );

        pane.pseudo_input.cursor = 0;
        let top_rows = format_pseudo_input_rows(&pane, MAX_INPUT_ROWS, 80);

        assert!(top_rows[0].contains("line0"), "{top_rows:?}");
        assert!(top_rows[9].contains('↓'), "{top_rows:?}");
    }

    #[test]
    fn input_growth_changes_bottom_rows_and_child_size() {
        let full = libc::winsize {
            ws_row: 30,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let empty = MonitorPane::new();
        let mut grown = MonitorPane::new();
        grown.pseudo_input.buffer = "one\ntwo\nthree\nfour\nfive".to_string();
        grown.pseudo_input.cursor = grown.pseudo_input.buffer.len();

        let empty_bottom =
            empty.bottom_rows(full.ws_row, full.ws_col, TypingProtection::inactive());
        let grown_bottom =
            grown.bottom_rows(full.ws_row, full.ws_col, TypingProtection::inactive());
        let empty_child = child_winsize_for_pane(&full, &empty, TypingProtection::inactive());
        let grown_child = child_winsize_for_pane(&full, &grown, TypingProtection::inactive());

        assert_eq!(grown_bottom - empty_bottom, 4);
        assert_eq!(empty_child.ws_row - grown_child.ws_row, 4);
    }

    #[test]
    fn short_terminal_caps_input_before_starving_top_pane() {
        let mut pane = MonitorPane::new();
        pane.pseudo_input.buffer = (0..12)
            .map(|index| format!("line{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();
        let bottom = pane.bottom_rows(20, 80, TypingProtection::active());

        assert_eq!(20 - bottom, INPUT_SAFE_TOP_PANE_MIN_ROWS);
    }

    #[test]
    fn tail_file_reads_bounded_tail() {
        let dir = std::env::temp_dir().join(format!("itui-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log.txt");
        std::fs::write(&path, "alpha\nbravo\ncharlie\n").unwrap();
        let full = tail_file(path.to_str().unwrap(), 1024);
        assert_eq!(full, vec!["alpha", "bravo", "charlie"]);
        let bounded = tail_file(path.to_str().unwrap(), 8);
        assert!(
            !bounded.is_empty() && bounded.iter().map(String::len).sum::<usize>() <= 8,
            "bounded tail: {bounded:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspecting_pane_renders_header_and_buffered_output() {
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node(
            "agent-bash:h1",
            MonitorNodeKind::AgentBashWorkload,
            MonitorStatus::Running,
            "cargo test",
        )];
        let mut pane = pane_with(snapshot, false, 0);
        pane.inspecting = true;
        pane.inspect = vec![
            "compiling crate".to_string(),
            "running 12 tests".to_string(),
        ];
        let parser = vt100::Parser::new(11, 60, 0);
        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();
        let text = screen_text(terminal.backend().buffer(), 20, 60);
        assert!(text.contains("detail"), "{text}");
        assert!(text.contains("running 12 tests"), "{text}");
    }

    #[test]
    fn selected_node_with_inspect_ref_opens_detail_without_manual_toggle() {
        let (_dir, path) = write_inspect_log_fixture("auto detail sentinel\n");
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node_with_log("agent-bash:h1", "cargo test", &path)];
        let mut pane = MonitorPane::new();

        pane.expand();
        pane.store_snapshot(Arc::new(snapshot));

        assert!(!pane.inspecting, "manual inspect override was not toggled");
        assert!(pane.detail_visible(), "inspect ref should auto-open detail");
        assert!(
            pane.inspect
                .iter()
                .any(|line| line == "auto detail sentinel"),
            "detail reads bounded inspect content: {:?}",
            pane.inspect
        );
    }

    #[test]
    fn keyboard_selection_updates_auto_detail_output() {
        let (_dir_a, path_a) = write_inspect_log_fixture("alpha output\n");
        let (_dir_b, path_b) = write_inspect_log_fixture("bravo output\n");
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node_with_log("agent-bash:a", "alpha", &path_a),
            node_with_log("agent-bash:b", "bravo", &path_b),
        ];
        let mut pane = MonitorPane::new();
        pane.expand();
        pane.store_snapshot(Arc::new(snapshot));

        assert!(pane.inspect.iter().any(|line| line == "alpha output"));
        pane.apply(MonitorCommand::SelectNext);

        assert_eq!(pane.selected_node_id.as_deref(), Some("agent-bash:b"));
        assert!(
            pane.inspect.iter().any(|line| line == "bravo output"),
            "selection should refresh detail: {:?}",
            pane.inspect
        );
        assert!(!pane.inspect.iter().any(|line| line == "alpha output"));
    }

    #[test]
    fn bottom_click_selection_opens_selected_node_detail() {
        let (_dir_a, path_a) = write_inspect_log_fixture("alpha output\n");
        let (_dir_b, path_b) = write_inspect_log_fixture("bravo output\n");
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node_with_log("agent-bash:a", "alpha", &path_a),
            node_with_log("agent-bash:b", "bravo", &path_b),
        ];
        let mut pane = MonitorPane::new();
        pane.expand();
        pane.store_snapshot(Arc::new(snapshot));
        pane.closed_detail_node_id = Some("agent-bash:a".to_string());
        pane.update_inspect();
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let list = bottom_list_area(areas.bottom, &pane).expect("tree list area");
        let bytes = sgr_press(0, list.x + 1, list.y + 2);

        let routed = route_real_input(
            &bytes,
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        for command in routed.commands {
            pane.apply(command);
        }

        assert_eq!(pane.selected_node_id.as_deref(), Some("agent-bash:b"));
        assert!(
            pane.inspect.iter().any(|line| line == "bravo output"),
            "click selection should open selected detail: {:?}",
            pane.inspect
        );
    }

    #[test]
    fn clicking_open_item_closes_detail_and_clicking_again_reopens() {
        let (_dir, path) = write_inspect_log_fixture("live detail\n");
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node_with_log("agent-bash:h1", "cargo test", &path)];
        let mut pane = MonitorPane::new();
        pane.expand();
        pane.store_snapshot(Arc::new(snapshot));

        assert!(pane.detail_visible());
        pane.apply(MonitorCommand::ToggleSelectIndex(0));
        assert!(!pane.detail_visible());
        assert!(pane.inspect.is_empty());

        pane.apply(MonitorCommand::ToggleSelectIndex(0));
        assert!(pane.detail_visible());
        assert!(pane.inspect.iter().any(|line| line == "live detail"));
    }

    #[test]
    fn status_bar_click_collapses_and_reopens_list_while_input_remains_visible() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node(
            "agent-bash:h1",
            MonitorNodeKind::AgentBashWorkload,
            MonitorStatus::Running,
            "cargo test",
        )];
        let mut pane = pane_with(snapshot, false, 0);
        pane.pseudo_input.buffer = "draft".to_string();
        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let expanded_areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let expanded_status = expanded_bottom_layout(expanded_areas.bottom, &pane).status;
        let close_bytes = sgr_press(0, 4, expanded_status.y + 1);

        let close = route_real_input(
            &close_bytes,
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        for command in close.commands {
            pane.apply(command);
        }
        assert!(pane.collapsed);

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let parser = vt100::Parser::new(15, 80, 0);
        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();
        let collapsed_text = screen_text(terminal.backend().buffer(), 20, 80);
        assert!(collapsed_text.contains("draft"), "{collapsed_text}");
        assert!(!collapsed_text.contains("input["), "{collapsed_text}");
        assert!(!collapsed_text.contains("cargo test"), "{collapsed_text}");
        let collapsed_areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let collapsed_status = expanded_bottom_layout(collapsed_areas.bottom, &pane).status;
        let open_bytes = sgr_press(0, 4, collapsed_status.y + 1);

        let open = route_real_input(
            &open_bytes,
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        for command in open.commands {
            pane.apply(command);
        }
        assert!(!pane.collapsed);
    }

    #[test]
    fn snapshot_refresh_preserves_selection_by_node_id() {
        let first = snapshot_with_nodes(3);
        let mut pane = pane_with(first, false, 1);
        let mut refreshed = empty_snapshot();
        refreshed.nodes = vec![
            node(
                "node:inserted",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "inserted",
            ),
            node(
                "node:2",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "node 2",
            ),
            node(
                "node:1",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "node 1",
            ),
            node(
                "node:0",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "node 0",
            ),
        ];

        pane.store_snapshot(Arc::new(refreshed));

        assert_eq!(pane.selected, 2);
        assert_eq!(pane.selected_node_id.as_deref(), Some("node:1"));
    }

    #[test]
    fn snapshot_refresh_falls_back_to_clamped_row_when_selected_id_disappears() {
        let first = snapshot_with_nodes(3);
        let mut pane = pane_with(first, false, 2);
        let (_dir, path) = write_inspect_log_fixture("fallback detail\n");
        let mut refreshed = empty_snapshot();
        refreshed.nodes = vec![
            node(
                "node:new",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "new",
            ),
            node_with_log("node:fallback", "fallback", &path),
        ];

        pane.store_snapshot(Arc::new(refreshed));

        assert_eq!(pane.selected, 1);
        assert_eq!(pane.selected_node_id.as_deref(), Some("node:fallback"));
        assert!(
            pane.inspect.iter().any(|line| line == "fallback detail"),
            "fallback row detail remains valid: {:?}",
            pane.inspect
        );
    }

    #[test]
    fn render_detail_consumes_prepared_rows_without_tailing_files() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node_with_log(
            "agent-bash:missing",
            "missing log",
            "/definitely/missing/agent-bash.log",
        )];
        let mut pane = pane_with(snapshot, false, 0);
        pane.inspect = vec!["prepared detail sentinel".to_string()];
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let parser = vt100::Parser::new(11, 60, 0);

        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();

        let text = screen_text(terminal.backend().buffer(), 20, 60);
        assert!(text.contains("prepared detail sentinel"), "{text}");
        assert!(
            !text.contains("cannot read log"),
            "render must not tail files: {text}"
        );
    }

    #[test]
    fn detail_refresh_tails_selected_inspect_ref_without_snapshot_refresh() {
        let (_dir, path) = write_inspect_log_fixture("first detail\n");
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node_with_log("agent-bash:h1", "cargo test", &path)];
        let mut pane = MonitorPane::new();
        pane.expand();
        pane.store_snapshot(Arc::new(snapshot));
        assert!(pane.inspect.iter().any(|line| line == "first detail"));

        std::fs::write(&path, "first detail\nsecond detail\n").unwrap();
        pane.last_inspect_refresh =
            Some(Instant::now() - DETAIL_REFRESH - Duration::from_millis(1));

        assert!(pane.refresh_detail_if_due(Instant::now()));
        assert!(
            pane.inspect.iter().any(|line| line == "second detail"),
            "local detail refresh should pick up the changed bounded tail: {:?}",
            pane.inspect
        );
    }

    #[test]
    fn detail_refresh_is_inactive_without_visible_inspect_ref_detail() {
        let mut pane = pane_with(snapshot_with_nodes(1), false, 0);
        pane.last_inspect_refresh =
            Some(Instant::now() - DETAIL_REFRESH - Duration::from_millis(1));

        assert!(!pane.detail_visible());
        assert!(!pane.refresh_detail_if_due(Instant::now()));
        assert!(pane.inspect.is_empty());
    }

    #[test]
    fn provider_format_session_transcripts_render_raw_tail_without_native_projection() {
        for format_id in ["provider-inspect-transcript-v1", "canonical-transcript-v1"] {
            let sentinel = format!("ProviderRawSentinel-{format_id}");
            let raw_line = native_projectable_line(&sentinel);
            let (_dir, path) = write_inspect_transcript_fixture(&raw_line);

            let lines = inspect_lines_for_transcript(&path, Some(format_id), Some("source-a"));
            let rendered = lines.join("\n");

            assert!(
                rendered.contains(&raw_line),
                "provider-owned format must preserve raw/canonical bytes for {format_id}: {rendered}"
            );
            assert!(
                !rendered.contains(&format!("agent: {sentinel}")),
                "provider-owned format must not run native projection for {format_id}: {rendered}"
            );
        }
    }

    #[test]
    fn local_session_transcript_without_format_keeps_native_projection() {
        let sentinel = "LocalProjectionSentinel";
        let raw_line = native_projectable_line(sentinel);
        let (_dir, path) = write_inspect_transcript_fixture(&raw_line);

        let lines = inspect_lines_for_transcript(&path, None, None);
        let rendered = lines.join("\n");

        assert!(
            rendered.contains(&format!("agent: {sentinel}")),
            "local/native transcript should render projected conversation lines: {rendered}"
        );
        assert!(
            !lines.iter().any(|line| line == &raw_line),
            "local/native projection should select projected lines instead of raw JSON: {rendered}"
        );
    }

    #[test]
    fn unknown_provider_format_renders_raw_tail_without_native_projection() {
        let sentinel = "UnknownFormatSentinel";
        let raw_line = native_projectable_line(sentinel);
        let (_dir, path) = write_inspect_transcript_fixture(&raw_line);

        let lines =
            inspect_lines_for_transcript(&path, Some("unmapped-transcript-v1"), Some("source-a"));
        let rendered = lines.join("\n");

        assert!(
            rendered.contains(&raw_line),
            "unsupported provider-owned format should render the bounded raw tail: {rendered}"
        );
        assert!(
            !rendered.contains(&format!("agent: {sentinel}")),
            "unsupported provider-owned format must not fail open to native projection: {rendered}"
        );
    }

    #[test]
    fn expanded_pane_lists_nodes_and_marks_selection() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.summary = snapshot_summary(MonitorStatus::Running, 1, 1, 0, 0);
        snapshot.nodes = vec![
            node(
                "process:a",
                MonitorNodeKind::ProviderProcess,
                MonitorStatus::Running,
                "provider turn",
            ),
            node(
                "agent-bash:h1",
                MonitorNodeKind::AgentBashWorkload,
                MonitorStatus::Running,
                "cargo test",
            ),
        ];
        let pane = pane_with(snapshot, false, 1);
        let parser = vt100::Parser::new(11, 80, 0);
        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();
        let text = screen_text(terminal.backend().buffer(), 20, 80);
        assert!(text.contains("provider turn"), "{text}");
        assert!(text.contains("cargo test"), "{text}");
        assert!(
            text.lines()
                .any(|line| line.contains('>') && line.contains("cargo test")),
            "selected row should be marked: {text}"
        );
    }

    #[test]
    fn collapsed_overlay_still_renders_input_and_no_list() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node(
            "agent-bash:h1",
            MonitorNodeKind::AgentBashWorkload,
            MonitorStatus::Running,
            "cargo test",
        )];
        let mut pane = pane_with(snapshot, true, 0);
        pane.pseudo_input.buffer = "draft".to_string();
        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();
        let parser = vt100::Parser::new(15, 80, 0);

        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Top, &pane, None))
            .unwrap();

        let text = screen_text(terminal.backend().buffer(), 20, 80);
        assert!(text.contains("draft"), "{text}");
        assert!(!text.contains("input["), "{text}");
        assert!(!text.contains("outbound:"), "{text}");
        assert!(
            !text.contains("cargo test"),
            "collapsed list should stay hidden: {text}"
        );
    }

    #[test]
    fn expanded_overlay_renders_input_above_status_filter_tabs_and_list() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node(
            "agent-bash:h1",
            MonitorNodeKind::AgentBashWorkload,
            MonitorStatus::Running,
            "cargo test",
        )];
        let mut pane = pane_with(snapshot, false, 0);
        pane.pseudo_input.buffer = "draft".to_string();
        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();
        let parser = vt100::Parser::new(12, 80, 0);

        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();

        let buf = terminal.backend().buffer();
        let input_row = (0..20)
            .find(|row| row_text(buf, *row, 80).contains("draft"))
            .expect("input row");
        let status_row = (0..20)
            .find(|row| row_text(buf, *row, 80).contains("OBS"))
            .expect("status row");
        let tabs_row = (0..20)
            .find(|row| row_text(buf, *row, 80).contains("filters:"))
            .expect("tabs row");
        let list_row = (0..20)
            .find(|row| row_text(buf, *row, 80).contains("cargo test"))
            .expect("list row");
        assert!(
            input_row < status_row,
            "input={input_row} status={status_row}"
        );
        assert!(status_row < tabs_row, "status={status_row} tabs={tabs_row}");
        assert!(tabs_row < list_row, "tabs={tabs_row} list={list_row}");
    }

    #[test]
    fn bottom_layout_rects_do_not_overlap_and_sum_to_pane_height() {
        let mut pane = pane_with(snapshot_with_nodes(2), false, 0);
        pane.pseudo_input.buffer = (0..12)
            .map(|index| format!("line{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        pane.pseudo_input.cursor = pane.pseudo_input.buffer.len();
        let bottom = Rect {
            x: 0,
            y: 10,
            width: 80,
            height: 14,
        };

        let layout = expanded_bottom_layout(bottom, &pane);

        assert_eq!(layout.input.y, bottom.y);
        assert_eq!(layout.status.y, layout.input.y + layout.input.height);
        assert_eq!(layout.content.y, layout.status.y + layout.status.height);
        assert_eq!(
            layout.content.y + layout.content.height,
            bottom.y + bottom.height
        );
        assert!(layout.input.height <= MAX_INPUT_ROWS);
        assert!(layout.input.y < layout.status.y);
        assert!(layout.status.height <= STATUS_ROW_ROWS);
    }

    #[test]
    fn bottom_status_drops_keyboard_hints_but_keeps_obs_counts() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.summary = snapshot_summary(MonitorStatus::Running, 3, 2, 1, 0);
        let pane = pane_with(snapshot, true, 0);
        let parser = vt100::Parser::new(15, 80, 0);

        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();

        let text = screen_text(terminal.backend().buffer(), 20, 80);
        assert!(text.contains("OBS"), "{text}");
        assert!(text.contains("running · 3 proc · 2 bash running"), "{text}");
        assert!(!text.contains("Enter queue"), "{text}");
        assert!(!text.contains("Ctrl+Enter"), "{text}");
        assert!(!text.contains("Ctrl+F"), "{text}");
        assert!(!text.contains("type draft"), "{text}");
    }

    #[test]
    fn default_filter_is_running_bash_and_category_changes_filter_rows() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node(
                "invocation:i",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "agent invocation",
            ),
            node(
                "agent-bash:h1",
                MonitorNodeKind::AgentBashWorkload,
                MonitorStatus::Running,
                "cargo test",
            ),
            node(
                "mailbox:m1",
                MonitorNodeKind::MailboxNotification,
                MonitorStatus::Pending,
                "pending mailbox",
            ),
        ];
        let mut pane = MonitorPane::new();
        pane.expand();
        pane.store_snapshot(Arc::new(snapshot));

        assert_eq!(pane.active_filter, MonitorFilterCategory::Bash);
        assert_eq!(filtered_labels_for_pane(&pane), vec!["cargo test"]);

        pane.apply(MonitorCommand::SelectFilter(MonitorFilterCategory::Procs));
        assert_eq!(filtered_labels_for_pane(&pane), vec!["agent invocation"]);

        pane.apply(MonitorCommand::SelectFilter(MonitorFilterCategory::Mailbox));
        assert_eq!(filtered_labels_for_pane(&pane), vec!["pending mailbox"]);
    }

    #[test]
    fn filter_tabs_are_clickable_and_ctrl_f_cycles_tabs() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node(
                "agent-bash:h1",
                MonitorNodeKind::AgentBashWorkload,
                MonitorStatus::Running,
                "cargo test",
            ),
            node(
                "mailbox:m1",
                MonitorNodeKind::MailboxNotification,
                MonitorStatus::Pending,
                "pending mailbox",
            ),
        ];
        let pane = pane_with(snapshot, false, 0);
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let tabs = bottom_filter_tabs_area(areas.bottom, &pane).expect("filter tabs");
        let procs_col = tabs.x
            + 1
            + " filters: ".chars().count() as u16
            + filter_tab_label_width(MonitorFilterCategory::Bash)
            + 2;
        let bytes = sgr_press(0, procs_col, tabs.y + 1);

        let routed = route_real_input(
            &bytes,
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        assert_eq!(
            routed.commands,
            vec![MonitorCommand::SelectFilter(MonitorFilterCategory::Procs)]
        );

        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        assert_eq!(
            router.route_input(&[0x06]).commands,
            vec![MonitorCommand::NextFilter]
        );
    }

    #[test]
    fn tree_mode_renders_indented_parent_child_rows() {
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut snapshot = empty_snapshot();
        snapshot.summary = snapshot_summary(MonitorStatus::Running, 3, 1, 0, 0);
        snapshot.nodes = vec![
            node(
                "session:s",
                MonitorNodeKind::Session,
                MonitorStatus::Running,
                "session s",
            ),
            parented(
                node(
                    "invocation:i",
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    "invocation i",
                ),
                "session:s",
            ),
            parented(
                node(
                    "agent-bash:h",
                    MonitorNodeKind::AgentBashWorkload,
                    MonitorStatus::Running,
                    "cargo test",
                ),
                "invocation:i",
            ),
        ];
        let mut pane = pane_with(snapshot, false, 0);
        pane.view_mode = MonitorViewMode::Tree;
        let parser = vt100::Parser::new(11, 80, 0);

        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();

        let text = screen_text(terminal.backend().buffer(), 20, 80);
        assert!(
            text.contains("tree"),
            "status row should expose tree mode: {text}"
        );
        assert!(
            text.contains("└─ invocation [running] invocation i")
                || text.contains("└─ invocation i"),
            "tree child row should include a branch glyph: {text}"
        );
        assert!(text.contains("   └─ bash [running] cargo test"), "{text}");
    }

    #[test]
    fn tree_mode_keyboard_selection_follows_projected_order_by_node_id() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            parented(
                node(
                    "agent-bash:h",
                    MonitorNodeKind::AgentBashWorkload,
                    MonitorStatus::Running,
                    "cargo test",
                ),
                "invocation:i",
            ),
            node(
                "session:s",
                MonitorNodeKind::Session,
                MonitorStatus::Running,
                "session s",
            ),
            parented(
                node(
                    "invocation:i",
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    "invocation i",
                ),
                "session:s",
            ),
        ];
        let mut pane = pane_with(snapshot, false, 1);
        pane.view_mode = MonitorViewMode::Tree;

        pane.apply(MonitorCommand::SelectNext);
        assert_eq!(pane.selected_node_id.as_deref(), Some("invocation:i"));
        pane.apply(MonitorCommand::SelectNext);
        assert_eq!(pane.selected_node_id.as_deref(), Some("agent-bash:h"));
        pane.apply(MonitorCommand::SelectPrev);
        assert_eq!(pane.selected_node_id.as_deref(), Some("invocation:i"));
    }

    #[test]
    fn tree_mode_toggle_preserves_selected_node_id() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            parented(
                node(
                    "agent-bash:h",
                    MonitorNodeKind::AgentBashWorkload,
                    MonitorStatus::Running,
                    "cargo test",
                ),
                "invocation:i",
            ),
            node(
                "session:s",
                MonitorNodeKind::Session,
                MonitorStatus::Running,
                "session s",
            ),
            parented(
                node(
                    "invocation:i",
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    "invocation i",
                ),
                "session:s",
            ),
        ];
        let mut pane = pane_with(snapshot, false, 0);

        pane.apply(MonitorCommand::ToggleTreeMode);

        assert_eq!(pane.view_mode, MonitorViewMode::Tree);
        assert_eq!(pane.selected_node_id.as_deref(), Some("agent-bash:h"));
        assert_eq!(
            pane.selected_node().map(|node| node.id.as_str()),
            Some("agent-bash:h")
        );
    }

    #[test]
    fn tree_mode_click_selection_opens_selected_detail() {
        let (_dir, path) = write_inspect_log_fixture("tree detail sentinel\n");
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node(
                "session:s",
                MonitorNodeKind::Session,
                MonitorStatus::Running,
                "session s",
            ),
            parented(
                node_with_log("agent-bash:h", "cargo test", &path),
                "session:s",
            ),
        ];
        let mut pane = pane_with(snapshot, false, 0);
        pane.view_mode = MonitorViewMode::Tree;
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let areas =
            pane_areas_for_winsize(&winsize, &pane, TypingProtection::for_focus(router.focus));
        let list = bottom_list_area(areas.bottom, &pane).expect("tree list area");
        let bytes = sgr_press(0, list.x + 1, list.y + 2);

        let routed = route_real_input(
            &bytes,
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );
        for command in routed.commands {
            pane.apply(command);
        }

        assert_eq!(pane.selected_node_id.as_deref(), Some("agent-bash:h"));
        assert!(
            pane.inspect
                .iter()
                .any(|line| line == "tree detail sentinel"),
            "tree click selection should reuse WU2 detail output: {:?}",
            pane.inspect
        );
    }

    #[test]
    fn tree_mode_snapshot_refresh_preserves_selection_and_valid_scroll() {
        let mut first = empty_snapshot();
        first.nodes = vec![
            node(
                "session:s",
                MonitorNodeKind::Session,
                MonitorStatus::Running,
                "session s",
            ),
            parented(
                node(
                    "agent-bash:h",
                    MonitorNodeKind::AgentBashWorkload,
                    MonitorStatus::Running,
                    "cargo test",
                ),
                "session:s",
            ),
        ];
        let mut pane = pane_with(first, false, 1);
        pane.view_mode = MonitorViewMode::Tree;
        let mut refreshed = empty_snapshot();
        refreshed.nodes = vec![
            node(
                "session:s",
                MonitorNodeKind::Session,
                MonitorStatus::Running,
                "session s",
            ),
            parented(
                node(
                    "invocation:new",
                    MonitorNodeKind::Invocation,
                    MonitorStatus::Running,
                    "inserted invocation",
                ),
                "session:s",
            ),
            parented(
                node(
                    "agent-bash:h",
                    MonitorNodeKind::AgentBashWorkload,
                    MonitorStatus::Running,
                    "cargo test",
                ),
                "invocation:new",
            ),
        ];

        pane.store_snapshot(Arc::new(refreshed));
        let snapshot = pane.snapshot.as_ref().expect("snapshot");
        let projected = pane.projected_rows(snapshot);
        let rows = visible_node_rows(
            &pane,
            snapshot,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 2,
            },
            &projected,
        );

        assert_eq!(pane.selected_node_id.as_deref(), Some("agent-bash:h"));
        assert!(
            rows.iter().any(|row| row.node.id == "agent-bash:h"),
            "selected tree node should remain visible after inserted rows: {:?}",
            rows.iter()
                .map(|row| row.node.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn bottom_focus_arrows_move_pseudo_input_cursor_and_printable_edits() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        assert_eq!(router.focus, Focus::Bottom);

        assert_eq!(
            router.route_input(b"j").pseudo_input,
            vec![PseudoInputAction::Insert('j')]
        );
        assert_eq!(
            router.route_input(b"k").pseudo_input,
            vec![PseudoInputAction::Insert('k')]
        );
        assert_eq!(
            router.route_input(&[0x1b, b'[', b'B']).pseudo_input,
            vec![PseudoInputAction::MoveDown]
        );
        assert_eq!(
            router.route_input(&[0x1b, b'[', b'A']).pseudo_input,
            vec![PseudoInputAction::MoveUp]
        );

        let routed = router.route_input(b"q");
        assert_eq!(routed.pseudo_input, vec![PseudoInputAction::Insert('q')]);
        assert_eq!(router.focus, Focus::Bottom);
    }

    #[test]
    fn pane_applies_selection_refresh_and_collapse() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node(
                "a",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "a",
            ),
            node(
                "b",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "b",
            ),
            node(
                "c",
                MonitorNodeKind::Invocation,
                MonitorStatus::Running,
                "c",
            ),
        ];
        let mut pane = pane_with(snapshot, false, 0);

        assert!(!pane.apply(MonitorCommand::SelectNext));
        assert_eq!(pane.selected, 1);
        pane.apply(MonitorCommand::SelectNext);
        pane.apply(MonitorCommand::SelectNext);
        assert_eq!(pane.selected, 2, "selection clamps at the last node");
        pane.apply(MonitorCommand::SelectPrev);
        assert_eq!(pane.selected, 1);

        assert!(
            pane.apply(MonitorCommand::Refresh),
            "refresh forces a snapshot"
        );
        assert!(!pane.apply(MonitorCommand::Collapse));
        assert!(pane.collapsed);
    }

    fn node_with_process_group_cancel(id: &str) -> MonitorNode {
        let mut node = node(
            id,
            MonitorNodeKind::ProviderProcess,
            MonitorStatus::Running,
            id,
        );
        node.cancel_ref = Some(crate::observability::CancelRef::ProcessGroup {
            pgid: 4321,
            identity: Some(crate::observability::MonitorProcessIdentity {
                os_pid: 4321,
                os_boot_id: "boot".to_string(),
                os_pid_starttime_ticks: 11,
            }),
        });
        node
    }

    fn cancel_pane() -> MonitorPane {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![
            node("s", MonitorNodeKind::Session, MonitorStatus::Running, "s"),
            node_with_process_group_cancel("p"),
        ];
        pane_with(snapshot, false, 0)
    }

    #[test]
    fn cancel_arms_only_for_nodes_with_a_cancel_ref() {
        let mut pane = cancel_pane();
        // Session node here has no cancel ref: arming is a no-op.
        pane.apply(MonitorCommand::RequestCancel);
        assert_eq!(pane.pending_cancel, None);
        // The process node exposes a cancel ref: arming records its id.
        pane.apply(MonitorCommand::SelectNext);
        pane.apply(MonitorCommand::RequestCancel);
        assert_eq!(pane.pending_cancel.as_deref(), Some("p"));
    }

    #[test]
    fn confirm_produces_a_request_only_for_the_armed_node() {
        let mut pane = cancel_pane();
        pane.apply(MonitorCommand::SelectNext);
        pane.apply(MonitorCommand::RequestCancel);
        pane.apply(MonitorCommand::ConfirmAction);
        assert_eq!(pane.pending_cancel, None, "confirm disarms");
        assert!(
            matches!(
                pane.take_cancel_request(),
                Some(CancelRequest::ProcessGroup { pgid: 4321, .. })
            ),
            "confirm yields the process-group request"
        );
    }

    #[test]
    fn abort_clears_the_armed_cancel_without_a_request() {
        let mut pane = cancel_pane();
        pane.apply(MonitorCommand::SelectNext);
        pane.apply(MonitorCommand::RequestCancel);
        pane.apply(MonitorCommand::AbortAction);
        assert_eq!(pane.pending_cancel, None);
        assert_eq!(pane.take_cancel_request(), None);
    }

    #[test]
    fn confirm_after_selection_moves_off_armed_node_does_not_cancel() {
        let mut pane = cancel_pane();
        pane.apply(MonitorCommand::SelectNext);
        pane.apply(MonitorCommand::RequestCancel);
        pane.apply(MonitorCommand::SelectPrev);
        pane.apply(MonitorCommand::ConfirmAction);
        assert_eq!(
            pane.take_cancel_request(),
            None,
            "selection moved off the armed node, so no request is produced"
        );
    }

    #[test]
    fn cancel_keys_route_to_cancel_commands() {
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;
        assert_eq!(
            router.route_input(&[0x18]).commands,
            vec![MonitorCommand::RequestCancel]
        );
        assert_eq!(
            router.route_input(&[0x19]).commands,
            vec![MonitorCommand::ConfirmAction]
        );
        assert_eq!(
            router.route_input(&[0x0e]).commands,
            vec![MonitorCommand::AbortAction]
        );
    }

    #[test]
    fn outbound_recovery_keys_arm_confirm_abort_and_revalidate_message() {
        let now = Instant::now();
        let mut pane = sent_pane("first", [], now);
        pane.outbound.set_status(
            1,
            OutboundStatus::Ambiguous,
            now,
            Some("consumption_timeout".to_string()),
        );
        let mut router = InputRouter::new();
        router.focus = Focus::Bottom;

        let hint = bottom_status_hint(&pane, false);
        assert!(hint.contains("Ctrl+P = retry"), "{hint}");
        assert!(hint.contains("Ctrl+D = discard"), "{hint}");

        assert_eq!(
            router.route_input(&[0x10]).commands,
            vec![MonitorCommand::RequestOutboundRetry]
        );
        pane.apply(MonitorCommand::RequestOutboundRetry);
        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Ambiguous));
        assert_eq!(
            pane.pending_outbound_recovery,
            Some(PendingOutboundRecovery {
                message_id: 1,
                action: OutboundRecoveryAction::Retry,
            })
        );
        pane.apply(MonitorCommand::AbortAction);
        assert_eq!(pane.pending_outbound_recovery, None);

        assert_eq!(
            router.route_input(&[0x04]).commands,
            vec![MonitorCommand::RequestOutboundDiscard]
        );
        pane.apply(MonitorCommand::RequestOutboundDiscard);
        pane.apply(MonitorCommand::ConfirmAction);
        assert_eq!(
            pane.take_outbound_recovery_request(),
            Some(PendingOutboundRecovery {
                message_id: 1,
                action: OutboundRecoveryAction::Discard,
            })
        );

        pane.apply(MonitorCommand::RequestOutboundRetry);
        apply_outbound_observation(
            &mut pane.outbound,
            &available_observation(2, [("late", Some("first"))]),
            now,
        );
        pane.apply(MonitorCommand::ConfirmAction);
        assert_eq!(pane.take_outbound_recovery_request(), None);
        assert!(
            pane.outbound_recovery_feedback
                .as_deref()
                .is_some_and(|feedback| feedback.contains("no longer ambiguous"))
        );
    }

    #[test]
    fn cancel_and_outbound_recovery_arming_are_mutually_exclusive() {
        let now = Instant::now();
        let mut pane = cancel_pane();
        pane.outbound = sent_pane("first", [], now).outbound;
        pane.outbound.set_status(
            1,
            OutboundStatus::Ambiguous,
            now,
            Some("consumption_timeout".to_string()),
        );
        pane.apply(MonitorCommand::SelectNext);
        pane.apply(MonitorCommand::RequestCancel);
        assert!(pane.pending_cancel.is_some());
        pane.apply(MonitorCommand::RequestOutboundRetry);
        assert!(pane.pending_cancel.is_none());
        assert!(pane.pending_outbound_recovery.is_some());
        pane.apply(MonitorCommand::RequestCancel);
        assert!(pane.pending_outbound_recovery.is_none());
        assert!(pane.pending_cancel.is_some());
    }

    fn diagnostic(code: &str, node_id: Option<&str>) -> MonitorDiagnostic {
        MonitorDiagnostic {
            code: code.to_string(),
            severity: MonitorDiagnosticSeverity::Warning,
            message: format!("{code} message"),
            node_id: node_id.map(str::to_string),
        }
    }

    #[test]
    fn diagnostics_for_node_selects_only_the_matching_node() {
        let diagnostics = vec![
            diagnostic("stale-runtime", Some("session:s")),
            diagnostic("global", None),
            diagnostic("other", Some("session:other")),
        ];
        let matching = diagnostics_for_node("session:s", &diagnostics);
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].code, "stale-runtime");
    }

    #[test]
    fn inspect_content_appends_scoped_diagnostics() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node(
            "session:s",
            MonitorNodeKind::Session,
            MonitorStatus::Stale,
            "s",
        )];
        snapshot.diagnostics = vec![diagnostic("stale-runtime", Some("session:s"))];
        let mut pane = pane_with(snapshot, false, 0);
        pane.inspecting = true;
        pane.update_inspect();
        assert!(
            pane.inspect.iter().any(|line| line == "— diagnostics —"),
            "inspect shows a diagnostics block: {:?}",
            pane.inspect
        );
        assert!(
            pane.inspect
                .iter()
                .any(|line| line.contains("stale-runtime: stale-runtime message")),
            "inspect shows the diagnostic detail: {:?}",
            pane.inspect
        );
    }

    #[test]
    fn inspect_content_omits_diagnostics_block_when_none_match() {
        let mut snapshot = empty_snapshot();
        snapshot.nodes = vec![node(
            "session:s",
            MonitorNodeKind::Session,
            MonitorStatus::Running,
            "s",
        )];
        snapshot.diagnostics = vec![diagnostic("other", Some("session:other"))];
        let mut pane = pane_with(snapshot, false, 0);
        pane.inspecting = true;
        pane.update_inspect();
        assert!(
            !pane.inspect.iter().any(|line| line == "— diagnostics —"),
            "no diagnostics block when none are scoped to the node: {:?}",
            pane.inspect
        );
    }

    #[test]
    fn pane_adopts_new_worker_snapshot_once() {
        let mut pane = MonitorPane::new();
        let snapshot = Arc::new(empty_snapshot());
        assert!(pane.adopt_snapshot(Some(Arc::clone(&snapshot))));
        assert!(!pane.adopt_snapshot(Some(snapshot)));
        assert!(pane.snapshot.is_some());
    }
}
