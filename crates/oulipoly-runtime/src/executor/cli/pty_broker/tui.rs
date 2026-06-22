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
//!   `ratatui` style; [`render_vt_screen`] maps the virtual screen grid into the
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
use super::transcript_view::project_transcript_tail;
use super::{
    ControlSocket, INJECT_WAIT_LIMIT, InputLineState, RELAY_BUFFER_BYTES, is_pty_eof_error,
    poll_relay_fds, poll_single_fd, read_control_request, read_fd, send_signal_to_child_group,
    set_pty_winsize, terminal_winsize, validate_peer_uid, winsize_eq, write_all_fd,
    write_control_response,
};
use crate::observability::{
    InspectRef, LivenessStatus, MonitorDiagnostic, MonitorDiagnosticSeverity, MonitorNode,
    MonitorNodeId, MonitorNodeKind, MonitorSnapshot, MonitorStatus, ObservabilityRoot,
    ObservabilitySnapshotPort, SnapshotLimits,
};
use base64::Engine as _;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

/// Rows reserved for the collapsed monitor row at the bottom of the screen.
const COLLAPSED_MONITOR_ROWS: u16 = 1;
/// Minimum terminal rows for the split to be usable.
const MIN_TERMINAL_ROWS: u16 = 10;
/// Minimum terminal columns for the split to be usable.
const MIN_TERMINAL_COLS: u16 = 40;
/// Reserved focus-toggle byte: Ctrl+O.
const FOCUS_TOGGLE_BYTE: u8 = 0x0f;

/// Bracketed-paste delimiters (DECSET 2004) the broker wraps an injected notification in
/// when the child has advertised the mode, so the body is treated as pasted content and
/// the trailing Enter submits it.
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
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

/// A bottom-pane (monitor) command decoded from keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorCommand {
    SelectNext,
    SelectPrev,
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
    Quit,
    UpArrow,
    DownArrow,
    VimUp,
    VimDown,
    Refresh,
    Inspect,
    Cancel,
    Confirm,
    Abort,
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
    Consume,
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
        [b'q', ..] => ParsedBottomKey {
            key: BottomKey::Quit,
            consumed: 1,
        },
        [0x1b, b'[', b'A', ..] => ParsedBottomKey {
            key: BottomKey::UpArrow,
            consumed: 3,
        },
        [0x1b, b'[', b'B', ..] => ParsedBottomKey {
            key: BottomKey::DownArrow,
            consumed: 3,
        },
        [b'k', ..] => ParsedBottomKey {
            key: BottomKey::VimUp,
            consumed: 1,
        },
        [b'j', ..] => ParsedBottomKey {
            key: BottomKey::VimDown,
            consumed: 1,
        },
        [b'r', ..] => ParsedBottomKey {
            key: BottomKey::Refresh,
            consumed: 1,
        },
        [b'i', ..] | [b'\r', ..] | [b'\n', ..] => ParsedBottomKey {
            key: BottomKey::Inspect,
            consumed: 1,
        },
        [b'x', ..] => ParsedBottomKey {
            key: BottomKey::Cancel,
            consumed: 1,
        },
        [b'y', ..] => ParsedBottomKey {
            key: BottomKey::Confirm,
            consumed: 1,
        },
        [b'n', ..] => ParsedBottomKey {
            key: BottomKey::Abort,
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
        BottomKey::Quit => BottomInputRoute::CommandAndFocusTop(MonitorCommand::Collapse),
        BottomKey::UpArrow | BottomKey::VimUp => {
            BottomInputRoute::Command(MonitorCommand::SelectPrev)
        }
        BottomKey::DownArrow | BottomKey::VimDown => {
            BottomInputRoute::Command(MonitorCommand::SelectNext)
        }
        BottomKey::Refresh => BottomInputRoute::Command(MonitorCommand::Refresh),
        BottomKey::Inspect => BottomInputRoute::Command(MonitorCommand::ToggleInspect),
        BottomKey::Cancel => BottomInputRoute::Command(MonitorCommand::RequestCancel),
        BottomKey::Confirm => BottomInputRoute::Command(MonitorCommand::ConfirmCancel),
        BottomKey::Abort => BottomInputRoute::Command(MonitorCommand::AbortCancel),
        BottomKey::Unknown => BottomInputRoute::Consume,
    }
}

fn apply_monitor_command(routed: &mut RoutedInput, command: MonitorCommand) {
    routed.commands.push(command);
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

fn pane_areas(area: Rect, pane: &MonitorPane) -> PaneAreas {
    let bottom_rows = pane.bottom_rows(area.height);
    let [top, bottom] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_rows)]).areas(area);
    PaneAreas { top, bottom }
}

fn pane_areas_for_winsize(winsize: &libc::winsize, pane: &MonitorPane) -> PaneAreas {
    pane_areas(
        Rect {
            x: 0,
            y: 0,
            width: winsize.ws_col,
            height: winsize.ws_row,
        },
        pane,
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
/// Expanded monitor target share of terminal height, and its floor.
const EXPANDED_MIN_ROWS: u16 = 8;
/// Rows always reserved for the interactive top pane.
const TOP_PANE_MIN_ROWS: u16 = 5;
/// Scrollback rows retained for the interactive top pane so the operator can wheel
/// back through the child's output (the child runs in the broker's alt-screen, which
/// has no native scrollback of its own).
const TOP_PANE_SCROLLBACK_ROWS: usize = 10_000;
/// Rows moved per wheel notch when scrolling the top-pane scrollback.
const TOP_SCROLL_STEP: i32 = 3;

/// Bottom-pane monitor state: collapse/expand, the latest read-only snapshot, and
/// the current selection. Holds no terminal or IO handles.
struct MonitorPane {
    collapsed: bool,
    selected: usize,
    snapshot: Option<MonitorSnapshot>,
    last_refresh: Option<Instant>,
    inspecting: bool,
    inspect: Vec<String>,
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
            selected: 0,
            snapshot: None,
            last_refresh: None,
            inspecting: false,
            inspect: Vec::new(),
            pending_cancel: None,
            cancel_request: None,
            cancel_feedback: None,
        }
    }

    /// Rows the monitor occupies at the bottom for the given full terminal height.
    fn bottom_rows(&self, full_rows: u16) -> u16 {
        if self.collapsed {
            return COLLAPSED_MONITOR_ROWS;
        }
        let target = (u32::from(full_rows) * 35 / 100) as u16;
        let ceiling = full_rows.saturating_sub(TOP_PANE_MIN_ROWS);
        target
            .max(EXPANDED_MIN_ROWS)
            .min(ceiling)
            .max(COLLAPSED_MONITOR_ROWS)
    }

    fn expand(&mut self) {
        self.collapsed = false;
    }

    fn refresh_interval(&self) -> Duration {
        if self.collapsed {
            COLLAPSED_REFRESH
        } else {
            EXPANDED_REFRESH
        }
    }

    fn refresh_due(&self, now: Instant) -> bool {
        match self.last_refresh {
            None => true,
            Some(last) => now.duration_since(last) >= self.refresh_interval(),
        }
    }

    fn refresh(
        &mut self,
        monitor: &dyn ObservabilitySnapshotPort,
        root: &ObservabilityRoot,
        now: Instant,
    ) {
        let snapshot = read_monitor_snapshot(monitor, root);
        self.store_snapshot(snapshot, now);
        self.update_inspect();
    }

    fn store_snapshot(&mut self, snapshot: MonitorSnapshot, now: Instant) {
        self.clamp_selection(snapshot_node_count(&snapshot));
        self.snapshot = Some(snapshot);
        self.last_refresh = Some(now);
    }

    fn node_count(&self) -> usize {
        self.snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.nodes.len())
    }

    fn clamp_selection(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }

    fn select_next(&mut self) {
        let len = self.node_count();
        if len > 0 {
            self.selected = (self.selected + 1).min(len - 1);
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
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
            MonitorCommand::Refresh => true,
            MonitorCommand::Collapse => {
                self.collapsed = true;
                self.inspecting = false;
                self.inspect.clear();
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

    fn selected_node(&self) -> Option<&MonitorNode> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.nodes.get(self.selected))
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
        self.inspect = if self.inspecting {
            self.build_inspect_content()
        } else {
            Vec::new()
        };
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

fn node_is_cancelable(node: &MonitorNode) -> bool {
    node.cancel_ref.is_some()
}

fn read_monitor_snapshot(
    monitor: &dyn ObservabilitySnapshotPort,
    root: &ObservabilityRoot,
) -> MonitorSnapshot {
    monitor.snapshot(root, SnapshotLimits::default())
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

/// Render the virtual terminal screen grid into the top-pane buffer cells.
fn render_vt_screen(
    buf: &mut Buffer,
    area: Rect,
    screen: &vt100::Screen,
    selection: Option<SelectionSpan>,
) {
    for row in 0..area.height {
        for col in 0..area.width {
            let (symbol, mut style) = vt_cell_render(screen.cell(row, col));
            if selection.is_some_and(|span| cell_in_selection(span, row, col)) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            buf.set_string(area.x + col, area.y + row, symbol, style);
        }
    }
}

/// Split the screen into the interactive top pane and the monitor, and render both.
fn render_frame(
    frame: &mut ratatui::Frame,
    screen: &vt100::Screen,
    focus: Focus,
    pane: &MonitorPane,
    selection: Option<SelectionSpan>,
) {
    let area = frame.area();
    let bottom_rows = pane.bottom_rows(area.height);
    let [top, bottom] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_rows)]).areas(area);
    render_vt_screen(frame.buffer_mut(), top, screen, selection);
    render_scrollback_indicator(frame.buffer_mut(), top, screen.scrollback());
    render_monitor(frame.buffer_mut(), bottom, pane, focus);
    // Suppress the child cursor while scrolled back or selecting — it belongs to the live
    // tail, not the history/selection the operator is reading.
    if focus == Focus::Top
        && !screen.hide_cursor()
        && screen.scrollback() == 0
        && selection.is_none()
    {
        let (crow, ccol) = screen.cursor_position();
        if crow < top.height && ccol < top.width {
            frame.set_cursor_position(Position::new(top.x + ccol, top.y + crow));
        }
    }
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
fn render_monitor(buf: &mut Buffer, area: Rect, pane: &MonitorPane, focus: Focus) {
    if area.height == 0 {
        return;
    }
    render_status_row(buf, Rect { height: 1, ..area }, pane, focus);
    if pane.collapsed || area.height <= 1 {
        return;
    }
    let body = Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    };
    if pane.inspecting && body.height >= 4 {
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

/// Render the inspect sub-pane: a header bar plus the selected node's bounded
/// live output (tail-anchored so the most recent lines stay visible).
fn render_inspect_pane(buf: &mut Buffer, area: Rect, pane: &MonitorPane) {
    if area.height == 0 {
        return;
    }
    buf.set_string(
        area.x,
        area.y,
        pad_to_width(" inspect — i/Enter to close ".to_string(), area.width),
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
fn render_status_row(buf: &mut Buffer, area: Rect, pane: &MonitorPane, focus: Focus) {
    let hint = status_hint(pane, focus);
    let label = pad_to_width(
        format!(" OBS  {}  —  {hint}", monitor_summary_text(pane)),
        area.width,
    );
    let style = Style::default()
        .fg(Color::White)
        .bg(Color::Indexed(236))
        .add_modifier(Modifier::BOLD);
    buf.set_string(area.x, area.y, label, style);
}

/// The status-row hint, reflecting focus and any armed/last cancel state.
fn status_hint(pane: &MonitorPane, focus: Focus) -> String {
    match focus {
        Focus::Top => "Ctrl+O focus".to_string(),
        Focus::Bottom => bottom_status_hint(pane),
    }
}

fn bottom_status_hint(pane: &MonitorPane) -> String {
    if pane.pending_cancel.is_some() {
        return "confirm cancel: y = SIGTERM · n = abort".to_string();
    }
    match pane.cancel_feedback.as_deref() {
        Some(feedback) => format!("j/k · i inspect · x cancel · q close  ({feedback})"),
        None => "j/k move · i inspect · x cancel · r refresh · q close".to_string(),
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
    for VisibleNodeRow { index, y, node } in visible_node_rows(pane, snapshot, area) {
        let row = Rect {
            y,
            height: 1,
            ..area
        };
        render_node_row(buf, row, node, index == pane.selected);
    }
}

struct VisibleNodeRow<'a> {
    index: usize,
    y: u16,
    node: &'a MonitorNode,
}

fn visible_node_rows<'a>(
    pane: &'a MonitorPane,
    snapshot: &'a MonitorSnapshot,
    area: Rect,
) -> impl Iterator<Item = VisibleNodeRow<'a>> {
    let rows = area.height as usize;
    let offset = scroll_offset(pane.selected, snapshot.nodes.len(), rows);
    visible_node_window(snapshot, offset, rows)
        .map(move |(index, node)| visible_node_row(area.y, offset, index, node))
}

fn visible_node_window(
    snapshot: &MonitorSnapshot,
    offset: usize,
    rows: usize,
) -> impl Iterator<Item = (usize, &MonitorNode)> {
    snapshot.nodes.iter().enumerate().skip(offset).take(rows)
}

fn visible_node_row(
    area_y: u16,
    offset: usize,
    index: usize,
    node: &MonitorNode,
) -> VisibleNodeRow<'_> {
    VisibleNodeRow {
        index,
        y: area_y + (index - offset) as u16,
        node,
    }
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
fn render_node_row(buf: &mut Buffer, area: Rect, node: &MonitorNode, selected: bool) {
    let marker = if selected { '>' } else { ' ' };
    let pid = node
        .pid
        .map(|pid| format!(" pid={pid}"))
        .unwrap_or_default();
    let text = pad_to_width(
        format!(
            "{marker} {} [{}]{}{} {}",
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

/// Run the split-pane relay until the child exits, returning its exit status.
pub(super) fn relay_until_exit_observed(
    input_fd: RawFd,
    writer: File,
    master: &File,
    control: Option<&ControlSocket>,
    child: &mut Child,
    monitor: &dyn ObservabilitySnapshotPort,
    root: &ObservabilityRoot,
) -> Result<ExitStatus, String> {
    let real_fd = input_fd;
    let master_fd = master.as_raw_fd();
    let alt_writer = clone_terminal_writer(&writer).map_err(format_tui_terminal_clone_error)?;
    let mut alt = AltScreenGuard::enter(alt_writer)?;
    let mut terminal = new_tui_terminal(writer).map_err(format_tui_terminal_init_error)?;

    let mut pane = MonitorPane::new();
    let initial = child_pane_winsize(real_fd, &pane);
    let mut parser =
        vt100::Parser::new(initial.ws_row, initial.ws_col, TOP_PANE_SCROLLBACK_ROWS);
    let mut top_scrollback: usize = 0;
    let mut selection: Option<TopSelection> = None;
    let mut clipboard = String::new();
    let mut router = InputRouter::new();
    let mut line_state = InputLineState::default();
    let mut applied: Option<(libc::winsize, u16)> = None;
    let mut buffer = vec![0_u8; RELAY_BUFFER_BYTES];
    let mut status = None;

    while status.is_none() {
        let mut dirty = apply_sizing(
            real_fd,
            master_fd,
            child.id(),
            &pane,
            &mut parser,
            &mut applied,
        );
        if pane.refresh_due(Instant::now()) {
            pane.refresh(monitor, root, Instant::now());
            dirty = true;
        }
        let ready = poll_relay_fds(real_fd, master_fd, control.map(ControlSocket::fd))?;
        if ready.real_input {
            let mut routed = forward_real_input(
                real_fd,
                master_fd,
                &mut router,
                &pane,
                mouse_request_from_screen(parser.screen()),
                &mut line_state,
                &mut buffer,
            )?;
            let scroll_lines = routed.top_scroll_lines;
            // Sending keystrokes to the child snaps the view back to the live tail, like
            // a terminal jumps to the prompt when you start typing.
            let typed_to_child = !routed.forward.is_empty();
            let right_click = routed.right_click;
            let gestures = std::mem::take(&mut routed.top_mouse);
            dirty |= apply_routed_to_pane(&mut pane, routed, monitor, root);
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
                    &mut alt,
                    top_scrollback,
                    &mut clipboard,
                )?;
            }
            if let Some(click) = right_click {
                let mut io = MouseActionIo {
                    alt: &mut alt,
                    clipboard: &mut clipboard,
                    master_fd,
                    line_state: &mut line_state,
                };
                dirty |=
                    handle_top_right_click(&mut selection, click, parser.screen(), top_scrollback, &mut io)?;
            }
        }
        if ready.pty_output {
            dirty |= pump_pty_output(master_fd, &mut parser, &mut buffer)?;
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
                buffer: &mut buffer,
            };
            let _ = service_control(control, &mut control_io);
            dirty = true;
        }
        alt.sync_mouse(effective_mouse_request(mouse_request_from_screen(
            parser.screen(),
        )))?;
        // Re-assert the scrollback view each frame (clamped to retained history) so it
        // survives child output and resizes; reading it back keeps our offset honest.
        parser.screen_mut().set_scrollback(top_scrollback);
        top_scrollback = parser.screen().scrollback();
        if dirty {
            let render_selection = selection.as_ref().and_then(|sel| {
                visible_selection_span(sel, top_scrollback, parser.screen().size().0)
            });
            draw(&mut terminal, &parser, router.focus, &pane, render_selection)?;
        }
        status = try_wait_child(child).map_err(format_interactive_child_poll_error)?;
    }

    drain_pty_output(master_fd, &mut parser, &mut buffer)?;
    draw(&mut terminal, &parser, router.focus, &pane, None)?;
    Ok(status.expect("status checked above"))
}

/// Initial child PTY size for the (collapsed) monitor at the current terminal size.
fn child_pane_winsize(real_fd: RawFd, pane: &MonitorPane) -> libc::winsize {
    let full = terminal_winsize_with_fallback(read_terminal_winsize(real_fd));
    child_winsize_for_pane(&full, pane)
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

fn child_winsize_for_pane(full: &libc::winsize, pane: &MonitorPane) -> libc::winsize {
    child_winsize(full, pane.bottom_rows(full.ws_row))
}

/// On a change to the terminal size OR the monitor's reserved rows, resize the
/// virtual terminal and the child PTY (top-pane sized) and notify the child group.
fn apply_sizing(
    real_fd: RawFd,
    master_fd: RawFd,
    child_pid: u32,
    pane: &MonitorPane,
    parser: &mut vt100::Parser,
    applied: &mut Option<(libc::winsize, u16)>,
) -> bool {
    let Some(full) = read_terminal_winsize(real_fd) else {
        return false;
    };
    let bottom = pane.bottom_rows(full.ws_row);
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
    master_fd: RawFd,
    router: &mut InputRouter,
    pane: &MonitorPane,
    mouse_request: MouseRequest,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<RoutedInput, String> {
    match read_real_input(real_fd, buffer) {
        Ok(0) => Ok(RoutedInput::default()),
        Ok(n) => {
            let full = terminal_winsize_with_fallback(read_terminal_winsize(real_fd));
            let routed = route_real_input(&buffer[..n], router, pane, mouse_request, &full);
            forward_routed_child_input(master_fd, line_state, &routed)?;
            Ok(routed)
        }
        Err(err) => Err(format_user_terminal_input_read_error(err)),
    }
}

fn route_real_input(
    bytes: &[u8],
    router: &mut InputRouter,
    pane: &MonitorPane,
    child_mouse: MouseRequest,
    winsize: &libc::winsize,
) -> RoutedInput {
    // The broker always captures the wheel on the real terminal (see
    // `effective_mouse_request`), so always parse mouse events: top-pane wheel scrolls
    // the scrollback, bottom-pane wheel scrolls the monitor, and non-wheel mouse is
    // forwarded only to a child that requested mouse input.
    route_mouse_aware_input(bytes, router, pane, child_mouse, winsize)
}

fn route_mouse_aware_input(
    bytes: &[u8],
    router: &mut InputRouter,
    pane: &MonitorPane,
    mouse_request: MouseRequest,
    winsize: &libc::winsize,
) -> RoutedInput {
    let areas = pane_areas_for_winsize(winsize, pane);
    let mut routed = RoutedInput::default();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(parsed) = parse_mouse_event(&bytes[i..]) {
            route_mouse_event(parsed.event, areas, mouse_request, &mut routed);
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
    mouse_request: MouseRequest,
    routed: &mut RoutedInput,
) {
    match route_mouse_to_pane(event, areas) {
        MousePaneRoute::Top(local) => route_top_mouse_event(local, mouse_request, routed),
        MousePaneRoute::Bottom(bottom) => route_bottom_mouse_event(bottom, routed),
        MousePaneRoute::Outside => {}
    }
}

fn route_top_mouse_event(event: MouseEvent, child_mouse: MouseRequest, routed: &mut RoutedInput) {
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
    alt: &mut AltScreenGuard,
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
                            alt.copy_to_clipboard(&text)?;
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
    alt: &'a mut AltScreenGuard,
    clipboard: &'a mut String,
    master_fd: RawFd,
    line_state: &'a mut InputLineState,
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
                io.alt.copy_to_clipboard(&text)?;
            }
        }
        *selection = None;
        Ok(true)
    } else {
        inject_clipboard_paste(io.master_fd, io.line_state, io.clipboard)?;
        Ok(false)
    }
}

/// Inject the broker clipboard into the child as a bracketed paste, so the child treats
/// it as pasted data rather than typed commands (no accidental command execution).
fn inject_clipboard_paste(
    master_fd: RawFd,
    line_state: &mut InputLineState,
    text: &str,
) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    line_state.observe_user_input(&bytes);
    write_child_input(master_fd, &bytes).map_err(format_user_input_write_error)
}

fn route_bottom_mouse_event(event: MouseEvent, routed: &mut RoutedInput) {
    if let Some(command) = mouse_wheel_command(event) {
        apply_monitor_command(routed, command);
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

fn forward_routed_child_input(
    master_fd: RawFd,
    line_state: &mut InputLineState,
    routed: &RoutedInput,
) -> Result<(), String> {
    let Some(bytes) = routed_child_input(routed) else {
        return Ok(());
    };
    line_state.observe_user_input(bytes);
    write_child_input(master_fd, bytes).map_err(format_user_input_write_error)
}

fn write_child_input(master_fd: RawFd, bytes: &[u8]) -> io::Result<()> {
    write_all_fd(master_fd, bytes)
}

fn format_user_input_write_error(err: io::Error) -> String {
    format!("Failed to write user input to PTY: {err}")
}

/// Apply routed monitor effects (expand/select/refresh/collapse) to the pane.
/// Returns whether a redraw is needed.
fn apply_routed_to_pane(
    pane: &mut MonitorPane,
    routed: RoutedInput,
    monitor: &dyn ObservabilitySnapshotPort,
    root: &ObservabilityRoot,
) -> bool {
    if routed.focus_bottom {
        pane.expand();
    }
    let force_refresh = apply_routed_commands(pane, &routed.commands);
    let cancelled = run_pending_cancel(pane);
    if pane_refresh_required(force_refresh, cancelled) {
        pane.refresh(monitor, root, Instant::now());
    }
    routed.redraw
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

/// Render one frame to the real terminal.
fn draw(
    terminal: &mut Terminal<CrosstermBackend<File>>,
    parser: &vt100::Parser,
    focus: Focus,
    pane: &MonitorPane,
    selection: Option<SelectionSpan>,
) -> Result<(), String> {
    let screen = parser.screen();
    terminal
        .draw(|frame| render_frame(frame, screen, focus, pane, selection))
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
    buffer: &'a mut [u8],
}

/// Service a control-socket notify injection while the TUI owns the screen:
/// inject the payload to the child at the next safe line boundary, pumping output
/// into the virtual terminal (never to the real terminal) during the wait.
fn service_control(control: &ControlSocket, io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let mut stream = accept_control_stream(control).map_err(format_control_accept_error)?;
    let response = inject_control_payload(&mut stream, io);
    let (ack, message) = control_response_message(response);
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
    wait_until_safe_to_inject(io)?;
    let bracketed_paste = io.parser.screen().bracketed_paste();
    submit_control_payload(io.master_fd, &payload, io.line_state, bracketed_paste)
}

fn validate_control_peer(stream: &UnixStream) -> Result<(), String> {
    validate_peer_uid(stream)
}

fn read_tui_control_payload(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    read_control_request(stream)
}

fn submit_control_payload(
    master_fd: RawFd,
    payload: &[u8],
    line_state: &mut InputLineState,
    bracketed_paste: bool,
) -> Result<(), String> {
    write_control_payload_to_pty(master_fd, payload, bracketed_paste)
        .map_err(format_pty_write_failed)?;
    write_control_submit_to_pty(master_fd).map_err(format_pty_submit_failed)?;
    line_state.mark_submitted();
    Ok(())
}

fn write_control_payload_to_pty(
    master_fd: RawFd,
    payload: &[u8],
    bracketed_paste: bool,
) -> io::Result<()> {
    write_all_fd(master_fd, &control_payload_bytes(payload, bracketed_paste))
}

/// The bytes to inject for a control payload. When the child advertised bracketed-paste
/// mode (DECSET 2004) the (multi-line) body is wrapped in paste markers so an Ink-style
/// TUI (e.g. Claude Code) treats it as pasted content and the trailing `\r` as a distinct
/// Enter keypress that submits it; without the markers the child batches the whole burst
/// as one paste and absorbs the submit, leaving the notification unsent in the input box.
fn control_payload_bytes(payload: &[u8], bracketed_paste: bool) -> Vec<u8> {
    if !bracketed_paste {
        return payload.to_vec();
    }
    let mut bytes = Vec::with_capacity(
        BRACKETED_PASTE_START.len() + payload.len() + BRACKETED_PASTE_END.len(),
    );
    bytes.extend_from_slice(BRACKETED_PASTE_START);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(BRACKETED_PASTE_END);
    bytes
}

fn write_control_submit_to_pty(master_fd: RawFd) -> io::Result<()> {
    // `\r` (the Enter byte) so raw-mode TUI children submit the injected notification;
    // see submit_control_request_payload. `\n` would leave it unsubmitted in the editor.
    write_all_fd(master_fd, b"\r")
}

fn format_pty_write_failed(err: io::Error) -> String {
    format!("pty_write_failed: {err}")
}

fn format_pty_submit_failed(err: io::Error) -> String {
    format!("pty_submit_failed: {err}")
}

/// Wait until the child input is at a safe line boundary, pumping output into the
/// virtual terminal and routing real input meanwhile, bounded by the inject limit.
fn wait_until_safe_to_inject(io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let start = Instant::now();
    while injection_wait_should_pump(start, io.line_state) {
        pump_inject_wait_io(io)?;
    }
    validate_safe_to_inject(io.line_state)
}

fn injection_wait_should_pump(start: Instant, line_state: &InputLineState) -> bool {
    inject_wait_remaining(start) && !safe_to_inject(line_state)
}

fn inject_wait_remaining(start: Instant) -> bool {
    start.elapsed() < INJECT_WAIT_LIMIT
}

fn safe_to_inject(line_state: &InputLineState) -> bool {
    line_state.is_safe_to_inject()
}

fn validate_safe_to_inject(line_state: &InputLineState) -> Result<(), String> {
    if safe_to_inject(line_state) {
        Ok(())
    } else {
        Err(unsafe_mid_line_error())
    }
}

fn pump_inject_wait_io(io: &mut ControlInjectionIo<'_>) -> Result<(), String> {
    let ready = poll_relay_fds(io.real_fd, io.master_fd, None)?;
    if ready.real_input {
        forward_real_input(
            io.real_fd,
            io.master_fd,
            io.router,
            io.pane,
            mouse_request_from_screen(io.parser.screen()),
            io.line_state,
            io.buffer,
        )?;
    }
    if ready.pty_output {
        let _ = pump_pty_output(io.master_fd, io.parser, io.buffer)?;
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
    fn bottom_focus_consumes_input_until_toggle_returns() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        assert_eq!(router.focus, Focus::Bottom);

        let routed = router.route_input(b"jjkk");
        assert!(routed.forward.is_empty());
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
        assert_eq!(apply_top_scroll(2, TOP_SCROLL_STEP), 2 + TOP_SCROLL_STEP as usize);
        assert_eq!(apply_top_scroll(1, -5), 0);
    }

    #[test]
    fn control_payload_bytes_wraps_only_for_bracketed_paste() {
        // No mode 2004: inject the body verbatim (then a `\r` submit follows).
        assert_eq!(control_payload_bytes(b"line1\nline2", false), b"line1\nline2".to_vec());
        // Mode 2004 advertised: wrap as a real paste so the trailing Enter submits.
        assert_eq!(
            control_payload_bytes(b"line1\nline2", true),
            b"\x1b[200~line1\nline2\x1b[201~".to_vec()
        );
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
        assert_eq!(extract_selection_text(parser.screen(), span), "hello\nworld");
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
        let monitor = FakeMonitor::new(empty_snapshot());
        let root = ObservabilityRoot::default();
        let relay = thread::spawn(move || {
            let mut child = child;
            let result = relay_until_exit_observed(
                input_fd, writer, &master, None, &mut child, &monitor, &root,
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
        let bottom = expanded.bottom_rows(30);
        let top_rows = 30 - bottom;
        let dirty = apply_sizing(
            outer.slave.as_raw_fd(),
            pty.master.as_raw_fd(),
            std::process::id(),
            &expanded,
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

    fn pane_with(snapshot: MonitorSnapshot, collapsed: bool, selected: usize) -> MonitorPane {
        MonitorPane {
            collapsed,
            selected,
            snapshot: Some(snapshot),
            last_refresh: None,
            inspecting: false,
            inspect: Vec::new(),
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

    fn native_projectable_line(sentinel: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{sentinel}"}}]}}}}"#
        )
    }

    #[test]
    fn collapsed_reserves_one_row_expanded_reserves_a_bounded_share() {
        let collapsed = MonitorPane::new();
        assert_eq!(collapsed.bottom_rows(40), 1);
        let mut expanded = MonitorPane::new();
        expanded.expand();
        let rows = expanded.bottom_rows(40);
        assert!(rows >= EXPANDED_MIN_ROWS, "rows={rows}");
        assert!(rows <= 40 - TOP_PANE_MIN_ROWS, "rows={rows}");
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
    fn inspect_keys_toggle_and_route() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        assert_eq!(
            router.route_input(b"i").commands,
            vec![MonitorCommand::ToggleInspect]
        );
        assert_eq!(
            router.route_input(b"\r").commands,
            vec![MonitorCommand::ToggleInspect]
        );
        let mut pane = pane_with(empty_snapshot(), false, 0);
        assert!(!pane.inspecting);
        pane.apply(MonitorCommand::ToggleInspect);
        assert!(pane.inspecting);
        pane.apply(MonitorCommand::ToggleInspect);
        assert!(!pane.inspecting);
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
        assert!(text.contains("inspect"), "{text}");
        assert!(text.contains("running 12 tests"), "{text}");
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
    fn bottom_focus_keys_decode_to_monitor_commands() {
        let mut router = InputRouter::new();
        router.route_input(&[FOCUS_TOGGLE_BYTE]);
        assert_eq!(router.focus, Focus::Bottom);

        assert_eq!(
            router.route_input(b"j").commands,
            vec![MonitorCommand::SelectNext]
        );
        assert_eq!(
            router.route_input(b"k").commands,
            vec![MonitorCommand::SelectPrev]
        );
        assert_eq!(
            router.route_input(b"r").commands,
            vec![MonitorCommand::Refresh]
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
        assert_eq!(routed.commands, vec![MonitorCommand::Collapse]);
        assert_eq!(router.focus, Focus::Top);
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
            router.route_input(b"x").commands,
            vec![MonitorCommand::RequestCancel]
        );
        assert_eq!(
            router.route_input(b"y").commands,
            vec![MonitorCommand::ConfirmCancel]
        );
        assert_eq!(
            router.route_input(b"n").commands,
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
    fn refresh_is_due_initially_then_respects_cadence() {
        let monitor = FakeMonitor::new(empty_snapshot());
        let root = ObservabilityRoot::default();
        let mut pane = MonitorPane::new();
        let now = Instant::now();
        assert!(pane.refresh_due(now), "first refresh is always due");
        pane.refresh(&monitor, &root, now);
        assert!(!pane.refresh_due(now), "not due immediately after refresh");
        assert!(
            pane.refresh_due(now + COLLAPSED_REFRESH),
            "due again after the collapsed interval"
        );
        assert!(pane.snapshot.is_some());
    }
}
