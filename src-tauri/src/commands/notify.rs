//! ## Declared roles
//!
//! `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`

use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, EnqueueResult, MailboxDb, MailboxRow};
use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRow, ProcessIdentity};
use oulipoly_state::{InvocationRecord, StateDb};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

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
        Err(NotifyError::MalformedMetadata(message)) => {
            render_notify_error(&args, 64, "malformed_metadata", Some(message), None)
        }
        Err(NotifyError::Storage(message)) => {
            render_notify_error(&args, 74, "storage_error", Some(message), None)
        }
        Err(NotifyError::Conflict { existing }) => {
            let message = format!("existing handle belongs to session {}", existing.session_id);
            render_notify_error(
                &args,
                73,
                "idempotency_conflict",
                Some(message),
                Some(&existing),
            )
        }
    }
}

enum NotifyOutcome {
    Enqueued {
        owner: ResolvedOwner,
        row: MailboxRow,
    },
    AlreadyEnqueued {
        owner: ResolvedOwner,
        row: MailboxRow,
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
    let metadata = read_metadata(args.meta)?;
    let caller_chain = parse_caller_chain(&metadata)?;
    let rc = read_rc(args.rc)?;
    let Some(owner) = resolve_owner(&caller_chain)? else {
        return Ok(NotifyOutcome::NoOwner);
    };
    let payload_json = payload_json(&metadata, args, rc, &owner)?;
    let enqueue = AgentBashCompleteEnqueue {
        session_id: &owner.session_id,
        handle: args.handle,
        payload_json: &payload_json,
        owner_invocation_uuid: Some(&owner.invocation_uuid),
        matched_os_pid: Some(owner.matched_identity.os_pid),
        matched_os_boot_id: Some(&owner.matched_identity.os_boot_id),
        matched_os_pid_starttime_ticks: Some(owner.matched_identity.os_pid_starttime_ticks),
        matched_chain_index: Some(owner.matched_chain_index as i64),
        state_dir: &path_string(args.state_dir),
        meta_path: &path_string(args.meta),
        log_path: &path_string(args.log),
        rc_path: &path_string(args.rc),
        rc,
    };
    let mut mailbox = MailboxDb::open_default().map_err(NotifyError::Storage)?;
    match mailbox
        .enqueue_agent_bash_complete(&enqueue)
        .map_err(NotifyError::Storage)?
    {
        EnqueueResult::Inserted(row) => Ok(NotifyOutcome::Enqueued { owner, row }),
        EnqueueResult::AlreadyEnqueued(row) => Ok(NotifyOutcome::AlreadyEnqueued { owner, row }),
        EnqueueResult::Conflict { existing } => Err(NotifyError::Conflict {
            existing: Box::new(existing),
        }),
    }
}

fn read_metadata(path: &Path) -> Result<Value, NotifyError> {
    let raw = std::fs::read_to_string(path).map_err(|err| {
        NotifyError::MalformedMetadata(format!("failed to read meta.json: {err}"))
    })?;
    serde_json::from_str(&raw)
        .map_err(|err| NotifyError::MalformedMetadata(format!("failed to parse meta.json: {err}")))
}

fn parse_caller_chain(metadata: &Value) -> Result<Vec<CallerIdentity>, NotifyError> {
    let Some(chain) = metadata.get("caller_chain").and_then(Value::as_array) else {
        return Err(NotifyError::MalformedMetadata(
            "meta.json must contain caller_chain array".to_string(),
        ));
    };
    if chain.is_empty() {
        return Err(NotifyError::MalformedMetadata(
            "meta.json caller_chain must not be empty".to_string(),
        ));
    }
    chain
        .iter()
        .enumerate()
        .map(parse_caller_identity)
        .collect()
}

fn parse_caller_identity(
    (chain_index, value): (usize, &Value),
) -> Result<CallerIdentity, NotifyError> {
    let pid = integer_field(value, &["pid", "os_pid"])?;
    let starttime_ticks = integer_field(value, &["starttime_ticks", "os_pid_starttime_ticks"])?;
    let boot_id = string_field(value, &["boot_id", "os_boot_id"])?;
    Ok(CallerIdentity {
        chain_index,
        identity: ProcessIdentity {
            os_pid: pid,
            os_boot_id: boot_id,
            os_pid_starttime_ticks: starttime_ticks,
        },
    })
}

fn integer_field(value: &Value, names: &[&str]) -> Result<i64, NotifyError> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_i64))
        .ok_or_else(|| {
            NotifyError::MalformedMetadata(format!("caller_chain entry missing integer {names:?}"))
        })
}

fn string_field(value: &Value, names: &[&str]) -> Result<String, NotifyError> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            NotifyError::MalformedMetadata(format!("caller_chain entry missing string {names:?}"))
        })
}

fn read_rc(path: &Path) -> Result<i32, NotifyError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|err| NotifyError::MalformedMetadata(format!("failed to read rc file: {err}")))?;
    raw.trim()
        .parse::<i32>()
        .map_err(|err| NotifyError::MalformedMetadata(format!("failed to parse rc file: {err}")))
}

fn resolve_owner(chain: &[CallerIdentity]) -> Result<Option<ResolvedOwner>, NotifyError> {
    let Some(sidecar) = open_sidecar_read_only_optional().map_err(NotifyError::Storage)? else {
        return Ok(None);
    };
    for entry in chain {
        let Some(row) = sidecar
            .lookup_by_identity(&entry.identity)
            .map_err(NotifyError::Storage)?
        else {
            continue;
        };
        if let Some(session_id) = row.session_id.clone() {
            return Ok(Some(resolved_owner(
                entry,
                &row,
                session_id,
                OwnerSessionSource::SidecarSessionId,
            )));
        }
        if let Some(session_id) = resolve_state_invocation_session(&row)? {
            return Ok(Some(resolved_owner(
                entry,
                &row,
                session_id,
                OwnerSessionSource::StateDbInvocationJoin,
            )));
        }
    }
    Ok(None)
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
    let record = state
        .get_invocation_by_uuid(&row.invocation_uuid)
        .map_err(NotifyError::Storage)?;
    Ok(record.as_ref().and_then(resolved_invocation_session_id))
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
) -> Result<String, NotifyError> {
    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "agent_bash_complete",
        "handle": args.handle,
        "rc": rc,
        "state_dir": path_string(args.state_dir),
        "meta_path": path_string(args.meta),
        "log_path": path_string(args.log),
        "rc_path": path_string(args.rc),
        "owner": {
            "session_id": owner.session_id,
            "invocation_uuid": owner.invocation_uuid,
            "matched_chain_index": owner.matched_chain_index,
        },
        "caller_chain": metadata.get("caller_chain").cloned().unwrap_or(Value::Null),
        "meta": metadata,
    });
    serde_json::to_string(&payload)
        .map_err(|err| NotifyError::Storage(format!("failed to serialize payload JSON: {err}")))
}

fn render_notify_success(
    args: &AgentBashCompleteArgs<'_>,
    outcome: NotifyOutcome,
) -> Result<i32, String> {
    match outcome {
        NotifyOutcome::Enqueued { owner, row } => {
            let response = notify_response(args, "enqueued", true, Some(&owner), Some(row.seq));
            render_response(&response, args.json)?;
            Ok(0)
        }
        NotifyOutcome::AlreadyEnqueued { owner, row } => {
            let response =
                notify_response(args, "already_enqueued", true, Some(&owner), Some(row.seq));
            render_response(&response, args.json)?;
            Ok(0)
        }
        NotifyOutcome::NoOwner => {
            let response = notify_response(args, "no_owner", false, None, None);
            render_response(&response, args.json)?;
            Ok(0)
        }
    }
}

fn render_notify_error(
    args: &AgentBashCompleteArgs<'_>,
    exit_code: i32,
    status: &str,
    message: Option<String>,
    existing: Option<&MailboxRow>,
) -> Result<i32, String> {
    if args.json {
        let mut value = serde_json::to_value(notify_response(args, status, false, None, None))
            .map_err(|err| format!("Failed to serialize notify response JSON: {err}"))?;
        if let Some(message) = message {
            value["message"] = Value::String(message);
        }
        if let Some(existing) = existing {
            value["existing"] = serde_json::to_value(existing)
                .map_err(|err| format!("Failed to serialize notify existing row JSON: {err}"))?;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|err| format!("Failed to render notify JSON: {err}"))?
        );
    } else if let Some(message) = message {
        eprintln!("{status}: {message}");
    } else {
        eprintln!("{status}");
    }
    Ok(exit_code)
}

fn notify_response(
    args: &AgentBashCompleteArgs<'_>,
    status: &str,
    enqueued: bool,
    owner: Option<&ResolvedOwner>,
    seq: Option<i64>,
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
    }
}

fn render_response(response: &NotifyResponse, json: bool) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(response)
                .map_err(|err| format!("Failed to render notify JSON: {err}"))?
        );
    } else {
        println!(
            "{} handle={} session={}",
            response.status,
            response.handle,
            response.owner_session_id.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
