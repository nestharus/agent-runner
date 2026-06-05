//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`,
//! `validator`
//!
//! `session of-pid`, `session alive`, and `session subtree` query the
//! independent PID identity sidecar and never mutate the versioned state DB.

use chrono::{SecondsFormat, Utc};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRow, ProcessIdentity};
use oulipoly_state::{InvocationRecord, StateDb};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Serialize)]
struct PidSessionResponse {
    found: bool,
    pid: u32,
    invocation_uuid: Option<String>,
    session_id: Option<String>,
    provider_name: Option<String>,
    model_name: Option<String>,
    os_boot_id: Option<String>,
    os_pid_starttime_ticks: Option<i64>,
    os_pgid: Option<i64>,
    recorded_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct AliveResponse {
    pid: u32,
    alive: bool,
    invocation_uuid: Option<String>,
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubtreeResponse {
    found: bool,
    pid: u32,
    root_invocation_uuid: Option<String>,
    generated_at: String,
    root: Option<SubtreeNode>,
}

#[derive(Debug, Serialize)]
struct SubtreeNode {
    row_id: i64,
    invocation_uuid: String,
    parent_invocation_uuid: Option<String>,
    session_id: Option<String>,
    provider_name: Option<String>,
    model_name: String,
    pid: Option<i64>,
    alive: bool,
    children: Vec<SubtreeNode>,
}

#[derive(Debug, Clone)]
struct PidAnnotation {
    pid: Option<i64>,
    alive: bool,
}

pub(crate) fn run_of_pid(pid: u32, json: bool) -> Result<i32, String> {
    let Some(row) = lookup_verified_live_row(pid)? else {
        return render_of_pid_not_found(pid, json);
    };
    let session_id = resolve_row_session_id(&row)?;
    let response = of_pid_response(pid, &row, session_id);
    render_of_pid_found(&response, json)
}

pub(crate) fn run_alive(pid: u32, json: bool) -> Result<i32, String> {
    let row = lookup_verified_live_row(pid)?;
    let response = alive_response(pid, row)?;
    render_alive(&response, json)
}

pub(crate) fn run_subtree(pid: u32, json: bool, max_depth: usize) -> Result<i32, String> {
    let Some(row) = lookup_verified_live_row(pid)? else {
        return render_subtree_not_found(pid, json);
    };
    let Some(response) = subtree_response(pid, &row, max_depth)? else {
        return render_subtree_not_found(pid, json);
    };
    render_subtree_found(&response, json)
}

fn subtree_response(
    pid: u32,
    row: &PidIdentityRow,
    max_depth: usize,
) -> Result<Option<SubtreeResponse>, String> {
    let state = open_state_read_only_required()?;
    let sidecar = open_sidecar_read_only_required()?;
    let Some(root_record) = state.get_invocation_by_uuid(&row.invocation_uuid)? else {
        return Ok(None);
    };
    let mut visited = HashSet::from([root_record.id]);
    let root_uuid = root_record.invocation_uuid.clone();
    let root = build_subtree_node(
        &state,
        &sidecar,
        root_record,
        None,
        0,
        max_depth,
        &mut visited,
    )?;
    Ok(Some(subtree_found_response(pid, root_uuid, root)))
}

fn subtree_found_response(pid: u32, root_uuid: String, root: SubtreeNode) -> SubtreeResponse {
    SubtreeResponse {
        found: true,
        pid,
        root_invocation_uuid: Some(root_uuid),
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        root: Some(root),
    }
}

fn lookup_verified_live_row(pid: u32) -> Result<Option<PidIdentityRow>, String> {
    let Some(identity) = live_identity_for_pid(pid)? else {
        return Ok(None);
    };
    let Some(sidecar) = open_sidecar_read_only_optional()? else {
        return Ok(None);
    };
    sidecar.lookup_by_identity(&identity)
}

fn live_identity_for_pid(pid: u32) -> Result<Option<ProcessIdentity>, String> {
    oulipoly_state::pid_identity::read_live_process_identity(i64::from(pid))
}

fn open_sidecar_read_only_optional() -> Result<Option<PidIdentityDb>, String> {
    let path = PidIdentityDb::default_path()?;
    if !path.exists() {
        return Ok(None);
    }
    PidIdentityDb::open_read_only(&path).map(Some)
}

fn open_sidecar_read_only_required() -> Result<PidIdentityDb, String> {
    require_sidecar(open_sidecar_read_only_optional()?)
}

fn open_state_read_only_optional() -> Result<Option<StateDb>, String> {
    let path = StateDb::default_path()?;
    if !path.exists() {
        return Ok(None);
    }
    StateDb::open_read_only(&path)
        .map(Some)
        .map_err(|err| format!("Failed to open state DB read-only: {err:?}"))
}

fn open_state_read_only_required() -> Result<StateDb, String> {
    require_state_db(open_state_read_only_optional()?)
}

fn require_sidecar(db: Option<PidIdentityDb>) -> Result<PidIdentityDb, String> {
    db.ok_or_else(|| "PID identity sidecar not found".to_string())
}

fn require_state_db(db: Option<StateDb>) -> Result<StateDb, String> {
    db.ok_or_else(|| "state.db not found".to_string())
}

fn resolve_row_session_id(row: &PidIdentityRow) -> Result<Option<String>, String> {
    if let Some(session_id) = sidecar_session_id(row) {
        return Ok(Some(session_id));
    }
    state_invocation_session_id(&row.invocation_uuid)
}

fn sidecar_session_id(row: &PidIdentityRow) -> Option<String> {
    row.session_id.clone()
}

fn state_invocation_session_id(invocation_uuid: &str) -> Result<Option<String>, String> {
    let Some(state) = open_state_read_only_optional()? else {
        return Ok(None);
    };
    Ok(state
        .get_invocation_by_uuid(invocation_uuid)?
        .as_ref()
        .and_then(resolved_invocation_session_id))
}

fn resolved_invocation_session_id(record: &InvocationRecord) -> Option<String> {
    record
        .provider_session_id
        .clone()
        .or_else(|| record.session_id.clone())
}

fn build_subtree_node(
    state: &StateDb,
    sidecar: &PidIdentityDb,
    record: InvocationRecord,
    parent_invocation_uuid: Option<String>,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<i64>,
) -> Result<SubtreeNode, String> {
    let annotation = pid_annotation_for_invocation(sidecar, &record.invocation_uuid)?;
    let children = subtree_children_for_depth(state, sidecar, &record, depth, max_depth, visited)?;
    let session_id = resolved_invocation_session_id(&record);
    Ok(subtree_node(
        record,
        parent_invocation_uuid,
        session_id,
        annotation,
        children,
    ))
}

fn subtree_children_for_depth(
    state: &StateDb,
    sidecar: &PidIdentityDb,
    record: &InvocationRecord,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<i64>,
) -> Result<Vec<SubtreeNode>, String> {
    if depth >= max_depth {
        return Ok(Vec::new());
    }
    build_subtree_children(state, sidecar, record, depth, max_depth, visited)
}

fn subtree_node(
    record: InvocationRecord,
    parent_invocation_uuid: Option<String>,
    session_id: Option<String>,
    annotation: PidAnnotation,
    children: Vec<SubtreeNode>,
) -> SubtreeNode {
    SubtreeNode {
        row_id: record.id,
        invocation_uuid: record.invocation_uuid,
        parent_invocation_uuid,
        session_id,
        provider_name: record.provider_name,
        model_name: record.model_name,
        pid: annotation.pid,
        alive: annotation.alive,
        children,
    }
}

fn build_subtree_children(
    state: &StateDb,
    sidecar: &PidIdentityDb,
    record: &InvocationRecord,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<i64>,
) -> Result<Vec<SubtreeNode>, String> {
    let mut children = Vec::new();
    for child in child_invocations(state, record.id)? {
        if !mark_child_visited(visited, child.id) {
            continue;
        }
        let child_id = child.id;
        let node =
            build_child_subtree_node(state, sidecar, record, child, depth, max_depth, visited)?;
        unmark_child_visited(visited, child_id);
        children.push(node);
    }
    Ok(children)
}

fn child_invocations(state: &StateDb, row_id: i64) -> Result<Vec<InvocationRecord>, String> {
    state.list_invocation_children(row_id)
}

fn mark_child_visited(visited: &mut HashSet<i64>, child_id: i64) -> bool {
    visited.insert(child_id)
}

fn unmark_child_visited(visited: &mut HashSet<i64>, child_id: i64) {
    visited.remove(&child_id);
}

fn build_child_subtree_node(
    state: &StateDb,
    sidecar: &PidIdentityDb,
    parent: &InvocationRecord,
    child: InvocationRecord,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<i64>,
) -> Result<SubtreeNode, String> {
    build_subtree_node(
        state,
        sidecar,
        child,
        Some(parent.invocation_uuid.clone()),
        depth + 1,
        max_depth,
        visited,
    )
}

fn pid_annotation_for_invocation(
    sidecar: &PidIdentityDb,
    invocation_uuid: &str,
) -> Result<PidAnnotation, String> {
    let rows = pid_rows_for_invocation(sidecar, invocation_uuid)?;
    Ok(pid_annotation_from_rows(rows))
}

fn pid_rows_for_invocation(
    sidecar: &PidIdentityDb,
    invocation_uuid: &str,
) -> Result<Vec<PidIdentityRow>, String> {
    sidecar.lookup_by_invocation_uuid(invocation_uuid)
}

fn pid_annotation_from_rows(rows: Vec<PidIdentityRow>) -> PidAnnotation {
    let mut fallback = None;
    for row in rows {
        let annotation = pid_annotation_for_row(&row);
        fallback = fallback.or_else(|| Some(annotation.clone()));
        if annotation.alive {
            return annotation;
        }
    }
    fallback.unwrap_or(PidAnnotation {
        pid: None,
        alive: false,
    })
}

fn pid_annotation_for_row(row: &PidIdentityRow) -> PidAnnotation {
    PidAnnotation {
        pid: Some(row.os_pid),
        alive: row_is_alive(row),
    }
}

fn row_is_alive(row: &PidIdentityRow) -> bool {
    live_identity_for_row_pid(row)
        .ok()
        .flatten()
        .is_some_and(|identity| identity == row.identity())
}

fn live_identity_for_row_pid(row: &PidIdentityRow) -> Result<Option<ProcessIdentity>, String> {
    oulipoly_state::pid_identity::read_live_process_identity(row.os_pid)
}

fn of_pid_response(
    pid: u32,
    row: &PidIdentityRow,
    session_id: Option<String>,
) -> PidSessionResponse {
    PidSessionResponse {
        found: true,
        pid,
        invocation_uuid: Some(row.invocation_uuid.clone()),
        session_id,
        provider_name: row.provider_name.clone(),
        model_name: row.model_name.clone(),
        os_boot_id: Some(row.os_boot_id.clone()),
        os_pid_starttime_ticks: Some(row.os_pid_starttime_ticks),
        os_pgid: row.os_pgid,
        recorded_at: Some(row.recorded_at.clone()),
    }
}

fn alive_response(pid: u32, row: Option<PidIdentityRow>) -> Result<AliveResponse, String> {
    let Some(row) = row else {
        return Ok(alive_not_found_response(pid));
    };
    alive_found_response(pid, row)
}

fn alive_found_response(pid: u32, row: PidIdentityRow) -> Result<AliveResponse, String> {
    Ok(AliveResponse {
        pid,
        alive: true,
        session_id: resolve_row_session_id(&row)?,
        invocation_uuid: Some(row.invocation_uuid),
    })
}

fn alive_not_found_response(pid: u32) -> AliveResponse {
    AliveResponse {
        pid,
        alive: false,
        invocation_uuid: None,
        session_id: None,
    }
}

fn render_of_pid_not_found(pid: u32, json: bool) -> Result<i32, String> {
    let response = of_pid_not_found_response(pid);
    if json {
        print_json(&response)?;
    } else {
        eprintln!("No verified session identity found for pid {pid}");
    }
    Ok(not_found_exit_code())
}

fn of_pid_not_found_response(pid: u32) -> PidSessionResponse {
    PidSessionResponse {
        found: false,
        pid,
        invocation_uuid: None,
        session_id: None,
        provider_name: None,
        model_name: None,
        os_boot_id: None,
        os_pid_starttime_ticks: None,
        os_pgid: None,
        recorded_at: None,
    }
}

fn render_of_pid_found(response: &PidSessionResponse, json: bool) -> Result<i32, String> {
    if json {
        print_json(response)?;
    } else {
        println!(
            "pid {} -> invocation {} session {}",
            response.pid,
            response.invocation_uuid.as_deref().unwrap_or("-"),
            response.session_id.as_deref().unwrap_or("-")
        );
    }
    Ok(success_exit_code())
}

fn render_alive(response: &AliveResponse, json: bool) -> Result<i32, String> {
    if json {
        print_json(response)?;
    } else {
        println!("{}", response.alive);
    }
    Ok(alive_exit_code(response.alive))
}

fn render_subtree_not_found(pid: u32, json: bool) -> Result<i32, String> {
    let response = subtree_not_found_response(pid);
    if json {
        print_json(&response)?;
    } else {
        eprintln!("No verified invocation subtree root found for pid {pid}");
    }
    Ok(not_found_exit_code())
}

fn subtree_not_found_response(pid: u32) -> SubtreeResponse {
    SubtreeResponse {
        found: false,
        pid,
        root_invocation_uuid: None,
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        root: None,
    }
}

fn render_subtree_found(response: &SubtreeResponse, json: bool) -> Result<i32, String> {
    if json {
        print_json(response)?;
    } else if let Some(root) = &response.root {
        render_subtree_node(root, 0);
    }
    Ok(success_exit_code())
}

fn render_subtree_node(node: &SubtreeNode, depth: usize) {
    println!("{}", subtree_node_line(node, depth));
    for child in &node.children {
        render_subtree_node(child, depth + 1);
    }
}

fn subtree_node_line(node: &SubtreeNode, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    format!(
        "{indent}{} pid={} alive={}",
        node.invocation_uuid,
        rendered_pid(node.pid),
        node.alive
    )
}

fn rendered_pid(pid: Option<i64>) -> String {
    pid.map(|pid| pid.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn success_exit_code() -> i32 {
    0
}

fn not_found_exit_code() -> i32 {
    1
}

fn alive_exit_code(alive: bool) -> i32 {
    if alive {
        success_exit_code()
    } else {
        not_found_exit_code()
    }
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|err| format!("Failed to serialize PID session response: {err}"))?;
    println!("{json}");
    Ok(())
}
