//! Durable process-completion event registration and delivery.
//!
//! ## Declared roles
//!
//! `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `validator`

use oulipoly_state::mailbox::{
    CompletionEventListenerRow, CompletionEventRegistrationInput,
    CompletionEventRegistrationResult, CompletionEventRow, CompletionEventTriggerInput,
    CompletionEventTriggerResult, MailboxDb, MailboxRow,
};
use oulipoly_state::pid_identity::{PidIdentityDb, ProcessIdentity, read_live_process_identity};
use oulipoly_state::{InvocationRecord, InvocationStatus, StateDb};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

use crate::mailbox_delivery::PtyMailboxDeliveryDiagnostic;
use crate::wake_coordinator::WakeDiagnostic;

#[derive(Debug, Clone, Copy)]
pub(crate) struct AgentBashRegisterArgs<'a> {
    pub handle: &'a str,
    pub delivery_mode: &'a str,
    pub state_dir: &'a Path,
    pub meta: &'a Path,
    pub log: &'a Path,
    pub rc: &'a Path,
    pub json: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AgentBashActivateArgs<'a> {
    pub handle: &'a str,
    pub json: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AgentBashCompleteArgs<'a> {
    pub caller_ppid: u32,
    pub handle: &'a str,
    pub state_dir: &'a Path,
    pub meta: &'a Path,
    pub log: &'a Path,
    pub rc: &'a Path,
    pub consumed: bool,
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct RegistrationResponse {
    status: String,
    event_id: String,
    event_state: String,
    listener_count: usize,
    owner_session_id: Option<String>,
    owner_invocation_uuid: Option<String>,
}

#[derive(Debug, Serialize)]
struct ActivateResponse {
    status: String,
    event_id: String,
    event_state: String,
    listener_count: usize,
    pty_delivery: Vec<PtyMailboxDeliveryDiagnostic>,
}

#[derive(Debug, Serialize)]
struct NotifyResponse {
    status: String,
    enqueued: bool,
    handle: String,
    caller_ppid: u32,
    matched_chain_index: Option<usize>,
    matched_pid: Option<i64>,
    owner_invocation_uuid: Option<String>,
    owner_generation_uuid: Option<String>,
    owner_session_id: Option<String>,
    session_source: Option<String>,
    seq: Option<i64>,
    pty_delivery: Option<PtyMailboxDeliveryDiagnostic>,
    pty_deliveries: Vec<PtyMailboxDeliveryDiagnostic>,
    payload_file_path: Option<String>,
    payload_sha256: Option<String>,
    payload_byte_len: Option<i64>,
    payload_retention_policy: Option<String>,
    wake: Option<WakeDiagnostic>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    status: String,
    handle: String,
    message: String,
}

#[derive(Debug, Clone)]
struct OwnerBinding {
    session_id: String,
    invocation_uuid: String,
}

#[derive(Debug, Deserialize)]
struct CallerIdentity {
    pid: i64,
    boot_id: String,
    starttime_ticks: i64,
}

impl CallerIdentity {
    fn process_identity(self) -> ProcessIdentity {
        ProcessIdentity {
            os_pid: self.pid,
            os_boot_id: self.boot_id,
            os_pid_starttime_ticks: self.starttime_ticks,
        }
    }
}

#[derive(Debug, Clone)]
struct NotifyPathStrings {
    state_dir: String,
    meta_path: String,
    log_path: String,
    rc_path: String,
}

pub(crate) fn run_agent_bash_register(args: AgentBashRegisterArgs<'_>) -> Result<i32, String> {
    match register_completion_event(&args) {
        Ok(result) => {
            let owner = result.listeners.first();
            render(
                &RegistrationResponse {
                    status: if result.inserted {
                        "registered".to_string()
                    } else {
                        "already_registered".to_string()
                    },
                    event_id: result.event.event_id,
                    event_state: result.event.state,
                    listener_count: result.listeners.len(),
                    owner_session_id: owner.map(|listener| listener.session_id.clone()),
                    owner_invocation_uuid: owner
                        .map(|listener| listener.owner_invocation_uuid.clone()),
                },
                args.json,
            )?;
            Ok(0)
        }
        Err(message) => render_error(args.handle, args.json, message),
    }
}

pub(crate) fn run_agent_bash_activate(args: AgentBashActivateArgs<'_>) -> Result<i32, String> {
    match activate_completion_event(&args) {
        Ok((result, pty_delivery)) => {
            render(
                &ActivateResponse {
                    status: "activated".to_string(),
                    event_id: result.event.event_id,
                    event_state: result.event.state,
                    listener_count: result.listeners.len(),
                    pty_delivery,
                },
                args.json,
            )?;
            Ok(0)
        }
        Err(message) => render_error(args.handle, args.json, message),
    }
}

pub(crate) fn run_agent_bash_complete(args: AgentBashCompleteArgs<'_>) -> Result<i32, String> {
    match trigger_completion_event(&args) {
        Ok((result, pty_deliveries, wake)) => {
            let owner = result.listeners.first();
            let row = result.mailbox_rows.first();
            let pty_delivery = pty_deliveries.first().cloned();
            render(
                &NotifyResponse {
                    status: if result.triggered {
                        "triggered".to_string()
                    } else {
                        "already_triggered".to_string()
                    },
                    enqueued: !result.mailbox_rows.is_empty(),
                    handle: args.handle.to_string(),
                    caller_ppid: args.caller_ppid,
                    matched_chain_index: None,
                    matched_pid: None,
                    owner_invocation_uuid: owner
                        .map(|listener| listener.owner_invocation_uuid.clone()),
                    owner_generation_uuid: None,
                    owner_session_id: owner.map(|listener| listener.session_id.clone()),
                    session_source: owner.map(|_| "completion_event_listener".to_string()),
                    seq: row.map(|row| row.seq),
                    pty_delivery,
                    pty_deliveries,
                    payload_file_path: row.and_then(|row| row.payload_file_path.clone()),
                    payload_sha256: row.and_then(|row| row.payload_sha256.clone()),
                    payload_byte_len: row.and_then(|row| row.payload_byte_len),
                    payload_retention_policy: row
                        .and_then(|row| row.payload_retention_policy.clone()),
                    wake,
                },
                args.json,
            )?;
            Ok(0)
        }
        Err(message) => render_error(args.handle, args.json, message),
    }
}

fn register_completion_event(
    args: &AgentBashRegisterArgs<'_>,
) -> Result<CompletionEventRegistrationResult, String> {
    let metadata = read_metadata(args.meta)?;
    let owner = parse_owner_binding(&metadata)?;
    validate_owner_binding(&owner, &metadata)?;
    let paths = notify_path_strings(args.state_dir, args.meta, args.log, args.rc);
    let mut mailbox = MailboxDb::open_default()?;
    mailbox.register_completion_event(CompletionEventRegistrationInput {
        event_id: args.handle,
        delivery_mode: args.delivery_mode,
        owner_session_id: Some(owner.session_id.as_str()),
        owner_invocation_uuid: Some(owner.invocation_uuid.as_str()),
        state_dir: &paths.state_dir,
        meta_path: &paths.meta_path,
        log_path: &paths.log_path,
        rc_path: &paths.rc_path,
    })
}

fn activate_completion_event(
    args: &AgentBashActivateArgs<'_>,
) -> Result<
    (
        CompletionEventTriggerResult,
        Vec<PtyMailboxDeliveryDiagnostic>,
    ),
    String,
> {
    let mut mailbox = MailboxDb::open_default()?;
    let result = mailbox.activate_completion_event_listeners(args.handle)?;
    let delivery = deliver_event_listeners(&mut mailbox, &result.mailbox_rows);
    Ok((result, delivery))
}

fn trigger_completion_event(
    args: &AgentBashCompleteArgs<'_>,
) -> Result<
    (
        CompletionEventTriggerResult,
        Vec<PtyMailboxDeliveryDiagnostic>,
        Option<WakeDiagnostic>,
    ),
    String,
> {
    let metadata = read_metadata(args.meta)?;
    let rc = read_rc(args.rc)?;
    let paths = notify_path_strings(args.state_dir, args.meta, args.log, args.rc);
    let mut mailbox = MailboxDb::open_default()?;
    let event = mailbox
        .completion_event(args.handle)?
        .ok_or_else(|| format!("Completion event {} is not registered", args.handle))?;
    let listeners = mailbox.completion_event_listeners(args.handle)?;
    let payload_json = render_payload_json(&payload_value(
        &metadata,
        args.handle,
        rc,
        &event,
        &listeners,
        &paths,
    ))?;
    let result = mailbox.trigger_completion_event(CompletionEventTriggerInput {
        event_id: args.handle,
        payload_json: &payload_json,
        state_dir: &paths.state_dir,
        meta_path: &paths.meta_path,
        log_path: &paths.log_path,
        rc_path: &paths.rc_path,
        rc,
        consumed: args.consumed,
    })?;
    let (delivery, wake) = deliver_and_wake_event_listeners(&mut mailbox, &result.mailbox_rows);
    Ok((result, delivery, wake))
}

fn deliver_and_wake_event_listeners(
    mailbox: &mut MailboxDb,
    rows: &[MailboxRow],
) -> (Vec<PtyMailboxDeliveryDiagnostic>, Option<WakeDiagnostic>) {
    let mut deliveries = Vec::new();
    let mut first_wake = None;
    for session_id in undelivered_listener_session_ids(rows) {
        let delivery = crate::mailbox_delivery::attempt_pty_mailbox_delivery_with_trigger(
            mailbox,
            &session_id,
            "completion-event",
        );
        let wake = wake_after_unsubmitted_delivery(&session_id, &delivery);
        if deliveries.is_empty() {
            first_wake = wake;
        }
        deliveries.push(delivery);
    }
    (deliveries, first_wake)
}

fn wake_after_unsubmitted_delivery(
    session_id: &str,
    delivery: &PtyMailboxDeliveryDiagnostic,
) -> Option<WakeDiagnostic> {
    (!delivery.submitted && delivery.status != "paused")
        .then(|| crate::wake_coordinator::trigger_notify_wake(session_id))
}

fn deliver_event_listeners(
    mailbox: &mut MailboxDb,
    rows: &[MailboxRow],
) -> Vec<PtyMailboxDeliveryDiagnostic> {
    undelivered_listener_session_ids(rows)
        .into_iter()
        .map(|session_id| {
            crate::mailbox_delivery::attempt_pty_mailbox_delivery_with_trigger(
                mailbox,
                &session_id,
                "completion-event",
            )
        })
        .collect()
}

fn undelivered_listener_session_ids(rows: &[MailboxRow]) -> BTreeSet<String> {
    rows.iter()
        .filter(|row| row.delivered_at.is_none())
        .map(|row| row.session_id.clone())
        .collect()
}

fn parse_owner_binding(metadata: &Value) -> Result<OwnerBinding, String> {
    let session_id = optional_nonempty_string(metadata, "owner_session_id")?;
    let invocation_uuid = optional_nonempty_string(metadata, "owner_invocation_uuid")?;
    match (session_id, invocation_uuid) {
        (Some(session_id), Some(invocation_uuid)) => Ok(OwnerBinding {
            session_id,
            invocation_uuid,
        }),
        _ => Err(
            "meta.json owner_session_id and owner_invocation_uuid are both required".to_string(),
        ),
    }
}

fn optional_nonempty_string(metadata: &Value, field: &str) -> Result<Option<String>, String> {
    let Some(value) = metadata.get(field) else {
        return Ok(None);
    };
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .map(Some)
        .ok_or_else(|| format!("meta.json {field} must be a non-empty string"))
}

fn validate_owner_binding(owner: &OwnerBinding, metadata: &Value) -> Result<(), String> {
    let path = StateDb::default_path()?;
    if !path.exists() {
        return Err("State DB is unavailable for completion listener validation".to_string());
    }
    let state = StateDb::open_read_only(&path)
        .map_err(|err| format!("Failed to open state DB read-only: {err:?}"))?;
    let record = state
        .get_invocation_by_uuid(&owner.invocation_uuid)?
        .ok_or_else(|| {
            format!(
                "Completion listener invocation {} does not exist",
                owner.invocation_uuid
            )
        })?;
    match resolved_invocation_session_id(&record) {
        Some(session_id) if session_id == owner.session_id => return Ok(()),
        Some(_) => return Err(owner_binding_error(owner)),
        None => {}
    }
    if record.status == InvocationStatus::Running && running_owner_binding_is_live(owner, metadata)?
    {
        return Ok(());
    }
    Err(owner_binding_error(owner))
}

fn resolved_invocation_session_id(record: &InvocationRecord) -> Option<String> {
    record
        .provider_session_id
        .clone()
        .or_else(|| record.session_id.clone())
}

fn running_owner_binding_is_live(owner: &OwnerBinding, metadata: &Value) -> Result<bool, String> {
    let caller_chain = metadata
        .get("caller_chain")
        .ok_or_else(|| "meta.json must contain caller_chain for a running owner".to_string())?;
    let callers: Vec<CallerIdentity> = serde_json::from_value(caller_chain.clone())
        .map_err(|err| format!("meta.json caller_chain is invalid: {err}"))?;
    let sidecar_path = PidIdentityDb::default_path()?;
    if !sidecar_path.exists() {
        return Ok(false);
    }
    let sidecar = PidIdentityDb::open_read_only(&sidecar_path)?;
    for caller in callers {
        let identity = caller.process_identity();
        let Some(row) = sidecar.lookup_by_identity(&identity)? else {
            continue;
        };
        if row.invocation_uuid != owner.invocation_uuid
            || row.session_id.as_deref() != Some(owner.session_id.as_str())
        {
            continue;
        }
        if read_live_process_identity(identity.os_pid)?.as_ref() == Some(&identity) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn owner_binding_error(owner: &OwnerBinding) -> String {
    format!(
        "Completion listener session {} is not bound to invocation {}",
        owner.session_id, owner.invocation_uuid
    )
}

fn payload_value(
    metadata: &Value,
    handle: &str,
    rc: i32,
    event: &CompletionEventRow,
    listeners: &[CompletionEventListenerRow],
    paths: &NotifyPathStrings,
) -> Value {
    serde_json::json!({
        "schema_version": 2,
        "kind": "agent_bash_complete",
        "event_id": event.event_id,
        "handle": handle,
        "rc": rc,
        "state_dir": paths.state_dir,
        "meta_path": paths.meta_path,
        "log_path": paths.log_path,
        "rc_path": paths.rc_path,
        "listeners": listeners.iter().map(|listener| serde_json::json!({
            "listener_id": listener.listener_id,
            "session_id": listener.session_id,
            "invocation_uuid": listener.owner_invocation_uuid,
        })).collect::<Vec<_>>(),
        "meta": metadata,
    })
}

fn read_metadata(path: &Path) -> Result<Value, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|err| format!("failed to read meta.json: {err}"))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|err| format!("failed to parse meta.json: {err}"))?;
    if !value.is_object() {
        return Err("meta.json must contain a JSON object".to_string());
    }
    Ok(value)
}

fn read_rc(path: &Path) -> Result<i32, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|err| format!("failed to read rc file: {err}"))?;
    raw.trim()
        .parse::<i32>()
        .map_err(|err| format!("failed to parse rc file: {err}"))
}

fn render_payload_json(payload: &Value) -> Result<String, String> {
    serde_json::to_string(payload)
        .map_err(|err| format!("failed to serialize completion event payload: {err}"))
}

fn notify_path_strings(state_dir: &Path, meta: &Path, log: &Path, rc: &Path) -> NotifyPathStrings {
    NotifyPathStrings {
        state_dir: path_string(state_dir),
        meta_path: path_string(meta),
        log_path: path_string(log),
        rc_path: path_string(rc),
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn render<T: Serialize>(response: &T, json: bool) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(response)
                .map_err(|err| format!("Failed to render notify JSON: {err}"))?
        );
    } else {
        println!(
            "{}",
            serde_json::to_string(response)
                .map_err(|err| format!("Failed to render notify response: {err}"))?
        );
    }
    Ok(())
}

fn render_error(handle: &str, json: bool, message: String) -> Result<i32, String> {
    render(
        &ErrorResponse {
            status: "notification_event_error".to_string(),
            handle: handle.to_string(),
            message,
        },
        json,
    )?;
    Ok(74)
}
