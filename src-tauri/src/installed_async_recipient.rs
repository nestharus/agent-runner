//! The installed headless root receives its own async Bash completion while it
//! is still live. An F request ID and the actual returned bytes are durable
//! before the original recipient spends its one-use ACK token.

use base64::Engine as _;
use oulipoly_kernel_broker::protocol::{self, FreshRecipientRequest};
use oulipoly_state::mailbox::{FreshRootTerminalChildReadback, FreshRootTerminalReadback};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Intent {
    d_key: String,
    root_id: String,
    session_id: String,
    delivery_request_id: String,
    /// A child-set member's own intent names it. A one-child root keeps the
    /// original per-D file and form, so existing intents still recover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child_request_id: Option<String>,
}

/// One member's delivery: the root it answers to and the member's own row.
/// `targeted` is set for a child-set member; a one-child root keeps its
/// untargeted Submit and per-D intent.
struct Target<'a> {
    terminal: &'a FreshRootTerminalReadback,
    member: FreshRootTerminalChildReadback,
    targeted: bool,
}

impl Target<'_> {
    fn intent_name(&self, d_key: &str) -> String {
        if self.targeted {
            format!("{d_key}.{}.intent.json", self.member.request_id)
        } else {
            format!("{d_key}.intent.json")
        }
    }

    fn current(
        &self,
        terminal: &FreshRootTerminalReadback,
    ) -> Option<FreshRootTerminalChildReadback> {
        terminal.member(&self.member.request_id)
    }
}

/// Disposable fixture only: members whose receipt this recipient certified.
static RECEIPTS_CERTIFIED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn settle_pending_original(
    socket: &Path,
    d_key: &str,
    disposable_fixture: bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    // A root with two or more admitted C reads back as a child set even
    // before any member's W is accepted.
    if read_terminal(socket, d_key)?.notification_origin != "child_set" {
        return settle_pending_scalar(socket, d_key, disposable_fixture, deadline);
    }
    settle_pending_child_set(
        deadline,
        || read_terminal(socket, d_key),
        |terminal, member| {
            settle_member(
                socket,
                d_key,
                Target {
                    terminal,
                    member,
                    targeted: true,
                },
                disposable_fixture,
            )
        },
        || {
            protocol::fresh_root_terminal_request_at(
                socket,
                &FreshRecipientRequest::RepairRootTerminal {
                    d_key: d_key.into(),
                },
            )
            .map(|_| ())
            .map_err(|e| format!("async original member repair unknown: {e}"))
        },
        Instant::now,
        || std::thread::sleep(Duration::from_millis(50)),
    )
}

// Keep the production loop testable with exact readbacks and a deterministic
// clock; delivery, repair and wait still use the existing operations above.
fn settle_pending_child_set(
    mut deadline: Instant,
    mut read: impl FnMut() -> Result<FreshRootTerminalReadback, String>,
    mut settle: impl FnMut(
        &FreshRootTerminalReadback,
        FreshRootTerminalChildReadback,
    ) -> Result<(), String>,
    mut repair: impl FnMut() -> Result<(), String>,
    mut now: impl FnMut() -> Instant,
    mut wait: impl FnMut(),
) -> Result<(), String> {
    // A child set: each notify member is delivered, receipted and ACKed on
    // its own. A member already settled is never sent again.
    loop {
        let terminal = read()?;
        // Before the freeze a member whose W is not yet accepted is listed
        // only as unresolved; the set is not settled while any C waits.
        let complete = terminal.unresolved_child_request_ids.is_empty();
        match terminal.notification_state.as_str() {
            "response_only" | "acked" if complete => return Ok(()),
            "response_only" | "acked" | "child_set_pending" => {}
            // State aggregates no selected members as N/A. Before freeze it
            // also reports execution_evidence_incomplete: that still refuses
            // closure, but this admitted, unresolved notification set can wait.
            "not_applicable"
                if terminal.notification_origin == "child_set"
                    && terminal.execution.is_none()
                    && terminal.execution_state == "unknown"
                    && terminal.terminal_state == "execution_unknown"
                    && terminal.children.is_empty()
                    && terminal.unresolved_child_request_ids.len() >= 2
                    && terminal.late_child_request_ids.is_empty()
                    && terminal.refusal.as_deref() == Some("execution_evidence_incomplete") => {}
            other => return Err(format!("async original child set state unknown: {other}")),
        }
        let mut progressed = false;
        for member in &terminal.children {
            match member.notification_state.as_str() {
                "not_applicable" | "response_only" | "acked" => {}
                "pending_f" | "f_unknown" | "f_submitted_native_pending" => {
                    settle(&terminal, member.clone())?;
                    progressed = true;
                    break;
                }
                "repair_required" => {
                    repair()?;
                }
                "awaiting_w" if member.listener_policy.as_deref() == Some("notify") => {}
                other => {
                    return Err(format!(
                        "async original member {} state unknown: {other}",
                        member.request_id
                    ));
                }
            }
        }
        if progressed {
            // The wait for a next deliverable member restarts after each
            // settled one; a set is not bounded by one member's budget.
            deadline = now() + Duration::from_secs(120);
            continue;
        }
        if now() >= deadline {
            return Err("async original child set did not become deliverable".into());
        }
        wait();
    }
}

fn settle_pending_scalar(
    socket: &Path,
    d_key: &str,
    disposable_fixture: bool,
    deadline: Instant,
) -> Result<(), String> {
    let terminal = loop {
        let terminal = read_terminal(socket, d_key)?;
        match terminal.notification_state.as_str() {
            "not_applicable"
                if terminal.listener_policy.as_deref() == Some("notify")
                    || terminal.child_request_id.is_some()
                    || !terminal.unresolved_child_request_ids.is_empty() => {}
            "not_applicable" | "response_only" | "acked" => return Ok(()),
            "pending_f" | "f_unknown" | "f_submitted_native_pending" => break terminal,
            "repair_required" => {
                protocol::fresh_root_terminal_request_at(
                    socket,
                    &FreshRecipientRequest::RepairRootTerminal {
                        d_key: d_key.into(),
                    },
                )
                .map_err(|e| format!("async original terminal repair unknown: {e}"))?;
            }
            "awaiting_w" if terminal.listener_policy.as_deref() == Some("notify") => {}
            other => return Err(format!("async original terminal state unknown: {other}")),
        }
        if Instant::now() >= deadline {
            return Err("async original selected W did not become deliverable".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let member = terminal
        .members()
        .into_iter()
        .next()
        .ok_or("selected async W request absent")?;
    settle_member(
        socket,
        d_key,
        Target {
            terminal: &terminal,
            member,
            targeted: false,
        },
        disposable_fixture,
    )
}

fn settle_member(
    socket: &Path,
    d_key: &str,
    target: Target<'_>,
    disposable_fixture: bool,
) -> Result<(), String> {
    let terminal = target.terminal;
    let member = &target.member;
    if member.listener_policy.as_deref() != Some("notify") {
        return Err("pending original F has no async notify listener".into());
    }
    let wake_request_id = &member.request_id;
    let event = member
        .selected_event
        .as_ref()
        .ok_or("selected async W physical event absent")?;
    if event.request_id != *wake_request_id || event.root_id != terminal.root_id {
        return Err("selected async W physical event changed".into());
    }
    let prior = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::ReadBashWakeSuccessorDecision {
            d_key: d_key.into(),
            wake_request_id: wake_request_id.clone(),
        },
    )
    .map_err(|e| format!("selected W decision readback unknown: {e}"))?;
    if prior["kind"] != "bash_wake_successor_decision_readback" {
        return Err("selected W decision readback kind changed".into());
    }
    let prior_offer = if prior["decision"].is_null() {
        None
    } else {
        if prior["decision"]["wake_request_id"] != *wake_request_id
            || prior["decision"]["obligation"]["root_id"] != terminal.root_id
            || prior["decision"]["obligation"]["session_id"] != terminal.session_id
            || prior["decision"]["obligation"]["seq"].as_i64() != member.mailbox_seq
        {
            return Err("selected W prior decision changed terminal row".into());
        }
        Some(
            prior["decision"]["offer_request_id"]
                .as_str()
                .ok_or("selected W prior offer ID absent")?,
        )
    };
    let selected = event
        .normal_provider_selection
        .as_ref()
        .ok_or("selected W provider lifecycle evidence absent")?;
    match selected.mode.as_str() {
        "sleeping"
            if selected.provider.is_none()
                && selected.provider_local_pid.is_none()
                && selected.provider_wait_status.is_some() =>
        {
            return settle_pending_successor(
                socket,
                d_key,
                &target,
                prior_offer,
                disposable_fixture
                    && std::env::var_os("AGE319_TEST_FEATURELESS_DROP_START_REPLY_V1").is_some(),
                disposable_fixture
                    && std::env::var_os("AGE319_TEST_FEATURELESS_SUCCESSOR_NEGATIVES_V1").is_some(),
            );
        }
        "busy"
            if selected.provider.is_some()
                && selected.provider_local_pid.is_some_and(|pid| pid > 1)
                && selected.provider_wait_status.is_none() =>
        {
            if prior_offer.is_some() {
                return Err("busy W already has a successor decision".into());
            }
        }
        _ => return Err("selected W provider lifecycle evidence malformed".into()),
    }
    let directory = receipt_directory()?;
    let intent_path = directory.join(target.intent_name(d_key));
    let intent = load_or_create_intent(&intent_path, d_key, &target)?;
    let request = FreshRecipientRequest::Submit {
        allocation_request_id: d_key.into(),
        delivery_request_id: intent.delivery_request_id.clone(),
        child_request_id: intent.child_request_id.clone(),
    };
    let submitted = if disposable_fixture
        && std::env::var_os("AGE319_TEST_FEATURELESS_DROP_F_REPLY_V1").is_some()
    {
        protocol::fresh_recipient_request_without_reply_at(socket, &request)
            .map_err(|e| format!("async original F send unknown: {e}"))?;
        Err(std::io::Error::other("test dropped F reply"))
    } else {
        protocol::fresh_recipient_request_at(socket, &request)
    };
    let reply = match submitted {
        Ok(reply) => reply,
        Err(send_error) => recover_delivery(socket, &intent.delivery_request_id, &send_error)?,
    };
    if reply["kind"] != "delivery" && reply["kind"] != "recovered_delivery" {
        return Err("async original F reply kind changed".into());
    }
    let grant = &reply["grant"];
    let encoded = reply["payload_base64"]
        .as_str()
        .ok_or("async original F bytes absent")?;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| e.to_string())?;
    let sha = format!("{:x}", Sha256::digest(&payload));
    validate_payload(&target, grant, &payload, &sha)?;
    let grant_id = grant["grant_id"]
        .as_str()
        .ok_or("async F grant ID absent")?;
    if protocol::persist_original_receiver_receipt(&reply, &intent.delivery_request_id)
        .map_err(|e| format!("async original receipt write unknown: {e}"))?
        != payload
    {
        return Err("async original receipt differs from actual F bytes".into());
    }
    let certified = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::CertifyOriginalReceipt {
            grant_id: grant_id.into(),
        },
    )
    .or_else(|_| {
        protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::ReadOriginalReceipt {
                grant_id: grant_id.into(),
            },
        )
    })
    .map_err(|e| format!("async original receipt certification unknown: {e}"))?;
    let receipt_identity = certified["receipt"].clone();
    if receipt_identity["grant_id"] != grant_id
        || receipt_identity["receipt_path"] != reply["receipt_path"]
        || receipt_identity["receipt_sha256"].as_str().is_none()
    {
        return Err("async original receipt certification changed F".into());
    }
    let certified_members =
        RECEIPTS_CERTIFIED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if disposable_fixture
        && certified_members == 2
        && std::env::var_os("AGE381_TEST_FEATURELESS_STOP_AFTER_SECOND_RECEIPT_V1").is_some()
    {
        return Err("disposable recipient stopped after a receipt, before its ACK".into());
    }
    let token = grant["delivery_token"]
        .as_str()
        .ok_or("async F token absent")?;
    let ack_request = FreshRecipientRequest::Acknowledge {
        grant_id: grant_id.into(),
        delivery_token: token.into(),
    };
    let ack = if disposable_fixture
        && std::env::var_os("AGE319_TEST_FEATURELESS_DROP_ACK_REPLY_V1").is_some()
    {
        protocol::fresh_recipient_request_without_reply_at(socket, &ack_request)
            .map_err(|e| format!("async original ACK send unknown: {e}"))?;
        Err(std::io::Error::other("test dropped ACK reply"))
    } else {
        protocol::fresh_recipient_request_at(socket, &ack_request)
    };
    let read = read_acked_grant(socket, &intent.delivery_request_id)?;
    if read["kind"] != "readback"
        || read["grant"]["grant_id"] != grant_id
        || read["grant"]["phase"] != "acked"
        || read["receipt"] != receipt_identity
        || ack
            .as_ref()
            .is_ok_and(|reply| reply["grant"] != read["grant"])
    {
        return Err("async original ACK readback did not settle exact F".into());
    }
    if disposable_fixture
        && std::env::var_os("AGE319_TEST_FEATURELESS_REPLAY_ACK_V1").is_some()
        && protocol::fresh_recipient_request_at(socket, &ack_request).is_ok()
    {
        return Err("async original F token accepted twice".into());
    }
    let settled = target
        .current(&read_terminal(socket, d_key)?)
        .ok_or("async original member left its root")?;
    if settled.notification_state != "acked"
        || settled.ack_basis.as_deref() != Some("manual_ack")
        || settled.delivery_grant_id.as_deref() != Some(grant_id)
        || serde_json::to_value(&settled.original_receipt).map_err(|e| e.to_string())?
            != serde_json::json!(receipt_identity)
    {
        return Err("async original terminal did not join exact ACK".into());
    }
    Ok(())
}

/// Original D-bound Runner alone chooses, starts and approves one installed
/// candidate. The Broker owns both the durable start and the actual spawn.
fn settle_pending_successor(
    socket: &Path,
    d_key: &str,
    target: &Target<'_>,
    prior_offer: Option<&str>,
    drop_start_reply: bool,
    send_negatives: bool,
) -> Result<(), String> {
    let terminal = target.terminal;
    let member = &target.member;
    let wake_request_id = &member.request_id;
    let offer_id = prior_offer
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let decision_request = FreshRecipientRequest::DecideBashWakeSuccessor {
        d_key: d_key.into(),
        wake_request_id: wake_request_id.clone(),
        offer_request_id: offer_id.clone(),
    };
    // The selected W's row is readable an instant before its wake
    // obligation is recorded. The decision is one immutable choice per W
    // with this same offer ID, so asking again only waits for that record.
    let obligation_deadline = Instant::now() + Duration::from_secs(10);
    let decision = loop {
        match protocol::fresh_recipient_request_at(socket, &decision_request) {
            Ok(decision) => break decision,
            Err(error)
                if error
                    .to_string()
                    .contains("selected Bash wake obligation absent")
                    && Instant::now() < obligation_deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("selected W decision refused: {error}")),
        }
    };
    let decision_read = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::ReadBashWakeSuccessorDecision {
            d_key: d_key.into(),
            wake_request_id: wake_request_id.clone(),
        },
    )
    .map_err(|e| format!("selected W decision readback unknown: {e}"))?;
    if decision["decision"] != decision_read["decision"]
        || decision["decision"]["offer_request_id"] != offer_id
        || decision["decision"]["obligation"]["root_id"] != terminal.root_id
        || decision["decision"]["obligation"]["session_id"] != terminal.session_id
        || decision["decision"]["obligation"]["seq"].as_i64() != member.mailbox_seq
    {
        return Err("selected W decision changed terminal row".into());
    }
    let start_request = FreshRecipientRequest::StartInstalledSuccessor {
        d_key: d_key.into(),
        offer_request_id: offer_id.clone(),
    };
    let started = if drop_start_reply {
        protocol::fresh_recipient_request_without_reply_at(socket, &start_request)
            .map_err(|e| format!("installed successor start send unknown: {e}"))?;
        Err(std::io::Error::other("disposable start reply dropped"))
    } else {
        protocol::fresh_recipient_request_at(socket, &start_request)
    };
    let start = started
        .or_else(|start_error| {
            let readback = protocol::fresh_recipient_request_at(
                socket,
                &FreshRecipientRequest::ReadInstalledSuccessorStart {
                    d_key: d_key.into(),
                    offer_request_id: offer_id.clone(),
                },
            )?;
            if !readback["candidate"].is_object() {
                return Err(std::io::Error::other(format!(
                    "start was spent without a candidate: {start_error}"
                )));
            }
            Ok(readback)
        })
        .map_err(|e| format!("installed successor start unknown: {e}"))?;
    let read_start = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::ReadInstalledSuccessorStart {
            d_key: d_key.into(),
            offer_request_id: offer_id.clone(),
        },
    )
    .map_err(|e| format!("installed successor start readback unknown: {e}"))?;
    if !read_start["candidate"].is_object()
        || read_start["start"] != start["start"]
        || read_start["candidate"] != start["candidate"]
        || read_start["start"]["decision"] != decision["decision"]
        || read_start["candidate"]["process"]["host_pid"]
            == read_start["start"]["original"]["host_pid"]
    {
        return Err("installed successor start changed exact W or original process".into());
    }
    let before_approval = target
        .current(&read_terminal(socket, d_key)?)
        .ok_or("selected successor member left its root")?;
    if before_approval.notification_state != "pending_f"
        || before_approval.successor_ack.is_some()
        || before_approval.ack_basis.is_some()
    {
        return Err("installed candidate start became delivery authority".into());
    }
    let prior_admission = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::ReadSuccessorAdmission {
            offer_request_id: offer_id.clone(),
        },
    )
    .map_err(|e| format!("installed candidate admission readback unknown: {e}"))?;
    if prior_admission["admission"].is_object()
        && prior_admission["admission"]["offer"]["offer_request_id"] != offer_id
    {
        return Err("installed candidate prior admission changed offer".into());
    }
    // The Broker refuses a replayed start and a wrong-offer start or approval
    // whatever this client sends. That refusal is tested against the real
    // Broker from a disposable fixture, not re-proved on every handoff.
    if send_negatives {
        require_successor_negatives_refused(socket, d_key, &start_request)?;
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let offer = loop {
        match protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::ReadInstalledSuccessorOffer {
                d_key: d_key.into(),
                offer_request_id: offer_id.clone(),
            },
        ) {
            Ok(reply) if reply["offer"].is_object() => break reply,
            _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            other => return Err(format!("installed successor offer absent: {other:?}")),
        }
    };
    if offer["offer"]["successor_identity"]["host_pid"]
        != read_start["candidate"]["process"]["host_pid"]
        || offer["offer"]["root_id"] != terminal.root_id
        || offer["offer"]["seq"].as_i64() != member.mailbox_seq
    {
        return Err("installed successor offer differs from selected W/start".into());
    }
    let admission = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::AdmitSuccessor {
            d_key: d_key.into(),
            offer_request_id: offer_id.clone(),
        },
    )
    .or_else(|_| {
        protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::ReadSuccessorAdmission {
                offer_request_id: offer_id.clone(),
            },
        )
    })
    .map_err(|e| format!("installed successor admission unknown: {e}"))?;
    let admission_read = protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::ReadSuccessorAdmission {
            offer_request_id: offer_id,
        },
    )
    .map_err(|e| format!("installed successor admission readback unknown: {e}"))?;
    if admission["admission"] != admission_read["admission"] {
        return Err("installed successor admission changed".into());
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let current = target
            .current(&read_terminal(socket, d_key)?)
            .ok_or("selected successor member left its root")?;
        if current.notification_state == "acked"
            && current.ack_basis.as_deref() == Some("successor_receiver_receipt_ack")
            && current.successor_ack.is_some()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "installed successor F/ACK remains pending: {}",
                current.notification_state
            ));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}

/// Disposable fixture only: this exact original sends a replayed start, a
/// start for a wrong offer and an approval of a wrong offer. Any acceptance
/// is a failure of the handoff.
fn require_successor_negatives_refused(
    socket: &Path,
    d_key: &str,
    start_request: &FreshRecipientRequest,
) -> Result<(), String> {
    if protocol::fresh_recipient_request_at(socket, start_request).is_ok()
        || protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::StartInstalledSuccessor {
                d_key: d_key.into(),
                offer_request_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .is_ok()
    {
        return Err("installed successor start replay or wrong offer accepted".into());
    }
    if protocol::fresh_recipient_request_at(
        socket,
        &FreshRecipientRequest::AdmitSuccessor {
            d_key: d_key.into(),
            offer_request_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .is_ok()
    {
        return Err("installed successor wrong approval accepted".into());
    }
    Ok(())
}

fn recover_delivery(
    socket: &Path,
    request_id: &str,
    sent: &std::io::Error,
) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Recover {
                delivery_request_id: request_id.into(),
            },
        ) {
            Ok(reply) => return Ok(reply),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(format!(
                    "async original F unknown: {sent}; recovery: {error}"
                ));
            }
        }
    }
}

fn read_acked_grant(socket: &Path, request_id: &str) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match protocol::fresh_recipient_request_at(
            socket,
            &FreshRecipientRequest::Read {
                delivery_request_id: request_id.into(),
            },
        ) {
            Ok(read) if read["grant"]["phase"] == "acked" => return Ok(read),
            Ok(_) | Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            other => return Err(format!("async original ACK readback unknown: {other:?}")),
        }
    }
}

fn read_terminal(socket: &Path, d_key: &str) -> Result<FreshRootTerminalReadback, String> {
    protocol::fresh_root_terminal_request_at(
        socket,
        &FreshRecipientRequest::ReadRootTerminal {
            d_key: d_key.into(),
        },
    )
    .map_err(|e| format!("async original terminal read unknown: {e}"))
}

fn validate_payload(
    target: &Target<'_>,
    grant: &serde_json::Value,
    bytes: &[u8],
    sha: &str,
) -> Result<(), String> {
    let terminal = target.terminal;
    let member = &target.member;
    if grant["root_id"] != terminal.root_id
        || grant["owner_generation"] != terminal.owner_generation
        || grant["session_id"] != terminal.session_id
        || grant["seq"].as_i64() != member.mailbox_seq
        || grant["payload_sha256"] != sha
        || grant["payload_byte_len"].as_u64() != Some(bytes.len() as u64)
        || member.delivery_payload_sha256.as_deref() != Some(sha)
    {
        return Err("async original F grant differs from selected terminal row".into());
    }
    let payload: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let source = &payload["source"];
    if payload["protocol"] != "fresh-bash-complete-v30"
        || source["request_id"].as_str() != Some(member.request_id.as_str())
        || source["root_id"] != terminal.root_id
        || source["source_id"] != grant["source_id"]
        || source["attempt_id"] != grant["attempt_id"]
        || source["lane_id"] != grant["lane_id"]
        || source["source_generation"] != grant["source_generation"]
        || source["tree_drained"] != true
        || source["output_closed"] != true
    {
        return Err("async original F payload differs from selected Bash W".into());
    }
    for stream in ["stdout", "stderr"] {
        let raw: Vec<u8> = serde_json::from_value(payload[format!("{stream}_bytes")].clone())
            .map_err(|e| e.to_string())?;
        if source[format!("{stream}_len")].as_u64() != Some(raw.len() as u64)
            || source[format!("{stream}_sha256")] != format!("{:x}", Sha256::digest(&raw))
        {
            return Err(format!("async original {stream} differs from Bash W"));
        }
    }
    Ok(())
}

fn receipt_directory() -> Result<PathBuf, String> {
    let directory = oulipoly_state::paths::data_dir()?.join("async-recipient-receipts");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() } {
        return Err("async recipient receipt directory untrusted".into());
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    Ok(directory)
}

fn load_or_create_intent(path: &Path, d_key: &str, target: &Target<'_>) -> Result<Intent, String> {
    let terminal = target.terminal;
    let child_request_id = target.targeted.then(|| target.member.request_id.clone());
    let intended = Intent {
        d_key: d_key.into(),
        root_id: terminal.root_id.clone(),
        session_id: terminal.session_id.clone(),
        delivery_request_id: uuid::Uuid::new_v4().to_string(),
        child_request_id: child_request_id.clone(),
    };
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(mut file) => {
            serde_json::to_writer(&mut file, &intended).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            File::open(path.parent().unwrap())
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(intended)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing: Intent = read_exact(path)?;
            if existing.d_key != d_key
                || existing.root_id != terminal.root_id
                || existing.session_id != terminal.session_id
                || existing.child_request_id != child_request_id
            {
                return Err("async original persisted request changed D binding".into());
            }
            Ok(existing)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn read_exact<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o777 != 0o400
        || meta.len() > 1024 * 1024
    {
        return Err("async original persisted receipt file untrusted".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod intent_tests {
    use super::*;

    fn terminal() -> FreshRootTerminalReadback {
        serde_json::from_value(serde_json::json!({
            "handoff_id": "handoff", "d_key": "d", "invocation_uuid": "j",
            "session_id": "session", "root_id": "root", "owner_generation": "owner",
            "actor": {"host_pid": 10, "boot_id": "boot", "starttime_ticks": 20,
                      "pidns_dev": 30, "pidns_ino": 40},
            "execution": null, "execution_state": "unknown",
            "terminal_state": "execution_unknown", "notification_state": "child_set_pending",
            "notification_origin": "child_set", "native_receipt_state": "not_observed",
            "listener_policy": null, "child_request_id": null,
            "unresolved_child_request_ids": [], "mailbox_seq": null,
            "delivery_request_id": null, "delivery_grant_id": null,
            "delivery_payload_sha256": null, "delivery_payload_byte_len": null,
            "ack_basis": null, "publication_state": "not_started",
            "publication_sha256": null, "unknown_stage": null, "unknown_stages": [],
            "refusal": null, "artifacts": []
        }))
        .unwrap()
    }

    fn member(request_id: &str) -> FreshRootTerminalChildReadback {
        serde_json::from_value(serde_json::json!({
            "request_id": request_id, "selected_event": null, "listener_policy": "notify",
            "notification_state": "pending_f", "notification_origin": "original_c_notify",
            "mailbox_seq": 1, "delivery_request_id": null, "delivery_grant_id": null,
            "delivery_payload_sha256": null, "delivery_payload_byte_len": null,
            "ack_basis": null
        }))
        .unwrap()
    }

    fn unselected_set() -> FreshRootTerminalReadback {
        unselected_set_with_ids(vec!["a".into(), "b".into()])
    }

    fn unselected_set_with_ids(ids: Vec<String>) -> FreshRootTerminalReadback {
        let mut readback = terminal();
        readback.notification_state = "not_applicable".into();
        // read_private_root_terminal records each unresolved C first, then
        // terminal_commit_absent when parent Q is complete but freeze awaits W.
        readback.unknown_stages = ids
            .iter()
            .map(|id| format!("child_c_unresolved:{id}"))
            .collect();
        readback
            .unknown_stages
            .push("terminal_commit_absent".into());
        readback.unknown_stage = readback.unknown_stages.last().cloned();
        readback.refusal = Some("execution_evidence_incomplete".into());
        readback.artifacts = vec!["released-d:d".into(), "invocation-j:j".into()];
        readback
            .artifacts
            .extend(ids.iter().map(|id| format!("unresolved-child-c:{id}")));
        readback.unresolved_child_request_ids = ids;
        readback
    }

    #[test]
    fn pending_dto_tracks_state_producer_and_still_refuses_closure() {
        // Source/DTO relationship control, not a native State/Broker run. Keep
        // the scripted positive input tied to the actual producer's branches;
        // a changed producer needs this fixture and its meaning reconsidered.
        let state = include_str!("../../crates/oulipoly-state/src/mailbox/fresh_root_terminal.rs");
        assert!(state.contains("if children.is_empty() {\n        \"not_applicable\""));
        let producer = state
            .split("pub fn read_private_root_terminal(")
            .nth(1)
            .unwrap()
            .split("fn validate_frozen_child_set(")
            .next()
            .unwrap();
        for source in [
            "execution: None,",
            "execution_state: \"unknown\".into(),",
            "None => (set.selected.clone(), set.unresolved.clone(), Vec::new()),",
            ".map_or(set.admitted() >= 2, |stored| !stored.children.is_empty());",
            "format!(\"child_c_unresolved:{id}\")",
            "result.unresolved_child_request_ids = unresolved;",
            "_ => \"terminal_commit_absent\".into(),",
            "(Err(e), _) => format!(\"parent_k_q:{e}\"),",
            "result.notification_origin = \"child_set\".into();",
            "result.notification_state = child_set_notification_state(&result.children).into();",
            "result.terminal_state = if result.execution_state == \"unknown\" {\n            result.refusal = Some(\"execution_evidence_incomplete\".into());\n            \"execution_unknown\"",
        ] {
            assert!(
                producer.contains(source),
                "State producer changed: {source}"
            );
        }
        let pending = unselected_set();
        // Broker serializes this DTO; protocol deserializes it unchanged.
        let decoded: FreshRootTerminalReadback =
            serde_json::from_slice(&serde_json::to_vec(&pending).unwrap()).unwrap();
        assert_eq!(decoded, pending);
        assert_eq!(decoded.notification_state, "not_applicable");
        assert_eq!(decoded.notification_origin, "child_set");
        assert_eq!(
            decoded.unknown_stages,
            [
                "child_c_unresolved:a",
                "child_c_unresolved:b",
                "terminal_commit_absent"
            ]
        );
        assert_eq!(
            decoded.unknown_stage.as_deref(),
            Some("terminal_commit_absent")
        );
        assert_eq!(
            decoded.refusal.as_deref(),
            Some("execution_evidence_incomplete")
        );
        assert_eq!(
            decoded.closure_refusal(),
            Some("execution_evidence_incomplete")
        );
        let (result, settled, waits, _) = run_set(
            vec![Ok(decoded.clone()), Ok(decoded)],
            Duration::from_secs(120),
        );
        assert_eq!(
            result.unwrap_err(),
            "async original child set did not become deliverable"
        );
        assert!(settled.is_empty());
        assert_eq!(waits, 1);
    }

    // This drives the product loop, not a parallel classifier. Reads and time
    // are explicit; settlement stands in for the unchanged F/receipt/ACK path.
    fn run_set(
        reads: Vec<Result<FreshRootTerminalReadback, String>>,
        wait_step: Duration,
    ) -> (Result<(), String>, Vec<String>, usize, usize) {
        let start = Instant::now();
        let clock = std::cell::Cell::new(start);
        let waits = std::cell::Cell::new(0);
        let mut reads = std::collections::VecDeque::from(reads);
        let mut settled = Vec::new();
        let result = settle_pending_child_set(
            start + Duration::from_secs(120),
            || reads.pop_front().expect("unexpected terminal read"),
            |_, member| {
                settled.push(member.request_id);
                Ok(())
            },
            || panic!("unexpected repair"),
            || clock.get(),
            || {
                waits.set(waits.get() + 1);
                clock.set(clock.get() + wait_step);
            },
        );
        (result, settled, waits.get(), reads.len())
    }

    #[test]
    fn admitted_zero_selected_set_waits_then_settles_each_member_once() {
        // Two is the C1 window; ten also exercises the historical member count
        // without pretending to run the installed ten-root workload.
        for count in [2, 10] {
            let ids: Vec<String> = (0..count).map(|i| format!("child-{i}")).collect();
            let pending = unselected_set_with_ids(ids.clone());
            // The other producer branch records incomplete parent Q while
            // the original remains live. It likewise cannot certify closure.
            let mut parent_pending = pending.clone();
            let stage = "parent_k_q:parent physical Q incomplete or changed".to_owned();
            *parent_pending.unknown_stages.last_mut().unwrap() = stage.clone();
            parent_pending.unknown_stage = Some(stage);
            let mut reads = vec![Ok(parent_pending), Ok(pending)];
            let mut selected = terminal();
            selected.children = ids.iter().map(|id| member(id)).collect();
            for index in 0..count {
                reads.push(Ok(selected.clone()));
                selected.children[index].notification_state = "acked".into();
            }
            selected.notification_state = "acked".into();
            reads.push(Ok(selected));
            let (result, settled, waits, remaining) = run_set(reads, Duration::from_secs(1));
            assert_eq!(result, Ok(()));
            assert_eq!(settled, ids);
            assert_eq!(waits, 2);
            assert_eq!(remaining, 0);
        }
    }

    #[test]
    fn unselected_set_fails_at_existing_deadline_without_delivery() {
        let pending = unselected_set();
        let (result, settled, waits, remaining) = run_set(
            vec![Ok(pending.clone()), Ok(pending)],
            Duration::from_secs(120),
        );
        assert_eq!(
            result.unwrap_err(),
            "async original child set did not become deliverable"
        );
        assert!(settled.is_empty());
        assert_eq!(waits, 1);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn inapplicable_unknown_unbound_and_gone_sets_refuse_without_wait_or_delivery() {
        let mut invalid = Vec::new();
        for state in ["unknown", "awaiting_w", "gone", "execution_unknown"] {
            let mut readback = unselected_set();
            readback.notification_state = state.into();
            invalid.push(Ok(readback));
        }
        for count in [0, 1] {
            let mut readback = unselected_set();
            readback.unresolved_child_request_ids.truncate(count);
            invalid.push(Ok(readback));
        }
        let mut readback = unselected_set();
        readback.notification_origin = "none".into();
        invalid.push(Ok(readback));
        let mut readback = unselected_set();
        readback.children.push(member("selected"));
        invalid.push(Ok(readback));
        let mut readback = unselected_set();
        readback.late_child_request_ids.push("a".into());
        invalid.push(Ok(readback));
        let mut readback = unselected_set();
        readback.refusal = Some("original_gone".into());
        invalid.push(Ok(readback));
        for refusal in [
            None,
            Some("unresolved_child_admission"),
            Some("child_admission_after_freeze"),
        ] {
            let mut readback = unselected_set();
            readback.refusal = refusal.map(str::to_owned);
            invalid.push(Ok(readback));
        }
        let mut readback = unselected_set();
        readback.execution_state = "success".into();
        invalid.push(Ok(readback));
        let mut readback = unselected_set();
        readback.terminal_state = "execution_completed".into();
        invalid.push(Ok(readback));
        let mut readback = unselected_set();
        readback.execution = Some(
            serde_json::from_value(serde_json::json!({
                "handoff_id": "handoff", "d_key": "d", "invocation_uuid": "j",
                "session_id": "session", "root_id": "root", "owner_generation": "owner",
                "actor": readback.actor,
                "parent": {
                    "grant_id": "k", "work_id": "q", "plan_sha256": "plan",
                    "wait_status": 0, "outcome": "success", "cancelled": false,
                    "stdout_sha256": "stdout", "stdout_len": 0,
                    "stderr_sha256": "stderr", "stderr_len": 0
                },
                "child_request_id": null, "child_event": null, "outcome": "success"
            }))
            .unwrap(),
        );
        invalid.push(Ok(readback));
        // Binding/gone errors from the reader must propagate, never become N/A.
        invalid.push(Err("root binding absent".into()));
        invalid.push(Err("original gone".into()));
        for readback in invalid {
            let (result, settled, waits, remaining) = run_set(vec![readback], Duration::ZERO);
            assert!(result.is_err());
            assert!(settled.is_empty());
            assert_eq!(waits, 0);
            assert_eq!(remaining, 0);
        }
    }

    #[test]
    fn settled_members_do_not_complete_a_set_with_unresolved_admissions() {
        for state in ["response_only", "acked"] {
            let mut readback = terminal();
            readback.notification_state = state.into();
            let mut child = member("a");
            child.notification_state = state.into();
            readback.children.push(child);
            readback.unresolved_child_request_ids.push("b".into());
            let (result, settled, waits, _) = run_set(
                vec![Ok(readback.clone()), Ok(readback)],
                Duration::from_secs(120),
            );
            assert_eq!(
                result.unwrap_err(),
                "async original child set did not become deliverable"
            );
            assert!(settled.is_empty());
            assert_eq!(waits, 1);
        }
    }

    #[test]
    fn next_member_keeps_its_existing_deadline_budget_after_progress() {
        let mut first = terminal();
        first.children.push(member("a"));
        first.unresolved_child_request_ids.push("b".into());
        let mut between = first.clone();
        between.children[0].notification_state = "acked".into();
        between.notification_state = "acked".into();
        let mut second = between.clone();
        second.children.push(member("b"));
        second.unresolved_child_request_ids.clear();
        second.notification_state = "child_set_pending".into();
        let mut done = second.clone();
        done.children[1].notification_state = "acked".into();
        done.notification_state = "acked".into();
        let (result, settled, waits, remaining) = run_set(
            vec![
                Ok(unselected_set()),
                Ok(first),
                Ok(between),
                Ok(second),
                Ok(done),
            ],
            Duration::from_secs(119),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(settled, ["a", "b"]);
        assert_eq!(waits, 2);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn complete_quiet_sets_return_and_mixed_members_keep_their_delivery_path() {
        for state in ["response_only", "acked"] {
            let mut readback = terminal();
            readback.notification_state = state.into();
            let (result, settled, waits, _) = run_set(vec![Ok(readback)], Duration::ZERO);
            assert_eq!(result, Ok(()));
            assert!(settled.is_empty());
            assert_eq!(waits, 0);
        }
        let mut readback = terminal();
        readback.children = vec![member("quiet"), member("done"), member("notify")];
        readback.children[0].notification_state = "response_only".into();
        readback.children[1].notification_state = "acked".into();
        let first = readback.clone();
        readback.children[2].notification_state = "acked".into();
        readback.notification_state = "acked".into();
        let (result, settled, waits, _) = run_set(vec![Ok(first), Ok(readback)], Duration::ZERO);
        assert_eq!(result, Ok(()));
        assert_eq!(settled, ["notify"]);
        assert_eq!(waits, 0);
    }

    fn write_0400(path: &Path, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o400)
            .open(path)
            .unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
    }

    /// An intent written by the one-child recipient before child sets is
    /// recovered unchanged, at its own path, with its own delivery request.
    /// Each set member has its own intent, which no other member or the
    /// one-child form can take over.
    #[test]
    fn one_child_intent_recovers_unchanged_and_member_intents_stay_their_own() {
        let directory = tempfile::tempdir().unwrap();
        let terminal = terminal();
        let scalar = Target {
            terminal: &terminal,
            member: member("a"),
            targeted: false,
        };
        assert_eq!(scalar.intent_name("d"), "d.intent.json");
        let historical =
            br#"{"d_key":"d","root_id":"root","session_id":"session","delivery_request_id":"r1"}"#;
        let scalar_path = directory.path().join(scalar.intent_name("d"));
        write_0400(&scalar_path, historical);
        let recovered = load_or_create_intent(&scalar_path, "d", &scalar).unwrap();
        assert_eq!(recovered.delivery_request_id, "r1");
        assert_eq!(recovered.child_request_id, None);
        assert_eq!(serde_json::to_vec(&recovered).unwrap(), historical);

        let first = Target {
            terminal: &terminal,
            member: member("a"),
            targeted: true,
        };
        let second = Target {
            terminal: &terminal,
            member: member("b"),
            targeted: true,
        };
        assert_eq!(first.intent_name("d"), "d.a.intent.json");
        assert_ne!(first.intent_name("d"), second.intent_name("d"));
        let first_path = directory.path().join(first.intent_name("d"));
        let created = load_or_create_intent(&first_path, "d", &first).unwrap();
        assert_eq!(created.child_request_id.as_deref(), Some("a"));
        assert_ne!(created.delivery_request_id, "r1");
        assert!(load_or_create_intent(&first_path, "d", &first).unwrap() == created);
        // Neither form can be recovered as another member's request.
        assert!(load_or_create_intent(&first_path, "d", &second).is_err());
        assert!(load_or_create_intent(&scalar_path, "d", &first).is_err());
        assert!(load_or_create_intent(&first_path, "d", &scalar).is_err());
    }
}
