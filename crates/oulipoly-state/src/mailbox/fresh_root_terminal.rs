// Private v30 terminal readback. Execution, recipient notification and caller
// presentation have distinct authorities; this module never launches work,
// transmits F to a recipient, or publishes output to a caller.
use serde::{Deserialize, Serialize};

const FRESH_ROOT_TERMINAL_VERIFY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshPhysicalTerminal {
    pub grant_id: String,
    pub work_id: String,
    pub plan_sha256: String,
    pub wait_status: i32,
    pub outcome: String,
    pub cancelled: bool,
    pub stdout_sha256: String,
    pub stdout_len: u64,
    pub stderr_sha256: String,
    pub stderr_len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootTerminalExecution {
    pub handoff_id: String,
    pub d_key: String,
    pub invocation_uuid: String,
    pub session_id: String,
    pub root_id: String,
    pub owner_generation: String,
    pub actor: FreshRecipientIdentity,
    pub parent: FreshPhysicalTerminal,
    pub child_request_id: Option<String>,
    pub child_event: Option<FreshBashSourceEvent>,
    pub outcome: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootTerminalReadback {
    pub handoff_id: String,
    pub d_key: String,
    pub invocation_uuid: String,
    pub session_id: String,
    pub root_id: String,
    pub owner_generation: String,
    pub actor: FreshRecipientIdentity,
    pub execution: Option<FreshRootTerminalExecution>,
    pub execution_state: String,
    pub terminal_state: String,
    pub notification_state: String,
    pub notification_origin: String,
    pub native_receipt_state: String,
    pub listener_policy: Option<String>,
    pub child_request_id: Option<String>,
    pub unresolved_child_request_ids: Vec<String>,
    pub mailbox_seq: Option<i64>,
    pub delivery_request_id: Option<String>,
    pub delivery_grant_id: Option<String>,
    pub delivery_payload_sha256: Option<String>,
    pub delivery_payload_byte_len: Option<i64>,
    pub ack_basis: Option<String>,
    pub publication_state: String,
    pub publication_sha256: Option<String>,
    pub unknown_stage: Option<String>,
    pub unknown_stages: Vec<String>,
    pub refusal: Option<String>,
    pub artifacts: Vec<String>,
}

impl FreshRootTerminalReadback {
    fn record_unknown(&mut self, stage: String) {
        self.unknown_stage = Some(stage.clone());
        self.unknown_stages.push(stage);
    }
}

fn physical_root_outcome(
    parent: &FreshPhysicalTerminal,
    child: Option<&FreshBashSourceEvent>,
) -> &'static str {
    if !parent.cancelled
        && libc::WIFEXITED(parent.wait_status)
        && libc::WEXITSTATUS(parent.wait_status) == 0
        && child.is_none_or(|event| {
            !event.cancelled
                && libc::WIFEXITED(event.wait_status)
                && libc::WEXITSTATUS(event.wait_status) == 0
        })
    {
        "success"
    } else {
        "failure"
    }
}

fn physical_provider_outcome(status: i32, cancelled: bool) -> &'static str {
    if cancelled {
        "cancelled"
    } else if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
        "exit_success"
    } else if libc::WIFEXITED(status) {
        "exit_failure"
    } else if libc::WIFSIGNALED(status) {
        "signaled"
    } else {
        "unknown"
    }
}

fn fresh_root_terminal_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE (type='table' AND name IN
         ('fresh_root_terminal','fresh_root_publication')) OR (type='trigger' AND name IN
         ('fresh_root_terminal_no_update','fresh_root_terminal_no_delete',
          'fresh_root_publication_no_update','fresh_root_publication_no_delete'))",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

fn verify_fresh_root_terminal_schema(state: &Connection) -> Result<(), String> {
    if fresh_root_terminal_schema_count(state)? != 6 {
        return Err("fresh root terminal schema incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_ROOT_TERMINAL_SCHEMA,
        "fresh_root_terminal",
        &[
            "fresh_root_publication",
            "fresh_root_terminal_no_update",
            "fresh_root_terminal_no_delete",
            "fresh_root_publication_no_update",
            "fresh_root_publication_no_delete",
        ],
    )
}

impl FreshV30Lane {
    fn physical_root_terminal(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<FreshPhysicalTerminal, String> {
        let directory = self
            .state_path
            .parent()
            .ok_or("fresh State parent absent")?
            .join(FRESH_PROVIDER_DIRECTORY);
        let grant = physical_json(&directory, &format!("{}.fresh-grant.json", root.handoff_id))?;
        let id = grant["id"].as_str().ok_or("parent K id absent")?;
        validate_request_id(id)?;
        let b = &grant["binding"];
        if grant["version"] != 3
            || b["root_id"] != root.old_release.prepared.root_id
            || b["handoff_id"] != root.handoff_id
            // The core provider binding leaves grant_key absent for a root K;
            // its effective key is handoff_id. A child K sets a distinct key.
            || !(b["grant_key"].is_null() || b["grant_key"] == root.handoff_id)
            || b["invocation_uuid"] != root.invocation_uuid
            || b["session_id"] != session.session_id
            || b["owner_generation"] != root.old_release.prepared.owner_generation
            || b["actor_pid"] != actor.host_pid
            || b["actor_starttime"] != actor.starttime_ticks
            || b["actor_boot_id"] != actor.boot_id
            || b["actor_pidns_dev"] != actor.pidns_dev
            || b["actor_pidns_ino"] != actor.pidns_ino
            || b["root_pid"] != root.old_release.prepared.root_init.host_pid
            || b["root_starttime"] != root.old_release.prepared.root_init.starttime_ticks
            || b["root_pidns_dev"] != root.old_release.prepared.root_init.pidns_dev
            || b["root_pidns_ino"] != root.old_release.prepared.root_init.pidns_ino
            || !b["causal_parent"].is_null()
        {
            return Err("parent K root/J/actor binding conflict".into());
        }
        if physical_json(&directory, &format!("{id}.consumed.json"))? != grant {
            return Err("parent K consumption absent or changed".into());
        }
        let attach = physical_json(&directory, &format!("{id}.attach.json"))?;
        let work = attach["work_id"].as_str().ok_or("parent work id absent")?;
        validate_request_id(work)?;
        let exit = physical_json(&directory, &format!("{id}.exit.json"))?;
        let drain = physical_json(&directory, &format!("{id}.drain.json"))?;
        let wait = physical_json(&directory, &format!("{id}.pid1-wait.json"))?;
        let status = exit["wait_status"]
            .as_i64()
            .ok_or("parent exit status absent")?;
        let status = i32::try_from(status).map_err(|_| "parent exit status overflow")?;
        if attach["version"] != 1
            || attach["grant_id"] != id
            || exit["version"] != 1
            || exit["grant_id"] != id
            || exit["work_id"] != work
            || exit["provider_local_pid"] != attach["provider_local_pid"]
            || drain["version"] != 1
            || drain["grant_id"] != id
            || drain["work_id"] != work
            || drain["zero_remaining"] != true
            || wait["version"] != 1
            || wait["grant_id"] != id
            || wait["work_id"] != work
            || wait["reaped"] != true
            || wait["pid1_parent_namespace_pid"] != attach["pid1_parent_namespace_pid"]
            || wait["wait_status"]
                .as_i64()
                .is_none_or(|s| !libc::WIFEXITED(s as i32) || libc::WEXITSTATUS(s as i32) != 0)
        {
            return Err("parent physical Q incomplete or changed".into());
        }
        let (stdout_sha256, stdout_len) = self.terminal_output(&directory, id, "stdout", &drain)?;
        let (stderr_sha256, stderr_len) = self.terminal_output(&directory, id, "stderr", &drain)?;
        let plan = grant["plan_sha256"]
            .as_str()
            .ok_or("parent selected plan absent")?;
        if plan.len() != 64
            || !plan
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("parent selected plan digest invalid".into());
        }
        Ok(FreshPhysicalTerminal {
            grant_id: id.into(),
            work_id: work.into(),
            plan_sha256: plan.into(),
            wait_status: status,
            outcome: physical_provider_outcome(status, drain["cancelled"] == true).into(),
            cancelled: drain["cancelled"] == true,
            stdout_sha256,
            stdout_len,
            stderr_sha256,
            stderr_len,
        })
    }

    fn terminal_output(
        &self,
        directory: &Path,
        grant: &str,
        stream: &str,
        drain: &serde_json::Value,
    ) -> Result<(String, u64), String> {
        let expected = &drain[stream];
        let sha = expected["sha256"]
            .as_str()
            .ok_or("parent output digest absent")?;
        let len = expected["bytes"]
            .as_u64()
            .ok_or("parent output length absent")?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.join(format!("{grant}.{stream}")))
            .map_err(|e| format!("parent {stream} absent: {e}"))?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file()
            || meta.uid() != 0
            || meta.nlink() != 1
            || expected["device"] != meta.dev()
            || expected["inode"] != meta.ino()
            || meta.len() != len
        {
            return Err(format!("parent {stream} inode/length changed"));
        }
        let mut digest = Sha256::new();
        let mut count = 0u64;
        let mut buffer = [0u8; FRESH_ROOT_TERMINAL_VERIFY_BUFFER_BYTES];
        loop {
            let size = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if size == 0 {
                break;
            }
            count = count
                .checked_add(size as u64)
                .ok_or("parent output length overflow")?;
            digest.update(&buffer[..size]);
        }
        if count != len || format!("{:x}", digest.finalize()) != sha {
            return Err(format!("parent {stream} bytes changed"));
        }
        Ok((sha.into(), len))
    }

    fn root_child_requests(&self, root_id: &str) -> Result<(Option<String>, Vec<String>), String> {
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut rows = state
            .prepare("SELECT c.request_id, e.request_id IS NOT NULL FROM fresh_bash_child c
                      LEFT JOIN fresh_bash_selected_event e ON e.request_id=c.request_id
                      WHERE c.root_id=?1 ORDER BY c.request_id")
            .map_err(|e| e.to_string())?;
        let ids = rows
            .query_map([root_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let selected: Vec<_> = ids.iter().filter(|(_, has_w)| *has_w).collect();
        if selected.len() > 1 {
            return Err("multiple accepted child W rows under root".into());
        }
        let selected_id = selected.first().map(|(id, _)| id.clone());
        let unresolved = ids.into_iter().filter_map(|(id, _)|
            (Some(&id) != selected_id.as_ref()).then_some(id)).collect();
        Ok((selected_id, unresolved))
    }

    /// Record complete physical work only. A missing parent Q or child W is
    /// read back as unknown; it never authorizes a second K.
    pub fn settle_private_root_terminal(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<FreshRootTerminalReadback, String> {
        self.require_released_invocation(root, actor, session)?;
        let parent = match self.physical_root_terminal(root, actor, session) {
            Ok(parent) => parent,
            Err(_) => return self.read_private_root_terminal(root, actor, session),
        };
        let (child_request_id, unresolved) =
            self.root_child_requests(&root.old_release.prepared.root_id)?;
        if child_request_id.is_none() && !unresolved.is_empty() {
            return self.read_private_root_terminal(root, actor, session);
        }
        let child_event = match &child_request_id {
            Some(id) => match self.selected_private_bash_event(id) {
                Ok(event) => Some(event),
                Err(_) => return self.read_private_root_terminal(root, actor, session),
            },
            None => None,
        };
        let outcome = physical_root_outcome(&parent, child_event.as_ref());
        let execution = FreshRootTerminalExecution {
            handoff_id: root.handoff_id.clone(),
            d_key: root.d_key.clone(),
            invocation_uuid: root.invocation_uuid.clone(),
            session_id: session.session_id.clone(),
            root_id: root.old_release.prepared.root_id.clone(),
            owner_generation: root.old_release.prepared.owner_generation.clone(),
            actor: actor.clone(),
            parent,
            child_request_id,
            child_event,
            outcome: outcome.into(),
        };
        let encoded = serde_json::to_string(&execution).map_err(|e| e.to_string())?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        state.execute(
            "INSERT OR IGNORE INTO fresh_root_terminal
             (handoff_id,invocation_uuid,session_id,root_id,owner_generation,actor_identity,execution_json,recorded_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![root.handoff_id,root.invocation_uuid,session.session_id,
                root.old_release.prepared.root_id,root.old_release.prepared.owner_generation,
                Self::recipient_identity_json(actor)?,encoded,Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        self.read_private_root_terminal(root, actor, session)
    }

    /// Explicit recovery visits only retained receipts. It never starts a
    /// provider K, retransmits F, or infers ACK from a sidecar row.
    pub fn repair_private_root_terminal(
        &mut self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<FreshRootTerminalReadback, String> {
        self.require_released_invocation(root, actor, session)?;
        let (selected, unresolved) =
            self.root_child_requests(&root.old_release.prepared.root_id)?;
        for id in selected.iter().chain(unresolved.iter()) {
            self.repair_captured_private_bash_source(id)?;
        }
        if let Some(id) = self.root_child_requests(&root.old_release.prepared.root_id)?.0 {
            let child = self.require_complete_bash_child(&id)?;
            if self.require_private_bash_listener(&child, None)? == FreshBashListenerPolicy::Notify
            {
                self.settle_private_bash_listener(&id)?;
            }
        }
        self.settle_private_root_terminal(root, actor, session)
    }

    /// This is a local exact readback. It performs no F, ACK, K or publication.
    pub fn read_private_root_terminal(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<FreshRootTerminalReadback, String> {
        self.require_released_invocation(root, actor, session)?;
        let mut result = FreshRootTerminalReadback {
            handoff_id: root.handoff_id.clone(),
            d_key: root.d_key.clone(),
            invocation_uuid: root.invocation_uuid.clone(),
            session_id: session.session_id.clone(),
            root_id: root.old_release.prepared.root_id.clone(),
            owner_generation: root.old_release.prepared.owner_generation.clone(),
            actor: actor.clone(),
            execution: None,
            execution_state: "unknown".into(),
            terminal_state: "execution_unknown".into(),
            notification_state: "not_applicable".into(),
            notification_origin: "none".into(),
            native_receipt_state: "not_observed".into(),
            listener_policy: None,
            child_request_id: None,
            unresolved_child_request_ids: Vec::new(),
            mailbox_seq: None,
            delivery_request_id: None,
            delivery_grant_id: None,
            delivery_payload_sha256: None,
            delivery_payload_byte_len: None,
            ack_basis: None,
            publication_state: "not_started".into(),
            publication_sha256: None,
            unknown_stage: None,
            unknown_stages: Vec::new(),
            refusal: None,
            artifacts: vec![
                format!("released-d:{}", root.d_key),
                format!("invocation-j:{}", root.invocation_uuid),
            ],
        };
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let encoded: Option<String> = state
            .query_row(
                "SELECT execution_json FROM fresh_root_terminal WHERE handoff_id=?1",
                [&root.handoff_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let actual_parent = self.physical_root_terminal(root, actor, session);
        let (child_id, unresolved) = self.root_child_requests(&result.root_id)?;
        result.child_request_id = child_id.clone();
        for id in &unresolved {
            result.artifacts.push(format!("unresolved-child-c:{id}"));
            result.record_unknown(format!("child_c_unresolved:{id}"));
        }
        result.unresolved_child_request_ids = unresolved;
        if let Some(id) = &child_id {
            result.artifacts.push(format!("child-c:{id}"));
        }
        let actual_child = child_id
            .as_ref()
            .map(|id| self.selected_private_bash_event(id))
            .transpose();
        if let Some(json) = encoded {
            let stored: FreshRootTerminalExecution =
                serde_json::from_str(&json).map_err(|e| e.to_string())?;
            if stored.handoff_id != root.handoff_id
                || stored.d_key != root.d_key
                || stored.invocation_uuid != root.invocation_uuid
                || stored.session_id != session.session_id
                || stored.root_id != result.root_id
                || stored.owner_generation != result.owner_generation
                || stored.actor != *actor
                || stored.child_request_id != child_id
            {
                return Err("root terminal immutable evidence conflict".into());
            }
            result
                .artifacts
                .push(format!("parent-k:{}", stored.parent.grant_id));
            result
                .artifacts
                .push(format!("parent-q:{}", stored.parent.work_id));
            result.artifacts.push(format!(
                "parent-stdout:{}:{}:{}",
                stored.parent.grant_id, stored.parent.stdout_sha256, stored.parent.stdout_len
            ));
            result.artifacts.push(format!(
                "parent-stderr:{}:{}:{}",
                stored.parent.grant_id, stored.parent.stderr_sha256, stored.parent.stderr_len
            ));
            if let Some(event) = &stored.child_event {
                result
                    .artifacts
                    .push(format!("child-k:{}", event.physical_grant_id));
                result
                    .artifacts
                    .push(format!("child-w:{}", event.source_id));
                result.artifacts.push(format!(
                    "child-stdout:{}:{}:{}",
                    event.physical_grant_id, event.stdout_sha256, event.stdout_len
                ));
                result.artifacts.push(format!(
                    "child-stderr:{}:{}:{}",
                    event.physical_grant_id, event.stderr_sha256, event.stderr_len
                ));
            }
            match (&actual_parent, &actual_child) {
                (Err(e), _) => result.record_unknown(format!("parent_k_q:{e}")),
                (_, Err(e)) => result.record_unknown(format!("child_c_k_q_w:{e}")),
                (Ok(parent), Ok(child)) => {
                    if stored.parent != *parent
                        || stored.child_event != *child
                        || stored.outcome != physical_root_outcome(parent, child.as_ref())
                    {
                        return Err("root terminal immutable physical evidence conflict".into());
                    }
                    result.execution_state = stored.outcome.clone();
                }
            }
            result.execution = Some(stored);
        } else {
            result.record_unknown(match (&actual_parent, &actual_child) {
                (Err(e), _) => format!("parent_k_q:{e}"),
                (_, Err(e)) => format!("child_c_k_q_w:{e}"),
                _ => "terminal_commit_absent".into(),
            });
        }
        if let Some(id) = child_id {
            let child = self.require_complete_bash_child(&id)?;
            let policy = self.require_private_bash_listener(&child, None)?;
            result.listener_policy = Some(policy.as_str().into());
            self.read_terminal_notification(&state, &id, actor, session, &mut result)?;
        }
        let publication: Option<(String, i64, String)> = state.query_row(
            "SELECT artifact_sha256,artifact_byte_len,phase FROM fresh_root_publication WHERE handoff_id=?1",
            [&root.handoff_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        if let Some((sha, len, phase)) = publication {
            if phase != "unknown" || len < 0 || result.execution.is_none() {
                return Err("root caller publication conflict".into());
            }
            result.publication_state = "unknown".into();
            result.publication_sha256 = Some(sha.clone());
            result
                .artifacts
                .push(format!("caller-artifact:{sha}:{len}"));
        }
        result.terminal_state = if result.execution_state == "unknown" {
            result.refusal = Some("execution_evidence_incomplete".into());
            "execution_unknown"
        } else if result.execution_state == "failure"
            && !result.unresolved_child_request_ids.is_empty()
        {
            result.refusal = Some("unresolved_child_admission".into());
            "execution_failed_child_admission_pending"
        } else if !result.unresolved_child_request_ids.is_empty() {
            result.refusal = Some("unresolved_child_admission".into());
            "execution_completed_child_admission_pending"
        } else if result.execution_state == "failure" {
            "execution_failed"
        } else if matches!(
            result.notification_state.as_str(),
            "not_applicable" | "response_only"
        ) {
            "execution_completed"
        } else if result.notification_state == "acked" {
            "execution_completed_notification_acked"
        } else {
            result.refusal = Some("notification_unsettled".into());
            "execution_completed_notification_pending"
        }
        .into();
        Ok(result)
    }

    fn read_terminal_notification(
        &self,
        state: &Connection,
        request_id: &str,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
        result: &mut FreshRootTerminalReadback,
    ) -> Result<(), String> {
        let state_request: Option<(String, String, String, String, String, String, String)> = state
            .query_row(
                "SELECT source_id,attempt_id,recipient_identity,listener_session_id,
             listener_invocation_uuid,root_id,owner_generation
             FROM fresh_bash_notify_request WHERE request_id=?1",
                [request_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((
            source,
            attempt,
            identity,
            listener_session,
            listener_invocation,
            root_id,
            owner_generation,
        )) = state_request
        else {
            if result.listener_policy.as_deref() == Some("response_only") {
                let child = self.require_complete_bash_child(request_id)?;
                let unsolicited: bool = self
                    .sidecar
                    .mailbox()
                    .conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM fresh_recipient_row_source
                     WHERE source_id=?1 AND attempt_id=?2)",
                        params![child.handle, child.invocation_uuid],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if unsolicited {
                    return Err("response-only C has unsolicited sidecar F row".into());
                }
            }
            if result.listener_policy.as_deref() == Some("notify") {
                result.notification_origin = "original_c_notify".into();
            }
            result.notification_state = if result.listener_policy.as_deref() == Some("notify") {
                if self.selected_private_bash_event(request_id).is_ok() {
                    "repair_required"
                } else {
                    "awaiting_w"
                }
            } else {
                "response_only"
            }
            .into();
            if result.notification_state == "repair_required" {
                result.record_unknown("notify_state_request_absent".into());
            }
            return Ok(());
        };
        if identity != Self::recipient_identity_json(actor)?
            || listener_session != session.session_id
            || listener_invocation != result.invocation_uuid
            || root_id != result.root_id
            || owner_generation != result.owner_generation
        {
            return Err("terminal F original listener identity conflict".into());
        }
        result.notification_origin = if result.listener_policy.as_deref() == Some("notify") {
            "original_c_notify"
        } else {
            "explicit_original_activation"
        }
        .into();
        result.notification_state = "repair_required".into();
        result
            .artifacts
            .push(format!("notify-request:{request_id}"));
        let row: Option<(i64, String, i64)> = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT r.seq,r.payload_sha256,r.payload_byte_len FROM fresh_recipient_row_source r
             WHERE r.session_id=?1 AND r.source_id=?2 AND r.attempt_id=?3",
                params![session.session_id, source, attempt],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((seq, sha, len)) = row else {
            result.record_unknown("notify_sidecar_row_absent_reconcile_required".into());
            return Ok(());
        };
        let bytes = self.lookup_payload(&self.identity.lane_id, &session.session_id, seq)?;
        if i64::try_from(bytes.len()).ok() != Some(len) || sha256_hex(&bytes) != sha {
            return Err("terminal F payload changed".into());
        }
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let event = self.selected_private_bash_event(request_id)?;
        if event.source_id != source
            || event.attempt_id != attempt
            || payload["protocol"] != "fresh-bash-complete-v30"
            || payload["source"] != serde_json::to_value(&event).map_err(|e| e.to_string())?
        {
            return Err("terminal F payload W identity changed".into());
        }
        for (name, expected_sha, expected_len) in [
            ("stdout_bytes", &event.stdout_sha256, event.stdout_len),
            ("stderr_bytes", &event.stderr_sha256, event.stderr_len),
        ] {
            let raw: Vec<u8> = serde_json::from_value(payload[name].clone())
                .map_err(|e| format!("terminal F {name} malformed: {e}"))?;
            if raw.len() as u64 != expected_len
                || format!("{:x}", Sha256::digest(&raw)) != *expected_sha
            {
                return Err(format!("terminal F {name} differs from physical W"));
            }
        }
        result.mailbox_seq = Some(seq);
        result.delivery_payload_sha256 = Some(sha.clone());
        result.delivery_payload_byte_len = Some(len);
        result
            .artifacts
            .push(format!("fresh-mailbox:{}:{seq}", session.session_id));
        let grant: Option<(String,String,String,String)> = self.sidecar.mailbox().conn.query_row(
            "SELECT grant_id,delivery_request_id,phase,recipient_identity FROM fresh_recipient_grant
             WHERE session_id=?1 AND seq=?2 AND source_id=?3 AND attempt_id=?4
               AND payload_sha256=?5 AND payload_byte_len=?6",
            params![session.session_id,seq,source,attempt,sha,len],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        ).optional().map_err(|e| e.to_string())?;
        let Some((grant_id, delivery_request, phase, recipient)) = grant else {
            result.notification_state = "pending_f".into();
            return Ok(());
        };
        if recipient != identity {
            return Err("terminal F grant recipient conflict".into());
        }
        result.delivery_request_id = Some(delivery_request.clone());
        result.delivery_grant_id = Some(grant_id.clone());
        result.artifacts.push(format!("fresh-f-grant:{grant_id}"));
        let read = self
            .read_recipient_delivery(&grant_id, actor)?
            .ok_or("terminal F grant readback absent")?;
        if read.phase != phase
            || read.seq != seq
            || read.source_id != source
            || read.attempt_id != attempt
            || read.session_id != session.session_id
            || read.lane_id != self.identity.lane_id
            || read.source_generation != self.identity.source_generation
            || read.root_id != result.root_id
            || read.owner_generation != result.owner_generation
            || read.payload_sha256 != sha
            || read.payload_byte_len != len
        {
            return Err("terminal F readback conflict".into());
        }
        result.notification_state = match phase.as_str() {
            "unknown" => "f_unknown",
            "submitted" => "f_submitted_native_pending",
            "acked" => {
                let evidence: Option<(String,String,String)> = self.sidecar.mailbox().conn.query_row(
                    "SELECT e.basis,e.delivery_token_sha256,g.delivery_token
                     FROM fresh_recipient_ack_evidence e
                     JOIN fresh_recipient_grant g ON g.grant_id=e.grant_id
                     JOIN mailbox m ON m.session_id=e.session_id AND m.seq=e.seq
                     JOIN fresh_recipient_row_source r ON r.session_id=e.session_id AND r.seq=e.seq
                     WHERE e.grant_id=?1 AND e.session_id=?2 AND e.seq=?3
                       AND e.source_id=?4 AND e.attempt_id=?5
                       AND e.recipient_identity=?6 AND e.payload_sha256=?7
                       AND e.payload_byte_len=?8 AND e.delivery_request_id=?9
                       AND g.delivery_request_id=e.delivery_request_id
                       AND g.session_id=e.session_id AND g.seq=e.seq
                       AND g.source_id=e.source_id AND g.attempt_id=e.attempt_id
                       AND g.recipient_identity=e.recipient_identity
                       AND g.payload_sha256=e.payload_sha256 AND g.payload_byte_len=e.payload_byte_len
                       AND g.phase='acked' AND g.acknowledged_at=e.acknowledged_at
                       AND m.delivered_at=e.acknowledged_at
                       AND m.payload_sha256=e.payload_sha256 AND m.payload_byte_len=e.payload_byte_len
                       AND r.source_id=e.source_id AND r.attempt_id=e.attempt_id
                       AND r.payload_sha256=e.payload_sha256 AND r.payload_byte_len=e.payload_byte_len
                       AND ((e.basis='manual_ack' AND e.delegation_id IS NULL
                             AND m.delivered_by_invocation_uuid=e.grant_id)
                         OR (e.basis='delegated_manual_ack' AND e.delegation_id IS NOT NULL
                             AND m.delivered_by_invocation_uuid=e.delegation_id
                             AND EXISTS (SELECT 1 FROM fresh_recipient_ack_delegation_item i
                               JOIN fresh_recipient_ack_delegation d ON d.delegation_id=i.delegation_id
                               WHERE i.grant_id=e.grant_id AND d.delegation_id=e.delegation_id
                                 AND d.session_id=e.session_id AND d.consumed_at IS NOT NULL)))",
                    params![grant_id,session.session_id,seq,source,attempt,identity,sha,len,delivery_request],
                    |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
                ).optional().map_err(|e| e.to_string())?;
                let (basis, token_sha, token) = evidence.ok_or("terminal fresh ACK evidence absent or changed")?;
                if sha256_hex(token.as_bytes()) != token_sha {
                    return Err("terminal fresh ACK token conflict".into());
                }
                result.ack_basis = Some(basis);
                "acked"
            }
            _ => return Err("terminal F phase invalid".into()),
        }.into();
        Ok(())
    }

    /// Persist uncertainty before any caller output/control write. The caller
    /// route is closed here, so this API cannot assert delivery.
    pub fn begin_private_root_publication(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
        artifact: &[u8],
    ) -> Result<FreshRootTerminalReadback, String> {
        let read = self.read_private_root_terminal(root, actor, session)?;
        if !read.unresolved_child_request_ids.is_empty() {
            return Err("root terminal has unresolved child C".into());
        }
        if read.execution.is_none() || read.execution_state == "unknown" {
            return Err("root terminal execution unknown".into());
        }
        let sha = format!("{:x}", Sha256::digest(artifact));
        let len = i64::try_from(artifact.len()).map_err(|_| "caller artifact too large")?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        state
            .execute(
                "INSERT OR IGNORE INTO fresh_root_publication VALUES(?1,?2,?3,'unknown',?4)",
                params![root.handoff_id, sha, len, Utc::now().to_rfc3339()],
            )
            .map_err(|e| e.to_string())?;
        let read = self.read_private_root_terminal(root, actor, session)?;
        if read.publication_sha256.as_deref() != Some(&sha) {
            return Err("caller publication artifact replay conflict".into());
        }
        Ok(read)
    }
}
