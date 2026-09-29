//! The installed headless root receives its own async Bash completion while it
//! is still live. An F request ID and the actual returned bytes are durable
//! before the original recipient spends its one-use ACK token.

use base64::Engine as _;
use oulipoly_kernel_broker::protocol::{self, FreshRecipientRequest};
use oulipoly_state::mailbox::FreshRootTerminalReadback;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
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

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Receipt {
    intent: Intent,
    grant: serde_json::Value,
    payload_base64: String,
    observed_sha256: String,
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
    let receipt = Receipt {
        intent: intent.clone(),
        grant: grant.clone(),
        payload_base64: encoded.into(),
        observed_sha256: sha,
    };
    let grant_id = grant["grant_id"]
        .as_str()
        .ok_or("async F grant ID absent")?;
    persist_exact(
        &directory.join(format!("{grant_id}.receipt.json")),
        &receipt,
    )?;
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
    {
        return Err("async original terminal did not join exact ACK".into());
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

fn persist_exact(path: &Path, receipt: &Receipt) -> Result<(), String> {
    let bytes = serde_json::to_vec(receipt).map_err(|e| e.to_string())?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            File::open(path.parent().unwrap())
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing: Receipt = read_exact(path)?;
            if existing != *receipt {
                return Err("async original persisted F bytes changed".into());
            }
            Ok(())
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
