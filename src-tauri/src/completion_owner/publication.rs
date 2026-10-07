//! Internal completion acceptance retained for the delivery driver (M-b).
//! Declared roles: accessor, validator, mapper, orchestration.
use oulipoly_state::StateDb;
use oulipoly_state::completion_continuation::{
    AdmittedSourceBinding, MAX_REGISTRATION_BYTES, PROTOCOL, SourceRegistration,
    VerifiedCompletion, read_source_file,
};
use oulipoly_state::mailbox::{CompletionEventTriggerInput, MailboxDb};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn response(binding: &AdmittedSourceBinding, status: &str) -> Result<Value, String> {
    let mut value = serde_json::to_value(binding.identity()?).map_err(|e| e.to_string())?;
    value["status"] = status.into();
    Ok(value)
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
