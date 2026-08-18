//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator`
//!
//! Bounded read-only discovery of agent-bash spooler metadata and retained logs.

use crate::observability::dto::{
    CancelRef, InspectRef, LivenessStatus, MonitorDiagnostic, MonitorDiagnosticSeverity,
    MonitorNode, MonitorNodeKind, MonitorStatus,
};
use crate::observability::invocation::{
    invocation_node_id, resolved_invocation_session_id, session_node_id,
};
use crate::observability::limits::SnapshotLimits;
use crate::observability::liveness::{process_identity_liveness, unverified_pid_liveness};
use crate::observability::mailbox::mailbox_state;
use crate::observability::state_access::storage_diagnostic;
use oulipoly_provider::client::CancellationToken;
use oulipoly_state::invocation_marker::INVOCATION_MARKER_PREFIX;
use oulipoly_state::mailbox::{AGENT_BASH_COMPLETE_KIND, MailboxRow};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRow, ProcessIdentity};
use oulipoly_state::{CompositeInvocationId, StateDb};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WORKLOAD_INVOCATION_MARKER_MAX_BYTES: u64 = 4096;

pub(crate) struct AgentBashProjection {
    pub(crate) nodes: Vec<MonitorNode>,
    pub(crate) diagnostics: Vec<MonitorDiagnostic>,
    pub(crate) running_count: usize,
}

pub(crate) struct AgentBashProjectInput<'a> {
    pub(crate) root_dir: Option<&'a Path>,
    pub(crate) cache: &'a AgentBashMetaCache,
    pub(crate) state: Option<&'a StateDb>,
    pub(crate) pid: Option<&'a PidIdentityDb>,
    pub(crate) session_id: Option<&'a str>,
    pub(crate) invocation_uuids: &'a HashSet<String>,
    pub(crate) mailbox_rows: &'a [MailboxRow],
    pub(crate) limits: SnapshotLimits,
    pub(crate) cancellation: &'a CancellationToken,
}

#[derive(Default)]
pub(crate) struct AgentBashMetaCache {
    entries: Mutex<HashMap<PathBuf, CachedAgentBashMeta>>,
}

#[derive(Debug, Clone)]
struct CachedAgentBashMeta {
    modified_at: Option<SystemTime>,
    size: u64,
    result: Result<AgentBashMeta, String>,
}

#[derive(Debug, Clone)]
struct AgentBashMeta {
    handle: String,
    state: String,
    caller_chain: Vec<CallerIdentity>,
    supervisor_pid: Option<i64>,
    workload_pid: Option<i64>,
    workload_identity: WorkloadIdentityEvidence,
    workload_pgid: Option<i64>,
    argv: Vec<String>,
    cwd: Option<String>,
    ready_at: Option<String>,
    completed_at: Option<String>,
    rc: Option<i32>,
    signal: Option<i32>,
    cgroup: Option<String>,
    log_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum WorkloadIdentityEvidence {
    Exact(ProcessIdentity),
    Legacy,
    Incomplete,
}

#[derive(Debug, Clone)]
struct CallerIdentity {
    chain_index: usize,
    identity: ProcessIdentity,
}

#[derive(Debug, Clone)]
struct ResolvedOwner {
    session_id: Option<String>,
    invocation_uuid: String,
    matched_chain_index: Option<usize>,
}

struct ProcessIdentityFields {
    os_pid: i64,
    os_boot_id: String,
    os_pid_starttime_ticks: i64,
}

#[derive(Debug)]
struct CandidateDir {
    state_dir: PathBuf,
    modified_at: Option<SystemTime>,
}

struct WorkloadLogSnapshot {
    path: PathBuf,
    tail: Option<String>,
}

pub(crate) fn default_agent_bash_root() -> Option<PathBuf> {
    default_agent_bash_state_root().map(agent_bash_root_dir)
}

fn default_agent_bash_state_root() -> Option<PathBuf> {
    xdg_state_home_value()
        .map(path_buf_from_os_string)
        .or_else(default_home_state_dir)
}

fn xdg_state_home_value() -> Option<OsString> {
    std::env::var_os("XDG_STATE_HOME")
}

fn path_buf_from_os_string(value: OsString) -> PathBuf {
    PathBuf::from(value)
}

fn agent_bash_root_dir(root: PathBuf) -> PathBuf {
    root.join("agent-bash")
}

pub(crate) fn project_agent_bash(input: AgentBashProjectInput<'_>) -> AgentBashProjection {
    let mut diagnostics = Vec::new();
    let mut seen_handles = HashSet::new();
    if input.cancellation.is_cancelled() {
        return agent_bash_projection(Vec::new(), diagnostics, 0);
    }
    let mut nodes = mailbox_workload_nodes(
        input.cache,
        input.session_id,
        input.invocation_uuids,
        input.mailbox_rows,
        input.limits,
        input.cancellation,
        &mut diagnostics,
        &mut seen_handles,
    );
    nodes.extend(scanned_workload_nodes(
        input.root_dir,
        input.cache,
        input.state,
        input.pid,
        input.session_id,
        input.invocation_uuids,
        input.limits,
        input.cancellation,
        &mut diagnostics,
        &mut seen_handles,
    ));
    let running_count = running_agent_bash_nodes(&nodes);
    agent_bash_projection(nodes, diagnostics, running_count)
}

fn running_agent_bash_nodes(nodes: &[MonitorNode]) -> usize {
    nodes
        .iter()
        .filter(|node| node.status == MonitorStatus::Running)
        .count()
}

fn agent_bash_projection(
    nodes: Vec<MonitorNode>,
    diagnostics: Vec<MonitorDiagnostic>,
    running_count: usize,
) -> AgentBashProjection {
    AgentBashProjection {
        nodes,
        diagnostics,
        running_count,
    }
}

fn default_home_state_dir() -> Option<PathBuf> {
    default_home_dir().map(home_state_dir)
}

fn default_home_dir() -> Option<PathBuf> {
    home_value().map(path_buf_from_os_string)
}

fn home_value() -> Option<OsString> {
    std::env::var_os("HOME")
}

fn home_state_dir(home: PathBuf) -> PathBuf {
    home.join(".local/state")
}

#[allow(clippy::too_many_arguments)]
fn mailbox_workload_nodes(
    cache: &AgentBashMetaCache,
    session_id: Option<&str>,
    invocation_uuids: &HashSet<String>,
    rows: &[MailboxRow],
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
    seen_handles: &mut HashSet<String>,
) -> Vec<MonitorNode> {
    let rows = agent_bash_complete_rows(rows);
    let mut nodes = Vec::new();
    for row in rows {
        if cancellation.is_cancelled() {
            break;
        }
        push_mailbox_workload_node(
            &mut nodes,
            cache,
            session_id,
            invocation_uuids,
            row,
            limits,
            diagnostics,
            seen_handles,
        );
    }
    nodes
}

fn agent_bash_complete_rows(rows: &[MailboxRow]) -> Vec<&MailboxRow> {
    rows.iter().filter(row_is_agent_bash).collect()
}

fn row_is_agent_bash(row: &&MailboxRow) -> bool {
    row.kind == AGENT_BASH_COMPLETE_KIND
}

fn remember_mailbox_handle(seen_handles: &mut HashSet<String>, row: &MailboxRow) {
    seen_handles.insert(row.handle.clone());
}

#[allow(clippy::too_many_arguments)]
fn push_mailbox_workload_node(
    nodes: &mut Vec<MonitorNode>,
    cache: &AgentBashMetaCache,
    session_id: Option<&str>,
    invocation_uuids: &HashSet<String>,
    row: &MailboxRow,
    limits: SnapshotLimits,
    diagnostics: &mut Vec<MonitorDiagnostic>,
    seen_handles: &mut HashSet<String>,
) {
    remember_mailbox_handle(seen_handles, row);
    nodes.push(mailbox_workload_node(
        cache,
        session_id,
        invocation_uuids,
        row,
        limits,
        diagnostics,
    ));
}

fn mailbox_workload_node(
    cache: &AgentBashMetaCache,
    session_id: Option<&str>,
    invocation_uuids: &HashSet<String>,
    row: &MailboxRow,
    limits: SnapshotLimits,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> MonitorNode {
    let state_dir = PathBuf::from(&row.state_dir);
    let meta_path = PathBuf::from(&row.meta_path);
    let meta = read_meta_for_node(cache, &state_dir, &meta_path, diagnostics);
    let log = mailbox_workload_log(row, meta.as_ref(), limits.log_tail_bytes);
    let liveness = meta
        .as_ref()
        .map(meta_workload_liveness)
        .unwrap_or(LivenessStatus::NotApplicable);
    MonitorNode {
        id: agent_bash_node_id(&row.handle),
        parent_id: workload_parent_id(
            row.owner_invocation_uuid.as_deref(),
            row.session_id.as_str(),
            invocation_uuids,
            session_id,
        ),
        kind: MonitorNodeKind::AgentBashWorkload,
        label: workload_label(
            &row.handle,
            meta.as_ref(),
            row.matched_chain_index
                .and_then(|index| usize::try_from(index).ok()),
        ),
        status: mailbox_workload_status(row, meta.as_ref(), liveness),
        pid: meta.as_ref().and_then(|meta| meta.workload_pid),
        pgid: meta.as_ref().and_then(|meta| meta.workload_pgid),
        liveness,
        started_at: meta.as_ref().and_then(|meta| meta.ready_at.clone()),
        updated_at: meta.as_ref().and_then(|meta| meta.completed_at.clone()),
        completed_at: meta.as_ref().and_then(|meta| meta.completed_at.clone()),
        last_output_excerpt: log.tail,
        inspect_ref: Some(InspectRef::AgentBashLog {
            path: log.path.to_string_lossy().into_owned(),
            max_tail_bytes: limits.log_tail_bytes,
        }),
        cancel_ref: Some(CancelRef::AgentBashHandle {
            handle: row.handle.clone(),
            state_dir: row.state_dir.clone(),
        }),
        wake: None,
        mailbox: Some(mailbox_state(row)),
    }
}

fn mailbox_workload_log(
    row: &MailboxRow,
    meta: Option<&AgentBashMeta>,
    max_bytes: usize,
) -> WorkloadLogSnapshot {
    let path = mailbox_log_path(row, meta);
    let tail = workload_log_tail(&path, max_bytes);
    workload_log_snapshot(path, tail)
}

#[allow(clippy::too_many_arguments)]
fn scanned_workload_nodes(
    root_dir: Option<&Path>,
    cache: &AgentBashMetaCache,
    state: Option<&StateDb>,
    pid: Option<&PidIdentityDb>,
    session_id: Option<&str>,
    invocation_uuids: &HashSet<String>,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
    seen_handles: &mut HashSet<String>,
) -> Vec<MonitorNode> {
    let Some(root_dir) = root_dir else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    for candidate in candidate_dirs(root_dir, limits, cancellation, diagnostics) {
        if cancellation.is_cancelled() {
            break;
        }
        if let Some(node) = scanned_workload_node(
            cache,
            state,
            pid,
            session_id,
            invocation_uuids,
            limits,
            cancellation,
            diagnostics,
            seen_handles,
            &candidate.state_dir,
        ) {
            nodes.push(node);
        }
    }
    nodes
}

#[allow(clippy::too_many_arguments)]
fn scanned_workload_node(
    cache: &AgentBashMetaCache,
    state: Option<&StateDb>,
    pid: Option<&PidIdentityDb>,
    session_id: Option<&str>,
    invocation_uuids: &HashSet<String>,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
    seen_handles: &mut HashSet<String>,
    state_dir: &Path,
) -> Option<MonitorNode> {
    if cancellation.is_cancelled() {
        return None;
    }
    let meta_path = state_dir.join("meta.json");
    let meta = read_meta_for_scan(cache, state_dir, &meta_path, diagnostics)?;
    if cancellation.is_cancelled() {
        return None;
    }
    if handle_was_seen(seen_handles, &meta.handle) {
        return None;
    }
    remember_meta_handle(seen_handles, &meta);
    let owner =
        resolve_owner(state, pid, &meta.caller_chain, cancellation, diagnostics).or_else(|| {
            (!cancellation.is_cancelled())
                .then(|| workload_marker_owner(state, state_dir, &meta, invocation_uuids))
                .flatten()
        })?;
    if !owner_matches_root(&owner, session_id, invocation_uuids) {
        return None;
    }
    Some(meta_workload_node(
        state_dir,
        &meta,
        &owner,
        session_id,
        invocation_uuids,
        limits,
    ))
}

fn handle_was_seen(seen_handles: &HashSet<String>, handle: &str) -> bool {
    seen_handles.contains(handle)
}

fn remember_meta_handle(seen_handles: &mut HashSet<String>, meta: &AgentBashMeta) {
    seen_handles.insert(meta.handle.clone());
}

fn meta_workload_node(
    state_dir: &Path,
    meta: &AgentBashMeta,
    owner: &ResolvedOwner,
    session_id: Option<&str>,
    invocation_uuids: &HashSet<String>,
    limits: SnapshotLimits,
) -> MonitorNode {
    let log = meta_workload_log(state_dir, meta, limits.log_tail_bytes);
    let liveness = meta_workload_liveness(meta);
    MonitorNode {
        id: agent_bash_node_id(&meta.handle),
        parent_id: workload_parent_id(
            Some(&owner.invocation_uuid),
            owner.session_id.as_deref().unwrap_or_default(),
            invocation_uuids,
            session_id,
        ),
        kind: MonitorNodeKind::AgentBashWorkload,
        label: workload_label(&meta.handle, Some(meta), owner.matched_chain_index),
        status: meta_workload_status(meta, liveness),
        pid: meta.workload_pid,
        pgid: meta.workload_pgid,
        liveness,
        started_at: meta.ready_at.clone(),
        updated_at: meta.completed_at.clone(),
        completed_at: meta.completed_at.clone(),
        last_output_excerpt: log.tail,
        inspect_ref: Some(InspectRef::AgentBashLog {
            path: log.path.to_string_lossy().into_owned(),
            max_tail_bytes: limits.log_tail_bytes,
        }),
        cancel_ref: Some(CancelRef::AgentBashHandle {
            handle: meta.handle.clone(),
            state_dir: state_dir.to_string_lossy().into_owned(),
        }),
        wake: None,
        mailbox: None,
    }
}

fn meta_workload_log(
    state_dir: &Path,
    meta: &AgentBashMeta,
    max_bytes: usize,
) -> WorkloadLogSnapshot {
    let path = meta_log_path(state_dir, meta);
    let tail = workload_log_tail(&path, max_bytes);
    workload_log_snapshot(path, tail)
}

fn workload_log_tail(path: &Path, max_bytes: usize) -> Option<String> {
    read_log_tail(path, max_bytes)
}

fn workload_log_snapshot(path: PathBuf, tail: Option<String>) -> WorkloadLogSnapshot {
    WorkloadLogSnapshot { tail, path }
}

fn candidate_dirs(
    root_dir: &Path,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Vec<CandidateDir> {
    let entries = match read_candidate_dir_entries(root_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            diagnostics.push(agent_bash_root_read_diagnostic(root_dir, err));
            return Vec::new();
        }
    };
    candidate_dirs_from_entries(entries, limits, cancellation)
}

fn read_candidate_dir_entries(root_dir: &Path) -> Result<std::fs::ReadDir, std::io::Error> {
    std::fs::read_dir(root_dir)
}

fn agent_bash_root_read_diagnostic(root_dir: &Path, err: std::io::Error) -> MonitorDiagnostic {
    storage_diagnostic(
        "agent-bash:state-root-read",
        format!(
            "failed to read agent-bash state root {}: {err}",
            root_dir.display()
        ),
    )
}

fn candidate_dirs_from_entries(
    entries: impl IntoIterator<Item = Result<std::fs::DirEntry, std::io::Error>>,
    limits: SnapshotLimits,
    cancellation: &CancellationToken,
) -> Vec<CandidateDir> {
    let limit = limits.agent_bash_scan_dirs;
    if limit == 0 {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    for entry in entries {
        if cancellation.is_cancelled() {
            return Vec::new();
        }
        if let Some(candidate) = entry.ok().and_then(candidate_dir) {
            retain_candidate_dir(&mut dirs, candidate, limit);
        }
    }
    dirs
}

fn retain_candidate_dir(dirs: &mut Vec<CandidateDir>, candidate: CandidateDir, limit: usize) {
    let insertion = dirs
        .binary_search_by(|existing| compare_candidate_dir(existing, &candidate))
        .unwrap_or_else(|insertion| insertion);
    if insertion >= limit {
        return;
    }
    if dirs.len() == limit {
        dirs.pop();
    }
    dirs.insert(insertion, candidate);
}

fn candidate_dir(entry: std::fs::DirEntry) -> Option<CandidateDir> {
    if !candidate_entry_is_dir(&entry) {
        return None;
    }
    let modified_at = candidate_handle_created_at(&entry.file_name())
        .or_else(|| candidate_dir_metadata(&entry).ok()?.modified().ok());
    Some(candidate_dir_from_parts(
        candidate_entry_path(&entry),
        modified_at,
    ))
}

fn candidate_entry_is_dir(entry: &std::fs::DirEntry) -> bool {
    entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
}

fn candidate_handle_created_at(name: &OsStr) -> Option<SystemTime> {
    let timestamp = name.to_str()?.strip_prefix("ab_")?.split('_').next()?;
    let unix_ms = u64::from_str_radix(timestamp, 16).ok()?;
    UNIX_EPOCH.checked_add(Duration::from_millis(unix_ms))
}

fn candidate_dir_metadata(entry: &std::fs::DirEntry) -> Result<std::fs::Metadata, std::io::Error> {
    entry.metadata()
}

fn candidate_entry_path(entry: &std::fs::DirEntry) -> PathBuf {
    entry.path()
}

fn candidate_dir_from_parts(state_dir: PathBuf, modified_at: Option<SystemTime>) -> CandidateDir {
    CandidateDir {
        state_dir,
        modified_at,
    }
}

fn compare_candidate_dir(left: &CandidateDir, right: &CandidateDir) -> std::cmp::Ordering {
    right
        .modified_at
        .cmp(&left.modified_at)
        .then_with(|| right.state_dir.cmp(&left.state_dir))
}

fn read_meta_for_node(
    cache: &AgentBashMetaCache,
    state_dir: &Path,
    meta_path: &Path,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<AgentBashMeta> {
    match cache.read(state_dir, meta_path) {
        Ok(meta) => Some(meta),
        Err(err) => {
            diagnostics.push(agent_bash_meta_diagnostic(state_dir, err));
            None
        }
    }
}

fn read_meta_for_scan(
    cache: &AgentBashMetaCache,
    state_dir: &Path,
    meta_path: &Path,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<AgentBashMeta> {
    match cache.read(state_dir, meta_path) {
        Ok(meta) => Some(meta),
        Err(err) => {
            diagnostics.push(agent_bash_meta_diagnostic(state_dir, err));
            None
        }
    }
}

impl AgentBashMetaCache {
    fn read(&self, state_dir: &Path, meta_path: &Path) -> Result<AgentBashMeta, String> {
        let metadata = match read_meta_metadata(meta_path) {
            Ok(metadata) => metadata,
            Err(err) => return Err(format_meta_stat_error(meta_path, err)),
        };
        let modified_at = metadata.modified().ok();
        let size = metadata.len();
        let mut entries = self.lock_entries();
        if let Some(cached) = matching_cached_meta(&entries, state_dir, modified_at, size) {
            return clone_cached_meta_result(cached);
        }
        let result = parse_meta_file(state_dir, meta_path);
        store_meta_result(&mut entries, state_dir, modified_at, size, result.clone());
        result
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, CachedAgentBashMeta>> {
        self.entries.lock().unwrap()
    }
}

fn read_meta_metadata(meta_path: &Path) -> Result<std::fs::Metadata, std::io::Error> {
    std::fs::metadata(meta_path)
}

fn format_meta_stat_error(meta_path: &Path, err: std::io::Error) -> String {
    format!("failed to stat meta.json {}: {err}", meta_path.display())
}

fn matching_cached_meta<'a>(
    entries: &'a HashMap<PathBuf, CachedAgentBashMeta>,
    state_dir: &Path,
    modified_at: Option<SystemTime>,
    size: u64,
) -> Option<&'a CachedAgentBashMeta> {
    let cached = cached_meta_entry(entries, state_dir)?;
    matching_meta_entry(cached, modified_at, size)
}

fn cached_meta_entry<'a>(
    entries: &'a HashMap<PathBuf, CachedAgentBashMeta>,
    state_dir: &Path,
) -> Option<&'a CachedAgentBashMeta> {
    entries.get(state_dir)
}

fn matching_meta_entry(
    cached: &CachedAgentBashMeta,
    modified_at: Option<SystemTime>,
    size: u64,
) -> Option<&CachedAgentBashMeta> {
    cached_meta_matches(cached, modified_at, size).then_some(cached)
}

fn cached_meta_matches(
    cached: &CachedAgentBashMeta,
    modified_at: Option<SystemTime>,
    size: u64,
) -> bool {
    cached.modified_at == modified_at && cached.size == size
}

fn clone_cached_meta_result(cached: &CachedAgentBashMeta) -> Result<AgentBashMeta, String> {
    cached.result.clone()
}

fn store_meta_result(
    entries: &mut HashMap<PathBuf, CachedAgentBashMeta>,
    state_dir: &Path,
    modified_at: Option<SystemTime>,
    size: u64,
    result: Result<AgentBashMeta, String>,
) {
    entries.insert(
        state_dir.to_path_buf(),
        cached_meta(modified_at, size, result),
    );
}

fn cached_meta(
    modified_at: Option<SystemTime>,
    size: u64,
    result: Result<AgentBashMeta, String>,
) -> CachedAgentBashMeta {
    CachedAgentBashMeta {
        modified_at,
        size,
        result,
    }
}

fn parse_meta_file(state_dir: &Path, meta_path: &Path) -> Result<AgentBashMeta, String> {
    let raw = match read_meta_json_text(meta_path) {
        Ok(raw) => raw,
        Err(err) => return Err(format_meta_read_error(meta_path, err)),
    };
    let value = match parse_meta_json_text(&raw) {
        Ok(value) => value,
        Err(err) => return Err(format_meta_parse_error(meta_path, err)),
    };
    parse_meta_value(state_dir, &value)
}

fn read_meta_json_text(meta_path: &Path) -> Result<String, std::io::Error> {
    std::fs::read_to_string(meta_path)
}

fn format_meta_read_error(meta_path: &Path, err: std::io::Error) -> String {
    format!("failed to read meta.json {}: {err}", meta_path.display())
}

fn parse_meta_json_text(raw: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(raw)
}

fn format_meta_parse_error(meta_path: &Path, err: serde_json::Error) -> String {
    format!("failed to parse meta.json {}: {err}", meta_path.display())
}

fn parse_meta_value(state_dir: &Path, value: &Value) -> Result<AgentBashMeta, String> {
    let handle = parse_meta_handle(state_dir, value)?;
    let caller_chain = parse_caller_chain(value)?;
    Ok(agent_bash_meta(value, handle, caller_chain))
}

fn parse_meta_handle(state_dir: &Path, value: &Value) -> Result<String, String> {
    required_meta_handle(parse_meta_handle_candidate(state_dir, value))
}

fn parse_meta_handle_candidate(state_dir: &Path, value: &Value) -> Option<String> {
    optional_string(value, &["handle"]).or_else(|| state_dir_handle(state_dir))
}

fn state_dir_handle(state_dir: &Path) -> Option<String> {
    state_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

fn required_meta_handle(handle: Option<String>) -> Result<String, String> {
    handle.ok_or_else(missing_meta_handle_error)
}

fn missing_meta_handle_error() -> String {
    "meta.json missing handle".to_string()
}

fn agent_bash_meta(
    value: &Value,
    handle: String,
    caller_chain: Vec<CallerIdentity>,
) -> AgentBashMeta {
    let workload_pid = optional_i64(value, &["workload_pid", "workloadPid"]);
    AgentBashMeta {
        handle,
        state: optional_string(value, &["state"]).unwrap_or_else(|| "UNKNOWN".to_string()),
        caller_chain,
        supervisor_pid: optional_i64(value, &["supervisor_pid", "supervisorPid"]),
        workload_pid,
        workload_identity: parse_workload_identity(value, workload_pid),
        workload_pgid: optional_i64(value, &["workload_pgid", "workloadPgid", "pgid"]),
        argv: optional_string_array(value, "argv"),
        cwd: optional_string(value, &["cwd"]),
        ready_at: optional_string(value, &["ready_at", "ready_time", "started_at"]),
        completed_at: optional_string(value, &["completed_at", "completed_time"]),
        rc: optional_i64(value, &["rc"]).and_then(|value| i32::try_from(value).ok()),
        signal: optional_i64(value, &["signal"]).and_then(|value| i32::try_from(value).ok()),
        cgroup: optional_string(value, &["cgroup"]),
        log_path: optional_string(value, &["log_path", "logPath"]).map(PathBuf::from),
    }
}

fn parse_workload_identity(value: &Value, workload_pid: Option<i64>) -> WorkloadIdentityEvidence {
    let boot_id_names = ["process_boot_id", "processBootId"];
    let starttime_names = ["workload_pid_starttime_ticks", "workloadPidStarttimeTicks"];
    let has_boot_id = has_any_field(value, &boot_id_names);
    let has_starttime = has_any_field(value, &starttime_names);
    if !has_boot_id && !has_starttime {
        return WorkloadIdentityEvidence::Legacy;
    }
    match (
        workload_pid,
        optional_string(value, &boot_id_names),
        optional_i64(value, &starttime_names),
    ) {
        (Some(os_pid), Some(os_boot_id), Some(os_pid_starttime_ticks)) => {
            WorkloadIdentityEvidence::Exact(ProcessIdentity {
                os_pid,
                os_boot_id,
                os_pid_starttime_ticks,
            })
        }
        _ => WorkloadIdentityEvidence::Incomplete,
    }
}

fn parse_caller_chain(value: &Value) -> Result<Vec<CallerIdentity>, String> {
    let chain = caller_chain_array(value).ok_or_else(missing_caller_chain_error)?;
    validate_caller_chain(chain)?;
    parse_caller_identities(chain)
}

fn caller_chain_array(value: &Value) -> Option<&[Value]> {
    value
        .get("caller_chain")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}

fn missing_caller_chain_error() -> String {
    "meta.json must contain caller_chain array".to_string()
}

fn validate_caller_chain(chain: &[Value]) -> Result<(), String> {
    if chain.is_empty() {
        return Err(empty_caller_chain_error());
    }
    Ok(())
}

fn empty_caller_chain_error() -> String {
    "meta.json caller_chain must not be empty".to_string()
}

fn parse_caller_identities(chain: &[Value]) -> Result<Vec<CallerIdentity>, String> {
    chain
        .iter()
        .enumerate()
        .map(parse_caller_identity)
        .collect()
}

fn parse_caller_identity((chain_index, value): (usize, &Value)) -> Result<CallerIdentity, String> {
    let fields = parse_process_identity_fields(value)?;
    Ok(caller_identity_from_fields(
        chain_index,
        process_identity_from_fields(fields),
    ))
}

fn parse_process_identity_fields(value: &Value) -> Result<ProcessIdentityFields, String> {
    Ok(ProcessIdentityFields {
        os_pid: required_i64(value, &["pid", "os_pid"])?,
        os_boot_id: required_string(value, &["boot_id", "os_boot_id"])?,
        os_pid_starttime_ticks: required_i64(
            value,
            &["starttime_ticks", "os_pid_starttime_ticks"],
        )?,
    })
}

fn process_identity_from_fields(fields: ProcessIdentityFields) -> ProcessIdentity {
    ProcessIdentity {
        os_pid: fields.os_pid,
        os_boot_id: fields.os_boot_id,
        os_pid_starttime_ticks: fields.os_pid_starttime_ticks,
    }
}

fn caller_identity_from_fields(chain_index: usize, identity: ProcessIdentity) -> CallerIdentity {
    CallerIdentity {
        chain_index,
        identity,
    }
}

fn resolve_owner(
    state: Option<&StateDb>,
    pid: Option<&PidIdentityDb>,
    chain: &[CallerIdentity],
    cancellation: &CancellationToken,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<ResolvedOwner> {
    resolve_owner_until(
        state,
        pid,
        chain,
        &|| cancellation.is_cancelled(),
        diagnostics,
    )
}

fn resolve_owner_until(
    state: Option<&StateDb>,
    pid: Option<&PidIdentityDb>,
    chain: &[CallerIdentity],
    is_cancelled: &dyn Fn() -> bool,
    diagnostics: &mut Vec<MonitorDiagnostic>,
) -> Option<ResolvedOwner> {
    let pid = pid?;
    for entry in chain {
        if is_cancelled() {
            return None;
        }
        match resolve_owner_for_entry(state, pid, entry) {
            Ok(Some(owner)) => return Some(owner),
            Ok(None) => {}
            Err(err) => diagnostics.push(storage_diagnostic("agent-bash:owner-resolution", err)),
        }
    }
    None
}

fn resolve_owner_for_entry(
    state: Option<&StateDb>,
    pid: &PidIdentityDb,
    entry: &CallerIdentity,
) -> Result<Option<ResolvedOwner>, String> {
    let Some(row) = lookup_owner_pid_row(pid, entry)? else {
        return Ok(None);
    };
    Ok(Some(resolved_owner_from_row(state, entry.chain_index, row)))
}

fn lookup_owner_pid_row(
    pid: &PidIdentityDb,
    entry: &CallerIdentity,
) -> Result<Option<PidIdentityRow>, String> {
    pid.lookup_by_identity(&entry.identity)
}

fn resolved_owner_from_row(
    state: Option<&StateDb>,
    matched_chain_index: usize,
    row: PidIdentityRow,
) -> ResolvedOwner {
    let session_id = owner_session_id(state, &row);
    ResolvedOwner {
        session_id,
        invocation_uuid: row.invocation_uuid,
        matched_chain_index: Some(matched_chain_index),
    }
}

fn workload_marker_owner(
    state: Option<&StateDb>,
    state_dir: &Path,
    meta: &AgentBashMeta,
    invocation_uuids: &HashSet<String>,
) -> Option<ResolvedOwner> {
    let invocation_uuid = workload_marker_invocation_uuid(&meta_log_path(state_dir, meta))?;
    if !invocation_uuids.contains(&invocation_uuid) {
        return None;
    }
    Some(ResolvedOwner {
        session_id: state_invocation_session(state, &invocation_uuid),
        invocation_uuid,
        matched_chain_index: None,
    })
}

fn workload_marker_invocation_uuid(path: &Path) -> Option<String> {
    let line = read_log_head_line(path)?;
    let payload = line.strip_prefix(INVOCATION_MARKER_PREFIX)?;
    // Ownership requires canonical JSON; the shared parser also accepts legacy
    // shell-mangled markers after a JSON syntax failure.
    serde_json::from_str::<Value>(payload).ok()?;
    CompositeInvocationId::parse_marker_payload(payload)
        .ok()
        .map(|invocation| invocation.id)
}

fn read_log_head_line(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(WORKLOAD_INVOCATION_MARKER_MAX_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    let line_end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..line_end])
        .ok()
        .map(|line| line.trim_end_matches('\r').to_string())
}

fn owner_session_id(state: Option<&StateDb>, row: &PidIdentityRow) -> Option<String> {
    row.session_id
        .clone()
        .or_else(|| state_invocation_session(state, &row.invocation_uuid))
}

fn state_invocation_session(state: Option<&StateDb>, invocation_uuid: &str) -> Option<String> {
    state
        .and_then(|state| state.get_invocation_by_uuid(invocation_uuid).ok().flatten())
        .as_ref()
        .and_then(resolved_invocation_session_id)
}

fn owner_matches_root(
    owner: &ResolvedOwner,
    session_id: Option<&str>,
    invocation_uuids: &HashSet<String>,
) -> bool {
    owner
        .session_id
        .as_deref()
        .zip(session_id)
        .is_some_and(|(owner, root)| owner == root)
        || invocation_uuids.contains(&owner.invocation_uuid)
}

fn workload_parent_id(
    invocation_uuid: Option<&str>,
    row_session_id: &str,
    invocation_uuids: &HashSet<String>,
    root_session_id: Option<&str>,
) -> Option<String> {
    workload_root_invocation_uuid(invocation_uuid, invocation_uuids)
        .map(invocation_node_id)
        .or_else(|| workload_session_parent_id(row_session_id, root_session_id))
}

fn workload_root_invocation_uuid<'a>(
    invocation_uuid: Option<&'a str>,
    invocation_uuids: &HashSet<String>,
) -> Option<&'a str> {
    invocation_uuid.filter(|uuid| invocation_uuids.contains(*uuid))
}

fn workload_session_parent_id(
    row_session_id: &str,
    root_session_id: Option<&str>,
) -> Option<String> {
    root_session_id
        .or(Some(row_session_id))
        .map(session_node_id)
}

fn workload_label(
    handle: &str,
    meta: Option<&AgentBashMeta>,
    matched_chain_index: Option<usize>,
) -> String {
    let state = meta.map(|meta| meta.state.as_str()).unwrap_or("complete");
    let argv = meta
        .and_then(|meta| (!meta.argv.is_empty()).then(|| meta.argv.join(" ")))
        .unwrap_or_else(|| handle.to_string());
    let cwd = meta
        .and_then(|meta| meta.cwd.as_deref())
        .map(|cwd| format!(" cwd={cwd}"))
        .unwrap_or_default();
    let supervisor = meta
        .and_then(|meta| meta.supervisor_pid)
        .map(|pid| format!(" supervisor={pid}"))
        .unwrap_or_default();
    let chain = matched_chain_index
        .map(|index| format!(" chain_index={index}"))
        .unwrap_or_default();
    let signal = meta
        .and_then(|meta| meta.signal)
        .map(|signal| format!(" signal={signal}"))
        .unwrap_or_default();
    let cgroup = meta
        .and_then(|meta| meta.cgroup.as_deref())
        .map(|cgroup| format!(" cgroup={cgroup}"))
        .unwrap_or_default();
    format!("agent-bash {handle} {state}: {argv}{cwd}{supervisor}{chain}{signal}{cgroup}")
}

fn mailbox_workload_status(
    row: &MailboxRow,
    meta: Option<&AgentBashMeta>,
    liveness: LivenessStatus,
) -> MonitorStatus {
    match meta.map(|meta| meta_workload_status(meta, liveness)) {
        Some(status) if is_terminal_meta_status(status) => status,
        _ => rc_status(row.rc),
    }
}

fn is_terminal_meta_status(status: MonitorStatus) -> bool {
    matches!(
        status,
        MonitorStatus::Succeeded
            | MonitorStatus::Failed
            | MonitorStatus::Error
            | MonitorStatus::Cancelled
    )
}

fn meta_workload_status(meta: &AgentBashMeta, liveness: LivenessStatus) -> MonitorStatus {
    let recorded_status = match meta.state.to_ascii_lowercase().as_str() {
        "running" | "ready" => MonitorStatus::Running,
        "pending" => MonitorStatus::Pending,
        "done" | "complete" | "completed" => {
            meta.rc.map(rc_status).unwrap_or(MonitorStatus::Succeeded)
        }
        "error" => MonitorStatus::Error,
        "failed" | "timeout" | "timed_out" => MonitorStatus::Failed,
        "cancelling" => MonitorStatus::Cancelling,
        "cancelled" | "canceled" => MonitorStatus::Cancelled,
        _ => meta.rc.map(rc_status).unwrap_or(MonitorStatus::Unknown),
    };
    if recorded_status != MonitorStatus::Running {
        return recorded_status;
    }
    match liveness {
        LivenessStatus::VerifiedLive | LivenessStatus::UnverifiedLive => MonitorStatus::Running,
        LivenessStatus::Dead | LivenessStatus::PidReused => MonitorStatus::Stale,
        LivenessStatus::Unknown | LivenessStatus::NotApplicable => MonitorStatus::Unknown,
    }
}

fn meta_workload_liveness(meta: &AgentBashMeta) -> LivenessStatus {
    match &meta.workload_identity {
        WorkloadIdentityEvidence::Exact(identity) => process_identity_liveness(identity),
        WorkloadIdentityEvidence::Legacy => unverified_pid_liveness(meta.workload_pid),
        WorkloadIdentityEvidence::Incomplete => LivenessStatus::Unknown,
    }
}

fn rc_status(rc: i32) -> MonitorStatus {
    if rc == 0 {
        MonitorStatus::Succeeded
    } else {
        MonitorStatus::Failed
    }
}

fn mailbox_log_path(row: &MailboxRow, meta: Option<&AgentBashMeta>) -> PathBuf {
    meta.and_then(|meta| meta.log_path.clone())
        .unwrap_or_else(|| PathBuf::from(&row.log_path))
}

fn meta_log_path(state_dir: &Path, meta: &AgentBashMeta) -> PathBuf {
    meta.log_path
        .clone()
        .unwrap_or_else(|| state_dir.join("log"))
}

fn read_log_tail(path: &Path, max_tail_bytes: usize) -> Option<String> {
    if !log_tail_requested(max_tail_bytes) {
        return None;
    }
    let buffer = read_requested_log_tail_bytes(path, max_tail_bytes)?;
    Some(decode_log_tail_bytes(&buffer))
}

fn read_requested_log_tail_bytes(path: &Path, max_tail_bytes: usize) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let offset = log_tail_offset(len, max_tail_bytes);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

fn log_tail_requested(max_tail_bytes: usize) -> bool {
    max_tail_bytes != 0
}

fn log_tail_offset(len: u64, max_tail_bytes: usize) -> u64 {
    len.saturating_sub(max_tail_bytes as u64)
}

fn decode_log_tail_bytes(buffer: &[u8]) -> String {
    String::from_utf8_lossy(buffer).into_owned()
}

fn agent_bash_meta_diagnostic(state_dir: &Path, message: String) -> MonitorDiagnostic {
    MonitorDiagnostic {
        code: "agent-bash:meta-corrupt".to_string(),
        severity: MonitorDiagnosticSeverity::Warning,
        message: agent_bash_meta_diagnostic_message(state_dir, message),
        node_id: None,
    }
}

fn agent_bash_meta_diagnostic_message(state_dir: &Path, message: String) -> String {
    format!("{}: {message}", state_dir.display())
}

fn agent_bash_node_id(handle: &str) -> String {
    format!("agent-bash:{handle}")
}

fn optional_string(value: &Value, names: &[&str]) -> Option<String> {
    let value = optional_raw_string(value, names)?;
    let value = non_empty_string(value)?;
    Some(owned_string(value))
}

fn has_any_field(value: &Value, names: &[&str]) -> bool {
    names.iter().any(|name| value.get(*name).is_some())
}

fn optional_raw_string<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
}

fn non_empty_string(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn owned_string(value: &str) -> String {
    value.to_string()
}

fn required_string(value: &Value, names: &[&str]) -> Result<String, String> {
    optional_string(value, names).ok_or_else(|| missing_string_field_error(names))
}

fn missing_string_field_error(names: &[&str]) -> String {
    format!("caller_chain entry missing string {names:?}")
}

fn optional_i64(value: &Value, names: &[&str]) -> Option<i64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_i64))
}

fn required_i64(value: &Value, names: &[&str]) -> Result<i64, String> {
    optional_i64(value, names).ok_or_else(|| missing_integer_field_error(names))
}

fn missing_integer_field_error(names: &[&str]) -> String {
    format!("caller_chain entry missing integer {names:?}")
}

fn optional_string_array(value: &Value, name: &str) -> Vec<String> {
    let Some(items) = string_array_items(value, name) else {
        return Vec::new();
    };
    let values = string_array_item_strings(items);
    owned_strings(values)
}

fn string_array_items<'a>(value: &'a Value, name: &str) -> Option<&'a [Value]> {
    value.get(name).and_then(Value::as_array).map(Vec::as_slice)
}

fn string_array_item_strings(items: &[Value]) -> Vec<&str> {
    present_string_array_items(parsed_string_array_items(items))
}

fn parsed_string_array_items(items: &[Value]) -> Vec<Option<&str>> {
    items.iter().map(Value::as_str).collect()
}

fn present_string_array_items(values: Vec<Option<&str>>) -> Vec<&str> {
    values.into_iter().flatten().collect()
}

fn owned_strings(values: Vec<&str>) -> Vec<String> {
    values.into_iter().map(str::to_string).collect()
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use oulipoly_state::pid_identity::PidIdentityRecord;

    struct CancelDuringDiscovery<'a> {
        entries: std::fs::ReadDir,
        cancellation: &'a CancellationToken,
        visited: &'a std::cell::Cell<usize>,
        cancel_after: usize,
    }

    impl Iterator for CancelDuringDiscovery<'_> {
        type Item = Result<std::fs::DirEntry, std::io::Error>;

        fn next(&mut self) -> Option<Self::Item> {
            let visited = self.visited.get();
            if visited == self.cancel_after {
                self.cancellation.cancel();
            }
            self.visited.set(visited + 1);
            self.entries.next()
        }
    }

    #[test]
    fn cancelled_candidate_discovery_stops_before_materializing_retained_directories() {
        let directory = tempfile::tempdir().unwrap();
        for index in 0..32 {
            std::fs::create_dir(directory.path().join(format!("ab_{index:08x}"))).unwrap();
        }
        let cancellation = CancellationToken::new();
        let visited = std::cell::Cell::new(0);
        let entries = CancelDuringDiscovery {
            entries: std::fs::read_dir(directory.path()).unwrap(),
            cancellation: &cancellation,
            visited: &visited,
            cancel_after: 4,
        };

        let candidates =
            candidate_dirs_from_entries(entries, SnapshotLimits::default(), &cancellation);

        assert!(candidates.is_empty());
        assert!(visited.get() < 32);
    }

    #[test]
    fn candidate_retention_never_exceeds_the_configured_bound() {
        let mut candidates = Vec::new();
        let mut peak_retained = 0;

        for age in 0..100 {
            retain_candidate_dir(
                &mut candidates,
                candidate_dir_from_parts(
                    PathBuf::from(format!("ab_{age:08x}")),
                    UNIX_EPOCH.checked_add(Duration::from_secs(age)),
                ),
                4,
            );
            peak_retained = peak_retained.max(candidates.len());
        }

        assert_eq!(peak_retained, 4);
        assert_eq!(candidates.len(), 4);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.modified_at.unwrap())
                .collect::<Vec<_>>(),
            [99, 98, 97, 96].map(|age| UNIX_EPOCH + Duration::from_secs(age))
        );
    }

    #[test]
    fn owner_resolution_stops_before_a_later_matching_chain_entry_after_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let pid = PidIdentityDb::open(&directory.path().join("pid-identity.db")).unwrap();
        let missing = ProcessIdentity {
            os_pid: 1001,
            os_boot_id: "boot-a".to_string(),
            os_pid_starttime_ticks: 11,
        };
        let matching = ProcessIdentity {
            os_pid: 1002,
            os_boot_id: "boot-a".to_string(),
            os_pid_starttime_ticks: 12,
        };
        pid.record_identity(PidIdentityRecord {
            identity: &matching,
            os_pgid: None,
            invocation_uuid: "matching-invocation",
            session_id: Some("matching-session"),
            provider_name: None,
            model_name: None,
            recorded_at: "2026-08-18T00:00:00Z",
        })
        .unwrap();
        let chain = [
            CallerIdentity {
                chain_index: 0,
                identity: missing,
            },
            CallerIdentity {
                chain_index: 1,
                identity: matching,
            },
        ];
        let checks = std::cell::Cell::new(0);
        let mut diagnostics = Vec::new();

        let cancelled = resolve_owner_until(
            None,
            Some(&pid),
            &chain,
            &|| {
                let check = checks.get();
                checks.set(check + 1);
                check == 1
            },
            &mut diagnostics,
        );

        assert!(cancelled.is_none());
        assert_eq!(checks.get(), 2);
        assert!(diagnostics.is_empty());
        let resolved =
            resolve_owner_until(None, Some(&pid), &chain, &|| false, &mut diagnostics).unwrap();
        assert_eq!(resolved.invocation_uuid, "matching-invocation");
        assert_eq!(resolved.matched_chain_index, Some(1));
    }
}
