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
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use std::fs::File;
use std::io;
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
    redraw: bool,
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

/// Collapsed monitor refresh cadence (slow; just the summary row).
const COLLAPSED_REFRESH: Duration = Duration::from_millis(2000);
/// Expanded monitor refresh cadence (the operator is actively watching).
const EXPANDED_REFRESH: Duration = Duration::from_millis(500);
/// Expanded monitor target share of terminal height, and its floor.
const EXPANDED_MIN_ROWS: u16 = 8;
/// Rows always reserved for the interactive top pane.
const TOP_PANE_MIN_ROWS: u16 = 5;

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
        }) => InspectContentSource::SessionTranscript {
            path,
            max_tail_bytes: *max_tail_bytes,
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
        } => inspect_transcript_content_lines(path, max_tail_bytes),
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
fn inspect_transcript_content_lines(path: &str, max_tail_bytes: usize) -> Vec<String> {
    let raw = tail_file(path, max_tail_bytes);
    let mut lines = vec![format_inspect_transcript_header(path)];
    let projected = project_transcript_tail(&raw);
    if projected.is_empty() {
        lines.extend(raw);
    } else {
        lines.extend(projected);
    }
    lines
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
fn render_vt_screen(buf: &mut Buffer, area: Rect, screen: &vt100::Screen) {
    for row in 0..area.height {
        for col in 0..area.width {
            let (symbol, style) = vt_cell_render(screen.cell(row, col));
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
) {
    let area = frame.area();
    let bottom_rows = pane.bottom_rows(area.height);
    let [top, bottom] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(bottom_rows)]).areas(area);
    render_vt_screen(frame.buffer_mut(), top, screen);
    render_monitor(frame.buffer_mut(), bottom, pane, focus);
    if focus == Focus::Top && !screen.hide_cursor() {
        let (crow, ccol) = screen.cursor_position();
        if crow < top.height && ccol < top.width {
            frame.set_cursor_position(Position::new(top.x + ccol, top.y + crow));
        }
    }
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
    let rows = body.height as usize;
    let skip = pane.inspect.len().saturating_sub(rows);
    for (line_index, line) in pane.inspect.iter().skip(skip).take(rows).enumerate() {
        buf.set_string(
            body.x,
            body.y + line_index as u16,
            pad_to_width(line.clone(), body.width),
            Style::default().fg(Color::Gray),
        );
    }
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
    let rows = area.height as usize;
    let offset = scroll_offset(pane.selected, snapshot.nodes.len(), rows);
    for (index, node) in snapshot.nodes.iter().enumerate().skip(offset).take(rows) {
        let row = Rect {
            y: area.y + (index - offset) as u16,
            height: 1,
            ..area
        };
        render_node_row(buf, row, node, index == pane.selected);
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
}

impl AltScreenGuard {
    fn enter(mut writer: File) -> Result<Self, String> {
        execute!(writer, EnterAlternateScreen)
            .map_err(|err| format!("Failed to enter alternate screen: {err}"))?;
        Ok(Self { writer })
    }
}

impl Drop for AltScreenGuard {
    fn drop(&mut self) {
        let _ = execute!(self.writer, LeaveAlternateScreen);
    }
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
    let _alt = AltScreenGuard::enter(alt_writer)?;
    let mut terminal = new_tui_terminal(writer).map_err(format_tui_terminal_init_error)?;

    let mut pane = MonitorPane::new();
    let initial = child_pane_winsize(real_fd, &pane);
    let mut parser = vt100::Parser::new(initial.ws_row, initial.ws_col, 0);
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
            let routed = forward_real_input(
                real_fd,
                master_fd,
                &mut router,
                &mut line_state,
                &mut buffer,
            )?;
            dirty |= apply_routed_to_pane(&mut pane, routed, monitor, root);
        }
        if ready.pty_output {
            dirty |= pump_pty_output(master_fd, &mut parser, &mut buffer)?;
        }
        if ready.control
            && let Some(control) = control
        {
            let _ = service_control(
                control,
                real_fd,
                master_fd,
                &mut router,
                &mut parser,
                &mut line_state,
                &mut buffer,
            );
            dirty = true;
        }
        if dirty {
            draw(&mut terminal, &parser, router.focus, &pane)?;
        }
        status = try_wait_child(child).map_err(format_interactive_child_poll_error)?;
    }

    drain_pty_output(master_fd, &mut parser, &mut buffer)?;
    draw(&mut terminal, &parser, router.focus, &pane)?;
    Ok(status.expect("status checked above"))
}

/// Initial child PTY size for the (collapsed) monitor at the current terminal size.
fn child_pane_winsize(real_fd: RawFd, pane: &MonitorPane) -> libc::winsize {
    let full = terminal_winsize(real_fd).unwrap_or(libc::winsize {
        ws_row: MIN_TERMINAL_ROWS,
        ws_col: MIN_TERMINAL_COLS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    });
    child_winsize(&full, pane.bottom_rows(full.ws_row))
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
    if sizing_already_applied(applied, &full, bottom) {
        return false;
    }
    let child = child_winsize(&full, bottom);
    resize_virtual_terminal(parser, &child);
    apply_child_pty_winsize(master_fd, child_pid, &child);
    record_applied_sizing(applied, full, bottom);
    true
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
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<RoutedInput, String> {
    match read_real_input(real_fd, buffer) {
        Ok(0) => Ok(RoutedInput::default()),
        Ok(n) => {
            let routed = router.route_input(&buffer[..n]);
            forward_routed_child_input(master_fd, line_state, &routed)?;
            Ok(routed)
        }
        Err(err) => Err(format_user_terminal_input_read_error(err)),
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
    let force_refresh = routed
        .commands
        .iter()
        .fold(false, |force, command| pane.apply(*command) || force);
    let cancelled = run_pending_cancel(pane);
    if force_refresh || cancelled {
        pane.refresh(monitor, root, Instant::now());
    }
    routed.redraw
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
    match output {
        PtyOutput::Empty => false,
        PtyOutput::Bytes(len) => {
            process_pty_bytes(parser, &buffer[..len]);
            true
        }
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
) -> Result<(), String> {
    let screen = parser.screen();
    terminal
        .draw(|frame| render_frame(frame, screen, focus, pane))
        .map(|_| ())
        .map_err(|err| format!("Failed to render TUI frame: {err}"))
}

/// Service a control-socket notify injection while the TUI owns the screen:
/// inject the payload to the child at the next safe line boundary, pumping output
/// into the virtual terminal (never to the real terminal) during the wait.
fn service_control(
    control: &ControlSocket,
    real_fd: RawFd,
    master_fd: RawFd,
    router: &mut InputRouter,
    parser: &mut vt100::Parser,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    let mut stream = accept_control_stream(control).map_err(format_control_accept_error)?;
    let response = inject_control_payload(
        &mut stream,
        real_fd,
        master_fd,
        router,
        parser,
        line_state,
        buffer,
    );
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
    real_fd: RawFd,
    master_fd: RawFd,
    router: &mut InputRouter,
    parser: &mut vt100::Parser,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    validate_control_peer(stream)?;
    let payload = read_tui_control_payload(stream)?;
    wait_until_safe_to_inject(real_fd, master_fd, router, parser, line_state, buffer)?;
    submit_control_payload(master_fd, &payload, line_state)
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
) -> Result<(), String> {
    write_control_payload_to_pty(master_fd, payload).map_err(format_pty_write_failed)?;
    write_control_submit_to_pty(master_fd).map_err(format_pty_submit_failed)?;
    line_state.mark_submitted();
    Ok(())
}

fn write_control_payload_to_pty(master_fd: RawFd, payload: &[u8]) -> io::Result<()> {
    write_all_fd(master_fd, payload)
}

fn write_control_submit_to_pty(master_fd: RawFd) -> io::Result<()> {
    write_all_fd(master_fd, b"\n")
}

fn format_pty_write_failed(err: io::Error) -> String {
    format!("pty_write_failed: {err}")
}

fn format_pty_submit_failed(err: io::Error) -> String {
    format!("pty_submit_failed: {err}")
}

/// Wait until the child input is at a safe line boundary, pumping output into the
/// virtual terminal and routing real input meanwhile, bounded by the inject limit.
fn wait_until_safe_to_inject(
    real_fd: RawFd,
    master_fd: RawFd,
    router: &mut InputRouter,
    parser: &mut vt100::Parser,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    let start = Instant::now();
    while inject_wait_remaining(start) {
        if safe_to_inject(line_state) {
            return Ok(());
        }
        pump_inject_wait_io(real_fd, master_fd, router, parser, line_state, buffer)?;
    }
    if safe_to_inject(line_state) {
        return Ok(());
    }
    Err(unsafe_mid_line_error())
}

fn inject_wait_remaining(start: Instant) -> bool {
    start.elapsed() < INJECT_WAIT_LIMIT
}

fn safe_to_inject(line_state: &InputLineState) -> bool {
    line_state.is_safe_to_inject()
}

fn pump_inject_wait_io(
    real_fd: RawFd,
    master_fd: RawFd,
    router: &mut InputRouter,
    parser: &mut vt100::Parser,
    line_state: &mut InputLineState,
    buffer: &mut [u8],
) -> Result<(), String> {
    let ready = poll_relay_fds(real_fd, master_fd, None)?;
    if ready.real_input {
        forward_real_input(real_fd, master_fd, router, line_state, buffer)?;
    }
    if ready.pty_output {
        let _ = pump_pty_output(master_fd, parser, buffer)?;
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
            .draw(|frame| render_frame(frame, screen, Focus::Top, &pane))
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
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                0x1b => {
                    i += 1;
                    match bytes.get(i) {
                        Some(b'[') => {
                            i += 1;
                            while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                                i += 1;
                            }
                            i += 1;
                        }
                        Some(b']') => {
                            i += 1;
                            while i < bytes.len() && bytes[i] != 0x07 && bytes[i] != 0x1b {
                                i += 1;
                            }
                            if bytes.get(i) == Some(&0x1b) {
                                i += 1;
                            }
                            i += 1;
                        }
                        Some(_) => i += 1,
                        None => {}
                    }
                }
                b'\r' | b'\n' => {
                    out.push(' ');
                    i += 1;
                }
                b if (0x20..0x7f).contains(&b) => {
                    out.push(b as char);
                    i += 1;
                }
                _ => i += 1,
            }
        }
        out
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
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Top, &pane))
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
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane))
            .unwrap();
        let text = screen_text(terminal.backend().buffer(), 16, 60);
        assert!(text.contains("inspect"), "{text}");
        assert!(text.contains("running 12 tests"), "{text}");
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
            .draw(|frame| render_frame(frame, parser.screen(), Focus::Bottom, &pane))
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
