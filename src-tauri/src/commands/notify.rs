//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`,
//! `validator`

use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, EnqueueResult, MailboxDb, MailboxRow};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRow, ProcessIdentity};
use oulipoly_state::{InvocationRecord, StateDb};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

use crate::wake_coordinator::WakeDiagnostic;

#[derive(Debug, Serialize)]
struct NotifyResponse {
    status: String,
    enqueued: bool,
    handle: String,
    caller_ppid: u32,
    matched_chain_index: Option<usize>,
    matched_pid: Option<i64>,
    owner_invocation_uuid: Option<String>,
    owner_session_id: Option<String>,
    session_source: Option<String>,
    seq: Option<i64>,
    wake: Option<WakeDiagnostic>,
}

#[derive(Debug, Clone)]
struct CallerIdentity {
    chain_index: usize,
    identity: ProcessIdentity,
}

#[derive(Debug, Clone)]
struct ResolvedOwner {
    session_id: String,
    invocation_uuid: String,
    matched_chain_index: usize,
    matched_identity: ProcessIdentity,
    source: OwnerSessionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerSessionSource {
    SidecarSessionId,
    StateDbInvocationJoin,
}

impl OwnerSessionSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::SidecarSessionId => "sidecar_session_id",
            Self::StateDbInvocationJoin => "state_db_invocation_join",
        }
    }
}

pub(crate) struct AgentBashCompleteArgs<'a> {
    pub caller_ppid: u32,
    pub handle: &'a str,
    pub state_dir: &'a Path,
    pub meta: &'a Path,
    pub log: &'a Path,
    pub rc: &'a Path,
    pub json: bool,
}

pub(crate) fn run_agent_bash_complete(args: AgentBashCompleteArgs<'_>) -> Result<i32, String> {
    match run_agent_bash_complete_inner(&args) {
        Ok(outcome) => render_notify_success(&args, outcome),
        Err(error) => render_mapped_notify_error(&args, error),
    }
}

fn render_mapped_notify_error(
    args: &AgentBashCompleteArgs<'_>,
    error: NotifyError,
) -> Result<i32, String> {
    let render = notify_error_render(error);
    render_notify_error(args, render)
}

struct NotifyErrorRender {
    exit_code: i32,
    status: &'static str,
    message: Option<String>,
    existing: Option<Box<MailboxRow>>,
}

fn notify_error_render(error: NotifyError) -> NotifyErrorRender {
    match error {
        NotifyError::MalformedMetadata(message) => NotifyErrorRender {
            exit_code: 64,
            status: "malformed_metadata",
            message: Some(message),
            existing: None,
        },
        NotifyError::Storage(message) => NotifyErrorRender {
            exit_code: 74,
            status: "storage_error",
            message: Some(message),
            existing: None,
        },
        NotifyError::Conflict { existing } => NotifyErrorRender {
            exit_code: 73,
            status: "idempotency_conflict",
            message: Some(conflict_message(&existing)),
            existing: Some(existing),
        },
    }
}

fn conflict_message(existing: &MailboxRow) -> String {
    format!("existing handle belongs to session {}", existing.session_id)
}

enum NotifyOutcome {
    Enqueued {
        owner: ResolvedOwner,
        row: MailboxRow,
        wake: WakeDiagnostic,
    },
    AlreadyEnqueued {
        owner: ResolvedOwner,
        row: MailboxRow,
        wake: WakeDiagnostic,
    },
    NoOwner,
}

enum NotifyError {
    MalformedMetadata(String),
    Storage(String),
    Conflict { existing: Box<MailboxRow> },
}

fn run_agent_bash_complete_inner(
    args: &AgentBashCompleteArgs<'_>,
) -> Result<NotifyOutcome, NotifyError> {
    let input = parse_notify_input(args)?;
    let Some(owner) = resolve_owner(&input.caller_chain)? else {
        return Ok(NotifyOutcome::NoOwner);
    };
    enqueue_owner_notification(args, &input.metadata, input.rc, owner)
}

struct NotifyInput {
    metadata: Value,
    caller_chain: Vec<CallerIdentity>,
    rc: i32,
}

fn parse_notify_input(args: &AgentBashCompleteArgs<'_>) -> Result<NotifyInput, NotifyError> {
    let metadata = read_metadata(args.meta)?;
    let caller_chain = parse_caller_chain(&metadata)?;
    let rc = read_rc(args.rc)?;
    Ok(NotifyInput {
        metadata,
        caller_chain,
        rc,
    })
}

fn enqueue_owner_notification(
    args: &AgentBashCompleteArgs<'_>,
    metadata: &Value,
    rc: i32,
    owner: ResolvedOwner,
) -> Result<NotifyOutcome, NotifyError> {
    let paths = notify_path_strings(args);
    let payload_json = payload_json(metadata, args, rc, &owner, &paths)?;
    let enqueue = agent_bash_complete_enqueue(args, &payload_json, rc, &owner, &paths);
    let mut mailbox = MailboxDb::open_default().map_err(NotifyError::Storage)?;
    match mailbox
        .enqueue_agent_bash_complete(&enqueue)
        .map_err(NotifyError::Storage)?
    {
        EnqueueResult::Inserted(row) => {
            let wake = crate::wake_coordinator::trigger_notify_wake(&owner.session_id);
            Ok(NotifyOutcome::Enqueued { owner, row, wake })
        }
        EnqueueResult::AlreadyEnqueued(row) => {
            let wake = crate::wake_coordinator::trigger_notify_wake(&owner.session_id);
            Ok(NotifyOutcome::AlreadyEnqueued { owner, row, wake })
        }
        EnqueueResult::Conflict { existing } => Err(NotifyError::Conflict {
            existing: Box::new(existing),
        }),
    }
}

fn agent_bash_complete_enqueue<'a>(
    args: &'a AgentBashCompleteArgs<'_>,
    payload_json: &'a str,
    rc: i32,
    owner: &'a ResolvedOwner,
    paths: &'a NotifyPathStrings,
) -> AgentBashCompleteEnqueue<'a> {
    AgentBashCompleteEnqueue {
        session_id: &owner.session_id,
        handle: args.handle,
        payload_json,
        owner_invocation_uuid: Some(&owner.invocation_uuid),
        matched_os_pid: Some(owner.matched_identity.os_pid),
        matched_os_boot_id: Some(&owner.matched_identity.os_boot_id),
        matched_os_pid_starttime_ticks: Some(owner.matched_identity.os_pid_starttime_ticks),
        matched_chain_index: Some(owner.matched_chain_index as i64),
        state_dir: &paths.state_dir,
        meta_path: &paths.meta_path,
        log_path: &paths.log_path,
        rc_path: &paths.rc_path,
        rc,
    }
}

struct NotifyPathStrings {
    state_dir: String,
    meta_path: String,
    log_path: String,
    rc_path: String,
}

fn notify_path_strings(args: &AgentBashCompleteArgs<'_>) -> NotifyPathStrings {
    NotifyPathStrings {
        state_dir: path_string(args.state_dir),
        meta_path: path_string(args.meta),
        log_path: path_string(args.log),
        rc_path: path_string(args.rc),
    }
}

fn read_metadata(path: &Path) -> Result<Value, NotifyError> {
    parse_metadata_json(&read_metadata_raw(path)?)
}

fn read_metadata_raw(path: &Path) -> Result<String, NotifyError> {
    std::fs::read_to_string(path).map_err(read_metadata_error)
}

fn parse_metadata_json(raw: &str) -> Result<Value, NotifyError> {
    serde_json::from_str(raw).map_err(parse_metadata_error)
}

fn read_metadata_error(err: std::io::Error) -> NotifyError {
    NotifyError::MalformedMetadata(format!("failed to read meta.json: {err}"))
}

fn parse_metadata_error(err: serde_json::Error) -> NotifyError {
    NotifyError::MalformedMetadata(format!("failed to parse meta.json: {err}"))
}

fn parse_caller_chain(metadata: &Value) -> Result<Vec<CallerIdentity>, NotifyError> {
    let chain = caller_chain_array(metadata)?;
    validate_non_empty_caller_chain(chain)?;
    parse_caller_identities(chain)
}

fn caller_chain_array(metadata: &Value) -> Result<&Vec<Value>, NotifyError> {
    metadata
        .get("caller_chain")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            NotifyError::MalformedMetadata("meta.json must contain caller_chain array".to_string())
        })
}

fn validate_non_empty_caller_chain(chain: &[Value]) -> Result<(), NotifyError> {
    if !chain.is_empty() {
        return Ok(());
    }
    Err(NotifyError::MalformedMetadata(
        "meta.json caller_chain must not be empty".to_string(),
    ))
}

fn parse_caller_identities(chain: &[Value]) -> Result<Vec<CallerIdentity>, NotifyError> {
    chain
        .iter()
        .enumerate()
        .map(parse_caller_identity)
        .collect()
}

fn parse_caller_identity(
    (chain_index, value): (usize, &Value),
) -> Result<CallerIdentity, NotifyError> {
    let fields = parse_caller_identity_fields(value)?;
    Ok(caller_identity(chain_index, fields))
}

struct CallerIdentityFields {
    pid: i64,
    starttime_ticks: i64,
    boot_id: String,
}

fn parse_caller_identity_fields(value: &Value) -> Result<CallerIdentityFields, NotifyError> {
    Ok(CallerIdentityFields {
        pid: integer_field(value, &["pid", "os_pid"])?,
        starttime_ticks: integer_field(value, &["starttime_ticks", "os_pid_starttime_ticks"])?,
        boot_id: string_field(value, &["boot_id", "os_boot_id"])?,
    })
}

fn caller_identity(chain_index: usize, fields: CallerIdentityFields) -> CallerIdentity {
    CallerIdentity {
        chain_index,
        identity: ProcessIdentity {
            os_pid: fields.pid,
            os_boot_id: fields.boot_id,
            os_pid_starttime_ticks: fields.starttime_ticks,
        },
    }
}

fn integer_field(value: &Value, names: &[&str]) -> Result<i64, NotifyError> {
    require_integer_field(find_integer_field(value, names), names)
}

fn find_integer_field(value: &Value, names: &[&str]) -> Option<i64> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_i64))
}

fn require_integer_field(value: Option<i64>, names: &[&str]) -> Result<i64, NotifyError> {
    value.ok_or_else(|| {
        NotifyError::MalformedMetadata(format!("caller_chain entry missing integer {names:?}"))
    })
}

fn string_field(value: &Value, names: &[&str]) -> Result<String, NotifyError> {
    let value = require_string_field(find_string_field(value, names), names)?;
    Ok(owned_string(value))
}

fn find_string_field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
}

fn require_string_field<'a>(
    value: Option<&'a str>,
    names: &[&str],
) -> Result<&'a str, NotifyError> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        NotifyError::MalformedMetadata(format!("caller_chain entry missing string {names:?}"))
    })
}

fn owned_string(value: &str) -> String {
    value.to_string()
}

fn read_rc(path: &Path) -> Result<i32, NotifyError> {
    parse_rc(&read_rc_raw(path)?)
}

fn read_rc_raw(path: &Path) -> Result<String, NotifyError> {
    std::fs::read_to_string(path).map_err(read_rc_error)
}

fn parse_rc(raw: &str) -> Result<i32, NotifyError> {
    raw.trim().parse::<i32>().map_err(parse_rc_error)
}

fn read_rc_error(err: std::io::Error) -> NotifyError {
    NotifyError::MalformedMetadata(format!("failed to read rc file: {err}"))
}

fn parse_rc_error(err: std::num::ParseIntError) -> NotifyError {
    NotifyError::MalformedMetadata(format!("failed to parse rc file: {err}"))
}

fn resolve_owner(chain: &[CallerIdentity]) -> Result<Option<ResolvedOwner>, NotifyError> {
    let Some(sidecar) = open_sidecar_read_only_optional().map_err(NotifyError::Storage)? else {
        return Ok(None);
    };
    nearest_resolved_owner(&sidecar, chain)
}

fn nearest_resolved_owner(
    sidecar: &PidIdentityDb,
    chain: &[CallerIdentity],
) -> Result<Option<ResolvedOwner>, NotifyError> {
    for entry in chain {
        if let Some(owner) = resolve_owner_for_chain_entry(sidecar, entry)? {
            return Ok(Some(owner));
        }
    }
    Ok(None)
}

fn resolve_owner_for_chain_entry(
    sidecar: &PidIdentityDb,
    entry: &CallerIdentity,
) -> Result<Option<ResolvedOwner>, NotifyError> {
    let Some(row) = lookup_chain_entry_pid_row(sidecar, entry)? else {
        return Ok(None);
    };
    owner_from_pid_row(entry, &row)
}

fn lookup_chain_entry_pid_row(
    sidecar: &PidIdentityDb,
    entry: &CallerIdentity,
) -> Result<Option<PidIdentityRow>, NotifyError> {
    sidecar
        .lookup_by_identity(&entry.identity)
        .map_err(NotifyError::Storage)
}

fn owner_from_pid_row(
    entry: &CallerIdentity,
    row: &PidIdentityRow,
) -> Result<Option<ResolvedOwner>, NotifyError> {
    if let Some(owner) = owner_from_sidecar_session(entry, row) {
        return Ok(Some(owner));
    }
    owner_from_state_invocation(entry, row)
}

fn owner_from_sidecar_session(
    entry: &CallerIdentity,
    row: &PidIdentityRow,
) -> Option<ResolvedOwner> {
    row.session_id.clone().map(|session_id| {
        resolved_owner(entry, row, session_id, OwnerSessionSource::SidecarSessionId)
    })
}

fn owner_from_state_invocation(
    entry: &CallerIdentity,
    row: &PidIdentityRow,
) -> Result<Option<ResolvedOwner>, NotifyError> {
    Ok(resolve_state_invocation_session(row)?.map(|session_id| {
        resolved_owner(
            entry,
            row,
            session_id,
            OwnerSessionSource::StateDbInvocationJoin,
        )
    }))
}

fn resolved_owner(
    entry: &CallerIdentity,
    row: &PidIdentityRow,
    session_id: String,
    source: OwnerSessionSource,
) -> ResolvedOwner {
    ResolvedOwner {
        session_id,
        invocation_uuid: row.invocation_uuid.clone(),
        matched_chain_index: entry.chain_index,
        matched_identity: entry.identity.clone(),
        source,
    }
}

fn resolve_state_invocation_session(row: &PidIdentityRow) -> Result<Option<String>, NotifyError> {
    let Some(state) = open_state_read_only_optional().map_err(NotifyError::Storage)? else {
        return Ok(None);
    };
    let record = invocation_record_for_pid_row(&state, row)?;
    Ok(record.as_ref().and_then(resolved_invocation_session_id))
}

fn invocation_record_for_pid_row(
    state: &StateDb,
    row: &PidIdentityRow,
) -> Result<Option<InvocationRecord>, NotifyError> {
    state
        .get_invocation_by_uuid(&row.invocation_uuid)
        .map_err(NotifyError::Storage)
}

fn resolved_invocation_session_id(record: &InvocationRecord) -> Option<String> {
    record
        .provider_session_id
        .clone()
        .or_else(|| record.session_id.clone())
}

fn open_sidecar_read_only_optional() -> Result<Option<PidIdentityDb>, String> {
    let path = PidIdentityDb::default_path()?;
    if !path.exists() {
        return Ok(None);
    }
    PidIdentityDb::open_read_only(&path).map(Some)
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

fn payload_json(
    metadata: &Value,
    args: &AgentBashCompleteArgs<'_>,
    rc: i32,
    owner: &ResolvedOwner,
    paths: &NotifyPathStrings,
) -> Result<String, NotifyError> {
    render_payload_json(&payload_value(metadata, args, rc, owner, paths))
}

fn payload_value(
    metadata: &Value,
    args: &AgentBashCompleteArgs<'_>,
    rc: i32,
    owner: &ResolvedOwner,
    paths: &NotifyPathStrings,
) -> Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "agent_bash_complete",
        "handle": args.handle,
        "rc": rc,
        "state_dir": paths.state_dir,
        "meta_path": paths.meta_path,
        "log_path": paths.log_path,
        "rc_path": paths.rc_path,
        "owner": {
            "session_id": owner.session_id,
            "invocation_uuid": owner.invocation_uuid,
            "matched_chain_index": owner.matched_chain_index,
        },
        "caller_chain": metadata.get("caller_chain").cloned().unwrap_or(Value::Null),
        "meta": metadata,
    })
}

fn render_payload_json(payload: &Value) -> Result<String, NotifyError> {
    serde_json::to_string(payload)
        .map_err(|err| NotifyError::Storage(format!("failed to serialize payload JSON: {err}")))
}

fn render_notify_success(
    args: &AgentBashCompleteArgs<'_>,
    outcome: NotifyOutcome,
) -> Result<i32, String> {
    let response = notify_success_response(args, outcome);
    render_response(&response, args.json)?;
    Ok(0)
}

fn notify_success_response(
    args: &AgentBashCompleteArgs<'_>,
    outcome: NotifyOutcome,
) -> NotifyResponse {
    match outcome {
        NotifyOutcome::Enqueued { owner, row, wake } => notify_response(
            args,
            "enqueued",
            true,
            Some(&owner),
            Some(row.seq),
            Some(wake),
        ),
        NotifyOutcome::AlreadyEnqueued { owner, row, wake } => notify_response(
            args,
            "already_enqueued",
            true,
            Some(&owner),
            Some(row.seq),
            Some(wake),
        ),
        NotifyOutcome::NoOwner => notify_response(args, "no_owner", false, None, None, None),
    }
}

fn render_notify_error(
    args: &AgentBashCompleteArgs<'_>,
    render: NotifyErrorRender,
) -> Result<i32, String> {
    if args.json {
        render_notify_error_json(args, &render)?;
    } else {
        render_notify_error_human(&render);
    }
    Ok(render.exit_code)
}

fn render_notify_error_human(render: &NotifyErrorRender) {
    if let Some(message) = &render.message {
        eprintln!("{}: {message}", render.status);
    } else {
        eprintln!("{}", render.status);
    }
}

fn render_notify_error_json(
    args: &AgentBashCompleteArgs<'_>,
    render: &NotifyErrorRender,
) -> Result<(), String> {
    println!(
        "{}",
        notify_error_json(render_notify_error_value(args, render)?)?
    );
    Ok(())
}

fn render_notify_error_value(
    args: &AgentBashCompleteArgs<'_>,
    render: &NotifyErrorRender,
) -> Result<Value, String> {
    let mut value = serde_json::to_value(notify_response(
        args,
        render.status,
        false,
        None,
        None,
        None,
    ))
    .map_err(|err| format!("Failed to serialize notify response JSON: {err}"))?;
    if let Some(message) = &render.message {
        value["message"] = Value::String(message.clone());
    }
    if let Some(existing) = &render.existing {
        value["existing"] = serde_json::to_value(existing)
            .map_err(|err| format!("Failed to serialize notify existing row JSON: {err}"))?;
    }
    Ok(value)
}

fn notify_error_json(value: Value) -> Result<String, String> {
    serde_json::to_string_pretty(&value)
        .map_err(|err| format!("Failed to render notify JSON: {err}"))
}

fn notify_response(
    args: &AgentBashCompleteArgs<'_>,
    status: &str,
    enqueued: bool,
    owner: Option<&ResolvedOwner>,
    seq: Option<i64>,
    wake: Option<WakeDiagnostic>,
) -> NotifyResponse {
    NotifyResponse {
        status: status.to_string(),
        enqueued,
        handle: args.handle.to_string(),
        caller_ppid: args.caller_ppid,
        matched_chain_index: owner.map(|owner| owner.matched_chain_index),
        matched_pid: owner.map(|owner| owner.matched_identity.os_pid),
        owner_invocation_uuid: owner.map(|owner| owner.invocation_uuid.clone()),
        owner_session_id: owner.map(|owner| owner.session_id.clone()),
        session_source: owner.map(|owner| owner.source.as_str().to_string()),
        seq,
        wake,
    }
}

fn render_response(response: &NotifyResponse, json: bool) -> Result<(), String> {
    if json {
        render_response_json(response)?;
    } else {
        render_response_human(response);
    }
    Ok(())
}

fn render_response_json(response: &NotifyResponse) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(response)
            .map_err(|err| format!("Failed to render notify JSON: {err}"))?
    );
    Ok(())
}

fn render_response_human(response: &NotifyResponse) {
    println!(
        "{} handle={} session={}",
        response.status,
        response.handle,
        response.owner_session_id.as_deref().unwrap_or("-")
    );
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
