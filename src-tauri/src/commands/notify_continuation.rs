//! Exact v2 notification operations. Original work is never launched or replayed.
//! Declared roles: accessor, parser, validator, mapper, orchestration.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use oulipoly_state::completion_continuation::{
    AdmittedSourceBinding, MAX_REGISTRATION_BYTES, PROTOCOL, SourceRegistration,
    VerifiedCompletion, copy_verified_raw, read_source_file, require_unchanged_output,
};
use oulipoly_state::mailbox::{CompletionEventTriggerInput, MailboxDb};
use oulipoly_state::{InvocationMutationAuthority, StateDb};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::Duration;

const COMPLETION_REGISTRATION_RETRY_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) fn recovery_list(session_id: Option<&str>, cursor: Option<&str>) -> Result<i32, String> {
    let decoded = cursor
        .map(|encoded| {
            if encoded.len() > 32 * 1024 || encoded.is_empty() {
                return Err("invalid recovery cursor length".to_string());
            }
            let bytes = URL_SAFE_NO_PAD
                .decode(encoded)
                .map_err(|_| "invalid recovery cursor encoding".to_string())?;
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|_| "invalid recovery cursor JSON".to_string())
        })
        .transpose()?;
    let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
    let (events, next) = mailbox.completion_recovery_events(session_id, decoded.as_ref())?;
    let next_cursor = next
        .map(|value| {
            serde_json::to_vec(&value)
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    emit(&json!({
        "status":"ok", "session_id":session_id,
        "page_limit":100, "events":events, "next_cursor":next_cursor,
    }))
}

pub(crate) fn recovery_read(event_id: &str, output: Option<&Path>) -> Result<i32, String> {
    let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
    let data_root = oulipoly_state::paths::data_dir()?;
    let result = recovery_read_from(&mailbox, &data_root, event_id, output)?;
    emit(&result)
}

pub(crate) fn recovery_attempts(event_id: &str, cursor: &str) -> Result<i32, String> {
    let decoded = decode_attempt_cursor(cursor)?;
    let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
    let record = mailbox
        .completion_recovery_record(event_id)?
        .ok_or("no accepted v2 completion with that event ID")?;
    let physical_drain = physical_drain_page(&mailbox, &record, Some(&decoded))?;
    emit(&json!({"status":"ok", "event_id":event_id, "physical_drain":physical_drain}))
}

fn decode_attempt_cursor(encoded: &str) -> Result<Value, String> {
    if encoded.is_empty() || encoded.len() > 32 * 1024 {
        return Err("invalid recovery attempt cursor length".into());
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "invalid recovery attempt cursor encoding".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "invalid recovery attempt cursor JSON".to_string())
}

fn physical_drain_page(
    mailbox: &MailboxDb,
    record: &Value,
    cursor: Option<&Value>,
) -> Result<Value, String> {
    let registration_id = required_string(record, "registration_id")?;
    let (attempts, next) = mailbox.completion_recovery_attempts(registration_id, cursor)?;
    let next_attempt_cursor = next
        .map(|value| {
            serde_json::to_vec(&value)
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    Ok(json!({
        "attempts":attempts,
        "attempt_page_limit":128,
        "attempt_search_complete":next_attempt_cursor.is_none(),
        "next_attempt_cursor":next_attempt_cursor,
        "association_completeness":mailbox.continuation_attempt_association_completeness(registration_id)?,
        "assessment":"per_attempt_receipts_only"
    }))
}

/// This is local manual inspection. No listener activation, notification, ACK,
/// physical drain, source retry, or workload execution is reachable here.
fn recovery_read_from(
    mailbox: &MailboxDb,
    data_root: &Path,
    event_id: &str,
    output: Option<&Path>,
) -> Result<Value, String> {
    let record = mailbox
        .completion_recovery_record(event_id)?
        .ok_or("no accepted v2 completion with that event ID")?;
    let bytes = mailbox
        .completion_recovery_payload(event_id)?
        .ok_or("accepted completion payload is unavailable")?;
    let payload: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if payload["schema_version"] != 2
        || payload["kind"] != "agent_bash_complete"
        || payload["completion_protocol"] != PROTOCOL
    {
        return Err("retained completion payload has an unsupported protocol/kind".into());
    }
    for (field, expected) in [
        ("event_id", event_id),
        ("handle", event_id),
        (
            "registration_id",
            required_string(&record, "registration_id")?,
        ),
        ("source_id", required_string(&record, "source_id")?),
        (
            "registration_digest",
            required_string(&record, "registration_digest")?,
        ),
    ] {
        if payload[field] != expected {
            return Err(format!("retained completion identity conflict at {field}"));
        }
    }
    for (field, expected) in [
        ("handle", event_id),
        (
            "registration_id",
            required_string(&record, "registration_id")?,
        ),
        ("source_id", required_string(&record, "source_id")?),
        ("domain_id", required_string(&record, "domain_id")?),
        (
            "registration_digest",
            required_string(&record, "registration_digest")?,
        ),
    ] {
        if payload["snapshot"][field] != expected {
            return Err(format!(
                "retained selected snapshot identity conflict at {field}"
            ));
        }
    }
    let selected = &payload["snapshot"]["output"];
    let output_info = if selected.is_string() {
        if !payload["output_artifact"].is_null() {
            return Err("legacy inline text conflicts with retained raw artifact".into());
        }
        if output.is_some() {
            return Err("legacy inline text has no exact raw bytes; no output file written".into());
        }
        let content = selected.as_str().unwrap();
        json!({"kind":"lossy_inline_legacy", "content":content,
            "retained_text_byte_len":content.len(),
            "retained_text_sha256":format!("{:x}", Sha256::digest(content.as_bytes())),
            "exact_raw_bytes_available":false})
    } else if selected["representation"] == "missing-original-output-v1" {
        if output.is_some() {
            return Err("selected original output is missing; no output file written".into());
        }
        if !payload["output_artifact"].is_null() {
            return Err("missing selected output conflicts with retained artifact".into());
        }
        json!({"kind":"selected_missing_output", "evidence":selected,
            "exact_raw_bytes_available":false})
    } else if selected["representation"] == "retained-output-v1" {
        read_retained_raw_output(
            selected,
            &payload["output_artifact"],
            required_string(&record, "domain_id")?,
            data_root,
            output,
        )?
    } else {
        return Err("unknown selected output representation".into());
    };
    let presentation = mailbox.completion_notification_diagnostics(event_id)?;
    let mut physical_drain = physical_drain_page(mailbox, &record, None)?;
    physical_drain["source_reported_original_tree_drained"] =
        payload["outcome"]["original_tree_drained"].clone();
    Ok(json!({"status":"verified", "event_id":event_id,
        "source_acceptance":{"phase":"accepted", "registration_id":record["registration_id"],
            "snapshot_sha256":record["snapshot_sha256"], "outcome_sha256":record["outcome_sha256"],
            "payload_sha256":record["payload_sha256"], "payload_byte_len":record["payload_byte_len"]},
        "selected_output":output_info,
        "presentation_and_ack":presentation,
        "physical_drain":physical_drain,
    }))
}

fn read_retained_raw_output(
    selected: &Value,
    artifact: &Value,
    domain: &str,
    data_root: &Path,
    output: Option<&Path>,
) -> Result<Value, String> {
    let digest = required_string(selected, "sha256")?;
    let len = selected["byte_len"]
        .as_u64()
        .ok_or("selected output length missing")?;
    if uuid::Uuid::parse_str(domain).is_err()
        || digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        || selected["relative"] != "completion-output-v2.bin"
        || !matches!(selected["encoding"].as_str(), Some("raw" | "utf8-lossy"))
    {
        return Err("unsupported selected raw output descriptor".into());
    }
    let path = data_root
        .join("completion-continuation")
        .join(domain)
        .join("outputs")
        .join(digest);
    if artifact["path"] != path.to_string_lossy().as_ref()
        || artifact["sha256"] != digest
        || artifact["byte_len"] != len
        || artifact["encoding"] != selected["encoding"]
    {
        return Err("retained raw artifact conflicts with selected output".into());
    }
    verify_and_copy_raw(&path, len, digest, output)?;
    Ok(json!({"kind":"raw_bytes", "byte_len":len, "sha256":digest,
            "verified":true, "output_file":output,
            "exact_raw_bytes_available":true}))
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("missing {field}"))
}

fn verify_and_copy_raw(
    path: &Path,
    expected_len: u64,
    expected_digest: &str,
    destination: Option<&Path>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() != expected_len {
        return Err("retained selected output length/type conflict".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    let mut input = options.open(path).map_err(|e| e.to_string())?;
    require_unchanged_output(&metadata, &input.metadata().map_err(|e| e.to_string())?)?;
    let mut staged = destination
        .map(|target| {
            tempfile::NamedTempFile::new_in(
                target
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )
            .map_err(|e| e.to_string())
        })
        .transpose()?;
    if let Some(file) = staged.as_mut() {
        copy_verified_raw(&mut input, expected_len, expected_digest, file)?;
    } else {
        copy_verified_raw(
            &mut input,
            expected_len,
            expected_digest,
            &mut std::io::sink(),
        )?;
    }
    require_unchanged_output(
        &metadata,
        &fs::symlink_metadata(path).map_err(|e| e.to_string())?,
    )?;
    if let (Some(file), Some(target)) = (staged, destination) {
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist_noclobber(target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A completion registration coordinates State and the PID mailbox sidecar.
/// The persistence layer deliberately releases State instead of waiting while
/// it owns State and finds the sidecar busy. Retry the complete operation here,
/// after that transaction has unwound, so ordinary concurrent sidecar writers
/// create backpressure rather than turning a valid launch into a terminal
/// registration-outcome-unknown failure.
pub(super) fn register_with_backpressure<T>(
    mut operation: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    loop {
        match operation() {
            Err(error) if error.contains("completion_authority_contention:") => {
                std::thread::sleep(COMPLETION_REGISTRATION_RETRY_INTERVAL);
            }
            result => return result,
        }
    }
}

pub(crate) fn load_binding(path: &Path) -> Result<AdmittedSourceBinding, String> {
    let directory = path.parent().ok_or("registration has no directory")?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid registration filename")?;
    let bytes = read_source_file(directory, name, MAX_REGISTRATION_BYTES)?;
    let source: SourceRegistration = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    source.validate()?;
    if Path::new(&source.handle_dir).join(&source.registration_relative) != path {
        return Err("registration path differs from immutable source binding".into());
    }
    let admission_id = super::notify::completion_obligation_admission_id(
        &source.handle,
        &source.owner_invocation_uuid,
    );
    AdmittedSourceBinding::new(&admission_id, &bytes)
}

pub(crate) fn response(binding: &AdmittedSourceBinding, status: &str) -> Result<Value, String> {
    let mut value = serde_json::to_value(binding.identity()?).map_err(|e| e.to_string())?;
    value["status"] = status.into();
    Ok(value)
}

pub(crate) fn emit(value: &Value) -> Result<i32, String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|e| e.to_string())?
    );
    Ok(
        if matches!(
            value["status"].as_str(),
            Some("conflict" | "unavailable" | "unsupported_transition_required")
        ) {
            1
        } else {
            0
        },
    )
}

pub(crate) fn register(
    args: super::notify::AgentBashRegisterArgs<'_>,
    path: &Path,
) -> Result<i32, String> {
    let binding = load_binding(path)?;
    let source = binding.registration()?;
    let result = (|| {
        super::notify::require_pinned_owner_work_id(&source.handle)?;
        if args.completion_protocol != Some(PROTOCOL) || args.repair_admitted {
            return Err(
                "v2 requires explicit protocol; registration replay is not recovery authority"
                    .into(),
            );
        }
        super::notify::validate_continuation_registration_context(&args, &binding)?;
        crate::completion_owner::require_owner(&source.domain_id)?;
        let authority =
            oulipoly_state::CompletionRegistrationAuthority::from_process_environment()?;
        let registration = register_with_backpressure(|| {
            let mut state = StateDb::open_default()?;
            state.register_completion_continuation_with_authority(
                InvocationMutationAuthority::Standalone,
                &authority,
                &binding,
            )
        })?;
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("registration-committed");
        let mut value = response(
            &binding,
            if registration.inserted {
                "registered"
            } else {
                "already_registered"
            },
        )?;
        value["registration_committed"] = true.into();
        value["continuation_owner_domain"] = source.domain_id.clone().into();
        value["listener_revision"] = json!(registration.listeners.len());
        value["listeners"] = serde_json::to_value(&source.listeners).map_err(|e| e.to_string())?;
        Ok(value)
    })();
    emit(&operation_result(&binding, result)?)
}

fn operation_result(
    binding: &AdmittedSourceBinding,
    result: Result<Value, String>,
) -> Result<Value, String> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let status = if error.contains("conflict") {
                "conflict"
            } else if error.contains("unsupported_transition_required") {
                "unsupported_transition_required"
            } else {
                "unavailable"
            };
            let mut value = response(binding, status)?;
            value["message"] = error.into();
            Ok(value)
        }
    }
}

pub(crate) fn readback(path: &Path, completion: bool) -> Result<i32, String> {
    let binding = load_binding(path)?;
    let result = (|| {
        super::notify::require_pinned_owner_work_id(&binding.registration()?.handle)?;
        let state =
            StateDb::open_read_only(&StateDb::default_path()?).map_err(|e| format!("{e:?}"))?;
        if state.admitted_completion_continuation(&binding)?.is_none() {
            return response(&binding, "absent");
        }
        let mut value = response(&binding, "exact_committed")?;
        value["registration_committed"] = true.into();
        value["registration"] =
            serde_json::to_value(binding.registration()?).map_err(|e| e.to_string())?;
        value["registration_bytes_utf8"] = std::str::from_utf8(binding.registration_bytes())
            .map_err(|e| e.to_string())?
            .into();
        if completion {
            add_completion_projection(&binding, &mut value)?;
        }
        Ok(value)
    })();
    emit(&operation_result(&binding, result)?)
}

fn add_completion_projection(
    binding: &AdmittedSourceBinding,
    value: &mut Value,
) -> Result<(), String> {
    let source = binding.registration()?;
    let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
    if let Some(projection) = mailbox.completion_continuation_acceptance(&source.registration_id)? {
        for key in [
            "phase",
            "snapshot_sha256",
            "outcome_sha256",
            "payload_sha256",
            "payload_byte_len",
        ] {
            value[key] = projection[key].clone();
        }
    } else {
        value["phase"] = "awaiting_sidecar_repair".into();
    }
    let listeners = mailbox.completion_event_listeners(&source.handle)?;
    value["notification_dispositions"] =
        json!(mailbox.completion_notification_diagnostics(&source.handle)?);
    value["listener_revision"] = json!(listeners.len());
    value["listeners"] = json!(
        listeners
            .iter()
            .map(|listener| json!({
                "listener_id": listener.listener_id, "session_id":listener.session_id,
                "owner_invocation_uuid":listener.owner_invocation_uuid,
                "mailbox_seq":listener.mailbox_seq, "acknowledged_at":listener.acknowledged_at,
                "acknowledgement_basis":listener.acknowledgement_reason,
            }))
            .collect::<Vec<_>>()
    );
    value["outstanding_attempt_ids"] =
        json!(mailbox.pending_continuation_attempt_ids(&source.registration_id)?);
    value["association_completeness"] =
        json!(mailbox.continuation_attempt_association_completeness(&source.registration_id)?);
    Ok(())
}

pub(crate) fn capability() -> Result<i32, String> {
    #[cfg(target_os = "linux")]
    if let Some(readback) = crate::completion_owner::read_v30_owner_if_present(None)? {
        return emit(&json!({"protocol":PROTOCOL,"status":"available",
            "domain_id":readback.owner.domain_id,"owner":readback.owner}));
    }
    let path = MailboxDb::default_path()?;
    if !path.exists() {
        return emit(&json!({"protocol":PROTOCOL,"status":"unavailable"}));
    }
    // This is the native launch handshake, not detached recovery inspection.
    // Copying the live sidecar makes every launch depend on a quiet database.
    // Use the existing native lane without creating or migrating the database.
    let mailbox = MailboxDb::open_existing_native_authority(&path)?;
    let Some(domain_id) = mailbox.completion_continuation_domain()? else {
        return emit(&json!({"protocol":PROTOCOL,"status":"unsupported_transition_required"}));
    };
    emit(
        &json!({"protocol":PROTOCOL,"status":"available","domain_id":domain_id,"owner":mailbox.completion_continuation_owner()?}),
    )
}

pub(crate) fn complete(
    path: &Path,
    snapshot: &Path,
    args: super::notify::AgentBashCompleteArgs<'_>,
) -> Result<i32, String> {
    let binding = load_binding(path)?;
    let source = binding.registration()?;
    let paths = source.paths();
    let result = if args.handle != source.handle
        || args.state_dir != Path::new(&source.handle_dir)
        || args.meta != Path::new(&paths[0])
        || args.log != Path::new(&paths[1])
        || args.rc != Path::new(&paths[2])
    {
        Err("completion CLI fields conflict with immutable registration".into())
    } else if let Err(error) = super::notify::require_pinned_owner_work_id(&source.handle) {
        Err(error)
    } else if args.completion_protocol == Some(PROTOCOL) {
        accept(&binding, snapshot)
    } else {
        Err("explicit completion-continuation-v2 protocol required".into())
    };
    emit(&operation_result(&binding, result)?)
}

pub(crate) fn accept(
    binding: &AdmittedSourceBinding,
    snapshot_path: &Path,
) -> Result<Value, String> {
    let source = binding.registration()?;
    let directory = Path::new(&source.handle_dir);
    if directory.join(&source.snapshot_relative) != snapshot_path {
        return Err("completion snapshot path conflict".into());
    }
    let bytes = read_source_file(
        directory,
        &source.registration_relative,
        MAX_REGISTRATION_BYTES,
    )?;
    if bytes != binding.registration_bytes() {
        return Err("completion source incarnation conflict".into());
    }
    let state = StateDb::open_read_only(&StateDb::default_path()?).map_err(|e| format!("{e:?}"))?;
    if state.admitted_completion_continuation(binding)?.is_none() {
        return Err("completion has no admitted registration".into());
    }
    let evidence = VerifiedCompletion::from_source_files(binding)?;
    let output_artifact = retain_output_artifact(&source, &evidence)?;
    if evidence.outcome.kind == "never_launched" {
        let fence: Value = serde_json::from_slice(&read_source_file(
            directory,
            "source-launch-v2.json",
            MAX_REGISTRATION_BYTES,
        )?)
        .map_err(|e| e.to_string())?;
        if fence["phase"] != "revoked_never_launched"
            || fence["revision"].as_u64() != Some(evidence.outcome.launch_fence_revision)
            || fence["source_id"] != source.source_id
            || fence["registration_id"] != source.registration_id
        {
            return Err("never-launched outcome lacks exact revoked launch fence".into());
        }
    }
    let paths = source.paths();
    let payload = serde_json::to_string(&json!({
        "schema_version":2, "kind":"agent_bash_complete", "event_id":source.handle,
        "handle":source.handle, "rc":evidence.snapshot.rc, "state_dir":source.handle_dir,
        "meta_path":paths[0], "log_path":paths[1], "rc_path":paths[2],
        "completion_protocol":PROTOCOL, "source_id":source.source_id,
        "registration_id":source.registration_id, "registration_digest":binding.registration_digest(),
        "snapshot":evidence.snapshot, "outcome":evidence.outcome,
        "output_artifact":output_artifact,
    })).map_err(|e| e.to_string())?;
    let mut mailbox = MailboxDb::open_default()?;
    let result = mailbox.trigger_completion_continuation(
        CompletionEventTriggerInput {
            event_id: &source.handle,
            payload_json: &payload,
            state_dir: &source.handle_dir,
            meta_path: &paths[0],
            log_path: &paths[1],
            rc_path: &paths[2],
            rc: evidence.snapshot.rc,
        },
        binding,
        &evidence,
    )?;
    #[cfg(feature = "age360-fault-fixtures")]
    oulipoly_state::completion_continuation::age360_fault_barrier("acceptance-committed");
    let mut response = response(
        binding,
        if result.triggered {
            "accepted"
        } else {
            "already_accepted"
        },
    )?;
    if evidence.original_output_missing() {
        response["original_output_missing"] = true.into();
    }
    response["snapshot_sha256"] = evidence.snapshot_sha256.into();
    response["outcome_sha256"] = evidence.outcome_sha256.into();
    response["payload_sha256"] = json!(result.event.payload_sha256);
    response["payload_byte_len"] = json!(result.event.payload_byte_len);
    response["listener_revision"] = json!(result.listeners.len());
    Ok(response)
}

/// Listener authority is the caller's live State capability; possession of the
/// original source file alone cannot attach another actor or acknowledge anyone.
pub(crate) fn listen(path: &Path, session_id: &str, invocation_uuid: &str) -> Result<i32, String> {
    let original = load_binding(path)?;
    let binding = original.for_listener(
        &super::notify::completion_obligation_admission_id(
            &original.registration()?.handle,
            invocation_uuid,
        ),
        oulipoly_state::completion_continuation::ListenerIdentity {
            listener_id: invocation_uuid.into(),
            session_id: session_id.into(),
            owner_invocation_uuid: invocation_uuid.into(),
        },
    )?;
    let result = (|| {
        crate::completion_owner::require_owner(&binding.registration()?.domain_id)?;
        let authority =
            oulipoly_state::CompletionRegistrationAuthority::from_process_environment()?;
        let registered = register_with_backpressure(|| {
            let mut state = StateDb::open_default()?;
            state.register_completion_continuation_with_authority(
                InvocationMutationAuthority::Standalone,
                &authority,
                &binding,
            )
        })?;
        let mut value = response(&binding, "listener_registered")?;
        value["listener_revision"] = json!(registered.listeners.len());
        value["listeners"] =
            serde_json::to_value(registered.listeners).map_err(|e| e.to_string())?;
        Ok(value)
    })();
    emit(&operation_result(&binding, result)?)
}

#[cfg(test)]
mod tests {
    use super::{recovery_list, register_with_backpressure, verify_and_copy_raw};
    use sha2::{Digest, Sha256};

    #[test]
    fn malformed_manual_recovery_cursor_is_rejected_before_database_open() {
        assert!(
            recovery_list(None, Some("invalid!"))
                .unwrap_err()
                .contains("cursor encoding")
        );
        assert!(
            recovery_list(None, Some(&"a".repeat(32 * 1024 + 1)))
                .unwrap_err()
                .contains("cursor length")
        );
    }

    #[test]
    fn manual_raw_read_rejects_corruption_without_publishing_destination() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("selected");
        let destination = root.path().join("recovered");
        let selected = b"a\xff\x00z";
        std::fs::write(&source, selected).unwrap();
        let digest = format!("{:x}", Sha256::digest(selected));
        verify_and_copy_raw(&source, selected.len() as u64, &digest, Some(&destination)).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), selected);
        std::fs::remove_file(&destination).unwrap();
        std::fs::write(&source, b"a\xff\x00x").unwrap();
        assert!(
            verify_and_copy_raw(&source, selected.len() as u64, &digest, Some(&destination))
                .is_err()
        );
        assert!(!destination.exists());
        std::fs::write(&source, selected).unwrap();
        assert!(
            verify_and_copy_raw(
                &source,
                selected.len() as u64 - 1,
                &digest,
                Some(&destination)
            )
            .is_err()
        );
        assert!(!destination.exists());
    }

    #[test]
    #[ignore = "streams/copies 1 GiB + 1 byte; explicit manual descriptor/export control"]
    fn artifact_above_old_cap_passes_manual_lookup_and_atomic_export() {
        use serde_json::json;
        use std::fs;
        let root = tempfile::tempdir().unwrap();
        let domain = uuid::Uuid::new_v4().to_string();
        let directory = root
            .path()
            .join("completion-continuation")
            .join(&domain)
            .join("outputs");
        fs::create_dir_all(&directory).unwrap();
        let digest = "6d9bfe50425f2dfe4e2ac07efee1f0bc9d567348ad4aed62704ffe6f5884e9a8";
        let length = 1024 * 1024 * 1024 + 1u64;
        let path = directory.join(digest);
        fs::File::create(&path).unwrap().set_len(length).unwrap();
        let selected = json!({"representation":"retained-output-v1", "relative":"completion-output-v2.bin", "sha256":digest, "byte_len":length, "encoding":"raw"});
        let artifact = json!({"path":path, "sha256":digest, "byte_len":length, "encoding":"raw"});
        let output = root.path().join("export.bin");
        let result = super::read_retained_raw_output(
            &selected,
            &artifact,
            &domain,
            root.path(),
            Some(&output),
        )
        .unwrap();
        assert_eq!(result["byte_len"], length);
        assert_eq!(result["verified"], true);
        assert!(serde_json::to_vec(&result).unwrap().len() < 512);
        assert_eq!(fs::metadata(&output).unwrap().len(), length);
        verify_and_copy_raw(&output, length, digest, None).unwrap();
        fs::remove_file(&output).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(length - 1)
            .unwrap();
        assert!(
            super::read_retained_raw_output(
                &selected,
                &artifact,
                &domain,
                root.path(),
                Some(&output)
            )
            .is_err()
        );
        assert!(!output.exists());
        println!("manual verified/exported bytes={length} sha256={digest}; no recipient ACK");
    }

    #[cfg(unix)]
    #[test]
    fn manual_raw_read_rejects_symlink_and_never_clobbers_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        let target = root.path().join("target");
        std::fs::write(&source, b"body").unwrap();
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let digest = format!("{:x}", Sha256::digest(b"body"));
        assert!(verify_and_copy_raw(&alias, 4, &digest, None).is_err());
        assert!(verify_and_copy_raw(&source, 4, &digest, Some(&target)).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 3);
    }

    #[test]
    fn completion_authority_contention_retries_after_the_state_transaction_unwinds() {
        let mut attempts = 0;
        let result = register_with_backpressure(|| {
            attempts += 1;
            if attempts < 3 {
                Err(
                    "process_integrity: completion_authority_contention: database is locked"
                        .to_string(),
                )
            } else {
                Ok("registered")
            }
        })
        .unwrap();

        assert_eq!(result, "registered");
        assert_eq!(attempts, 3);
    }

    #[test]
    fn non_contention_registration_failures_are_not_retried() {
        let mut attempts = 0;
        let error = register_with_backpressure(|| {
            attempts += 1;
            Err::<(), _>("immutable v2 owner binding conflict".to_string())
        })
        .unwrap_err();

        assert_eq!(error, "immutable v2 owner binding conflict");
        assert_eq!(attempts, 1);
    }
}

/// Own the complete immutable body before materializing any notification. The
/// source producer can subsequently disappear without losing output access.
fn retain_output_artifact(
    source: &SourceRegistration,
    evidence: &VerifiedCompletion,
) -> Result<Option<Value>, String> {
    use oulipoly_state::completion_continuation::CompletionOutput;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let CompletionOutput::Artifact(artifact) = &evidence.snapshot.output else {
        return Ok(None);
    };
    let directory = oulipoly_state::paths::data_dir()?
        .join("completion-continuation")
        .join(&source.domain_id)
        .join("outputs");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let retained = directory.join(&artifact.sha256);
    let temp = directory.join(format!(".copy-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options.open(&temp).map_err(|e| e.to_string())?;
        artifact.copy_verified(Path::new(&source.handle_dir), &mut output)?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        // A competing exact copy has the same content hash. Atomic publication
        // never exposes a prefix; no acceptance points to the temporary path.
        std::fs::rename(&temp, &retained).map_err(|e| e.to_string())?;
        std::fs::File::open(&directory)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(Some(
            json!({"path":retained,"sha256":artifact.sha256,"byte_len":artifact.byte_len,"encoding":artifact.encoding}),
        ))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}
