//! Exact v2 notification operations. Original work is never launched or replayed.
//! Declared roles: accessor, parser, validator, mapper, orchestration.
use oulipoly_state::completion_continuation::{
    AdmittedSourceBinding, MAX_REGISTRATION_BYTES, PROTOCOL, SourceRegistration,
    VerifiedCompletion, read_source_file,
};
use oulipoly_state::mailbox::{CompletionEventTriggerInput, MailboxDb};
use oulipoly_state::{InvocationMutationAuthority, StateDb};
use serde_json::{Value, json};
use std::path::Path;

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
        let mut state = StateDb::open_default()?;
        let registration = state.register_completion_continuation_with_authority(
            InvocationMutationAuthority::Standalone,
            &authority,
            &binding,
        )?;
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
    Ok(())
}

pub(crate) fn capability() -> Result<i32, String> {
    let path = MailboxDb::default_path()?;
    if !path.exists() {
        return emit(&json!({"protocol":PROTOCOL,"status":"unavailable"}));
    }
    let mailbox = MailboxDb::open_read_only(&path)?;
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
        let mut state = StateDb::open_default()?;
        let registered = state.register_completion_continuation_with_authority(
            InvocationMutationAuthority::Standalone,
            &authority,
            &binding,
        )?;
        let mut value = response(&binding, "listener_registered")?;
        value["listener_revision"] = json!(registered.listeners.len());
        value["listeners"] =
            serde_json::to_value(registered.listeners).map_err(|e| e.to_string())?;
        Ok(value)
    })();
    emit(&operation_result(&binding, result)?)
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
