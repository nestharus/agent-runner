//! Failure-only observations, not delivery authority. Reads are sequential snapshots;
//! receipt does not imply settlement, and an observed exit does not date that exit.
use super::*;
use std::os::unix::process::ExitStatusExt;

pub(super) fn retain(
    fixture: &Fixture,
    received: &Path,
    value: &Value,
    repl: &mut Child,
    provider: &ProcessIdentity,
) {
    retain_runner(repl);
    retain_provider(provider);
    let attempt = read_attempt(&fixture.sidecar_path());
    retain_attempt(&attempt);
    let attempt_id = attempt.as_ref().ok().map(|row| row.0.as_str());
    readiness_diagnostic("live_notify", notify_record(value, attempt_id));
    match read_receipt(received) {
        Ok(bytes) => readiness_diagnostic("live_receipt", receipt_record(&bytes, attempt_id)),
        Err(_) => readiness_diagnostic("live_receipt", json!({"event": "read_error"})),
    }
}

// One exact fixture attempt only. Read-only, no schema initialization, no busy wait,
// no retry, at most two rows: ambiguity and missing/unreadable state stay unknown.
fn read_attempt(path: &Path) -> rusqlite::Result<(String, bool, bool, bool)> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::ZERO)?;
    let mut stmt = db.prepare(
        "SELECT attempt_id, submission_started_at IS NOT NULL,
                acknowledged_at IS NOT NULL, resolved_at IS NOT NULL
         FROM mailbox_delivery_attempts LIMIT 2",
    )?;
    let mut rows = stmt.query([])?;
    let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let result = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
    if rows.next()?.is_some() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(result)
}

fn retain_attempt(attempt: &rusqlite::Result<(String, bool, bool, bool)>) {
    let record = match attempt {
        Ok((_, started, acked, resolved)) => json!({
            "submission_started": started, "acknowledged": acked, "resolved": resolved,
        }),
        Err(_) => json!({"event": "read_error"}),
    };
    readiness_diagnostic("live_attempt", record);
}

fn read_receipt(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(DIAGNOSTIC_CAPTURE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn receipt_record(bytes: &[u8], attempt: Option<&str>) -> Value {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(DIAGNOSTIC_CAPTURE_LIMIT)]);
    json!({
        "byte_count": bytes.len(),
        "complete_capture": bytes.len() <= DIAGNOSTIC_CAPTURE_LIMIT,
        "notification_marker": text.contains("[OULIPOLY NOTIFICATIONS]"),
        "handle_marker": text.contains("handle: h-e2e-live"),
        "read_count": text.lines().filter(|line| *line == "[END OULIPOLY NOTIFICATIONS]").count(),
        "exact_attempt_marker": attempt.map(|id| text.contains(&format!("[OULIPOLY-DELIVERY {id}]"))),
    })
}

// Codes describe the actual exposed message, NOT an inferred internal stage:
// 0 unknown/missing; 1 exact broker uncertain nonce; 2 exact broker ACK nonce;
// 3 exact OS-error string (errno retained); 4 exact unexpected EOF text.
// The client uses the same I/O error formatter for multiple stages. Broker drain
// and confirm errors also collapse to code1: neither can be split from this message.
fn message_code(message: Option<&str>, attempt: Option<&str>) -> (u64, Option<u64>) {
    let Some(message) = message else {
        return (0, None);
    };
    if attempt.is_some_and(|id| message == format!("delivery_submission_uncertain:{id}")) {
        return (1, None);
    }
    if attempt.is_some_and(|id| message == format!("delivery_ack:{id}")) {
        return (2, None);
    }
    if message == "failed to fill whole buffer" {
        return (4, None);
    }
    // Finite errno vocabulary; no arbitrary suffix parsing or string retention.
    let errno = (1..=133).find(|code| io::Error::from_raw_os_error(*code).to_string() == message);
    match errno {
        Some(code) => (3, Some(code as u64)),
        None => (0, None),
    }
}

fn notify_record(value: &Value, attempt: Option<&str>) -> Value {
    let diagnostic = &value["pty_delivery"];
    let (code, errno) = message_code(diagnostic["message"].as_str(), attempt);
    json!({
        "stage_code": code, "os_error": errno,
        "submitted": diagnostic["submitted"].as_bool(),
        "wake_absent": value.get("wake").map(Value::is_null),
        "read_count": diagnostic["delivered_seqs"].as_array().map(Vec::len),
    })
}

fn retain_runner(repl: &mut Child) {
    // 0 error, 1 still live, 2 exited. Exit code and signal stay distinct.
    let record = match repl.try_wait() {
        Ok(None) => json!({"stage_code": 1}),
        Ok(Some(status)) => json!({"stage_code": 2, "success": status.success(),
            "exit_code": status.code(), "signal": status.signal()}),
        Err(_) => json!({"stage_code": 0, "event": "read_error"}),
    };
    readiness_diagnostic("live_runner", record);
}

fn retain_provider(expected: &ProcessIdentity) {
    // 0 unreadable, 1 same live identity, 2 absent, 3 different identity.
    let code = match read_live_process_identity(expected.os_pid) {
        Ok(Some(actual)) if actual == *expected => 1,
        Ok(Some(_)) => 3,
        Ok(None) => 2,
        Err(_) => 0,
    };
    readiness_diagnostic("live_provider", json!({"stage_code": code}));
}

#[test]
fn uncertain_projection_preserves_distinctions_without_raw_values() {
    let secret = "sensitive-sentinel";
    assert_eq!(
        message_code(
            Some(&format!("delivery_submission_uncertain:{secret}")),
            Some(secret)
        ),
        (1, None)
    );
    assert_eq!(
        message_code(Some(&format!("delivery_ack:{secret}")), Some(secret)),
        (2, None)
    );
    assert_eq!(
        message_code(Some("delivery_ack:wrong"), Some(secret)),
        (0, None)
    );
    assert_eq!(message_code(Some(secret), None), (0, None));
    assert_eq!(
        message_code(Some("failed to fill whole buffer"), None),
        (4, None)
    );
    let error = io::Error::from_raw_os_error(libc::EAGAIN).to_string();
    assert_eq!(
        message_code(Some(&error), None),
        (3, Some(libc::EAGAIN as u64))
    );
    let value = json!({"pty_delivery": {"message": format!("delivery_submission_uncertain:{secret}"),
        "submitted": true, "delivered_seqs": [], "control_path": secret}, "wake": null});
    let record = safe_readiness_record("live_notify", &notify_record(&value, Some(secret)));
    assert_eq!(record["stage_code"], 1);
    assert_eq!(record["submitted"], true);
    assert_eq!(record["wake_absent"], true);
    assert!(!record.to_string().contains(secret));
    assert!(record.to_string().len() <= DIAGNOSTIC_RECORD_LIMIT);
    let receipt = format!(
        "[OULIPOLY NOTIFICATIONS]\nhandle: h-e2e-live\n[OULIPOLY-DELIVERY {secret}]\n[END OULIPOLY NOTIFICATIONS]\n"
    );
    let record = safe_readiness_record(
        "live_receipt",
        &receipt_record(receipt.as_bytes(), Some(secret)),
    );
    assert_eq!(record["read_count"], 1);
    assert_eq!(record["exact_attempt_marker"], true);
    assert!(!record.to_string().contains(secret));
    assert!(record.to_string().len() <= DIAGNOSTIC_RECORD_LIMIT);
    assert_eq!(
        receipt_record(&vec![b'x'; DIAGNOSTIC_CAPTURE_LIMIT + 1], None)["complete_capture"],
        false
    );
    assert!(receipt_record(b"", None)["exact_attempt_marker"].is_null());
}
