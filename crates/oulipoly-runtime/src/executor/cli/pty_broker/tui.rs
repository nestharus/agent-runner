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
use super::snapshot_worker::{MonitorSnapshotProvider, MonitorSnapshotWorker};
use super::transcript_view::project_transcript_tail;
use super::{
    ChildOutputState, ControlSocket, INJECT_WAIT_LIMIT, InputLineState, PendingChildInput,
    RELAY_BUFFER_BYTES, flush_pending_child_input, is_pty_eof_error, poll_fds, poll_master_fd,
    poll_relay_fds, poll_single_fd, queue_control_injection, read_control_request, read_fd,
    readable, send_signal_to_child_group, set_pty_winsize, terminal_winsize, validate_peer_uid,
    winsize_eq, writable, write_control_response,
};
use crate::observability::{
    InspectRef, LivenessStatus, MonitorDiagnostic, MonitorDiagnosticSeverity, MonitorNode,
    MonitorNodeId, MonitorNodeKind, MonitorSnapshot, MonitorStatus, ObservabilityRoot,
};
#[cfg(test)]
use crate::observability::{ObservabilitySnapshotPort, SnapshotLimits};
use base64::Engine as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::collections::{HashMap, HashSet};
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

/// Rows reserved for the collapsed monitor row at the bottom of the screen.
const COLLAPSED_MONITOR_ROWS: u16 = 1;
/// Minimum terminal rows for the split to be usable.
const MIN_TERMINAL_ROWS: u16 = 10;
/// Minimum terminal columns for the split to be usable.
const MIN_TERMINAL_COLS: u16 = 40;
/// Reserved focus-toggle byte: Ctrl+O.
const FOCUS_TOGGLE_BYTE: u8 = 0x0f;
/// Target render cadence while the monitor overlay owns focus.
pub(super) const FOREGROUND_RENDER_FPS: u64 = 60;
/// Target render cadence while the monitor overlay is collapsed/backgrounded.
pub(super) const BACKGROUND_RENDER_FPS: u64 = 10;

/// Bracketed-paste delimiters (DECSET 2004) the broker wraps an injected notification in
/// when the child has advertised the mode, so the body is treated as pasted content and
/// the trailing Enter submits it.
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
/// Pause between writing an injected notification's body and the Enter that submits it.
/// An Ink-style child (Claude Code) commits a bracketed paste to its input buffer on a
/// later async render tick; a `\r` written back-to-back races ahead of that commit and is
/// dropped, leaving the notification unsent until the operator presses Enter themselves.
/// Waiting lets the paste commit land first so the Enter actually submits.
const CONTROL_SUBMIT_DELAY: Duration = Duration::from_millis(400);
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

/// Map the real terminal window size to the child PTY window size, reserving the
/// collapsed monitor row so the provider believes its terminal is the top pane.
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
    SelectIndex(usize),
    ToggleTreeMode,
    Refresh,
    Collapse,
    ToggleInspect,
    RequestCancel,
    ConfirmCancel,
    AbortCancel,
}

/// Outcome of routing a chunk of real-terminal input.
#[derive(Debug, Default, PartialEq, Eq)]
struct RoutedInput {
    forward: Vec<u8>,
    commands: Vec<MonitorCommand>,
    pseudo_input: Vec<PseudoInputAction>,
    /// The operator toggled focus into the monitor (expand + focus bottom).
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

enum TopInputRoute {
    FocusBottom,
    Forward(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BottomKey {
    FocusToggle,
    Enter,
    Backspace,
    Delete,
    LeftArrow,
    RightArrow,
    UpArrow,
    DownArrow,
    MoveStart,
    MoveEnd,
    Clear,
    TreeMode,
    Refresh,
    Collapse,
    Inspect,
    Cancel,
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
    FocusTop,
    Command(MonitorCommand),
    CommandAndFocusTop(MonitorCommand),
    PseudoInput(PseudoInputAction),
    Consume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PseudoInputAction {
    Insert(char),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
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
/// child verbatim except the reserved focus-toggle; in the bottom pane keys are
/// decoded into monitor commands and never reach the child.
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
        match classify_top_input(byte) {
            TopInputRoute::FocusBottom => self.focus_bottom(routed),
            TopInputRoute::Forward(byte) => routed.forward.push(byte),
        }
        1
    }

    fn focus_bottom(&mut self, routed: &mut RoutedInput) {
        self.focus = Focus::Bottom;
        routed.focus_bottom = true;
        routed.redraw = true;
    }

    fn route_bottom_key(&mut self, bytes: &[u8], routed: &mut RoutedInput) -> usize {
        let parsed = parse_bottom_key(bytes);
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
            BottomInputRoute::FocusTop => self.focus_top(routed),
            BottomInputRoute::Command(command) => apply_monitor_command(routed, command),
            BottomInputRoute::CommandAndFocusTop(command) => {
                apply_monitor_command(routed, command);
                self.focus_top(routed);
            }
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
    match bytes {
        [FOCUS_TOGGLE_BYTE, ..] => ParsedBottomKey {
            key: BottomKey::FocusToggle,
            consumed: 1,
        },
        [b'\r', ..] | [b'\n', ..] => ParsedBottomKey {
            key: BottomKey::Enter,
            consumed: 1,
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

fn bottom_key_route(key: BottomKey) -> BottomInputRoute {
    match key {
        BottomKey::FocusToggle => BottomInputRoute::FocusTop,
        BottomKey::Enter => BottomInputRoute::PseudoInput(PseudoInputAction::Submit),
        BottomKey::Backspace => BottomInputRoute::PseudoInput(PseudoInputAction::Backspace),
        BottomKey::Delete => BottomInputRoute::PseudoInput(PseudoInputAction::Delete),
        BottomKey::LeftArrow => BottomInputRoute::PseudoInput(PseudoInputAction::MoveLeft),
        BottomKey::RightArrow => BottomInputRoute::PseudoInput(PseudoInputAction::MoveRight),
        BottomKey::UpArrow => BottomInputRoute::Command(MonitorCommand::SelectPrev),
        BottomKey::DownArrow => BottomInputRoute::Command(MonitorCommand::SelectNext),
        BottomKey::MoveStart => BottomInputRoute::PseudoInput(PseudoInputAction::MoveStart),
        BottomKey::MoveEnd => BottomInputRoute::PseudoInput(PseudoInputAction::MoveEnd),
        BottomKey::Clear => BottomInputRoute::PseudoInput(PseudoInputAction::Clear),
        BottomKey::TreeMode => BottomInputRoute::Command(MonitorCommand::ToggleTreeMode),
        BottomKey::Refresh => BottomInputRoute::Command(MonitorCommand::Refresh),
        BottomKey::Collapse => BottomInputRoute::CommandAndFocusTop(MonitorCommand::Collapse),
        BottomKey::Inspect => BottomInputRoute::Command(MonitorCommand::ToggleInspect),
        BottomKey::Cancel => BottomInputRoute::Command(MonitorCommand::RequestCancel),
        BottomKey::Confirm => BottomInputRoute::Command(MonitorCommand::ConfirmCancel),
        BottomKey::Abort => BottomInputRoute::Command(MonitorCommand::AbortCancel),
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

fn classify_top_input(byte: u8) -> TopInputRoute {
    if byte == FOCUS_TOGGLE_BYTE {
        TopInputRoute::FocusBottom
    } else {
        TopInputRoute::Forward(byte)
    }
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
    let bottom_rows = pane.bottom_rows(area.height, protection);
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
const EXPANDED_MIN_ROWS: u16 = 8;
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
/// Rows reserved at the bottom of the expanded overlay for the pseudo input lane.
const PSEUDO_INPUT_ROWS: u16 = 2;
/// Rows reserved below the pseudo input for outbound message state.
const OUTBOUND_STATUS_ROWS: u16 = 1;
/// Broker-owned input messages larger than this fail before reaching the child.
const PSEUDO_INPUT_MAX_BYTES: usize = 64 * 1024;
/// Poll cadence for a live recent-turn reader while a message awaits consumption.
const RECENT_TURN_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Sent messages fail safe to ambiguous when no consumption proof appears in time.
const OUTBOUND_CONSUMPTION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
struct PseudoInputState {
    buffer: String,
    cursor: usize,
}

impl PseudoInputState {
    fn apply(&mut self, action: PseudoInputAction) -> Option<String> {
        match action {
            PseudoInputAction::Insert(ch) => self.insert(ch),
            PseudoInputAction::Backspace => self.backspace(),
            PseudoInputAction::Delete => self.delete(),
            PseudoInputAction::MoveLeft => self.move_left(),
            PseudoInputAction::MoveRight => self.move_right(),
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

    fn cursor_chars(&self) -> usize {
        self.buffer[..self.cursor].chars().count()
    }
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
enum OutboundStatus {
    Queued,
    Sending,
    Sent,
    Consumed,
    Ambiguous,
    Failed,
}

#[derive(Debug, Clone)]
struct OutboundMessage {
    id: u64,
    body: String,
    status: OutboundStatus,
    created_at: Instant,
    sent_at: Option<Instant>,
    baseline_turn_count: Option<u64>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct OutboundQueue {
    next_id: u64,
    messages: Vec<OutboundMessage>,
    active: Option<ActiveOutboundSend>,
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

impl OutboundQueue {
    fn enqueue(&mut self, body: String, now: Instant) -> u64 {
        let id = self.next_message_id();
        self.messages.push(OutboundMessage {
            id,
            body,
            status: OutboundStatus::Queued,
            created_at: now,
            sent_at: None,
            baseline_turn_count: None,
            detail: None,
        });
        id
    }

    fn next_message_id(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1).max(1);
        self.next_id
    }

    fn has_sent_or_sending(&self) -> bool {
        self.messages.iter().any(|message| {
            matches!(
                message.status,
                OutboundStatus::Sending | OutboundStatus::Sent
            )
        })
    }

    fn has_unresolved_blocker(&self) -> bool {
        self.messages.iter().any(|message| {
            matches!(
                message.status,
                OutboundStatus::Sending
                    | OutboundStatus::Sent
                    | OutboundStatus::Ambiguous
                    | OutboundStatus::Failed
            )
        })
    }

    fn next_queued_id(&self) -> Option<u64> {
        self.messages
            .iter()
            .find(|message| message.status == OutboundStatus::Queued)
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

    fn mark_sending(&mut self, id: u64, baseline_turn_count: Option<u64>) -> bool {
        let Some(message) = self.message_mut(id) else {
            return false;
        };
        message.status = OutboundStatus::Sending;
        message.baseline_turn_count = baseline_turn_count;
        message.detail = None;
        true
    }

    #[cfg(test)]
    fn status(&self, id: u64) -> Option<OutboundStatus> {
        self.message(id).map(|message| message.status)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentUserTurn {
    ordinal: u64,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentTurnSnapshot {
    user_turns: Vec<RecentUserTurn>,
    turn_count: u64,
    complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
enum RecentTurnRead {
    Available(RecentTurnSnapshot),
    Unavailable(String),
    Failed(String),
}

trait RecentTurnReader {
    fn read_recent_turns(&mut self) -> RecentTurnRead;
}

struct RecentTurnPump {
    reader: Option<Box<dyn RecentTurnReader + Send>>,
    next_poll_at: Instant,
    last_snapshot: Option<RecentTurnSnapshot>,
}

impl RecentTurnPump {
    fn disabled(now: Instant) -> Self {
        Self {
            reader: None,
            next_poll_at: now + RECENT_TURN_POLL_INTERVAL,
            last_snapshot: None,
        }
    }

    #[cfg(test)]
    fn with_reader(reader: Box<dyn RecentTurnReader + Send>, now: Instant) -> Self {
        Self {
            reader: Some(reader),
            next_poll_at: now,
            last_snapshot: None,
        }
    }

    fn last_turn_count(&self) -> Option<u64> {
        self.last_snapshot
            .as_ref()
            .map(|snapshot| snapshot.turn_count)
    }

    fn poll_if_due(&mut self, now: Instant, outbound: &mut OutboundQueue) -> bool {
        if !outbound.has_sent_or_sending() || now < self.next_poll_at {
            return false;
        }
        self.next_poll_at = now + RECENT_TURN_POLL_INTERVAL;
        let Some(reader) = self.reader.as_mut() else {
            return false;
        };
        match reader.read_recent_turns() {
            RecentTurnRead::Available(snapshot) => {
                let dirty = apply_recent_turn_snapshot(outbound, &snapshot, now);
                self.last_snapshot = Some(snapshot);
                dirty
            }
            RecentTurnRead::Unavailable(_) | RecentTurnRead::Failed(_) => false,
        }
    }
}

fn apply_recent_turn_snapshot(
    outbound: &mut OutboundQueue,
    snapshot: &RecentTurnSnapshot,
    now: Instant,
) -> bool {
    let _ = snapshot.complete;
    let mut dirty = false;
    let sent_ids: Vec<u64> = outbound
        .messages
        .iter()
        .filter(|message| message.status == OutboundStatus::Sent)
        .map(|message| message.id)
        .collect();
    for id in sent_ids {
        dirty |= apply_recent_turn_snapshot_to_message(outbound, id, snapshot, now);
    }
    dirty
}

fn apply_recent_turn_snapshot_to_message(
    outbound: &mut OutboundQueue,
    id: u64,
    snapshot: &RecentTurnSnapshot,
    now: Instant,
) -> bool {
    let Some(message) = outbound.message(id).cloned() else {
        return false;
    };
    let candidates = candidate_turns_after_baseline(snapshot, message.baseline_turn_count);
    if candidates.is_empty() {
        return false;
    }
    let matches = exact_matching_turn_count(&message.body, candidates.iter().copied());
    match matches {
        1 => outbound.set_status(id, OutboundStatus::Consumed, now, None),
        0 => outbound.set_status(
            id,
            OutboundStatus::Ambiguous,
            now,
            Some("new_user_turn_did_not_match".to_string()),
        ),
        _ => outbound.set_status(
            id,
            OutboundStatus::Ambiguous,
            now,
            Some("duplicate_matching_user_turns".to_string()),
        ),
    }
}

fn candidate_turns_after_baseline(
    snapshot: &RecentTurnSnapshot,
    baseline: Option<u64>,
) -> Vec<&RecentUserTurn> {
    snapshot
        .user_turns
        .iter()
        .filter(|turn| baseline.is_none_or(|count| turn.ordinal > count))
        .collect()
}

fn exact_matching_turn_count<'a>(
    body: &str,
    turns: impl Iterator<Item = &'a RecentUserTurn>,
) -> usize {
    let wanted = normalize_message_body(body);
    turns
        .filter(|turn| normalize_message_body(&turn.body) == wanted)
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

/// Bottom-pane monitor state: collapse/expand, the latest read-only snapshot, and
/// the current selection. Holds no terminal or IO handles.
#[derive(Clone)]
struct MonitorPane {
    collapsed: bool,
    view_mode: MonitorViewMode,
    selected: usize,
    selected_node_id: Option<MonitorNodeId>,
    snapshot: Option<Arc<MonitorSnapshot>>,
    pseudo_input: PseudoInputState,
    outbound: OutboundQueue,
    /// Manual detail override for nodes without an InspectRef. Nodes with an
    /// InspectRef show detail automatically while selected in the expanded pane.
    inspecting: bool,
    inspect: Vec<String>,
    last_inspect_refresh: Option<Instant>,
    /// The node id awaiting cancel confirmation, if the operator pressed `x`.
    pending_cancel: Option<MonitorNodeId>,
    /// A cancel request the operator confirmed, drained and executed by the loop.
    cancel_request: Option<CancelRequest>,
    /// The last cancel outcome message, surfaced to the operator.
    cancel_feedback: Option<String>,
}

impl MonitorPane {
    fn new() -> Self {
        Self {
            collapsed: true,
            view_mode: MonitorViewMode::Flat,
            selected: 0,
            selected_node_id: None,
            snapshot: None,
            pseudo_input: PseudoInputState::default(),
            outbound: OutboundQueue::default(),
            inspecting: false,
            inspect: Vec::new(),
            last_inspect_refresh: None,
            pending_cancel: None,
            cancel_request: None,
            cancel_feedback: None,
        }
    }

    /// Rows the monitor occupies at the bottom for the given full terminal height.
    fn bottom_rows(&self, full_rows: u16, protection: TypingProtection) -> u16 {
        if self.collapsed {
            return COLLAPSED_MONITOR_ROWS;
        }
        let target = (u32::from(full_rows) * 35 / 100) as u16;
        let ceiling = full_rows.saturating_sub(protection.top_min_rows());
        target
            .max(EXPANDED_MIN_ROWS)
            .min(ceiling)
            .max(COLLAPSED_MONITOR_ROWS)
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
        let rows = projected_monitor_rows(snapshot, self.view_mode);
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
            self.selected = index;
            self.sync_selected_node_id();
        }
    }

    fn toggle_view_mode(&mut self) {
        self.view_mode = self.view_mode.toggled();
        self.sync_selected_node_id();
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
            MonitorCommand::SelectIndex(index) => {
                self.select_index(index);
                self.update_inspect();
                false
            }
            MonitorCommand::ToggleTreeMode => {
                self.toggle_view_mode();
                false
            }
            MonitorCommand::Refresh => true,
            MonitorCommand::Collapse => {
                self.collapsed = true;
                self.inspecting = false;
                self.inspect.clear();
                self.last_inspect_refresh = None;
                self.pending_cancel = None;
                false
            }
            MonitorCommand::ToggleInspect => {
                self.inspecting = !self.inspecting;
                self.update_inspect();
                false
            }
            MonitorCommand::RequestCancel => {
                self.request_cancel();
                false
            }
            MonitorCommand::ConfirmCancel => {
                self.confirm_cancel();
                false
            }
            MonitorCommand::AbortCancel => {
                self.abort_cancel();
                false
            }
        }
    }

    fn apply_pseudo_input(&mut self, action: PseudoInputAction, now: Instant) {
        if let Some(body) = self.pseudo_input.apply(action) {
            self.outbound.enqueue(body, now);
        }
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
        self.pending_cancel = self.selected_cancelable_node().map(node_id);
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

    /// The selected node, but only when its id matches.
    fn selected_node_with_id(&self, id: &str) -> Option<&MonitorNode> {
        self.selected_node().filter(|node| node.id == id)
    }

    fn abort_cancel(&mut self) {
        self.pending_cancel = None;
    }

    fn take_cancel_request(&mut self) -> Option<CancelRequest> {
        self.cancel_request.take()
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
        !self.collapsed && (self.inspecting || self.selected_node_has_inspect_ref())
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
    typing_protection: TypingProtection,
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

    fn capture_with_typing_protection(
        screen: &vt100::Screen,
        focus: Focus,
        pane: &MonitorPane,
        selection: Option<SelectionSpan>,
        typing_protection: TypingProtection,
    ) -> Self {
        Self {
            screen: ScreenRenderSnapshot::from_screen(screen),
            focus,
            pane: pane.clone(),
            selection,
            mouse_request: effective_mouse_request(mouse_request_from_screen(screen)),
            typing_protection,
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
    let bottom_rows = snapshot
        .pane
        .bottom_rows(area.height, snapshot.typing_protection);
    let overlay_constrained =
        overlay_constrained_for_typing(&snapshot.pane, area.height, snapshot.typing_protection);
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
    protection: TypingProtection,
) -> bool {
    !pane.collapsed
        && protection.active
        && pane.bottom_rows(full_rows, protection)
            < pane.bottom_rows(full_rows, TypingProtection::inactive())
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

/// Render the monitor: a status row always, plus the node list when expanded.
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
    render_status_row(
        buf,
        Rect { height: 1, ..area },
        pane,
        focus,
        overlay_constrained,
    );
    if pane.collapsed || area.height <= 1 {
        return;
    }
    let body = bottom_body_area(area);
    let layout = expanded_bottom_layout(body);
    render_monitor_body(buf, layout.content, pane);
    render_pseudo_input(buf, layout.input, pane, focus);
    render_outbound_status(buf, layout.outbound, pane);
}

#[derive(Debug, Clone, Copy)]
struct ExpandedBottomLayout {
    content: Rect,
    input: Rect,
    outbound: Rect,
}

fn bottom_body_area(area: Rect) -> Rect {
    Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    }
}

fn expanded_bottom_layout(body: Rect) -> ExpandedBottomLayout {
    let outbound_rows = OUTBOUND_STATUS_ROWS.min(body.height);
    let input_rows = PSEUDO_INPUT_ROWS.min(body.height.saturating_sub(outbound_rows));
    let content_rows = body.height.saturating_sub(input_rows + outbound_rows);
    ExpandedBottomLayout {
        content: Rect {
            height: content_rows,
            ..body
        },
        input: Rect {
            y: body.y + content_rows,
            height: input_rows,
            ..body
        },
        outbound: Rect {
            y: body.y + content_rows + input_rows,
            height: outbound_rows,
            ..body
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
    let draft = format_pseudo_input_line(pane);
    buf.set_string(area.x, area.y, pad_to_width(draft, area.width), style);
    if area.height > 1 {
        let help = "Enter queue · arrows move list/cursor · Ctrl+O top · Ctrl+U clear";
        buf.set_string(
            area.x,
            area.y + 1,
            pad_to_width(help.to_string(), area.width),
            style,
        );
    }
}

fn format_pseudo_input_line(pane: &MonitorPane) -> String {
    let mut text = pane.pseudo_input.buffer.clone();
    let cursor = pane.pseudo_input.cursor_chars();
    let marker = if text.is_empty() { "" } else { " " };
    text.insert(pane.pseudo_input.cursor, '▌');
    format!(" input[{cursor}] >{marker}{text}")
}

fn render_outbound_status(buf: &mut Buffer, area: Rect, pane: &MonitorPane) {
    if area.height == 0 {
        return;
    }
    buf.set_string(
        area.x,
        area.y,
        pad_to_width(outbound_summary_text(&pane.outbound), area.width),
        Style::default().fg(Color::Cyan).bg(Color::Indexed(234)),
    );
}

fn outbound_summary_text(outbound: &OutboundQueue) -> String {
    let visible: Vec<String> = outbound
        .messages
        .iter()
        .rev()
        .take(4)
        .rev()
        .map(format_outbound_message_summary)
        .collect();
    if visible.is_empty() {
        " outbound: idle".to_string()
    } else {
        format!(" outbound: {}", visible.join(" · "))
    }
}

fn format_outbound_message_summary(message: &OutboundMessage) -> String {
    let age = message.created_at.elapsed().as_secs();
    match message.detail.as_deref() {
        Some(detail) => format!(
            "#{} {} {age}s ({detail})",
            message.id,
            outbound_status_word(message.status)
        ),
        None => format!(
            "#{} {} {age}s",
            message.id,
            outbound_status_word(message.status)
        ),
    }
}

fn outbound_status_word(status: OutboundStatus) -> &'static str {
    match status {
        OutboundStatus::Queued => "queued",
        OutboundStatus::Sending => "sending",
        OutboundStatus::Sent => "sent",
        OutboundStatus::Consumed => "consumed",
        OutboundStatus::Ambiguous => "ambiguous",
        OutboundStatus::Failed => "failed",
    }
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
    let label = pad_to_width(
        format!(
            " OBS  {} · {}  —  {hint}",
            monitor_summary_text(pane),
            view_mode_word(pane.view_mode)
        ),
        area.width,
    );
    let style = Style::default()
        .fg(Color::White)
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD);
    buf.set_string(area.x, area.y, label, style);
}

/// The status-row hint, reflecting focus and any armed/last cancel state.
fn status_hint(pane: &MonitorPane, focus: Focus, overlay_constrained: bool) -> String {
    match focus {
        Focus::Top if overlay_constrained => "input viewport protected · Ctrl+O focus".to_string(),
        Focus::Top => "Ctrl+O focus".to_string(),
        Focus::Bottom => bottom_status_hint(pane, overlay_constrained),
    }
}

fn bottom_status_hint(pane: &MonitorPane, overlay_constrained: bool) -> String {
    let protected_suffix = if overlay_constrained {
        " · input viewport protected"
    } else {
        ""
    };
    if pane.pending_cancel.is_some() {
        return format!("confirm cancel: y = SIGTERM · n = abort{protected_suffix}");
    }
    match pane.cancel_feedback.as_deref() {
        Some(feedback) => {
            format!(
                "type to draft · Enter queue · ↑/↓ move · Ctrl+T tree · Ctrl+O top  ({feedback}){protected_suffix}"
            )
        }
        None => {
            format!(
                "type to draft · Enter queue · ↑/↓ move · Ctrl+T tree · Ctrl+O top{protected_suffix}"
            )
        }
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
    let Some(snapshot) = pane.snapshot.as_ref() else {
        return;
    };
    if snapshot.nodes.is_empty() {
        buf.set_string(
            area.x,
            area.y,
            pad_to_width(" (no active workloads)".to_string(), area.width),
            Style::default().fg(Color::DarkGray),
        );
        return;
    }
    for VisibleNodeRow {
        index,
        y,
        node,
        prefix,
    } in visible_node_rows(pane, snapshot, area)
    {
        let row = Rect {
            y,
            height: 1,
            ..area
        };
        render_node_row(buf, row, node, &prefix, index == pane.selected);
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
) -> Vec<VisibleNodeRow<'a>> {
    let projected = projected_monitor_rows(snapshot, pane.view_mode);
    let rows = area.height as usize;
    let selected_position = selected_projected_position(&projected, pane.selected).unwrap_or(0);
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
}

impl AltScreenGuard {
    fn enter(mut writer: File) -> Result<Self, String> {
        execute!(writer, EnterAlternateScreen)
            .map_err(|err| format!("Failed to enter alternate screen: {err}"))?;
        Ok(Self {
            writer,
            mouse: TerminalMouseState::new(),
        })
    }

    fn sync_mouse(&mut self, request: MouseRequest) -> Result<(), String> {
        sync_terminal_mouse(&mut self.writer, &mut self.mouse, request)
            .map_err(format_tui_mouse_sync_error)
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
        let _ = self.writer.write_all(&terminal_mouse_restore_sequence());
        let _ = execute!(self.writer, LeaveAlternateScreen);
    }
}

fn format_tui_mouse_sync_error(err: io::Error) -> String {
    format!("Failed to sync terminal mouse mode: {err}")
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
struct RenderShared {
    snapshot: Mutex<Option<Arc<RenderSnapshot>>>,
    clipboard: Mutex<Vec<String>>,
    shutdown: AtomicBool,
    error: Mutex<Option<String>>,
    wake: Condvar,
    #[cfg(test)]
    frame_count: AtomicUsize,
}

impl RenderShared {
    fn publish(&self, snapshot: RenderSnapshot) {
        *lock_or_recover(&self.snapshot) = Some(Arc::new(snapshot));
        self.wake.notify_one();
    }

    fn latest_snapshot(&self) -> Option<Arc<RenderSnapshot>> {
        lock_or_recover(&self.snapshot).clone()
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
        let guard = lock_or_recover(&self.snapshot);
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
    loop {
        if shared.shutdown_requested() {
            render_latest_snapshot(shared, terminal, alt)?;
            return Ok(());
        }
        let now = Instant::now();
        if now < next_frame {
            shared.wait_until(next_frame);
            continue;
        }
        let Some(snapshot) = shared.latest_snapshot() else {
            next_frame = Instant::now() + render_frame_interval(BACKGROUND_RENDER_FPS);
            continue;
        };
        render_snapshot(shared, terminal, alt, &snapshot)?;
        next_frame = Instant::now() + snapshot_frame_interval(&snapshot);
    }
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
    snapshot.focus == Focus::Bottom && !snapshot.pane.collapsed
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
pub(super) fn relay_until_exit_observed(
    input_fd: RawFd,
    writer: File,
    master: &File,
    control: Option<&ControlSocket>,
    child: &mut Child,
    monitor: MonitorSnapshotProvider,
    root: ObservabilityRoot,
) -> Result<ExitStatus, String> {
    let real_fd = input_fd;
    let master_fd = master.as_raw_fd();
    let renderer = RenderThread::start(writer)?;
    let publisher = renderer.publisher();

    let mut pane = MonitorPane::new();
    let snapshot_worker = MonitorSnapshotWorker::start(monitor, root, pane.refresh_interval())?;
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
    let mut recent_turns = RecentTurnPump::disabled(Instant::now());
    let mut status = None;
    publish_render_snapshot(
        &publisher,
        &parser,
        router.focus,
        &pane,
        None,
        typing_protection(router.focus, &line_state),
    );

    while status.is_none() {
        publisher.check_error()?;
        let mut dirty = pane.adopt_snapshot(snapshot_worker.latest_snapshot());
        dirty |= pane.refresh_detail_if_due(Instant::now());
        let mut protection = typing_protection(router.focus, &line_state);
        dirty |= apply_sizing(
            real_fd,
            master_fd,
            child.id(),
            &pane,
            protection,
            &mut parser,
            &mut applied,
        );
        dirty |= pump_outbound_queue(
            &mut pane,
            &mut pending_child_input,
            &mut line_state,
            parser.screen().bracketed_paste(),
            &mut recent_turns,
            Instant::now(),
        );
        let ready = poll_relay_fds(
            real_fd,
            master_fd,
            control.map(ControlSocket::fd),
            !pending_child_input.is_empty(),
        )?;
        if ready.pty_writable {
            flush_pending_child_input(master_fd, &mut pending_child_input)?;
            dirty |= pump_outbound_queue(
                &mut pane,
                &mut pending_child_input,
                &mut line_state,
                parser.screen().bracketed_paste(),
                &mut recent_turns,
                Instant::now(),
            );
        }
        if ready.real_input {
            let mut routed = forward_real_input(
                real_fd,
                &mut router,
                &pane,
                mouse_request_from_screen(parser.screen()),
                &mut line_state,
                &mut pending_child_input,
                &mut buffer,
            )?;
            let scroll_lines = routed.top_scroll_lines;
            // Sending keystrokes to the child snaps the view back to the live tail, like
            // a terminal jumps to the prompt when you start typing.
            let typed_to_child = !routed.forward.is_empty();
            let right_click = routed.right_click;
            let gestures = std::mem::take(&mut routed.top_mouse);
            dirty |= apply_routed_to_pane(&mut pane, routed, &snapshot_worker);
            dirty |= pump_outbound_queue(
                &mut pane,
                &mut pending_child_input,
                &mut line_state,
                parser.screen().bracketed_paste(),
                &mut recent_turns,
                Instant::now(),
            );
            if typed_to_child {
                // Typing snaps to the live tail and drops the selection highlight.
                selection = None;
                if top_scrollback != 0 {
                    top_scrollback = 0;
                }
                dirty = true;
            } else if scroll_lines != 0 {
                // Keep the selection — its highlight follows the content as we scroll.
                top_scrollback = apply_top_scroll(top_scrollback, scroll_lines);
                dirty = true;
            }
            if !gestures.is_empty() {
                dirty |= apply_selection_gestures(
                    &mut selection,
                    &gestures,
                    parser.screen(),
                    &publisher,
                    top_scrollback,
                    &mut clipboard,
                )?;
            }
            if let Some(click) = right_click {
                let mut io = MouseActionIo {
                    clipboard: &mut clipboard,
                    renderer: &publisher,
                    line_state: &mut line_state,
                    pending_child_input: &mut pending_child_input,
                };
                dirty |= handle_top_right_click(
                    &mut selection,
                    click,
                    parser.screen(),
                    top_scrollback,
                    &mut io,
                )?;
            }
        }
        if ready.pty_output && pump_pty_output(master_fd, &mut parser, &mut buffer)? {
            child_output_state.observe_child_output();
            dirty = true;
        }
        if ready.control
            && let Some(control) = control
        {
            let mut control_io = ControlInjectionIo {
                real_fd,
                master_fd,
                router: &mut router,
                pane: &pane,
                parser: &mut parser,
                line_state: &mut line_state,
                child_output_state: &mut child_output_state,
                pending_child_input: &mut pending_child_input,
                buffer: &mut buffer,
                child_pid: Some(child.id()),
            };
            let _ = service_control(control, &mut control_io);
            dirty = true;
        }
        // Re-assert the scrollback view each frame (clamped to retained history) so it
        // survives child output and resizes; reading it back keeps our offset honest.
        protection = typing_protection(router.focus, &line_state);
        dirty |= apply_sizing(
            real_fd,
            master_fd,
            child.id(),
            &pane,
            protection,
            &mut parser,
            &mut applied,
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
    );
    renderer.shutdown_and_join()?;
    snapshot_worker.shutdown_and_join()?;
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
    child_winsize(full, pane.bottom_rows(full.ws_row, protection))
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
    let bottom = pane.bottom_rows(full.ws_row, protection);
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
fn forward_real_input(
    real_fd: RawFd,
    router: &mut InputRouter,
    pane: &MonitorPane,
    mouse_request: MouseRequest,
    line_state: &mut InputLineState,
    pending_child_input: &mut PendingChildInput,
    buffer: &mut [u8],
) -> Result<RoutedInput, String> {
    match read_real_input(real_fd, buffer) {
        Ok(0) => Ok(RoutedInput::default()),
        Ok(n) => {
            let full = terminal_winsize_with_fallback(read_terminal_winsize(real_fd));
            let protection = typing_protection(router.focus, line_state);
            let routed = route_real_input_with_protection(
                &buffer[..n],
                router,
                pane,
                mouse_request,
                &full,
                protection,
            );
            enqueue_routed_child_input(line_state, pending_child_input, &routed);
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
    let mut routed = RoutedInput::default();
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
        inject_clipboard_paste(io.line_state, io.pending_child_input, io.clipboard);
        Ok(false)
    }
}

/// Inject the broker clipboard into the child as a bracketed paste, so the child treats
/// it as pasted data rather than typed commands (no accidental command execution).
fn inject_clipboard_paste(
    line_state: &mut InputLineState,
    pending_child_input: &mut PendingChildInput,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let bytes = wrap_real_terminal_paste(text.as_bytes());
    line_state.observe_user_input(&bytes);
    pending_child_input.enqueue(&bytes);
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
        if let Some(index) = bottom_visible_row_index(event, bottom, pane) {
            apply_monitor_command(routed, MonitorCommand::SelectIndex(index));
        }
    }
}

fn bottom_visible_row_index(event: MouseEvent, bottom: Rect, pane: &MonitorPane) -> Option<usize> {
    let list = bottom_list_area(bottom, pane)?;
    if !rect_contains_mouse(list, event) {
        return None;
    }
    let snapshot = pane.snapshot.as_ref()?;
    let row = usize::from(event.row.saturating_sub(list.y.saturating_add(1)));
    let projected = projected_monitor_rows(snapshot, pane.view_mode);
    let selected_position = selected_projected_position(&projected, pane.selected).unwrap_or(0);
    let offset = scroll_offset(selected_position, projected.len(), list.height as usize);
    projected.get(offset + row).map(|row| row.index)
}

fn bottom_list_area(bottom: Rect, pane: &MonitorPane) -> Option<Rect> {
    if pane.collapsed || bottom.height <= 1 {
        return None;
    }
    let body = expanded_bottom_layout(bottom_body_area(bottom)).content;
    if body.height == 0 {
        return None;
    }
    if pane.detail_visible() && body.height >= 4 {
        Some(Rect {
            height: (body.height / 2).max(2),
            ..body
        })
    } else {
        Some(body)
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
    routed: &RoutedInput,
) {
    let Some(bytes) = routed_child_input(routed) else {
        return;
    };
    let child_bytes = child_input_for_real_read(bytes);
    line_state.observe_user_input(&child_bytes);
    pending_child_input.enqueue(&child_bytes);
}

/// Apply routed monitor effects (expand/select/refresh/collapse) to the pane.
/// Returns whether a redraw is needed.
fn apply_routed_to_pane(
    pane: &mut MonitorPane,
    routed: RoutedInput,
    snapshot_worker: &MonitorSnapshotWorker,
) -> bool {
    if routed.focus_bottom {
        pane.expand();
    }
    apply_routed_pseudo_input(pane, &routed.pseudo_input);
    let force_refresh = apply_routed_commands(pane, &routed.commands);
    let cancelled = run_pending_cancel(pane);
    snapshot_worker.set_interval(pane.refresh_interval());
    if pane_refresh_required(force_refresh, cancelled) {
        snapshot_worker.request_refresh();
    }
    routed.redraw
}

fn apply_routed_pseudo_input(pane: &mut MonitorPane, actions: &[PseudoInputAction]) {
    let now = Instant::now();
    for action in actions {
        pane.apply_pseudo_input(*action, now);
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
) {
    publisher.publish(RenderSnapshot::capture_with_typing_protection(
        parser.screen(),
        focus,
        pane,
        selection,
        typing_protection,
    ));
}

fn pump_outbound_queue(
    pane: &mut MonitorPane,
    pending_child_input: &mut PendingChildInput,
    line_state: &mut InputLineState,
    bracketed_paste: bool,
    recent_turns: &mut RecentTurnPump,
    now: Instant,
) -> bool {
    let mut dirty = recent_turns.poll_if_due(now, &mut pane.outbound);
    dirty |= mark_outbound_timeouts(&mut pane.outbound, now);
    dirty |= advance_active_outbound_send(&mut pane.outbound, pending_child_input, line_state, now);
    dirty |= start_next_outbound_message(
        &mut pane.outbound,
        pending_child_input,
        line_state,
        bracketed_paste,
        recent_turns.last_turn_count(),
        now,
    );
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
    bracketed_paste: bool,
    baseline_turn_count: Option<u64>,
    now: Instant,
) -> bool {
    if outbound.active.is_some()
        || outbound.has_unresolved_blocker()
        || !pending_child_input.is_empty()
        || !line_state.is_safe_to_inject()
    {
        return false;
    }
    let Some(id) = outbound.next_queued_id() else {
        return false;
    };
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
    let child_bytes = control_payload_bytes(body.as_bytes(), bracketed_paste);
    line_state.observe_user_input(&child_bytes);
    pending_child_input.enqueue(&child_bytes);
    outbound.mark_sending(id, baseline_turn_count);
    outbound.active = Some(ActiveOutboundSend {
        message_id: id,
        phase: OutboundSendPhase::Body,
        bracketed_paste,
    });
    true
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
    real_fd: RawFd,
    master_fd: RawFd,
    router: &'a mut InputRouter,
    pane: &'a MonitorPane,
    parser: &'a mut vt100::Parser,
    line_state: &'a mut InputLineState,
    child_output_state: &'a mut ChildOutputState,
    pending_child_input: &'a mut PendingChildInput,
    buffer: &'a mut [u8],
    child_pid: Option<u32>,
}

/// Service a control-socket notify injection while the TUI owns the screen:
/// inject the payload to the child at the next safe line boundary, pumping output
/// into the virtual terminal (never to the real terminal) during the wait.
fn service_control(control: &ControlSocket, io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let mut stream = accept_control_stream(control).map_err(format_control_accept_error)?;
    let response = inject_control_payload(&mut stream, io);
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

fn control_response_message(response: Result<(), String>) -> (bool, String) {
    match response {
        Ok(()) => (true, "ok".to_string()),
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
) -> Result<(), String> {
    validate_control_peer(stream)?;
    let payload = read_tui_control_payload(stream)?;
    // Match the non-TUI broker contract: only submit proactive control payloads
    // when the agent owns the foreground process group, child output has cleared
    // the short debounce, and the line is either at a parsed boundary or user
    // input has been idle long enough for the submit-parser fallback.
    wait_until_safe_to_inject(io)?;
    let bracketed_paste = io.parser.screen().bracketed_paste();
    submit_control_payload(io, &payload, bracketed_paste)?;
    Ok(())
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
    drain_control_payload_body(io)?;
    // Let the child commit the (pasted) body to its input buffer before the Enter, so the
    // submit doesn't race ahead of the async paste commit and get dropped.
    if bracketed_paste {
        std::thread::sleep(CONTROL_SUBMIT_DELAY);
    }
    io.pending_child_input.enqueue(b"\r");
    io.line_state.mark_submitted();
    Ok(())
}

fn drain_control_payload_body(io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let start = Instant::now();
    while !io.pending_child_input.is_empty() {
        if start.elapsed() >= INJECT_WAIT_LIMIT {
            return Err("control_submit_body_drain_timeout".to_string());
        }
        let ready = poll_control_submit_pty(io.master_fd)?;
        if ready.pty_writable {
            flush_pending_child_input(io.master_fd, io.pending_child_input)?;
        }
        if ready.pty_output {
            if pump_pty_output(io.master_fd, io.parser, io.buffer)? {
                io.child_output_state.observe_child_output();
            } else {
                return Err("control_submit_pty_closed".to_string());
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

/// Wait until proactive injection is safe, pumping output into the virtual
/// terminal and routing real input meanwhile, bounded by the inject limit.
fn wait_until_safe_to_inject(io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let start = Instant::now();
    while injection_wait_should_pump(
        start,
        io.master_fd,
        io.child_pid,
        io.line_state,
        io.child_output_state,
        io.pending_child_input,
    ) {
        pump_inject_wait_io(io)?;
    }
    validate_safe_to_inject(
        io.master_fd,
        io.child_pid,
        io.line_state,
        io.child_output_state,
        io.pending_child_input,
    )
}

fn injection_wait_should_pump(
    start: Instant,
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &InputLineState,
    child_output_state: &ChildOutputState,
    pending_child_input: &PendingChildInput,
) -> bool {
    inject_wait_remaining(start)
        && !safe_to_inject_now(
            master_fd,
            child_pid,
            line_state,
            child_output_state,
            pending_child_input,
        )
}

fn inject_wait_remaining(start: Instant) -> bool {
    start.elapsed() < INJECT_WAIT_LIMIT
}

fn safe_to_inject_now(
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &InputLineState,
    child_output_state: &ChildOutputState,
    pending_child_input: &PendingChildInput,
) -> bool {
    pending_child_input.is_empty()
        && super::safe_to_inject(master_fd, child_pid, line_state, child_output_state).is_ok()
}

fn validate_safe_to_inject(
    master_fd: RawFd,
    child_pid: Option<u32>,
    line_state: &InputLineState,
    child_output_state: &ChildOutputState,
    pending_child_input: &PendingChildInput,
) -> Result<(), String> {
    if !pending_child_input.is_empty() {
        return Err(unsafe_mid_line_error());
    }
    if super::safe_to_inject(master_fd, child_pid, line_state, child_output_state).is_ok() {
        Ok(())
    } else {
        Err(
            super::safe_to_inject(master_fd, child_pid, line_state, child_output_state)
                .err()
                .map(super::unsafe_reason_message)
                .unwrap_or_else(unsafe_mid_line_error),
        )
    }
}

fn pump_inject_wait_io(io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let ready = poll_relay_fds(
        io.real_fd,
        io.master_fd,
        None,
        !io.pending_child_input.is_empty(),
    )?;
    if ready.pty_writable {
        flush_pending_child_input(io.master_fd, io.pending_child_input)?;
    }
    if ready.real_input {
        forward_real_input(
            io.real_fd,
            io.router,
            io.pane,
            mouse_request_from_screen(io.parser.screen()),
            io.line_state,
            io.pending_child_input,
            io.buffer,
        )?;
    }
    if ready.pty_output && pump_pty_output(io.master_fd, io.parser, io.buffer)? {
        io.child_output_state.observe_child_output();
    }
    Ok(())
}

fn unsafe_mid_line_error() -> String {
    "unsafe_mid_line".to_string()
}

#[cfg(test)]
mod tests {
    use super::super::{PtyPair, configure_child_pty};
    use super::*;
    use ratatui::backend::TestBackend;
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    fn row_text(buf: &Buffer, area_y: u16, width: u16) -> String {
        (0..width)
            .map(|x| buf[(x, area_y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn top_pane_reserves_the_monitor_row() {
        let full = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let top = top_pane_winsize(&full);
        assert_eq!(top.ws_row, 23);
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
    fn top_focus_forwards_bytes_and_toggle_does_not() {
        let mut router = InputRouter::new();
        let routed = router.route_input(b"abc");
        assert_eq!(routed.forward, b"abc".to_vec());
        assert!(!routed.redraw);
        assert_eq!(router.focus, Focus::Top);

        let routed = router.route_input(&[FOCUS_TOGGLE_BYTE]);
        assert!(routed.forward.is_empty());
        assert!(routed.redraw);
        assert_eq!(router.focus, Focus::Bottom);
    }

    #[test]
    fn bottom_focus_edits_draft_until_toggle_returns_to_top() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
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

        let routed = router.route_input(&[FOCUS_TOGGLE_BYTE]);
        assert!(routed.redraw);
        assert_eq!(router.focus, Focus::Top);

        let routed = router.route_input(b"x");
        assert_eq!(routed.forward, b"x".to_vec());
    }

    #[test]
    fn doubled_toggle_returns_to_top_without_forwarding() {
        // Ctrl+O then Ctrl+O: Top -> Bottom -> Top, nothing forwarded.
        let mut router = InputRouter::new();
        let routed = router.route_input(&[FOCUS_TOGGLE_BYTE, FOCUS_TOGGLE_BYTE]);
        assert!(routed.forward.is_empty());
        assert_eq!(router.focus, Focus::Top);
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
    fn bottom_status_row_primary_click_focuses_bottom_and_requests_expansion() {
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let winsize = libc::winsize {
            ws_row: 10,
            ws_col: 20,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(
            b"\x1b[<0;4;10M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );

        assert_eq!(router.focus, Focus::Bottom);
        assert!(routed.focus_bottom);
        assert!(routed.redraw);
        assert!(routed.forward.is_empty());
        assert!(routed.commands.is_empty());
    }

    #[test]
    fn top_pane_primary_click_restores_top_focus_from_bottom() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
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
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
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
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        let pane = pane_with(snapshot_with_nodes(12), false, 9);
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(
            b"\x1b[<0;4;14M",
            &mut router,
            &pane,
            MouseRequest::disabled(),
            &winsize,
        );

        assert_eq!(router.focus, Focus::Bottom);
        assert!(routed.focus_bottom);
        assert_eq!(routed.commands, vec![MonitorCommand::SelectIndex(6)]);
        let mut applied = pane.clone();
        applied.apply(routed.commands[0]);
        assert_eq!(applied.selected, 6);
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

        let routed = route_real_input(b"\x1b[<0;4;19M", &mut router, &pane, child, &winsize);

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
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
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
    fn control_submit_drains_body_before_queueing_enter() {
        let (mut read_end, write_end) = pipe_files();
        let mut router = InputRouter::new();
        let pane = MonitorPane::new();
        let mut parser = vt100::Parser::new(10, 20, 0);
        let mut line_state = InputLineState::default();
        let mut child_output_state = ChildOutputState::default();
        let mut pending_child_input = PendingChildInput::new();
        let mut buffer = vec![0_u8; RELAY_BUFFER_BYTES];
        let mut io = ControlInjectionIo {
            real_fd: read_end.as_raw_fd(),
            master_fd: write_end.as_raw_fd(),
            router: &mut router,
            pane: &pane,
            parser: &mut parser,
            line_state: &mut line_state,
            child_output_state: &mut child_output_state,
            pending_child_input: &mut pending_child_input,
            buffer: &mut buffer,
            child_pid: None,
        };

        submit_control_payload(&mut io, b"body", true).expect("submit payload");

        let expected_body = control_payload_bytes(b"body", true);
        let mut received_body = vec![0_u8; expected_body.len()];
        read_end
            .read_exact(&mut received_body)
            .expect("body should drain before submit returns");
        assert_eq!(received_body, expected_body);
        assert_eq!(pending_child_input.pending_len(), 1);

        flush_pending_child_input(write_end.as_raw_fd(), &mut pending_child_input)
            .expect("flush queued enter");
        let mut enter = [0_u8; 1];
        read_end
            .read_exact(&mut enter)
            .expect("submit enter should remain queued after body drain");
        assert_eq!(enter, [b'\r']);
    }

    #[test]
    fn pseudo_input_draft_bytes_do_not_forward_before_enter() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        let mut pane = MonitorPane::new();
        let routed = router.route_input(b"draft only");

        assert!(routed.forward.is_empty());
        apply_routed_pseudo_input(&mut pane, &routed.pseudo_input);

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
            pane.apply_pseudo_input(action, now);
        }
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let mut recent = RecentTurnPump::disabled(now);

        assert_eq!(pane.pseudo_input.buffer, "");
        assert_eq!(pane.outbound.messages.len(), 1);
        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
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
            &mut recent,
            now,
        ));
        assert_eq!(
            pending.pending_len(),
            pending_after_start,
            "no duplicate send"
        );
    }

    #[test]
    fn queued_message_transitions_sending_to_sent_after_body_and_enter_drain() {
        let now = Instant::now();
        let (mut read_end, write_end) = pipe_files();
        let mut pane = queued_pane("hello", now);
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let mut recent = RecentTurnPump::disabled(now);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"hello");
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"\r");

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
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
        let mut recent = RecentTurnPump::disabled(now);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            &mut recent,
            now,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, &control_payload_bytes(b"hello", true));
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            &mut recent,
            now,
        );
        assert_eq!(pending.pending_len(), 0, "submit waits for paste delay");

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            true,
            &mut recent,
            now + CONTROL_SUBMIT_DELAY,
        );
        flush_pending_child_input(write_end.as_raw_fd(), &mut pending).unwrap();
        assert_pipe_bytes(&mut read_end, b"\r");
    }

    #[test]
    fn sent_message_becomes_consumed_on_exact_recent_user_turn_match() {
        let now = Instant::now();
        let mut pane = sent_pane("hello", Some(0), now);
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let mut recent = RecentTurnPump::with_reader(
            Box::new(FakeRecentTurnReader::new(vec![RecentTurnRead::Available(
                recent_snapshot([(1, "hello")]),
            )])),
            now,
        );

        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        ));

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Consumed));
    }

    #[test]
    fn duplicate_or_transformed_turns_mark_sent_message_ambiguous() {
        let now = Instant::now();
        let mut duplicate = sent_pane("hello", Some(0), now);
        assert!(apply_recent_turn_snapshot(
            &mut duplicate.outbound,
            &recent_snapshot([(1, "hello"), (2, "hello")]),
            now,
        ));
        assert_eq!(
            duplicate.outbound.status(1),
            Some(OutboundStatus::Ambiguous)
        );

        let mut transformed = sent_pane("hello", Some(0), now);
        assert!(apply_recent_turn_snapshot(
            &mut transformed.outbound,
            &recent_snapshot([(1, "HELLO")]),
            now,
        ));
        assert_eq!(
            transformed.outbound.status(1),
            Some(OutboundStatus::Ambiguous)
        );
    }

    #[test]
    fn unavailable_turn_source_times_out_sent_message_to_ambiguous() {
        let now = Instant::now();
        let mut pane = sent_pane(
            "hello",
            Some(0),
            now - OUTBOUND_CONSUMPTION_TIMEOUT - Duration::from_millis(1),
        );
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let mut recent = RecentTurnPump::disabled(now);

        assert!(pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        ));

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Ambiguous));
    }

    #[test]
    fn single_flight_blocks_later_message_until_first_is_consumed() {
        let now = Instant::now();
        let mut pane = MonitorPane::new();
        pane.outbound.enqueue("first".to_string(), now);
        pane.outbound.enqueue("second".to_string(), now);
        let mut pending = PendingChildInput::new();
        let mut line_state = InputLineState::default();
        let mut recent = RecentTurnPump::disabled(now);

        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        );
        drain_pending_without_pipe(&mut pending);
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        );
        drain_pending_without_pipe(&mut pending);
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        );

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Sent));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Queued));
        assert!(pending.is_empty());

        apply_recent_turn_snapshot(&mut pane.outbound, &recent_snapshot([(1, "first")]), now);
        pump_outbound_queue(
            &mut pane,
            &mut pending,
            &mut line_state,
            false,
            &mut recent,
            now,
        );

        assert_eq!(pane.outbound.status(1), Some(OutboundStatus::Consumed));
        assert_eq!(pane.outbound.status(2), Some(OutboundStatus::Sending));
    }

    struct FakeRecentTurnReader {
        reads: Vec<RecentTurnRead>,
    }

    impl FakeRecentTurnReader {
        fn new(reads: Vec<RecentTurnRead>) -> Self {
            Self { reads }
        }
    }

    impl RecentTurnReader for FakeRecentTurnReader {
        fn read_recent_turns(&mut self) -> RecentTurnRead {
            if self.reads.is_empty() {
                RecentTurnRead::Unavailable("empty_fake_reader".to_string())
            } else {
                self.reads.remove(0)
            }
        }
    }

    fn queued_pane(body: &str, now: Instant) -> MonitorPane {
        let mut pane = MonitorPane::new();
        pane.outbound.enqueue(body.to_string(), now);
        pane
    }

    fn sent_pane(body: &str, baseline: Option<u64>, sent_at: Instant) -> MonitorPane {
        let mut pane = queued_pane(body, sent_at);
        pane.outbound.mark_sending(1, baseline);
        pane.outbound
            .set_status(1, OutboundStatus::Sent, sent_at, None);
        pane
    }

    fn recent_snapshot<const N: usize>(turns: [(u64, &str); N]) -> RecentTurnSnapshot {
        RecentTurnSnapshot {
            turn_count: turns.last().map(|(ordinal, _)| *ordinal).unwrap_or(0),
            user_turns: turns
                .into_iter()
                .map(|(ordinal, body)| RecentUserTurn {
                    ordinal,
                    body: body.to_string(),
                })
                .collect(),
            complete: true,
        }
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
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        // top pane is 4 rows (5 - 1 monitor row), 20 cols.
        let mut parser = vt100::Parser::new(4, 20, 0);
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
        let bottom_row = row_text(buf, 4, 20);
        assert!(bottom_row.contains("OBS"), "bottom row: {bottom_row:?}");
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
                input_fd, writer, &master, None, &mut child, monitor, root,
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

        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(
            r#"[ -t 0 ] || exit 7; IFS= read -r -t 1 line || exit 6; [ "$line" = "ping" ] && exit 42 || exit 8"#,
        );
        configure_child_pty(&mut cmd, &pty).expect("configure child pty");
        let child = cmd.spawn().expect("spawn child");
        drop(pty.slave);

        let writer = outer.slave.try_clone().expect("clone writer");
        let input_fd = outer.slave.as_raw_fd();
        let master = pty.master;
        let done = Arc::new(AtomicBool::new(false));
        let done_relay = Arc::clone(&done);
        let monitor = Box::new(SlowMonitor::new(
            empty_snapshot(),
            Duration::from_millis(1500),
        ));
        let root = ObservabilityRoot::default();
        let relay = thread::spawn(move || {
            let mut child = child;
            let result = relay_until_exit_observed(
                input_fd, writer, &master, None, &mut child, monitor, root,
            );
            done_relay.store(true, Ordering::SeqCst);
            result
        });

        set_nonblocking(outer.master.as_raw_fd());
        let mut buf = [0_u8; 8192];
        let start = Instant::now();
        let mut injected = false;
        loop {
            let _ = (&outer.master).read(&mut buf);
            if !injected && start.elapsed() >= Duration::from_millis(200) {
                (&outer.master).write_all(b"ping\n").expect("write input");
                injected = true;
            }
            if done.load(Ordering::SeqCst) {
                break;
            }
            assert!(
                start.elapsed() <= Duration::from_secs(6),
                "relay should finish even while the snapshot worker is sleeping"
            );
            thread::sleep(Duration::from_millis(5));
        }

        let status = relay
            .join()
            .expect("relay thread panicked")
            .expect("relay error");
        assert_eq!(
            status.code(),
            Some(42),
            "child read timeout proves input was forwarded without waiting for the slow snapshot"
        );
    }

    // The child PTY is resized to the TOP pane (one row reserved) on terminal
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

        // Collapsed: child PTY reserves one monitor row (30 -> 29).
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
            (29, 100),
            "collapsed virtual terminal"
        );
        let mut ws = unsafe { std::mem::zeroed::<libc::winsize>() };
        let rc = unsafe { libc::ioctl(pty.master.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
        assert_eq!(rc, 0);
        assert_eq!((ws.ws_row, ws.ws_col), (29, 100), "collapsed child PTY");

        // Expanding the monitor shrinks the top pane and the child PTY.
        let mut expanded = MonitorPane::new();
        expanded.expand();
        let protection = TypingProtection::inactive();
        let bottom = expanded.bottom_rows(30, protection);
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

    struct FakeMonitor {
        snapshot: MonitorSnapshot,
    }

    impl FakeMonitor {
        fn new(snapshot: MonitorSnapshot) -> Self {
            Self { snapshot }
        }
    }

    impl ObservabilitySnapshotPort for FakeMonitor {
        fn snapshot(&self, _root: &ObservabilityRoot, _limits: SnapshotLimits) -> MonitorSnapshot {
            self.snapshot.clone()
        }
    }

    struct SlowMonitor {
        snapshot: MonitorSnapshot,
        delay: Duration,
    }

    impl SlowMonitor {
        fn new(snapshot: MonitorSnapshot, delay: Duration) -> Self {
            Self { snapshot, delay }
        }
    }

    impl ObservabilitySnapshotPort for SlowMonitor {
        fn snapshot(&self, _root: &ObservabilityRoot, _limits: SnapshotLimits) -> MonitorSnapshot {
            thread::sleep(self.delay);
            self.snapshot.clone()
        }
    }

    fn pane_with(snapshot: MonitorSnapshot, collapsed: bool, selected: usize) -> MonitorPane {
        let selected_node_id = snapshot.nodes.get(selected).map(node_id);
        MonitorPane {
            collapsed,
            view_mode: MonitorViewMode::Flat,
            selected,
            selected_node_id,
            snapshot: Some(Arc::new(snapshot)),
            pseudo_input: PseudoInputState::default(),
            outbound: OutboundQueue::default(),
            inspecting: false,
            inspect: Vec::new(),
            last_inspect_refresh: None,
            pending_cancel: None,
            cancel_request: None,
            cancel_feedback: None,
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
    fn collapsed_reserves_one_row_expanded_reserves_a_bounded_share() {
        let collapsed = MonitorPane::new();
        assert_eq!(collapsed.bottom_rows(40, TypingProtection::inactive()), 1);
        let mut expanded = MonitorPane::new();
        expanded.expand();
        let rows = expanded.bottom_rows(40, TypingProtection::inactive());
        assert!(rows >= EXPANDED_MIN_ROWS, "rows={rows}");
        assert!(rows <= 40 - TOP_PANE_MIN_ROWS, "rows={rows}");
    }

    #[test]
    fn input_safe_floor_reduces_expanded_bottom_rows_across_terminal_sizes() {
        let mut pane = MonitorPane::new();
        pane.expand();
        for full_rows in [15, 20, 40] {
            let bottom = pane.bottom_rows(full_rows, TypingProtection::active());
            let top = full_rows.saturating_sub(bottom);
            assert!(
                top >= INPUT_SAFE_TOP_PANE_MIN_ROWS || bottom == COLLAPSED_MONITOR_ROWS,
                "full_rows={full_rows} top={top} bottom={bottom}"
            );
        }
        assert_eq!(pane.bottom_rows(15, TypingProtection::active()), 1);
        assert!(pane.bottom_rows(20, TypingProtection::active()) < EXPANDED_MIN_ROWS);
        assert_eq!(pane.bottom_rows(40, TypingProtection::active()), 14);
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
        let row = row_text(terminal.backend().buffer(), 5, 80);
        assert!(row.contains("OBS"), "status row: {row:?}");
        assert!(row.contains("running"), "status row: {row:?}");
        assert!(row.contains("3 mailbox pending"), "status row: {row:?}");
    }

    #[test]
    fn ctrl_t_routes_to_tree_mode_toggle_without_forwarding() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);

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
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        let mut pane = pane_with(empty_snapshot(), false, 0);
        let routed = router.route_input(b"hi\x1b[D!\r");

        apply_routed_pseudo_input(&mut pane, &routed.pseudo_input);

        assert_eq!(pane.pseudo_input.buffer, "");
        assert_eq!(pane.outbound.messages.len(), 1);
        assert_eq!(pane.outbound.messages[0].body, "h!i");
        assert_eq!(pane.outbound.messages[0].status, OutboundStatus::Queued);
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
        let backend = TestBackend::new(60, 16);
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
        let parser = vt100::Parser::new(5, 60, 0);
        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();
        let text = screen_text(terminal.backend().buffer(), 16, 60);
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
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(
            b"\x1b[<0;4;15M",
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
        let backend = TestBackend::new(60, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let parser = vt100::Parser::new(5, 60, 0);

        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();

        let text = screen_text(terminal.backend().buffer(), 16, 60);
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
        let backend = TestBackend::new(80, 12);
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
        let parser = vt100::Parser::new(5, 80, 0);
        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();
        let text = screen_text(terminal.backend().buffer(), 12, 80);
        assert!(text.contains("provider turn"), "{text}");
        assert!(text.contains("cargo test"), "{text}");
        assert!(
            text.lines()
                .any(|line| line.contains('>') && line.contains("cargo test")),
            "selected row should be marked: {text}"
        );
    }

    #[test]
    fn tree_mode_renders_indented_parent_child_rows() {
        let backend = TestBackend::new(80, 12);
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
        let parser = vt100::Parser::new(5, 80, 0);

        terminal
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane, None))
            .unwrap();

        let text = screen_text(terminal.backend().buffer(), 12, 80);
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
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        let winsize = libc::winsize {
            ws_row: 20,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let routed = route_real_input(
            b"\x1b[<0;4;15M",
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
        let rows = visible_node_rows(
            &pane,
            snapshot,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 2,
            },
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
    fn bottom_focus_arrows_decode_to_monitor_commands_and_printable_edits() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
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
            router.route_input(&[0x1b, b'[', b'B']).commands,
            vec![MonitorCommand::SelectNext]
        );
        assert_eq!(
            router.route_input(&[0x1b, b'[', b'A']).commands,
            vec![MonitorCommand::SelectPrev]
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
        pane.apply(MonitorCommand::ConfirmCancel);
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
        pane.apply(MonitorCommand::AbortCancel);
        assert_eq!(pane.pending_cancel, None);
        assert_eq!(pane.take_cancel_request(), None);
    }

    #[test]
    fn confirm_after_selection_moves_off_armed_node_does_not_cancel() {
        let mut pane = cancel_pane();
        pane.apply(MonitorCommand::SelectNext);
        pane.apply(MonitorCommand::RequestCancel);
        pane.apply(MonitorCommand::SelectPrev);
        pane.apply(MonitorCommand::ConfirmCancel);
        assert_eq!(
            pane.take_cancel_request(),
            None,
            "selection moved off the armed node, so no request is produced"
        );
    }

    #[test]
    fn cancel_keys_route_to_cancel_commands() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        assert_eq!(
            router.route_input(&[0x18]).commands,
            vec![MonitorCommand::RequestCancel]
        );
        assert_eq!(
            router.route_input(&[0x19]).commands,
            vec![MonitorCommand::ConfirmCancel]
        );
        assert_eq!(
            router.route_input(&[0x0e]).commands,
            vec![MonitorCommand::AbortCancel]
        );
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
