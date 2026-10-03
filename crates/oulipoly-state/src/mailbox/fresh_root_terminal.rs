// Private v30 terminal readback. Execution, recipient notification and caller
// presentation have distinct authorities; this module never launches work,
// transmits F to a recipient, or writes output to a caller.
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
    /// The one-member form, kept byte-identical for zero- and one-child roots
    /// and for historical records.
    pub child_request_id: Option<String>,
    pub child_event: Option<FreshBashSourceEvent>,
    pub outcome: String,
    /// The frozen member set of a root that admitted two or more children.
    /// When present, the scalar child fields above are absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<FreshRootTerminalChild>,
}

/// One admitted child, accounted for once with its own exact W.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootTerminalChild {
    pub request_id: String,
    pub event: FreshBashSourceEvent,
}

impl FreshRootTerminalExecution {
    /// Every child request the immutable record accounts for.
    pub fn child_request_ids(&self) -> Vec<String> {
        if self.children.is_empty() {
            self.child_request_id.iter().cloned().collect()
        } else {
            self.children.iter().map(|child| child.request_id.clone()).collect()
        }
    }

    fn child_events(&self) -> Vec<&FreshBashSourceEvent> {
        if self.children.is_empty() {
            self.child_event.iter().collect()
        } else {
            self.children.iter().map(|child| &child.event).collect()
        }
    }
}

/// Per-member notification readback for a root whose terminal accounts for a
/// child set. Each member has its own recipient row, delivery and ACK.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootTerminalChildReadback {
    pub request_id: String,
    pub selected_event: Option<FreshBashSourceEvent>,
    pub listener_policy: Option<String>,
    pub notification_state: String,
    pub notification_origin: String,
    pub mailbox_seq: Option<i64>,
    pub delivery_request_id: Option<String>,
    pub delivery_grant_id: Option<String>,
    pub delivery_payload_sha256: Option<String>,
    pub delivery_payload_byte_len: Option<i64>,
    pub ack_basis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_receipt: Option<FreshOriginalReceiptIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_ack: Option<FreshSuccessorTerminalAck>,
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
    /// Exact selected W remains readable while the parent provider Q is
    /// pending and no root terminal execution has been committed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_child_event: Option<FreshBashSourceEvent>,
    pub unresolved_child_request_ids: Vec<String>,
    pub mailbox_seq: Option<i64>,
    pub delivery_request_id: Option<String>,
    pub delivery_grant_id: Option<String>,
    pub delivery_payload_sha256: Option<String>,
    pub delivery_payload_byte_len: Option<i64>,
    pub ack_basis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_receipt: Option<FreshOriginalReceiptIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_ack: Option<FreshSuccessorTerminalAck>,
    pub publication_state: String,
    pub publication_sha256: Option<String>,
    pub unknown_stage: Option<String>,
    pub unknown_stages: Vec<String>,
    pub refusal: Option<String>,
    pub artifacts: Vec<String>,
    /// Present only for a root with two or more admitted children. The scalar
    /// child and notification fields then describe no single child.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<FreshRootTerminalChildReadback>,
    /// Admissions that surfaced after the terminal froze its set. They stay
    /// in `unresolved_child_request_ids` and are never discharged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub late_child_request_ids: Vec<String>,
}

/// Admitted children under one root: those with an accepted W, and those
/// still without one. Both lists are ordered by request ID.
struct RootChildSet {
    selected: Vec<String>,
    unresolved: Vec<String>,
}

impl RootChildSet {
    fn admitted(&self) -> usize {
        self.selected.len() + self.unresolved.len()
    }

    fn contains(&self, request_id: &str) -> bool {
        self.selected.iter().chain(&self.unresolved).any(|id| id == request_id)
    }
}

/// Byte identity offered by the original root before its caller-visible write.
/// The State lane compares this with broker-owned Q, then records an immutable
/// publication intent. A matching intent is not a consumer acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootCallerResult {
    pub parent_grant_id: String,
    pub wait_status: i32,
    pub stdout_sha256: String,
    pub stdout_len: u64,
    pub stderr_sha256: String,
    pub stderr_len: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_f: Option<FreshNativeFCallerResult>,
}

/// A caller presentation sourced from the exact acknowledged second Codex
/// turn. The parent K still has its own, cancelled, physical Q above.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshNativeFCallerResult {
    pub delivery_request_id: String,
    pub turn_id: String,
    pub assistant_response_sha256: String,
    pub stdout_sha256: String,
    pub stdout_len: u64,
    pub stderr_sha256: String,
    pub stderr_len: u64,
    pub exit_code: u8,
}

impl FreshRootTerminalReadback {
    fn record_unknown(&mut self, stage: String) {
        self.unknown_stage = Some(stage.clone());
        self.unknown_stages.push(stage);
    }
}

fn physical_root_outcome(
    parent: &FreshPhysicalTerminal,
    children: &[&FreshBashSourceEvent],
) -> &'static str {
    if !parent.cancelled
        && libc::WIFEXITED(parent.wait_status)
        && libc::WEXITSTATUS(parent.wait_status) == 0
        && children.iter().all(|event| {
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

/// A set is settled only when every member is: response-only members need no
/// F, and every notify member needs its own ACK. Anything else stays pending.
fn child_set_notification_state(children: &[FreshRootTerminalChildReadback]) -> &'static str {
    let quiet = |state: &str| matches!(state, "not_applicable" | "response_only");
    if children.is_empty() {
        "not_applicable"
    } else if children.iter().all(|child| quiet(&child.notification_state)) {
        "response_only"
    } else if children
        .iter()
        .all(|child| quiet(&child.notification_state) || child.notification_state == "acked")
    {
        "acked"
    } else {
        "child_set_pending"
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

fn fresh_root_caller_settlement_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE (type='table' AND name='fresh_root_caller_settlement')
             OR (type='trigger' AND name IN ('fresh_root_caller_settlement_no_update',
             'fresh_root_caller_settlement_no_delete'))",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn verify_fresh_root_caller_settlement_schema(state: &Connection) -> Result<(), String> {
    if fresh_root_caller_settlement_schema_count(state)? != 3 {
        return Err("fresh root caller settlement schema incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_ROOT_CALLER_SETTLEMENT_SCHEMA,
        "fresh_root_caller_settlement",
        &[
            "fresh_root_caller_settlement_no_update",
            "fresh_root_caller_settlement_no_delete",
        ],
    )
}

impl FreshV30Lane {
    fn native_f_caller_result(
        &self,
        read: &FreshRootTerminalReadback,
    ) -> Result<FreshNativeFCallerResult, String> {
        let execution = read.execution.as_ref().ok_or("native F parent Q absent")?;
        if !execution.parent.cancelled
            || execution.parent.outcome != "cancelled"
            || read.ack_basis.as_deref() != Some("native_codex_f_assistant_ack")
            || read.notification_state != "acked"
            || !read.unresolved_child_request_ids.is_empty()
        {
            return Err("native F caller result lacks cancelled Q or exact F ACK".into());
        }
        let delivery = read.delivery_request_id.as_deref().ok_or("native F delivery absent")?;
        let ack = self.read_headless_native_f_ack(delivery, &read.actor)?
            .ok_or("native F ACK readback absent")?;
        if ack.proof.native_session_id.is_empty()
            || ack.proof.delivery_request_id != delivery
            || ack.proof.recipient != read.actor
            || ack.proof.turn_id == ack.proof.first_turn_id
        {
            return Err("native F ACK identity changed".into());
        }
        let stdout = format!("AGE319_F_ACK {}\n", ack.proof.delivery_token);
        Ok(FreshNativeFCallerResult {
            delivery_request_id: delivery.into(),
            turn_id: ack.proof.turn_id,
            assistant_response_sha256: ack.proof.assistant_response_sha256,
            stdout_sha256: sha256_hex(stdout.as_bytes()),
            stdout_len: stdout.len() as u64,
            stderr_sha256: sha256_hex(b""),
            stderr_len: 0,
            exit_code: 0,
        })
    }

    fn caller_artifact(&self, read: &FreshRootTerminalReadback) -> Result<Vec<u8>, String> {
        let execution = read.execution.as_ref().ok_or("root execution absent")?;
        if execution.parent.cancelled {
            serde_json::to_vec(&(execution, self.native_f_caller_result(read)?))
                .map_err(|error| error.to_string())
        } else {
            serde_json::to_vec(execution).map_err(|error| error.to_string())
        }
    }

    pub fn begin_private_root_caller_result(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
        offered: &FreshRootCallerResult,
    ) -> Result<FreshRootTerminalReadback, String> {
        let read = self.read_private_root_terminal(root, actor, session)?;
        if !read.unresolved_child_request_ids.is_empty() {
            return Err("root terminal has unresolved child C".into());
        }
        if read.execution_state == "unknown" {
            return Err("root terminal execution unknown".into());
        }
        let execution = read.execution.as_ref().ok_or("root terminal execution absent")?;
        let parent = &execution.parent;
        if offered.parent_grant_id != parent.grant_id
            || offered.wait_status != parent.wait_status
            || offered.stdout_sha256 != parent.stdout_sha256
            || offered.stdout_len != parent.stdout_len
            || offered.stderr_sha256 != parent.stderr_sha256
            || offered.stderr_len != parent.stderr_len
        {
            return Err("caller result differs from verified parent Q".into());
        }
        if parent.cancelled {
            if offered.native_f.is_none() {
                return Err("caller result terminal was cancelled".into());
            }
            if offered.native_f.as_ref() != Some(&self.native_f_caller_result(&read)?) {
                return Err("caller result differs from verified native F ACK".into());
            }
        } else if offered.native_f.is_some() {
            return Err("native F caller result has no cancelled parent Q".into());
        }
        // Include the original D/J/actor and parent Q in the committed
        // artifact. The caller retains the raw stream bytes.
        let artifact = self.caller_artifact(&read)?;
        self.begin_private_root_publication(root, actor, session, &artifact)
    }

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
        if self.normal_provider_k_recorded(&root.handoff_id)? {
            if physical_entry_exists(&directory, &format!("{}.fresh-grant.json", root.handoff_id))? {
                return Err("ambiguous parent provider K".into());
            }
            return self.physical_normal_root_terminal(root, actor, session);
        }
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

    fn physical_normal_root_terminal(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<FreshPhysicalTerminal, String> {
        let k = self.read_normal_provider_k(root, actor, session)?
            .ok_or("normal parent K absent")?;
        let store = self.state_path.parent().ok_or("fresh State parent absent")?
            .join("normal-provider");
        let directory = store.join(&k.admission_id);
        for path in [&store, &directory] {
            let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
            if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != 0
                || meta.mode() & 0o077 != 0
            {
                return Err("normal parent physical directory untrusted".into());
            }
        }
        let q = physical_json(&directory, "q.json")?;
        let wait = physical_json(&directory, "parent-wait.json")?;
        let status = q["provider_wait_status"].as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or("normal parent wait status invalid")?;
        let parent_status = wait["pid1_wait_status"].as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or("normal parent PID1 wait status invalid")?;
        if q["admission_id"] != k.admission_id
            || q["plan_sha256"] != k.plan_sha256
            || q["tree_drained"] != true
            || wait["admission_id"] != k.admission_id
            || wait["pid1_parent_namespace_pid"].as_i64().is_none_or(|pid| pid <= 0)
            || !libc::WIFEXITED(parent_status)
            || libc::WEXITSTATUS(parent_status) != 0
        {
            return Err("normal parent physical Q or PID1 wait changed".into());
        }
        let (stdout_sha256, stdout_len) = self.physical_terminal_output(
            &directory, "stdout", "stdout", &q,
        )?;
        let (stderr_sha256, stderr_len) = self.physical_terminal_output(
            &directory, "stderr", "stderr", &q,
        )?;
        // The normal physical store keys both K and Q by the one admission.
        Ok(FreshPhysicalTerminal {
            grant_id: k.admission_id.clone(),
            work_id: k.admission_id,
            plan_sha256: k.plan_sha256,
            wait_status: status,
            outcome: physical_provider_outcome(status, false).into(),
            cancelled: false,
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
        self.physical_terminal_output(directory, &format!("{grant}.{stream}"), stream, drain)
    }

    fn physical_terminal_output(
        &self,
        directory: &Path,
        name: &str,
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
            .open(directory.join(name))
            .map_err(|e| format!("parent {stream} absent: {e}"))?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file()
            || meta.uid() != 0
            || meta.nlink() != 1
            || meta.mode() & 0o077 != 0
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

    fn root_child_set(&self, root_id: &str) -> Result<RootChildSet, String> {
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut rows = state
            .prepare(
                "SELECT c.request_id, e.request_id IS NOT NULL FROM fresh_bash_child c
                      LEFT JOIN fresh_bash_selected_event e ON e.request_id=c.request_id
                      WHERE c.root_id=?1 ORDER BY c.request_id",
            )
            .map_err(|e| e.to_string())?;
        let ids = rows
            .query_map([root_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let (selected, unresolved): (Vec<_>, Vec<_>) =
            ids.into_iter().partition(|(_, has_w)| *has_w);
        Ok(RootChildSet {
            selected: selected.into_iter().map(|(id, _)| id).collect(),
            unresolved: unresolved.into_iter().map(|(id, _)| id).collect(),
        })
    }

    /// A member's exact W. A W naming another request or another root is a
    /// real anomaly, not an unresolved member.
    fn root_member_event(
        &self,
        root_id: &str,
        request_id: &str,
    ) -> Result<Result<FreshBashSourceEvent, String>, String> {
        let event = match self.selected_private_bash_event(request_id) {
            Ok(event) => event,
            Err(error) => return Ok(Err(error)),
        };
        if event.request_id != request_id || event.root_id != root_id {
            return Err("child W identity or root conflict".into());
        }
        Ok(Ok(event))
    }

    /// Offline CLI close has no parent K/Q. A child C or selected W under
    /// the released root is therefore debt, even if no Broker work remains.
    pub fn no_root_child_requests(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<bool, String> {
        self.require_released_invocation(root, actor, session)?;
        Ok(self.root_child_set(&root.old_release.prepared.root_id)?.admitted() == 0)
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
        // The set freezes here, after the parent's physical Q proves its tree
        // drained. Every admitted C must already have its own W; an admission
        // that surfaces later is retained by readback as an anomaly.
        let root_id = &root.old_release.prepared.root_id;
        let set = self.root_child_set(root_id)?;
        if !set.unresolved.is_empty() {
            return self.read_private_root_terminal(root, actor, session);
        }
        let mut members = Vec::with_capacity(set.selected.len());
        for id in &set.selected {
            match self.root_member_event(root_id, id)? {
                Ok(event) => members.push(FreshRootTerminalChild {
                    request_id: id.clone(),
                    event,
                }),
                Err(_) => return self.read_private_root_terminal(root, actor, session),
            }
        }
        let events: Vec<_> = members.iter().map(|member| &member.event).collect();
        let outcome = physical_root_outcome(&parent, &events);
        let (child_request_id, child_event, children) = if members.len() == 1 {
            let member = members.pop().unwrap();
            (Some(member.request_id), Some(member.event), Vec::new())
        } else {
            (None, None, members)
        };
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
            children,
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
        let root_id = &root.old_release.prepared.root_id;
        let set = self.root_child_set(root_id)?;
        for id in set.selected.iter().chain(set.unresolved.iter()) {
            self.repair_captured_private_bash_source(id)?;
        }
        // Each member's listener is settled on its own; repair never treats
        // one member's evidence as another's.
        for id in self.root_child_set(root_id)?.selected {
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
            selected_child_event: None,
            unresolved_child_request_ids: Vec::new(),
            mailbox_seq: None,
            delivery_request_id: None,
            delivery_grant_id: None,
            delivery_payload_sha256: None,
            delivery_payload_byte_len: None,
            ack_basis: None,
            original_receipt: None,
            successor_ack: None,
            publication_state: "not_started".into(),
            publication_sha256: None,
            unknown_stage: None,
            unknown_stages: Vec::new(),
            refusal: None,
            artifacts: vec![
                format!("released-d:{}", root.d_key),
                format!("invocation-j:{}", root.invocation_uuid),
            ],
            children: Vec::new(),
            late_child_request_ids: Vec::new(),
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
        let set = self.root_child_set(&result.root_id)?;
        let stored: Option<FreshRootTerminalExecution> = encoded
            .map(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
            .transpose()?;
        // Members are the frozen set once the terminal is committed, otherwise
        // every admitted C that already has its W.
        let (members, unresolved, late) = match &stored {
            Some(stored) => {
                Self::validate_frozen_child_set(stored)?;
                let frozen = stored.child_request_ids();
                if frozen.iter().any(|id| !set.selected.contains(id)) {
                    return Err("root terminal immutable evidence conflict".into());
                }
                let late: Vec<String> = set
                    .selected
                    .iter()
                    .chain(&set.unresolved)
                    .filter(|id| !frozen.contains(id))
                    .cloned()
                    .collect();
                (frozen, late.clone(), late)
            }
            None => (set.selected.clone(), set.unresolved.clone(), Vec::new()),
        };
        debug_assert!(members.iter().all(|id| set.contains(id)));
        let set_form = stored
            .as_ref()
            .map_or(set.admitted() >= 2, |stored| !stored.children.is_empty());
        for id in &unresolved {
            result.artifacts.push(format!("unresolved-child-c:{id}"));
            result.record_unknown(if late.contains(id) {
                format!("child_c_after_freeze:{id}")
            } else {
                format!("child_c_unresolved:{id}")
            });
        }
        result.unresolved_child_request_ids = unresolved;
        result.late_child_request_ids = late;
        for id in &members {
            result.artifacts.push(format!("child-c:{id}"));
        }
        let mut actual_children = Vec::with_capacity(members.len());
        for id in &members {
            actual_children.push(self.root_member_event(&result.root_id, id)?);
        }
        let actual_child: Result<Vec<FreshBashSourceEvent>, String> =
            actual_children.iter().cloned().collect();
        if !set_form {
            result.child_request_id = members.first().cloned();
            result.selected_child_event = actual_children
                .first()
                .and_then(|event| event.as_ref().ok().cloned());
        }
        if let Some(stored) = stored {
            if stored.handoff_id != root.handoff_id
                || stored.d_key != root.d_key
                || stored.invocation_uuid != root.invocation_uuid
                || stored.session_id != session.session_id
                || stored.root_id != result.root_id
                || stored.owner_generation != result.owner_generation
                || stored.actor != *actor
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
            let stored_events = stored.child_events();
            for event in &stored_events {
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
                (Ok(parent), Ok(children)) => {
                    if stored.parent != *parent
                        || stored_events.len() != children.len()
                        || stored_events
                            .iter()
                            .zip(children)
                            .any(|(stored, actual)| *stored != actual)
                        || stored.outcome != physical_root_outcome(parent, &stored_events)
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
        if set_form {
            result.notification_origin = "child_set".into();
            for (id, event) in members.iter().zip(&actual_children) {
                let member = self.read_member_notification(&state, id, actor, session, &result)?;
                for stage in &member.1 {
                    result.record_unknown(format!("child:{id}:{stage}"));
                }
                result.artifacts.extend(member.2);
                let mut member = member.0;
                member.selected_event = event.as_ref().ok().cloned();
                result.children.push(member);
            }
            result.notification_state = child_set_notification_state(&result.children).into();
        } else if let Some(id) = members.first() {
            let child = self.require_complete_bash_child(id)?;
            let policy = self.require_private_bash_listener(&child, None)?;
            result.listener_policy = Some(policy.as_str().into());
            self.read_terminal_notification(&state, id, actor, session, &mut result)?;
        }
        let publication: Option<(String, i64, String)> = state.query_row(
            "SELECT artifact_sha256,artifact_byte_len,phase FROM fresh_root_publication WHERE handoff_id=?1",
            [&root.handoff_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        let settlement: Option<(String, i64)> = state
            .query_row(
                "SELECT artifact_sha256,artifact_byte_len FROM fresh_root_caller_settlement
                 WHERE handoff_id=?1",
                [&root.handoff_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if publication.is_none() && settlement.is_some() {
            return Err("root caller settlement without publication".into());
        }
        if let Some((sha, len, phase)) = publication {
            if phase != "unknown" || len < 0 || result.execution.is_none() {
                return Err("root caller publication conflict".into());
            }
            result.publication_state = "unknown".into();
            result.publication_sha256 = Some(sha.clone());
            result
                .artifacts
                .push(format!("caller-artifact:{sha}:{len}"));
            if let Some((settled_sha, settled_len)) = settlement {
                let execution = self.caller_artifact(&result)?;
                if settled_sha != sha
                    || settled_len != len
                    || sha != format!("{:x}", Sha256::digest(&execution))
                    || len != i64::try_from(execution.len()).map_err(|_| "caller artifact too large")?
                {
                    return Err("root caller settlement identity conflict".into());
                }
                result.publication_state = "settled".into();
                result.artifacts.push(format!("caller-settled:{sha}:{len}"));
            }
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
        if !result.late_child_request_ids.is_empty() {
            result.refusal = Some("child_admission_after_freeze".into());
        }
        Ok(result)
    }

    /// A committed set names each member once, in request order, with its own
    /// W. The one-member form never carries a set.
    fn validate_frozen_child_set(stored: &FreshRootTerminalExecution) -> Result<(), String> {
        let valid = if stored.children.is_empty() {
            stored.child_request_id.is_some() == stored.child_event.is_some()
                && stored.child_event.as_ref().is_none_or(|event| {
                    Some(&event.request_id) == stored.child_request_id.as_ref()
                })
        } else {
            stored.child_request_id.is_none()
                && stored.child_event.is_none()
                && stored.children.len() >= 2
                && stored
                    .children
                    .iter()
                    .all(|child| child.event.request_id == child.request_id)
                && stored
                    .children
                    .windows(2)
                    .all(|pair| pair[0].request_id < pair[1].request_id)
        };
        if valid {
            Ok(())
        } else {
            Err("root terminal child set record invalid".into())
        }
    }

    /// Reads one member's notification through the same exact reader as the
    /// one-child form, isolated from every other member's evidence.
    fn read_member_notification(
        &self,
        state: &Connection,
        request_id: &str,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
        base: &FreshRootTerminalReadback,
    ) -> Result<(FreshRootTerminalChildReadback, Vec<String>, Vec<String>), String> {
        let child = self.require_complete_bash_child(request_id)?;
        let policy = self.require_private_bash_listener(&child, None)?;
        let mut scratch = base.clone();
        scratch.children.clear();
        scratch.listener_policy = Some(policy.as_str().into());
        scratch.notification_state = "not_applicable".into();
        scratch.notification_origin = "none".into();
        scratch.mailbox_seq = None;
        scratch.delivery_request_id = None;
        scratch.delivery_grant_id = None;
        scratch.delivery_payload_sha256 = None;
        scratch.delivery_payload_byte_len = None;
        scratch.ack_basis = None;
        scratch.original_receipt = None;
        scratch.successor_ack = None;
        scratch.unknown_stage = None;
        scratch.unknown_stages = Vec::new();
        scratch.artifacts = Vec::new();
        self.read_terminal_notification(state, request_id, actor, session, &mut scratch)?;
        Ok((
            FreshRootTerminalChildReadback {
                request_id: request_id.into(),
                selected_event: None,
                listener_policy: scratch.listener_policy,
                notification_state: scratch.notification_state,
                notification_origin: scratch.notification_origin,
                mailbox_seq: scratch.mailbox_seq,
                delivery_request_id: scratch.delivery_request_id,
                delivery_grant_id: scratch.delivery_grant_id,
                delivery_payload_sha256: scratch.delivery_payload_sha256,
                delivery_payload_byte_len: scratch.delivery_payload_byte_len,
                ack_basis: scratch.ack_basis,
                original_receipt: scratch.original_receipt,
                successor_ack: scratch.successor_ack,
            },
            scratch.unknown_stages,
            scratch.artifacts,
        ))
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
        let state_successor: Option<String> = state.query_row(
            "SELECT offer_request_id FROM fresh_lane_successor_admission
             WHERE session_id=?1 AND seq=?2",
            params![session.session_id,seq], |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?;
        let sidecar_successor: bool = self.sidecar.mailbox().conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM fresh_successor_admission
             WHERE session_id=?1 AND seq=?2)",
            params![session.session_id,seq], |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if let Some(offer_request_id) = state_successor {
            result.notification_origin = "admitted_successor".into();
            result.notification_state = "pending_f".into();
            match self.read_successor_terminal_ack(
                &offer_request_id,actor,session,seq,&source,&attempt,
                &result.root_id,&result.owner_generation,&sha,len,
            ) {
                Ok(Some(ack)) => {
                    result.delivery_request_id = Some(ack.delivery_request_id.clone());
                    result.delivery_grant_id = Some(ack.grant_id.clone());
                    result.ack_basis = Some("successor_receiver_receipt_ack".into());
                    result.artifacts.push(format!("successor-admission:{}",ack.generation));
                    result.artifacts.push(format!("successor-f-grant:{}",ack.grant_id));
                    result.artifacts.push(format!("successor-receipt:{}",ack.receipt_sha256));
                    result.successor_ack = Some(ack);
                    result.notification_state = "acked".into();
                }
                Ok(None) => {}
                Err(error) => result.record_unknown(format!("successor_ack:{error}")),
            }
            return Ok(());
        }
        if sidecar_successor {
            result.notification_state = "pending_f".into();
            result.record_unknown("successor_state_admission_absent".into());
            return Ok(());
        }
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
                     LEFT JOIN fresh_original_receipt c ON c.grant_id=e.grant_id
                     LEFT JOIN fresh_original_grant_uid u ON u.grant_id=e.grant_id
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
                             AND m.delivered_by_invocation_uuid=e.grant_id
                             AND c.delivery_request_id=e.delivery_request_id
                             AND c.session_id=e.session_id AND c.seq=e.seq
                             AND c.source_id=e.source_id AND c.attempt_id=e.attempt_id
                             AND c.recipient_identity=e.recipient_identity
                             AND c.recipient_identity=u.recipient_identity
                             AND c.recipient_uid=u.recipient_uid
                             AND c.payload_sha256=e.payload_sha256
                             AND c.payload_byte_len=e.payload_byte_len
                             AND c.delivery_token_sha256=e.delivery_token_sha256)
                         OR (e.basis='delegated_manual_ack' AND e.delegation_id IS NOT NULL
                             AND m.delivered_by_invocation_uuid=e.delegation_id
                             AND EXISTS (SELECT 1 FROM fresh_recipient_ack_delegation_item i
                               JOIN fresh_recipient_ack_delegation d ON d.delegation_id=i.delegation_id
                               WHERE i.grant_id=e.grant_id AND d.delegation_id=e.delegation_id
                                 AND d.session_id=e.session_id AND d.consumed_at IS NOT NULL)))",
                    params![grant_id,session.session_id,seq,source,attempt,identity,sha,len,delivery_request],
                    |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
                ).optional().map_err(|e| e.to_string())?;
                let (basis, token_sha, token) = if let Some(evidence) = evidence {
                    evidence
                } else {
                    let legacy = self.sidecar.mailbox().conn.query_row(
                        "SELECT a.basis,a.delivery_token_sha256,g.delivery_token
                         FROM fresh_native_f_auto_ack a
                         JOIN fresh_native_f_receipt n ON n.preparation_request_id=a.preparation_request_id
                         JOIN fresh_native_f_transport t ON t.preparation_request_id=n.preparation_request_id
                         JOIN fresh_recipient_grant g ON g.grant_id=a.grant_id
                         JOIN mailbox m ON m.session_id=a.session_id AND m.seq=a.seq
                         JOIN fresh_recipient_row_source r ON r.session_id=a.session_id AND r.seq=a.seq
                         WHERE a.grant_id=?1 AND a.session_id=?2 AND a.seq=?3
                           AND a.source_id=?4 AND a.attempt_id=?5 AND a.recipient_identity=?6
                           AND a.payload_sha256=?7 AND a.payload_byte_len=?8
                           AND a.delivery_request_id=?9 AND g.phase='acked'
                           AND g.acknowledged_at=a.acknowledged_at
                           AND g.delivery_request_id=a.delivery_request_id
                           AND g.session_id=a.session_id AND g.seq=a.seq
                           AND g.source_id=a.source_id AND g.attempt_id=a.attempt_id
                           AND g.recipient_identity=a.recipient_identity
                           AND g.payload_sha256=a.payload_sha256
                           AND g.payload_byte_len=a.payload_byte_len
                           AND n.grant_id=a.grant_id AND n.turn_id=a.turn_id
                           AND n.recipient_identity=a.recipient_identity
                           AND n.provider_session_id=a.session_id
                           AND n.payload_sha256=a.payload_sha256
                           AND t.grant_id=a.grant_id
                           AND t.recipient_identity=a.recipient_identity
                           AND m.delivered_at=a.acknowledged_at
                           AND m.delivered_by_invocation_uuid=a.grant_id
                           AND m.payload_sha256=a.payload_sha256 AND m.payload_byte_len=a.payload_byte_len
                           AND r.source_id=a.source_id AND r.attempt_id=a.attempt_id
                           AND r.payload_sha256=a.payload_sha256 AND r.payload_byte_len=a.payload_byte_len",
                        params![grant_id,session.session_id,seq,source,attempt,identity,sha,len,delivery_request],
                        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
                    ).optional().map_err(|e| e.to_string())?;
                    if let Some(legacy) = legacy {
                        legacy
                    } else {
                            let ack = self.read_headless_native_f_ack(&delivery_request, actor)?
                                .ok_or("terminal fresh ACK evidence absent or changed")?;
                            let reserved = self.read_headless_native_f_attempt(&delivery_request, actor)?
                                .ok_or("terminal headless F reservation absent")?;
                            let fresh = &reserved.candidate.fresh;
                            if ack.proof.fresh_grant_id != grant_id
                                || ack.proof.native_session_id != reserved.candidate.native_session_id
                                || ack.proof.first_turn_id != reserved.candidate.original_turn_id
                                || ack.proof.turn_id == ack.proof.first_turn_id
                                || ack.proof.envelope_sha256 != reserved.envelope_sha256
                                || ack.proof.assistant_response_sha256
                                    != sha256_hex(format!("AGE319_F_ACK {}", ack.proof.delivery_token).as_bytes())
                                || ack.basis != "native_codex_f_assistant_ack"
                                || fresh.session_id != session.session_id || fresh.seq != seq
                                || fresh.source_id != source || fresh.attempt_id != attempt
                                || fresh.payload_sha256 != sha || fresh.payload_byte_len != len
                            {
                                return Err("terminal headless F ACK evidence changed".into());
                            }
                            (ack.basis, reserved.delivery_token_sha256, ack.proof.delivery_token)
                    }
                };
                if sha256_hex(token.as_bytes()) != token_sha {
                    return Err("terminal fresh ACK token conflict".into());
                }
                if basis == "manual_ack" {
                    let uid: Option<u32> = self.sidecar.mailbox().conn.query_row(
                        "SELECT recipient_uid FROM fresh_original_grant_uid WHERE grant_id=?1",
                        [&grant_id], |r| r.get(0),
                    ).optional().map_err(|e| e.to_string())?;
                    let receipt = uid.ok_or_else(|| "terminal original F UID absent".to_string())
                        .and_then(|uid| self.read_original_receipt(&grant_id,actor,uid)
                            .and_then(|r| r.ok_or("terminal original receipt absent".into())));
                    match receipt {
                        Ok(receipt) => {
                            result.artifacts.push(format!("original-receipt:{}",receipt.receipt_sha256));
                            result.original_receipt = Some(receipt);
                        }
                        Err(error) => {
                            result.notification_state = "f_unknown".into();
                            result.record_unknown(format!("original_receipt:{error}"));
                            return Ok(());
                        }
                    }
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

    /// Only the original pinned caller can request this after its two
    /// Q-verified stream writes and flushes returned success. A lost reply
    /// reopens the same immutable record; an interrupted write leaves unknown.
    pub fn settle_private_root_caller_result(
        &self,
        root: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
        offered: &FreshRootCallerResult,
    ) -> Result<FreshRootTerminalReadback, String> {
        let read = self.begin_private_root_caller_result(root, actor, session, offered)?;
        if read.execution_state == "unknown" || !read.unresolved_child_request_ids.is_empty() {
            return Err("root caller settlement terminal unresolved".into());
        }
        let sha = read
            .publication_sha256
            .as_ref()
            .ok_or("root caller publication absent")?;
        let artifact = self.caller_artifact(&read)?;
        let len = i64::try_from(artifact.len()).map_err(|_| "caller artifact too large")?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|error| error.to_string())?;
        state
            .execute(
                "INSERT OR IGNORE INTO fresh_root_caller_settlement VALUES(?1,?2,?3,?4)",
                params![root.handoff_id, sha, len, Utc::now().to_rfc3339()],
            )
            .map_err(|error| error.to_string())?;
        let settled = self.read_private_root_terminal(root, actor, session)?;
        if settled.publication_state != "settled"
            || settled.publication_sha256.as_ref() != Some(sha)
            || settled.execution != read.execution
        {
            return Err("root caller settlement readback changed".into());
        }
        Ok(settled)
    }
}

#[cfg(test)]
mod root_terminal_child_set_tests {
    use super::*;

    fn identity() -> FreshRecipientIdentity {
        FreshRecipientIdentity {
            host_pid: 10,
            boot_id: "boot".into(),
            starttime_ticks: 20,
            pidns_dev: 30,
            pidns_ino: 40,
        }
    }

    fn event(request_id: &str, wait_status: i32) -> FreshBashSourceEvent {
        FreshBashSourceEvent {
            request_id: request_id.into(),
            source_id: format!("source-{request_id}"),
            attempt_id: format!("attempt-{request_id}"),
            state_admission_id: "admission".into(),
            registration_digest: "digest".into(),
            lane_id: "lane".into(),
            source_generation: "generation".into(),
            session_id: "session".into(),
            root_id: "root".into(),
            owner_generation: "owner".into(),
            parent_work_grant_id: "parent-grant".into(),
            parent_work_id: "parent-work".into(),
            physical_grant_id: format!("grant-{request_id}"),
            physical_work_id: format!("work-{request_id}"),
            completion_policy: "notify".into(),
            selected_kind: "physical".into(),
            wait_status,
            cancelled: false,
            cancel_grant_id: None,
            tree_drained: true,
            output_closed: true,
            stdout_sha256: "out".into(),
            stdout_len: 1,
            stderr_sha256: "err".into(),
            stderr_len: 1,
            normal_provider_selection: None,
        }
    }

    fn parent() -> FreshPhysicalTerminal {
        FreshPhysicalTerminal {
            grant_id: "parent-grant".into(),
            work_id: "parent-work".into(),
            plan_sha256: "plan".into(),
            wait_status: 0,
            outcome: "exit_success".into(),
            cancelled: false,
            stdout_sha256: "out".into(),
            stdout_len: 0,
            stderr_sha256: "err".into(),
            stderr_len: 0,
        }
    }

    fn execution(
        child_request_id: Option<&str>,
        children: &[&str],
    ) -> FreshRootTerminalExecution {
        FreshRootTerminalExecution {
            handoff_id: "handoff".into(),
            d_key: "d".into(),
            invocation_uuid: "j".into(),
            session_id: "session".into(),
            root_id: "root".into(),
            owner_generation: "owner".into(),
            actor: identity(),
            parent: parent(),
            child_request_id: child_request_id.map(Into::into),
            child_event: child_request_id.map(|id| event(id, 0)),
            outcome: "success".into(),
            children: children
                .iter()
                .map(|id| FreshRootTerminalChild {
                    request_id: (*id).into(),
                    event: event(id, 0),
                })
                .collect(),
        }
    }

    fn member(state: &str) -> FreshRootTerminalChildReadback {
        FreshRootTerminalChildReadback {
            request_id: "member".into(),
            selected_event: None,
            listener_policy: None,
            notification_state: state.into(),
            notification_origin: "none".into(),
            mailbox_seq: None,
            delivery_request_id: None,
            delivery_grant_id: None,
            delivery_payload_sha256: None,
            delivery_payload_byte_len: None,
            ack_basis: None,
            original_receipt: None,
            successor_ack: None,
        }
    }

    /// Zero- and one-child records written before child sets existed read
    /// back unchanged and serialize to the same bytes, so stored digests hold.
    #[test]
    fn historical_zero_and_one_child_records_keep_their_bytes() {
        for record in [execution(None, &[]), execution(Some("a"), &[])] {
            let historical = serde_json::to_string(&record).unwrap();
            assert!(!historical.contains("\"children\""));
            let read: FreshRootTerminalExecution = serde_json::from_str(&historical).unwrap();
            assert_eq!(serde_json::to_string(&read).unwrap(), historical);
            assert!(FreshV30Lane::validate_frozen_child_set(&read).is_ok());
            assert_eq!(
                read.child_request_ids(),
                record.child_request_id.iter().cloned().collect::<Vec<_>>()
            );
        }
        let set = execution(None, &["a", "b"]);
        let encoded = serde_json::to_string(&set).unwrap();
        let read: FreshRootTerminalExecution = serde_json::from_str(&encoded).unwrap();
        assert_eq!(read.child_request_ids(), vec!["a".to_owned(), "b".to_owned()]);
    }

    /// Each member is named once, in request order, with its own W. A
    /// duplicate, a W naming another request, a one-member set, or a set
    /// mixed with the scalar form is refused.
    #[test]
    fn frozen_set_refuses_duplicate_foreign_or_mixed_members() {
        assert!(FreshV30Lane::validate_frozen_child_set(&execution(None, &["a", "b", "c"])).is_ok());
        for invalid in [
            execution(None, &["a", "a"]),
            execution(None, &["b", "a"]),
            execution(None, &["a"]),
            execution(Some("a"), &["b", "c"]),
        ] {
            assert!(FreshV30Lane::validate_frozen_child_set(&invalid).is_err());
        }
        let mut foreign = execution(None, &["a", "b"]);
        foreign.children[1].event.request_id = "a".into();
        assert!(FreshV30Lane::validate_frozen_child_set(&foreign).is_err());
        let mut half = execution(Some("a"), &[]);
        half.child_event = None;
        assert!(FreshV30Lane::validate_frozen_child_set(&half).is_err());
        let mut renamed = execution(Some("a"), &[]);
        renamed.child_event = Some(event("b", 0));
        assert!(FreshV30Lane::validate_frozen_child_set(&renamed).is_err());
    }

    /// The set settles only when every member has: a receipt without ACK,
    /// an unknown F, a pending F or a missing W keeps the whole set pending.
    #[test]
    fn child_set_notification_requires_every_member_settled() {
        let set = |states: &[&str]| {
            child_set_notification_state(&states.iter().map(|s| member(s)).collect::<Vec<_>>())
        };
        assert_eq!(set(&[]), "not_applicable");
        assert_eq!(set(&["response_only", "response_only"]), "response_only");
        assert_eq!(set(&["response_only", "acked"]), "acked");
        assert_eq!(set(&["acked", "acked", "acked"]), "acked");
        for pending in [
            "f_submitted_native_pending",
            "f_unknown",
            "pending_f",
            "awaiting_w",
            "repair_required",
        ] {
            assert_eq!(set(&["acked", pending]), "child_set_pending", "{pending}");
            assert_eq!(set(&[pending, "response_only"]), "child_set_pending", "{pending}");
        }
    }

    /// The root succeeds only if its parent and every member exited cleanly.
    #[test]
    fn root_outcome_accounts_for_every_member() {
        let ok = event("a", 0);
        let failed = event("b", 1 << 8);
        assert_eq!(physical_root_outcome(&parent(), &[]), "success");
        assert_eq!(physical_root_outcome(&parent(), &[&ok, &ok]), "success");
        assert_eq!(physical_root_outcome(&parent(), &[&ok, &failed]), "failure");
        assert_eq!(physical_root_outcome(&parent(), &[&failed, &ok]), "failure");
    }
}
