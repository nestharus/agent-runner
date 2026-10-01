//! The installed headless root receives its own async Bash completion while it
//! is still live. An F request ID and the actual returned bytes are durable
//! before the original recipient spends its one-use ACK token.

use base64::Engine as _;
use oulipoly_kernel_broker::protocol::{self, FreshRecipientRequest};
use oulipoly_state::mailbox::FreshRootTerminalReadback;
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
}

pub(crate) fn settle_pending_original(
    socket: &Path,
    d_key: &str,
    disposable_fixture: bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
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
    if terminal.listener_policy.as_deref() != Some("notify") {
        return Err("pending original F has no async notify listener".into());
    }
    let wake_request_id = terminal
        .child_request_id
        .as_ref()
        .ok_or("selected async W request absent")?;
    let event = terminal
        .selected_child_event
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
            || prior["decision"]["obligation"]["seq"].as_i64() != terminal.mailbox_seq
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
                &terminal,
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
    let intent_path = directory.join(format!("{d_key}.intent.json"));
    let intent = load_or_create_intent(&intent_path, d_key, &terminal)?;
    let request = FreshRecipientRequest::Submit {
        allocation_request_id: d_key.into(),
        delivery_request_id: intent.delivery_request_id.clone(),
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
    validate_payload(&terminal, grant, &payload, &sha)?;
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
    let terminal = read_terminal(socket, d_key)?;
    if terminal.notification_state != "acked"
        || terminal.ack_basis.as_deref() != Some("manual_ack")
        || terminal.delivery_grant_id.as_deref() != Some(grant_id)
        || serde_json::to_value(&terminal.original_receipt).map_err(|e| e.to_string())?
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
    terminal: &FreshRootTerminalReadback,
    prior_offer: Option<&str>,
    drop_start_reply: bool,
    send_negatives: bool,
) -> Result<(), String> {
    let wake_request_id = terminal
        .child_request_id
        .as_ref()
        .ok_or("selected successor W request absent")?;
    let offer_id = prior_offer
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let decision_request = FreshRecipientRequest::DecideBashWakeSuccessor {
        d_key: d_key.into(),
        wake_request_id: wake_request_id.clone(),
        offer_request_id: offer_id.clone(),
    };
    let decision = protocol::fresh_recipient_request_at(socket, &decision_request)
        .map_err(|e| format!("selected W decision refused: {e}"))?;
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
        || decision["decision"]["obligation"]["seq"].as_i64() != terminal.mailbox_seq
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
    let before_approval = read_terminal(socket, d_key)?;
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
        || offer["offer"]["seq"].as_i64() != terminal.mailbox_seq
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
        let terminal = read_terminal(socket, d_key)?;
        if terminal.notification_state == "acked"
            && terminal.ack_basis.as_deref() == Some("successor_receiver_receipt_ack")
            && terminal.successor_ack.is_some()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "installed successor F/ACK remains pending: {}",
                terminal.notification_state
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
    terminal: &FreshRootTerminalReadback,
    grant: &serde_json::Value,
    bytes: &[u8],
    sha: &str,
) -> Result<(), String> {
    if grant["root_id"] != terminal.root_id
        || grant["owner_generation"] != terminal.owner_generation
        || grant["session_id"] != terminal.session_id
        || grant["seq"].as_i64() != terminal.mailbox_seq
        || grant["payload_sha256"] != sha
        || grant["payload_byte_len"].as_u64() != Some(bytes.len() as u64)
        || terminal.delivery_payload_sha256.as_deref() != Some(sha)
    {
        return Err("async original F grant differs from selected terminal row".into());
    }
    let payload: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let source = &payload["source"];
    if payload["protocol"] != "fresh-bash-complete-v30"
        || source["request_id"].as_str() != terminal.child_request_id.as_deref()
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

fn load_or_create_intent(
    path: &Path,
    d_key: &str,
    terminal: &FreshRootTerminalReadback,
) -> Result<Intent, String> {
    let intended = Intent {
        d_key: d_key.into(),
        root_id: terminal.root_id.clone(),
        session_id: terminal.session_id.clone(),
        delivery_request_id: uuid::Uuid::new_v4().to_string(),
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
