//! Frozen completed turns. Explicit recovery never starts a provider or wake.
//! ## Declared roles
//! orchestration, accessor, validator, mapper, formatter
use super::finalization::CompletedAttemptInput;
use oulipoly_runtime::executor::{
    ExecutionOutputSpool,
    prompt_acceptance::{ExpectedPromptAcceptance, promote_prompt_acceptance_attestation},
};
use oulipoly_state::{CompletedTurnEffects, CompletedTurnRecord, StateDb, mailbox::MailboxDb};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const RETAINED_BODY_READ_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Body {
    path: PathBuf,
    len: u64,
    sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Context {
    version: u32,
    provider_session: String,
    mailbox_session: String,
    chain: String,
    seqs: Vec<i64>,
    nonce: Option<String>,
    prompt_attestation: Option<oulipoly_provider::generated::PromptAcceptedMarkerValueV1>,
    observed_turn: Option<String>,
    original_wake_claim: Option<String>,
    sidecar_generation: Option<String>,
    stdout: Body,
    stderr: Body,
    classification: serde_json::Value,
}

pub(super) fn original_wake_claim(state: &Path, session: &str) -> Result<Option<String>, String> {
    let path = MailboxDb::path_for_state_db(state);
    if !path.exists() {
        return Ok(None);
    }
    let db = MailboxDb::open(&path)?;
    Ok(db
        .wake_session_reader()
        .wake_claim(session)?
        .map(|c| c.claim_token))
}

fn open_body(body: &Body) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(&body.path)
        .map_err(|e| format!("completed_turn_body_unavailable: {e}"))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() != body.len {
        return Err("completed_turn_body_incomplete".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err("completed_turn_body_not_private".into());
        }
    }
    let mut digest = Sha256::new();
    let mut buf = [0u8; RETAINED_BODY_READ_BUFFER_BYTES];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    if format!("{:x}", digest.finalize()) != body.sha256 {
        return Err("completed_turn_body_corrupt".into());
    }
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    Ok(file)
}
fn verify_context(
    state: &StateDb,
    record: &CompletedTurnRecord,
    context: &Context,
) -> Result<Option<MailboxDb>, String> {
    if context.version != 1 {
        return Err("completed_turn_context_version".into());
    }
    let paths = state
        .invocation_output_artifact_paths(&record.invocation_uuid)?
        .ok_or("completed_turn_requires_durable_state")?;
    if context.stdout.path != paths.stdout || context.stderr.path != paths.stderr {
        return Err("completed_turn_body_identity_conflict".into());
    }
    open_body(&context.stdout)?;
    open_body(&context.stderr)?;
    let retained_mailbox = context
        .sidecar_generation
        .as_ref()
        .map(|generation| {
            let path = MailboxDb::path_for_state_db(state.path());
            if !path.exists() {
                return Err("completed_turn_history_missing".to_string());
            }
            let db = MailboxDb::open(&path)?;
            if db.sidecar_generation()? != *generation {
                return Err("completed_turn_sidecar_generation_conflict".into());
            }
            Ok(db)
        })
        .transpose()?;
    if record.effects.delivery_ids.is_empty() {
        if !context.seqs.is_empty() {
            let nonce = context
                .nonce
                .as_deref()
                .ok_or("completed_turn_nonce_missing")?;
            let db = MailboxDb::open(&MailboxDb::path_for_state_db(state.path()))?;
            if !db.delivery_attempt_fully_settled(
                nonce,
                &context.mailbox_session,
                Some(&context.chain),
                &context.seqs,
            )? {
                return Err("completed_turn_missing_confirmation".into());
            }
            let window = db
                .delivery_attempt_window(nonce)?
                .ok_or("completed_turn_history_missing")?;
            if window.delivery_invocation_uuid != record.invocation_uuid {
                return Err("completed_turn_delivery_identity_conflict".into());
            }
        }
        return Ok(retained_mailbox);
    }
    if record.effects.delivery_ids.len() != 1 {
        return Err("completed_turn_delivery_count".into());
    }
    let nonce = &record.effects.delivery_ids[0];
    let db = MailboxDb::open(&MailboxDb::path_for_state_db(state.path()))?;
    db.delivery_attempt_fully_settled(
        nonce,
        &context.mailbox_session,
        Some(&context.chain),
        &context.seqs,
    )?;
    let window = db
        .delivery_attempt_window(nonce)?
        .ok_or("completed_turn_history_missing")?;
    if window.delivery_invocation_uuid != record.invocation_uuid
        || record.effects.turn_generation_id != record.invocation_uuid
        || record.effects.session_id != context.mailbox_session
    {
        return Err("completed_turn_delivery_identity_conflict".into());
    }
    let prompt = record
        .effects
        .submitted_evidence
        .as_deref()
        .ok_or("completed_turn_prompt_evidence_missing")?;
    let confirmed = if let Some(attestation) = &context.prompt_attestation {
        let accepted = promote_prompt_acceptance_attestation(
            ExpectedPromptAcceptance {
                provider_session_id: &context.provider_session,
                prompt_sha256: prompt,
                delivery_nonce: Some(nonce),
            },
            attestation,
        )
        .ok_or("completed_turn_attestation_invalid")?;
        format!(
            "{};prompt_sha256={}",
            accepted.protocol(),
            accepted.prompt_sha256()
        )
    } else {
        let observed = db
            .delivery_observation_confirmation(nonce)?
            .ok_or("completed_turn_observation_missing")?;
        if context.observed_turn.as_deref() != Some(&observed) {
            return Err("completed_turn_observation_conflict".into());
        }
        format!("observed_mailbox_delivery;turn_id={observed};prompt_sha256={prompt}")
    };
    if record.effects.confirmed_evidence.as_deref() != Some(&confirmed) {
        return Err("completed_turn_confirmation_conflict".into());
    }
    Ok(retained_mailbox)
}

pub(super) fn complete(
    input: &mut CompletedAttemptInput<'_, '_>,
    category: Option<&str>,
) -> Result<(), String> {
    let state = &input.env.state;
    let acceptance = input.result.resume_acceptance.as_ref();
    let settlement = input.confirmed_delivery;
    let effects = CompletedTurnEffects {
        invocation_row_id: input.invocation_row_id,
        delivery_ids: settlement
            .map(|s| vec![s.delivery_id.into()])
            .unwrap_or_default(),
        session_id: settlement
            .map(|s| s.session_id)
            .unwrap_or(input.mailbox_session_id)
            .into(),
        turn_generation_id: input.invocation.id.clone(),
        submitted_evidence: settlement.map(|s| s.submitted_evidence.into()),
        confirmed_evidence: settlement.map(|s| s.confirmed_evidence.into()),
        observed_at: settlement.map(|s| s.observed_at).unwrap_or(0),
        returned_artifacts: input.result.returned_artifacts.clone(),
        resume_acceptance_status: acceptance.map(|a| a.status.db_value().into()),
        resume_acceptance_evidence: acceptance.and_then(|a| a.evidence.clone()),
        success: true,
        exit_code: input.result.exit_code,
        error_category: category.map(str::to_string),
        terminal_reason: input.result.terminal_reason.clone(),
    };
    let record = if let Some(record) = state.completed_turn(&input.invocation.id)? {
        if record.effects != effects {
            return Err("completed_turn_admission_conflict".into());
        }
        record
    } else {
        let spool = match &input.result.output_spool {
            Some(spool) => spool.clone(),
            None => ExecutionOutputSpool::from_complete_bytes(
                &input.result.stdout,
                input.result.stderr.as_bytes(),
            )
            .map_err(|e| e.to_string())?,
        };
        let summary = spool.summary().map_err(|e| e.to_string())?;
        let paths = state
            .invocation_output_artifact_paths(&input.invocation.id)?
            .ok_or("completed_turn_requires_durable_state")?;
        spool.persist_for_invocation(state, input.invocation_row_id, &input.invocation.id)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&paths.stdout, &paths.stderr] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
            }
        }
        let mut observed = None;
        if let Some(nonce) = input.mailbox_nonce {
            let db = MailboxDb::open(&MailboxDb::path_for_state_db(state.path()))?;
            db.retain_completed_delivery(
                &input.invocation.id,
                nonce,
                input.mailbox_session_id,
                input.chain_id,
                input.mailbox_seqs,
            )?;
            observed = db.delivery_observation_confirmation(nonce)?;
        }
        let context = Context {
            version: 1,
            provider_session: input.provider_session_id.into(),
            mailbox_session: input.mailbox_session_id.into(),
            chain: input.chain_id.into(),
            seqs: input.mailbox_seqs.to_vec(),
            nonce: input.mailbox_nonce.map(str::to_string),
            prompt_attestation: input.result.prompt_acceptance_attestation.clone(),
            observed_turn: observed,
            original_wake_claim: input.original_wake_claim.map(str::to_string),
            sidecar_generation: {
                let path = MailboxDb::path_for_state_db(state.path());
                if path.exists() {
                    Some(MailboxDb::open(&path)?.sidecar_generation()?)
                } else {
                    None
                }
            },
            stdout: Body {
                path: paths.stdout,
                len: summary.stdout_bytes,
                sha256: summary.stdout_sha256,
            },
            stderr: Body {
                path: paths.stderr,
                len: summary.stderr_bytes,
                sha256: summary.stderr_sha256,
            },
            classification: serde_json::json!({"recovered_generic_nonzero":input.recovered_generic_nonzero,"terminal_completion_confirmed":input.terminal_completion_confirmed,"produced_assistant_response":input.result.produced_assistant_response,"terminal_signal":format!("{:?}",input.result.terminal_signal),"session_capture":format!("{:?}",input.result.session_capture),"captured_children":input.result.captured_child_invocations.iter().map(|c|serde_json::json!({"id":c.composite_id,"marker":c.raw_marker_line})).collect::<Vec<_>>() }),
        };
        let payload = serde_json::to_value(&context).map_err(|e| e.to_string())?;
        let candidate = CompletedTurnRecord {
            invocation_uuid: input.invocation.id.clone(),
            settlement_id: String::new(),
            effects: effects.clone(),
            context: payload.clone(),
            committed: false,
            tails: serde_json::json!({}),
        };
        let _history = verify_context(state, &candidate, &context)?;
        state.admit_completed_turn(
            state
                .invocation_mutation_scope(input.invocation_row_id)
                .authority(),
            &effects,
            &payload,
        )?;
        state
            .completed_turn(&input.invocation.id)?
            .ok_or("completed_turn_admission_unreadable")?
    };
    // Once admitted, no destructor may convert a genuine completed turn into a
    // synthetic failure, regardless of the settlement refusal's error spelling.
    input.guard.mark_finalized();
    let context: Context =
        serde_json::from_value(record.context.clone()).map_err(|e| e.to_string())?;
    let _history = verify_context(state, &record, &context)?;
    state.settle_completed_turn(&record).map_err(|error|format!("completed_turn_pending: invocation={}; recover with completed-turn --invocation {} --settle; {error}",record.invocation_uuid,record.invocation_uuid))
}

/// No registry, execution service, resume loop or wake-launch function enters
/// this selector, including when a DISTINCT next delivery remains pending.
pub(crate) fn command(invocation: Option<&str>, settle: bool, output: bool) -> Result<i32, String> {
    let state = StateDb::open_default()?;
    let Some(uuid) = invocation else {
        if settle || output {
            return Err("--invocation is required for settlement/output".into());
        }
        println!(
            "{}",
            serde_json::to_string(&state.completed_turn_identities()?).map_err(|e| e.to_string())?
        );
        return Ok(0);
    };
    let record = state
        .completed_turn(uuid)?
        .ok_or("completed_turn_missing_or_partial; no provider execution performed")?;
    let context: Context =
        serde_json::from_value(record.context.clone()).map_err(|e| e.to_string())?;
    let _history = verify_context(&state, &record, &context)?;
    if settle {
        state.settle_completed_turn(&record)?;
        recovery_tails(&state, &record, &context)?;
    }
    if output {
        std::io::copy(
            &mut open_body(&context.stdout)?,
            &mut std::io::stdout().lock(),
        )
        .map_err(|e| e.to_string())?;
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    } else {
        let current = state
            .completed_turn(uuid)?
            .ok_or("completed_turn_missing")?;
        println!(
            "{}",
            serde_json::json!({"invocation":uuid,"settlement_id":current.settlement_id,"committed":current.committed,"tails":current.tails,"output_delivery":"not_asserted","recovery":"completed-turn --invocation <uuid> --settle"})
        );
    }
    Ok(0)
}
fn recovery_tails(
    state: &StateDb,
    record: &CompletedTurnRecord,
    context: &Context,
) -> Result<(), String> {
    let path = MailboxDb::path_for_state_db(state.path());
    let mut tails = serde_json::json!({"native":"pending","delivery":"pending","idle":"pending","wake":"pending_recheck","wake_owner":"root/operator","wake_action":"separate authorized session advance; recovery never launches"});
    state.record_completed_turn_tails(&record.invocation_uuid, &record.settlement_id, &tails)?;
    state.complete_completed_turn_native(&record.invocation_uuid, &record.settlement_id)?;
    tails["native"] = serde_json::json!("complete_or_standalone");
    if path.exists() {
        let mut db = MailboxDb::open(&path)?;
        if !context.seqs.is_empty() {
            db.mark_delivered(
                &context.mailbox_session,
                Some(&context.chain),
                &context.seqs,
                &record.invocation_uuid,
            )?;
        }
        tails["delivery"] = serde_json::json!("complete");
        db.finish_completed_turn_bookkeeping(
            &context.provider_session,
            &record.invocation_uuid,
            &record.settlement_id,
            context.original_wake_claim.as_deref(),
            record.effects.exit_code,
        )?;
        tails["idle"] = serde_json::json!("complete");
        let count = crate::mailbox_delivery::deliverable_pending_count_on(
            &mut db,
            state,
            &context.mailbox_session,
        )?;
        tails["wake"] = serde_json::json!(if count == 0 {
            "no_pending_at_recheck"
        } else {
            "distinct_next_turn_pending"
        });
        tails["pending_count"] = serde_json::json!(count);
    } else if context.seqs.is_empty() {
        tails["delivery"] = serde_json::json!("not_applicable");
        tails["idle"] = serde_json::json!("no_runtime");
        tails["wake"] = serde_json::json!("no_mailbox");
    } else {
        return Err("completed_turn_history_missing".into());
    }
    state.record_completed_turn_tails(&record.invocation_uuid, &record.settlement_id, &tails)
}

#[cfg(test)]
mod admission_tests;
