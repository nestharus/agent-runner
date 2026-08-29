//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

use oulipoly_runtime::delivery_evidence::PtyTransportAcknowledgementEvidence;
use oulipoly_runtime::provider_turn_contract::MAILBOX_BATCH_MAX_ROWS;
#[cfg(test)]
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, CreateRuntimeGeneration, EnqueueResult, RuntimeGenerationId,
};
use oulipoly_state::mailbox::{
    ExactProcessEvidence, ExitRuntimeGenerationNonOrderly, GenerationMutation, MailboxDb,
    MailboxDeliveryEvidenceObligation, MailboxRow, RuntimeGenerationFence, RuntimeLifecycleState,
    RuntimeTerminalReason, SUBMITTED_INPUT_KIND, SessionGenerationProjection,
    SessionMetadataUpsert, mailbox_row_is_deliverable_pending,
};
use oulipoly_state::pid_identity::{ProcessIdentityObservation, observe_live_process_identity};
use oulipoly_state::{DeliveryEvidence, DeliveryEvidenceKind, SessionLifecycleRepository, StateDb};
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

#[cfg(unix)]
use oulipoly_runtime::executor::cli::pty_broker::{
    PtyControlClientErrorKind, USER_INPUT_IDLE_INJECT_MS, append_notify_trace_record,
    inject_control_envelope, notify_trace_decision, notify_trace_inject_status,
    pty_delivery_ack_message, pty_delivery_uncertain_message, trace_token,
    unlink_control_socket_if_owned,
};

const MAILBOX_PREFIX_MAX_BYTES: usize = 64 * 1024;
const DELIVERY_NONCE_PREFIX: &str = "[OULIPOLY-DELIVERY ";
const DELIVERY_NONCE_SUFFIX: &str = "]";
const DELIVERY_NONCE_LENGTH_PLACEHOLDER: &str = "00000000-0000-4000-8000-000000000000";

pub(crate) struct PreparedMailboxDelivery {
    pub answer: Option<String>,
    pub session_id: String,
    pub seqs: Vec<i64>,
    pub delivery_nonce: Option<String>,
    pub requires_turn_confirmation: bool,
}

pub(crate) struct PreparedPtyMailboxDelivery {
    pub envelope: String,
    pub attempt_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PtyMailboxDeliveryDiagnostic {
    pub(crate) attempted: bool,
    pub(crate) status: String,
    pub(crate) control_path: Option<String>,
    pub(crate) submitted: bool,
    pub(crate) delivered_seqs: Vec<i64>,
    pub(crate) remaining_pending: Option<usize>,
    pub(crate) message: Option<String>,
}

#[cfg(unix)]
struct PtyRuntimeAuthority {
    control_path: String,
    delivery_invocation_uuid: String,
    turn_generation_id: String,
    provider_name: String,
}

#[cfg(unix)]
enum DeliverySubmissionRead {
    Started,
    NotStarted,
    Unreadable(String),
}

#[cfg(unix)]
pub(crate) fn attempt_pty_mailbox_delivery_with_trigger(
    mailbox: &mut MailboxDb,
    session_id: &str,
    trigger: &str,
) -> PtyMailboxDeliveryDiagnostic {
    let diagnostic = attempt_pty_mailbox_delivery_inner(mailbox, session_id);
    trace_notify_pty_attempt(trigger, session_id, &diagnostic);
    diagnostic
}

#[cfg(unix)]
fn attempt_pty_mailbox_delivery_inner(
    mailbox: &mut MailboxDb,
    session_id: &str,
) -> PtyMailboxDeliveryDiagnostic {
    if let Err(err) = acknowledge_injected_pty_delivery_attempts(mailbox, session_id) {
        tracing::warn!(
            session_id,
            "Failed to acknowledge injected PTY delivery: {err}"
        );
    }
    if matches!(mailbox.notifications_paused(session_id), Ok(true)) {
        return pty_status(
            false,
            "paused",
            None,
            Vec::new(),
            pending_count(mailbox, session_id),
            None,
        );
    }
    let authority = match pty_runtime_authority(mailbox, session_id) {
        Ok(authority) => authority,
        Err(diagnostic) => return diagnostic,
    };
    let control_path = authority.control_path;
    let delivery_invocation_uuid = authority.delivery_invocation_uuid;
    let turn_generation_id = authority.turn_generation_id;
    let provider_name = authority.provider_name;
    let Some(prepared) = (match prepare_pty_mailbox_delivery_with_transcript_reconciliation(
        mailbox,
        session_id,
        &delivery_invocation_uuid,
        &turn_generation_id,
        &provider_name,
    ) {
        Ok(prepared) => prepared,
        Err(err) => {
            if err.starts_with("mailbox_delivery_submission_uncertain:") {
                return pty_status(
                    true,
                    "submission_uncertain",
                    Some(control_path),
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    Some(err),
                );
            }
            return pty_status(
                false,
                "protocol_error",
                Some(control_path),
                Vec::new(),
                None,
                Some(err),
            );
        }
    }) else {
        return pty_status(
            false,
            "no_pending",
            Some(control_path),
            Vec::new(),
            Some(0),
            None,
        );
    };
    if prepared.envelope.len() > mailbox_prefix_max_bytes() {
        return pty_status(
            false,
            "protocol_error",
            Some(control_path),
            Vec::new(),
            pending_count(mailbox, session_id),
            Some("oversize_frame".to_string()),
        );
    }
    match inject_control_envelope(&control_path, &prepared.envelope) {
        Ok(response)
            if response.ack
                && response.message == pty_delivery_ack_message(&prepared.attempt_id) =>
        {
            acknowledge_pty_batch_injected(mailbox, session_id, &prepared.attempt_id, control_path)
        }
        Ok(response)
            if response.ack
                && response.message == pty_delivery_uncertain_message(&prepared.attempt_id) =>
        {
            pty_status(
                true,
                "submission_uncertain",
                Some(control_path),
                Vec::new(),
                pending_count(mailbox, session_id),
                Some(response.message),
            )
        }
        Ok(response) if response.ack => {
            match delivery_attempt_submission_read(mailbox, &prepared.attempt_id) {
                DeliverySubmissionRead::Started => {
                    submission_uncertain_status(mailbox, session_id, control_path, response.message)
                }
                DeliverySubmissionRead::Unreadable(error) => submission_uncertain_status(
                    mailbox,
                    session_id,
                    control_path,
                    format!("{}; {error}", response.message),
                ),
                DeliverySubmissionRead::NotStarted => mark_unconfirmed_pty_ack(
                    mailbox,
                    session_id,
                    &prepared.attempt_id,
                    control_path,
                    response.message,
                ),
            }
        }
        Ok(response) => {
            match delivery_attempt_submission_read(mailbox, &prepared.attempt_id) {
                DeliverySubmissionRead::Started => {
                    return submission_uncertain_status(
                        mailbox,
                        session_id,
                        control_path,
                        response.message,
                    );
                }
                DeliverySubmissionRead::Unreadable(error) => {
                    return submission_uncertain_status(
                        mailbox,
                        session_id,
                        control_path,
                        format!("{}; {error}", response.message),
                    );
                }
                DeliverySubmissionRead::NotStarted => {}
            }
            resolve_unacknowledged_pty_attempt_or_warn(mailbox, session_id, &prepared.attempt_id);
            let status = pty_nack_status(&response.message).to_string();
            pty_status(
                true,
                &status,
                Some(control_path),
                Vec::new(),
                pending_count(mailbox, session_id),
                Some(response.message),
            )
        }
        Err(err) => {
            if !pty_client_error_may_have_reached_broker(&err.kind) {
                resolve_unacknowledged_pty_attempt_or_warn(
                    mailbox,
                    session_id,
                    &prepared.attempt_id,
                );
            }
            match delivery_attempt_submission_read(mailbox, &prepared.attempt_id) {
                DeliverySubmissionRead::Started => {
                    submission_uncertain_status(mailbox, session_id, control_path, err.message)
                }
                DeliverySubmissionRead::Unreadable(error) => submission_uncertain_status(
                    mailbox,
                    session_id,
                    control_path,
                    format!("{}; {error}", err.message),
                ),
                DeliverySubmissionRead::NotStarted => pty_client_error_status(
                    mailbox,
                    session_id,
                    control_path,
                    err.kind,
                    err.message,
                ),
            }
        }
    }
}

#[cfg(unix)]
fn pty_runtime_authority(
    mailbox: &mut MailboxDb,
    session_id: &str,
) -> Result<PtyRuntimeAuthority, PtyMailboxDeliveryDiagnostic> {
    match mailbox
        .runtime_lifecycle_reader()
        .session_generation_projection(session_id)
    {
        Ok(SessionGenerationProjection::One(generation)) => {
            let control_path = generation.pty_control_path.clone();
            if terminalize_stale_runtime_generation(
                mailbox,
                session_id,
                control_path.as_deref().unwrap_or_default(),
            ) {
                return Err(pty_status(
                    false,
                    "stale_generation",
                    control_path,
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    Some("runtime generation process identity is no longer live".to_string()),
                ));
            }
            if generation.runtime_mode != "pty_interactive" {
                return Err(pty_status(
                    false,
                    "not_pty",
                    None,
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    None,
                ));
            }
            let Some(control_path) = control_path.filter(|path| !path.is_empty()) else {
                return Err(pty_status(
                    false,
                    "no_socket",
                    None,
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    None,
                ));
            };
            if generation.lifecycle_state != RuntimeLifecycleState::Running {
                return Err(pty_status(
                    false,
                    "no_socket",
                    Some(control_path),
                    Vec::new(),
                    pending_count(mailbox, session_id),
                    Some(format!(
                        "runtime generation is {:?}",
                        generation.lifecycle_state
                    )),
                ));
            }
            Ok(PtyRuntimeAuthority {
                control_path,
                delivery_invocation_uuid: generation.spawn_invocation_uuid.clone(),
                turn_generation_id: generation.generation_id.to_string(),
                provider_name: generation.provider_name.clone(),
            })
        }
        Ok(SessionGenerationProjection::Multiple(_)) => Err(pty_status(
            false,
            "ambiguous_runtime",
            None,
            Vec::new(),
            pending_count(mailbox, session_id),
            Some("multiple nonterminal runtime generations".to_string()),
        )),
        Ok(SessionGenerationProjection::None) => Err(pty_status(
            false,
            "no_runtime",
            None,
            Vec::new(),
            pending_count(mailbox, session_id),
            None,
        )),
        Err(err) => Err(pty_status(
            false,
            "no_runtime",
            None,
            Vec::new(),
            None,
            Some(err.to_string()),
        )),
    }
}

#[cfg(unix)]
fn prepare_pty_mailbox_delivery_with_transcript_reconciliation(
    mailbox: &mut MailboxDb,
    session_id: &str,
    delivery_invocation_uuid: &str,
    turn_generation_id: &str,
    provider_name: &str,
) -> Result<Option<PreparedPtyMailboxDelivery>, String> {
    let prepared = prepare_pty_mailbox_delivery(
        mailbox,
        session_id,
        delivery_invocation_uuid,
        turn_generation_id,
    );
    let uncertain_attempt_id = match &prepared {
        Ok(Some(prepared))
            if mailbox.delivery_attempt_submission_started(&prepared.attempt_id)? =>
        {
            Some(prepared.attempt_id.as_str())
        }
        Err(error) => error.strip_prefix("mailbox_delivery_submission_uncertain:"),
        _ => None,
    };
    let Some(attempt_id) = uncertain_attempt_id else {
        return prepared;
    };
    match reconcile_transcript_confirmed_pty_attempt(mailbox, session_id, provider_name, attempt_id)
    {
        Ok(true) => prepare_pty_mailbox_delivery(
            mailbox,
            session_id,
            delivery_invocation_uuid,
            turn_generation_id,
        ),
        Ok(false) => Err(format!(
            "mailbox_delivery_submission_uncertain:{attempt_id}"
        )),
        Err(error) => Err(format!(
            "mailbox_delivery_submission_uncertain:{attempt_id}; transcript reconciliation failed: {error}"
        )),
    }
}

#[cfg(unix)]
fn reconcile_transcript_confirmed_pty_attempt(
    mailbox: &mut MailboxDb,
    session_id: &str,
    provider_name: &str,
    attempt_id: &str,
) -> Result<bool, String> {
    let state = StateDb::open_default()?;
    if !state.has_session_user_turn_containing(provider_name, session_id, attempt_id)? {
        let sessions_path = oulipoly_state::paths::config_dir()?.join("sessions.toml");
        let sessions = match oulipoly_config::SessionsConfig::load(&sessions_path) {
            Ok(sessions) => sessions,
            Err(error) => {
                tracing::warn!(
                    session_id,
                    provider_name,
                    attempt_id,
                    "Failed to load sessions config for uncertain PTY delivery: {error}"
                );
                return Ok(false);
            }
        };
        let report = oulipoly_runtime::sessions::scan_provider_session(
            provider_name,
            &sessions,
            &state,
            session_id,
        );
        if !report.errors.is_empty() {
            tracing::warn!(
                session_id,
                provider_name,
                attempt_id,
                errors = ?report.errors,
                "Provider session scan could not fully reconcile uncertain PTY delivery"
            );
        }
        if !state.has_session_user_turn_containing(provider_name, session_id, attempt_id)? {
            return Ok(false);
        }
    }

    let Some(window) = mailbox.delivery_attempt_window(attempt_id)? else {
        return Ok(false);
    };
    if window.session_id != session_id
        || window.submission_started_at.is_none()
        || window.acknowledged_at.is_some()
        || window.resolved_at.is_some()
    {
        return Ok(false);
    }
    let seqs = window.rows.iter().map(|row| row.seq).collect::<Vec<_>>();
    if seqs.is_empty() {
        return Ok(false);
    }
    mailbox.mark_delivered(session_id, None, &seqs, &window.delivery_invocation_uuid)?;
    tracing::info!(
        session_id,
        provider_name,
        attempt_id,
        delivered = seqs.len(),
        "Reconciled uncertain PTY mailbox delivery from an exact provider user turn"
    );
    Ok(true)
}

#[cfg(unix)]
fn resolve_unacknowledged_pty_attempt_or_warn(
    mailbox: &mut MailboxDb,
    session_id: &str,
    attempt_id: &str,
) {
    if let Err(err) = mailbox.resolve_unacknowledged_delivery_attempt(attempt_id) {
        tracing::warn!(
            session_id,
            attempt_id,
            "Failed to resolve unacknowledged PTY delivery attempt: {err}"
        );
    }
}

#[cfg(not(unix))]
pub(crate) fn attempt_pty_mailbox_delivery_with_trigger(
    _mailbox: &mut MailboxDb,
    _session_id: &str,
    _trigger: &str,
) -> PtyMailboxDeliveryDiagnostic {
    // This platform has no Unix PTY control endpoint; durable listeners remain pending.
    pty_status(false, "not_pty", None, Vec::new(), None, None)
}

pub(crate) fn prepare_pty_mailbox_delivery(
    db: &mut MailboxDb,
    session_id: &str,
    delivery_invocation_uuid: &str,
    turn_generation_id: &str,
) -> Result<Option<PreparedPtyMailboxDelivery>, String> {
    let pending = pending_mailbox_rows(db, session_id, None)?;
    if !has_pending_rows(&pending) {
        return Ok(None);
    }
    let batch = select_batch(&pending);
    let seqs = batch_seqs(&batch);
    let candidate_attempt_id = new_delivery_nonce();
    let attempt_id = db.register_or_reuse_delivery_attempt(
        &candidate_attempt_id,
        session_id,
        delivery_invocation_uuid,
        turn_generation_id,
        &seqs,
        batch.remaining_count,
    )?;
    let window = db
        .delivery_attempt_window(&attempt_id)?
        .ok_or_else(|| format!("Mailbox delivery attempt {attempt_id} disappeared"))?;
    if window.rows.is_empty() {
        return Ok(None);
    }
    let envelope = render_mailbox_prefix(&window.rows, window.remaining_count, &attempt_id)?;
    Ok(Some(PreparedPtyMailboxDelivery {
        envelope,
        attempt_id,
    }))
}

pub(crate) fn mailbox_prefix_max_bytes() -> usize {
    MAILBOX_PREFIX_MAX_BYTES
}

#[cfg(unix)]
fn acknowledge_pty_batch_injected(
    mailbox: &mut MailboxDb,
    session_id: &str,
    attempt_id: &str,
    control_path: String,
) -> PtyMailboxDeliveryDiagnostic {
    let seqs = mailbox
        .delivery_attempt_item_seqs(attempt_id)
        .unwrap_or_default();
    match reconcile_pty_transport_evidence(mailbox, session_id, Some(attempt_id)) {
        Ok(()) => pty_status(
            true,
            "acked",
            Some(control_path),
            seqs,
            pending_count(mailbox, session_id),
            Some("ok".to_string()),
        ),
        Err(error) => pty_status(
            true,
            "evidence_pending",
            Some(control_path),
            seqs,
            pending_count(mailbox, session_id),
            Some(error),
        ),
    }
}

#[cfg(unix)]
fn mark_unconfirmed_pty_ack(
    mailbox: &mut MailboxDb,
    session_id: &str,
    attempt_id: &str,
    control_path: String,
    message: String,
) -> PtyMailboxDeliveryDiagnostic {
    let seqs = mailbox
        .delivery_attempt_window(attempt_id)
        .ok()
        .flatten()
        .map(|window| {
            window
                .rows
                .into_iter()
                .map(|row| row.seq)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    resolve_unacknowledged_pty_attempt_or_warn(mailbox, session_id, attempt_id);
    let failure = mailbox
        .mark_delivery_failed(session_id, None, &seqs, "mailbox_delivery_unconfirmed")
        .err();
    pty_status(
        true,
        "unconfirmed_ack",
        Some(control_path),
        Vec::new(),
        pending_count(mailbox, session_id),
        Some(
            failure
                .map(|failure| format!("{message}; {failure}"))
                .unwrap_or(message),
        ),
    )
}

#[cfg(unix)]
fn pty_client_error_status(
    mailbox: &mut MailboxDb,
    session_id: &str,
    control_path: String,
    kind: PtyControlClientErrorKind,
    message: String,
) -> PtyMailboxDeliveryDiagnostic {
    let status = match kind {
        PtyControlClientErrorKind::Connect
            if terminalize_stale_runtime_generation(mailbox, session_id, &control_path) =>
        {
            "stale_generation"
        }
        PtyControlClientErrorKind::Connect => "connect_error",
        PtyControlClientErrorKind::Protocol
        | PtyControlClientErrorKind::Oversize
        | PtyControlClientErrorKind::EmptyPayload => "protocol_error",
    };
    pty_status(
        true,
        status,
        Some(control_path),
        Vec::new(),
        pending_count(mailbox, session_id),
        Some(message),
    )
}

#[cfg(unix)]
fn terminalize_stale_runtime_generation(
    mailbox: &mut MailboxDb,
    session_id: &str,
    control_path: &str,
) -> bool {
    let generation = match mailbox
        .runtime_lifecycle_reader()
        .session_generation_projection(session_id)
    {
        Ok(SessionGenerationProjection::One(generation)) => generation,
        Ok(SessionGenerationProjection::None | SessionGenerationProjection::Multiple(_))
        | Err(_) => return false,
    };
    let ExactProcessEvidence::Recorded(identity) = &generation.exact_process_evidence else {
        return false;
    };
    let stale = match observe_live_process_identity(identity.os_pid) {
        ProcessIdentityObservation::ExactLive(live) => live != *identity,
        ProcessIdentityObservation::Dead => true,
        ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => false,
    };
    if !stale {
        return false;
    }
    let fence = RuntimeGenerationFence {
        generation_id: &generation.generation_id,
        spawn_invocation_uuid: &generation.spawn_invocation_uuid,
    };
    let terminalized = mailbox
        .runtime_lifecycle()
        .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
            fence,
            reason: RuntimeTerminalReason::RecoveredDead,
            exit_code: None,
        })
        .is_ok_and(|result| {
            matches!(
                result,
                GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_)
            )
        });
    let _ = unlink_control_socket_if_owned(control_path);
    terminalized
}

#[cfg(unix)]
fn pty_client_error_may_have_reached_broker(kind: &PtyControlClientErrorKind) -> bool {
    matches!(kind, PtyControlClientErrorKind::Protocol)
}

fn pending_count(mailbox: &MailboxDb, session_id: &str) -> Option<usize> {
    pending_mailbox_rows(mailbox, session_id, None)
        .map(|rows| rows.len())
        .ok()
}

fn pty_status(
    attempted: bool,
    status: &str,
    control_path: Option<String>,
    delivered_seqs: Vec<i64>,
    remaining_pending: Option<usize>,
    message: Option<String>,
) -> PtyMailboxDeliveryDiagnostic {
    PtyMailboxDeliveryDiagnostic {
        attempted,
        status: status.to_string(),
        control_path,
        submitted: pty_status_implies_submit(status),
        delivered_seqs,
        remaining_pending,
        message,
    }
}

fn pty_status_implies_submit(status: &str) -> bool {
    matches!(
        status,
        "acked" | "evidence_pending" | "mark_delivered_error" | "submission_uncertain"
    )
}

#[cfg(unix)]
fn submission_uncertain_status(
    mailbox: &MailboxDb,
    session_id: &str,
    control_path: String,
    message: String,
) -> PtyMailboxDeliveryDiagnostic {
    pty_status(
        true,
        "submission_uncertain",
        Some(control_path),
        Vec::new(),
        pending_count(mailbox, session_id),
        Some(message),
    )
}

fn acknowledge_injected_pty_delivery_attempts(
    mailbox: &mut MailboxDb,
    session_id: &str,
) -> Result<(), String> {
    reconcile_pty_transport_evidence(mailbox, session_id, None)
}

fn reconcile_pty_transport_evidence(
    mailbox: &mut MailboxDb,
    session_id: &str,
    only_attempt_id: Option<&str>,
) -> Result<(), String> {
    let obligations = mailbox.pending_delivery_evidence_obligations(session_id)?;
    let mut matched = only_attempt_id.is_none();
    for obligation in obligations
        .into_iter()
        .filter(|obligation| only_attempt_id.is_none_or(|id| obligation.attempt_id == id))
    {
        matched = true;
        reconcile_pty_transport_evidence_obligation(mailbox, &obligation)?;
    }
    if !matched {
        let attempt_id = only_attempt_id.expect("an exact obligation was requested");
        match mailbox.delivery_evidence_obligation(attempt_id)? {
            Some(obligation) => verify_exact_pty_transport_evidence(&obligation)?,
            None => verify_retained_pty_transport_evidence(session_id, attempt_id)?,
        }
    }
    Ok(())
}

fn verify_retained_pty_transport_evidence(
    session_id: &str,
    attempt_id: &str,
) -> Result<(), String> {
    let state = StateDb::open_default()?;
    let evidence_id = format!("pty_transport_ack:{attempt_id}");
    let evidence = state
        .delivery_evidence(&evidence_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Mailbox evidence obligation {attempt_id} is missing"))?;
    if evidence.kind == DeliveryEvidenceKind::PtyTransportAck
        && evidence.delivery_id == attempt_id
        && evidence.session_id == session_id
    {
        Ok(())
    } else {
        Err(format!(
            "Retained State evidence for mailbox attempt {attempt_id} conflicts with its exact identity"
        ))
    }
}

fn reconcile_pty_transport_evidence_obligation(
    mailbox: &mut MailboxDb,
    obligation: &MailboxDeliveryEvidenceObligation,
) -> Result<(), String> {
    let mut obligation = obligation.clone();
    let mut evidence = pty_transport_evidence(&obligation);
    let expected = expected_pty_transport_evidence(&evidence);
    let mut state = StateDb::open_default()?;
    if obligation.legacy
        && let Some(existing) = state
            .delivery_evidence(&expected.evidence_id)
            .map_err(|error| error.to_string())?
        && delivery_evidence_identity_matches(&existing, &expected)
        && existing.observed_at != expected.observed_at
    {
        if !mailbox.adopt_legacy_delivery_evidence_observed_at(
            &obligation.attempt_id,
            existing.observed_at,
        )? {
            return Err(format!(
                "Legacy mailbox evidence obligation {} changed before State evidence could be adopted",
                obligation.attempt_id
            ));
        }
        obligation.observed_at = existing.observed_at;
        evidence.observed_at = existing.observed_at;
    }
    let expected = expected_pty_transport_evidence(&evidence);
    evidence
        .record(&mut state)
        .map_err(|error| error.to_string())?;
    verify_exact_pty_transport_evidence_in_state(&state, &obligation, &expected)?;
    wait_at_evidence_clear_barrier_for_test(&obligation.attempt_id);
    if mailbox.mark_delivery_evidence_reconciled(&obligation.attempt_id)? {
        return Ok(());
    }
    if mailbox
        .delivery_evidence_obligation(&obligation.attempt_id)?
        .as_ref()
        == Some(&obligation)
    {
        verify_exact_pty_transport_evidence(&obligation)?;
        return Ok(());
    }
    Err(format!(
        "Mailbox evidence obligation {} changed before it could be cleared",
        obligation.attempt_id
    ))
}

fn delivery_evidence_identity_matches(
    actual: &DeliveryEvidence,
    expected: &DeliveryEvidence,
) -> bool {
    actual.evidence_id == expected.evidence_id
        && actual.kind == expected.kind
        && actual.delivery_id == expected.delivery_id
        && actual.session_id == expected.session_id
        && actual.turn_generation_id == expected.turn_generation_id
}

#[cfg(test)]
pub(crate) static DATA_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
type EvidenceClearBarrier = Option<(String, std::sync::Arc<std::sync::Barrier>)>;

#[cfg(test)]
std::thread_local! {
    static EVIDENCE_CLEAR_BARRIER_REACHED: std::cell::Cell<bool> = const {
        std::cell::Cell::new(false)
    };
}

#[cfg(test)]
fn evidence_clear_barrier_slot() -> &'static std::sync::Mutex<EvidenceClearBarrier> {
    static SLOT: std::sync::OnceLock<std::sync::Mutex<EvidenceClearBarrier>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
fn wait_at_evidence_clear_barrier_for_test(attempt_id: &str) {
    let barrier = evidence_clear_barrier_slot()
        .lock()
        .unwrap()
        .as_ref()
        .filter(|(expected_attempt_id, _)| expected_attempt_id == attempt_id)
        .map(|(_, barrier)| std::sync::Arc::clone(barrier));
    if let Some(barrier) = barrier {
        EVIDENCE_CLEAR_BARRIER_REACHED.set(true);
        barrier.wait();
    }
}

#[cfg(test)]
fn reset_evidence_clear_barrier_reached() {
    EVIDENCE_CLEAR_BARRIER_REACHED.set(false);
}

#[cfg(test)]
fn evidence_clear_barrier_was_reached() -> bool {
    EVIDENCE_CLEAR_BARRIER_REACHED.get()
}

#[cfg(not(test))]
fn wait_at_evidence_clear_barrier_for_test(_attempt_id: &str) {}

fn pty_transport_evidence(
    obligation: &MailboxDeliveryEvidenceObligation,
) -> PtyTransportAcknowledgementEvidence {
    PtyTransportAcknowledgementEvidence {
        evidence_id: format!("pty_transport_ack:{}", obligation.attempt_id),
        delivery_attempt_id: obligation.attempt_id.clone(),
        session_id: obligation.session_id.clone(),
        turn_generation_id: obligation.turn_generation_id.clone(),
        observed_at: obligation.observed_at,
    }
}

fn expected_pty_transport_evidence(
    evidence: &PtyTransportAcknowledgementEvidence,
) -> DeliveryEvidence {
    DeliveryEvidence {
        evidence_id: evidence.evidence_id.clone(),
        kind: DeliveryEvidenceKind::PtyTransportAck,
        delivery_id: evidence.delivery_attempt_id.clone(),
        session_id: evidence.session_id.clone(),
        turn_generation_id: evidence.turn_generation_id.clone(),
        observed_at: evidence.observed_at,
    }
}

fn verify_exact_pty_transport_evidence(
    obligation: &MailboxDeliveryEvidenceObligation,
) -> Result<(), String> {
    let evidence = pty_transport_evidence(obligation);
    let expected = expected_pty_transport_evidence(&evidence);
    let state = StateDb::open_default()?;
    verify_exact_pty_transport_evidence_in_state(&state, obligation, &expected)
}

fn verify_exact_pty_transport_evidence_in_state(
    state: &StateDb,
    obligation: &MailboxDeliveryEvidenceObligation,
    expected: &DeliveryEvidence,
) -> Result<(), String> {
    if state
        .delivery_evidence(&expected.evidence_id)
        .map_err(|error| error.to_string())?
        != Some(expected.clone())
    {
        return Err(format!(
            "State evidence readback mismatch for mailbox delivery attempt {}",
            obligation.attempt_id
        ));
    }
    Ok(())
}

pub(crate) fn reconcile_pending_pty_delivery_evidence(
    mailbox: &mut MailboxDb,
    session_id: &str,
) -> Result<(), String> {
    reconcile_pty_transport_evidence(mailbox, session_id, None)
}

#[cfg(unix)]
fn delivery_attempt_submission_read(
    mailbox: &MailboxDb,
    attempt_id: &str,
) -> DeliverySubmissionRead {
    match mailbox.delivery_attempt_submission_started(attempt_id) {
        Ok(true) => DeliverySubmissionRead::Started,
        Ok(false) => DeliverySubmissionRead::NotStarted,
        Err(error) => DeliverySubmissionRead::Unreadable(error),
    }
}

pub(crate) fn finalize_pty_mailbox_delivery_handoff(
    session_id: Option<&str>,
    invocation_uuid: &str,
    exit_code: i32,
) -> Result<bool, String> {
    let Some(session_id) = session_id else {
        return Ok(false);
    };
    let Some(mut mailbox) = MailboxDb::open_default_if_exists()? else {
        return Ok(false);
    };
    reconcile_pending_pty_delivery_evidence(&mut mailbox, session_id)?;
    let attempt_ids = mailbox
        .accepted_delivery_attempt_windows(session_id)?
        .into_iter()
        .filter(|window| window.delivery_invocation_uuid == invocation_uuid)
        .map(|window| window.attempt_id)
        .collect::<Vec<_>>();
    if attempt_ids.is_empty() {
        return Ok(false);
    }
    for attempt_id in attempt_ids {
        if !mailbox.confirm_delivery_attempt(&attempt_id)? {
            return Err(format!(
                "Mailbox delivery attempt {attempt_id} cannot be confirmed without an exact generation-bound State evidence obligation"
            ));
        }
    }
    reconcile_pending_pty_delivery_evidence(&mut mailbox, session_id)?;
    crate::wake_coordinator::mark_session_idle_after_turn(
        session_id,
        invocation_uuid,
        Some(exit_code),
    )?;
    Ok(true)
}

fn pty_nack_status(message: &str) -> &str {
    match message {
        "unsafe_provider_starting" => message,
        _ => "protocol_error",
    }
}

pub(crate) fn trace_notify_enabled() -> bool {
    matches!(
        std::env::var("OULIPOLY_TRACE_NOTIFY").ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

fn trace_notify_pty_attempt(
    trigger: &str,
    session_id: &str,
    diagnostic: &PtyMailboxDeliveryDiagnostic,
) {
    let base_decision = notify_trace_base_decision(diagnostic);
    let trace_status = notify_trace_status(diagnostic);
    let inject_status = notify_trace_inject_status(base_decision, trace_status);
    let record = format_notify_trace_record(
        trigger,
        session_id,
        diagnostic,
        base_decision,
        trace_status,
        &inject_status,
    );
    append_notify_trace_record(&record);
    if trace_notify_enabled() {
        emit_notify_trace_record(&record);
    }
}

fn notify_trace_base_decision(diagnostic: &PtyMailboxDeliveryDiagnostic) -> &'static str {
    if diagnostic.submitted {
        "inject"
    } else {
        "skip"
    }
}

fn notify_trace_status(diagnostic: &PtyMailboxDeliveryDiagnostic) -> &str {
    if diagnostic.status == "stale_generation" {
        "connect_error"
    } else {
        &diagnostic.status
    }
}

fn notify_trace_control_path_present(diagnostic: &PtyMailboxDeliveryDiagnostic) -> bool {
    diagnostic
        .control_path
        .as_deref()
        .is_some_and(|path| !path.is_empty())
}

fn notify_trace_remaining_pending(diagnostic: &PtyMailboxDeliveryDiagnostic) -> String {
    diagnostic
        .remaining_pending
        .map_or_else(|| "unknown".to_string(), |value| value.to_string())
}

fn notify_trace_message(diagnostic: &PtyMailboxDeliveryDiagnostic) -> String {
    diagnostic
        .message
        .as_deref()
        .map(trace_token)
        .unwrap_or_else(|| "none".to_string())
}

fn format_notify_trace_record(
    trigger: &str,
    session_id: &str,
    diagnostic: &PtyMailboxDeliveryDiagnostic,
    base_decision: &str,
    trace_status: &str,
    inject_status: &str,
) -> String {
    format!(
        "trigger={} session_id={} attempted={} input_empty=unknown at_boundary=unknown \
         mid_escape=unknown last_user_input_ms=unknown user_input_idle_ms=unknown \
         user_input_idle=unknown user_input_idle_threshold_ms={} boundary_probe=unknown \
         quiescent=unknown last_child_output_ms=unknown foreground=unknown \
         control_path_present={} decision={} inject_status={} submitted={} \
         delivered_count={} remaining_pending={} message={} reason={} consumed=unknown",
        trace_token(trigger),
        trace_token(session_id),
        diagnostic.attempted,
        USER_INPUT_IDLE_INJECT_MS,
        notify_trace_control_path_present(diagnostic),
        notify_trace_decision(base_decision, inject_status),
        inject_status,
        diagnostic.submitted,
        diagnostic.delivered_seqs.len(),
        notify_trace_remaining_pending(diagnostic),
        notify_trace_message(diagnostic),
        notify_trace_summary_reason(trace_status),
    )
}

fn emit_notify_trace_record(record: &str) {
    eprintln!("{}", format_notify_trace_output(record));
}

fn format_notify_trace_output(record: &str) -> String {
    format!("oulipoly_notify_trace {record}")
}

fn notify_trace_summary_reason(status: &str) -> &'static str {
    match status {
        "acked" | "mark_delivered_error" => "control_ack",
        "connect_error" => "connect_error",
        "no_runtime" => "no_runtime",
        "not_pty_interactive" => "not_pty_interactive",
        _ => "protocol_or_delivery_error",
    }
}

pub(crate) fn prepare_headless_resume_delivery(
    resolved: &oulipoly_state::ResolvedResume,
    answer: Option<String>,
    models_dir: Option<&Path>,
) -> Result<PreparedMailboxDelivery, String> {
    let session_id = delivery_session_id(resolved);
    let Some(mut db) = open_mailbox_sidecar()? else {
        return Ok(empty_delivery(answer, session_id));
    };
    record_headless_session_metadata(&mut db, resolved, models_dir)?;
    let state = StateDb::open_default()?;
    reconcile_confirmed_headless_deliveries_on(&mut db, &state, &session_id)?;
    if db.notifications_paused(&session_id)? {
        return Ok(empty_delivery(answer, session_id));
    }
    let pending = pending_mailbox_rows(&db, &session_id, Some(&resolved.chain_id))?;
    delivery_for_pending(
        &mut db,
        session_id,
        Some(&resolved.chain_id),
        pending,
        answer,
    )
}

pub(crate) fn deliverable_pending_count(session_id: &str) -> Result<usize, String> {
    let Some(mut db) = open_mailbox_sidecar()? else {
        return Ok(0);
    };
    let state = StateDb::open_default()?;
    deliverable_pending_count_on(&mut db, &state, session_id)
}

pub(crate) fn deliverable_pending_count_on(
    db: &mut MailboxDb,
    state: &StateDb,
    session_id: &str,
) -> Result<usize, String> {
    reconcile_confirmed_headless_deliveries_on(db, state, session_id)?;
    if notifications_paused_on(db, session_id)? {
        return Ok(0);
    }
    pending_mailbox_row_count(db, session_id)
}

fn notifications_paused_on(db: &MailboxDb, session_id: &str) -> Result<bool, String> {
    db.notifications_paused(session_id)
}

fn pending_mailbox_row_count(db: &MailboxDb, session_id: &str) -> Result<usize, String> {
    pending_mailbox_rows(db, session_id, None).map(|rows| rows.len())
}

fn delivery_session_id(resolved: &oulipoly_state::ResolvedResume) -> String {
    resolved.active_session_id.clone()
}

fn pending_mailbox_rows(
    db: &MailboxDb,
    session_id: &str,
    chain_id: Option<&str>,
) -> Result<Vec<MailboxRow>, String> {
    let rows = load_pending_mailbox_rows(db, session_id, chain_id)?;
    verify_pending_mailbox_payloads(db, &rows)?;
    Ok(deliverable_pending_rows(rows))
}

fn load_pending_mailbox_rows(
    db: &MailboxDb,
    session_id: &str,
    chain_id: Option<&str>,
) -> Result<Vec<MailboxRow>, String> {
    db.list_pending_for_delivery(session_id, chain_id)
}

fn verify_pending_mailbox_payloads(db: &MailboxDb, rows: &[MailboxRow]) -> Result<(), String> {
    for row in rows {
        db.payloads()
            .verify_mailbox_row_payload(row)
            .map_err(|err| format_mailbox_payload_unavailable(row.seq, err))?;
    }
    Ok(())
}

fn format_mailbox_payload_unavailable(mailbox_seq: i64, err: String) -> String {
    format!("Mailbox row {mailbox_seq} payload unavailable: {err}")
}

fn deliverable_pending_rows(rows: Vec<MailboxRow>) -> Vec<MailboxRow> {
    rows.into_iter()
        .filter(mailbox_row_is_deliverable_pending)
        .collect()
}

fn delivery_for_pending(
    db: &mut MailboxDb,
    session_id: String,
    chain_id: Option<&str>,
    pending: Vec<MailboxRow>,
    answer: Option<String>,
) -> Result<PreparedMailboxDelivery, String> {
    if !has_pending_rows(&pending) {
        return Ok(empty_delivery(answer, session_id));
    }

    let batch = select_batch(&pending);
    delivery_for_batch(db, session_id, chain_id, batch, answer)
}

fn open_mailbox_sidecar() -> Result<Option<MailboxDb>, String> {
    MailboxDb::open_default_if_exists()
}

fn empty_delivery(answer: Option<String>, session_id: String) -> PreparedMailboxDelivery {
    PreparedMailboxDelivery {
        answer,
        session_id,
        seqs: Vec::new(),
        delivery_nonce: None,
        requires_turn_confirmation: false,
    }
}

fn has_pending_rows(rows: &[MailboxRow]) -> bool {
    !rows.is_empty()
}

fn batch_seqs(batch: &MailboxBatch) -> Vec<i64> {
    batch.rows.iter().map(|row| row.seq).collect()
}

fn delivery_for_batch(
    db: &mut MailboxDb,
    session_id: String,
    chain_id: Option<&str>,
    batch: MailboxBatch,
    answer: Option<String>,
) -> Result<PreparedMailboxDelivery, String> {
    let seqs = batch_seqs(&batch);
    let requires_turn_confirmation = batch
        .rows
        .iter()
        .any(|row| row.kind != SUBMITTED_INPUT_KIND);
    let delivery_nonce = new_delivery_nonce();
    db.register_headless_delivery_attempt(
        &delivery_nonce,
        &session_id,
        chain_id,
        &delivery_nonce,
        &seqs,
        batch.remaining_count,
    )?;
    let prefix = render_mailbox_prefix(&batch.rows, batch.remaining_count, &delivery_nonce)?;
    Ok(prepared_delivery(
        session_id,
        seqs,
        prefix,
        answer,
        delivery_nonce,
        requires_turn_confirmation,
    ))
}

pub(crate) fn bind_headless_resume_delivery_attempt(
    session_id: &str,
    delivery_nonce: Option<&str>,
    seqs: &[i64],
    invocation_uuid: &str,
) -> Result<(), String> {
    if seqs.is_empty() {
        return Ok(());
    }
    let delivery_nonce = delivery_nonce
        .ok_or_else(|| "headless mailbox delivery is missing its durable nonce".to_string())?;
    let Some(mut db) = open_mailbox_sidecar()? else {
        return Err("mailbox sidecar missing while binding headless delivery".to_string());
    };
    db.bind_delivery_attempt_invocation(delivery_nonce, session_id, invocation_uuid)
}

fn reconcile_confirmed_headless_deliveries_on(
    db: &mut MailboxDb,
    state: &StateDb,
    session_id: &str,
) -> Result<(), String> {
    for window in db.unresolved_delivery_attempt_windows(session_id)? {
        let Some(acknowledgement) = state
            .acknowledgement(&window.attempt_id)
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        if acknowledgement.confirmed_at.is_none() {
            continue;
        }
        if acknowledgement.session_id != window.session_id
            || acknowledgement.turn_generation_id != window.delivery_invocation_uuid
        {
            return Err(format!(
                "Confirmed delivery {} conflicts with its mailbox attempt identity",
                window.attempt_id
            ));
        }
        let seqs = window.rows.iter().map(|row| row.seq).collect::<Vec<_>>();
        let mut chain_ids = window
            .rows
            .iter()
            .filter(|row| row.target_kind.as_deref() == Some("chain"))
            .filter_map(|row| row.target_id.as_deref());
        let chain_id = chain_ids.next();
        if chain_ids.any(|candidate| Some(candidate) != chain_id) {
            return Err(format!(
                "Confirmed mailbox delivery attempt {} spans multiple chains",
                window.attempt_id
            ));
        }
        db.mark_delivered(
            &window.session_id,
            chain_id,
            &seqs,
            &window.delivery_invocation_uuid,
        )?;
    }
    Ok(())
}

fn prepared_delivery(
    session_id: String,
    seqs: Vec<i64>,
    prefix: String,
    answer: Option<String>,
    delivery_nonce: String,
    requires_turn_confirmation: bool,
) -> PreparedMailboxDelivery {
    PreparedMailboxDelivery {
        answer: Some(compose_answer(prefix, answer)),
        session_id,
        seqs,
        delivery_nonce: Some(delivery_nonce),
        requires_turn_confirmation,
    }
}

pub(crate) fn mark_headless_resume_delivered(
    session_id: &str,
    chain_id: Option<&str>,
    seqs: &[i64],
    delivered_by_invocation_uuid: &str,
) -> Result<(), String> {
    if seqs.is_empty() {
        return Ok(());
    }
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Err("mailbox sidecar missing while marking delivered rows".to_string());
    };
    db.mark_delivered(session_id, chain_id, seqs, delivered_by_invocation_uuid)
}

pub(crate) fn mark_headless_resume_delivery_failed(
    session_id: &str,
    chain_id: Option<&str>,
    seqs: &[i64],
    delivery_error: &str,
) -> Result<(), String> {
    if seqs.is_empty() {
        return Ok(());
    }
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Err("mailbox sidecar missing while marking failed delivery rows".to_string());
    };
    db.mark_delivery_failed(session_id, chain_id, seqs, delivery_error)
}

struct MailboxBatch {
    rows: Vec<MailboxRow>,
    remaining_count: usize,
}

fn record_headless_session_metadata(
    db: &mut MailboxDb,
    resolved: &oulipoly_state::ResolvedResume,
    models_dir: Option<&Path>,
) -> Result<(), String> {
    let models_dir = models_dir_string(models_dir);
    let input = headless_session_metadata_upsert(resolved, models_dir.as_deref());
    db.wake_sessions().upsert_session_metadata(input)
}

fn headless_session_metadata_upsert<'a>(
    resolved: &'a oulipoly_state::ResolvedResume,
    models_dir: Option<&'a str>,
) -> SessionMetadataUpsert<'a> {
    SessionMetadataUpsert {
        session_id: &resolved.active_session_id,
        mode: "headless",
        invocation_uuid: None,
        provider_name: Some(&resolved.active_provider),
        model_name: resolved.model_name.as_deref(),
        models_dir,
        effective_cwd: None,
    }
}

fn models_dir_string(path: Option<&Path>) -> Option<String> {
    path.map(|path| path.to_string_lossy().into_owned())
}

fn select_batch(pending: &[MailboxRow]) -> MailboxBatch {
    let prefix_lengths = candidate_prefix_lengths(pending);
    select_batch_by_prefix_lengths(pending, &prefix_lengths)
}

fn candidate_prefix_lengths(pending: &[MailboxRow]) -> Vec<usize> {
    (1..=candidate_count(pending))
        .map(|row_count| candidate_prefix_len(pending, row_count))
        .collect()
}

fn candidate_count(pending: &[MailboxRow]) -> usize {
    pending.len().min(MAILBOX_BATCH_MAX_ROWS)
}

fn candidate_prefix_len(pending: &[MailboxRow], row_count: usize) -> usize {
    let remaining_count = pending.len().saturating_sub(row_count);
    notification_prefix_len(&pending[..row_count], remaining_count)
}

fn select_batch_by_prefix_lengths(
    pending: &[MailboxRow],
    prefix_lengths: &[usize],
) -> MailboxBatch {
    let selected_count = selected_batch_len(pending, prefix_lengths);
    let rows = pending
        .iter()
        .take(selected_count)
        .cloned()
        .collect::<Vec<_>>();
    MailboxBatch {
        remaining_count: pending.len().saturating_sub(rows.len()),
        rows,
    }
}

fn selected_batch_len(pending: &[MailboxRow], prefix_lengths: &[usize]) -> usize {
    if pending.is_empty() {
        return 0;
    }
    prefix_lengths
        .iter()
        .position(|length| *length > MAILBOX_PREFIX_MAX_BYTES)
        .map(|index| selected_len_before_limit(index + 1))
        .unwrap_or(prefix_lengths.len())
}

fn selected_len_before_limit(row_count: usize) -> usize {
    if row_count > 1 { row_count - 1 } else { 1 }
}

fn notification_prefix_len(rows: &[MailboxRow], remaining_count: usize) -> usize {
    render_mailbox_prefix(rows, remaining_count, DELIVERY_NONCE_LENGTH_PLACEHOLDER)
        .map(|prefix| prefix.len())
        .unwrap_or_else(|_| MAILBOX_PREFIX_MAX_BYTES.saturating_add(1))
}

fn render_mailbox_prefix(
    rows: &[MailboxRow],
    remaining_count: usize,
    delivery_nonce: &str,
) -> Result<String, String> {
    let mut rendered = String::new();
    let contains_input = rows.iter().any(|row| row.kind == SUBMITTED_INPUT_KIND);
    if contains_input {
        rendered.push_str("[OULIPOLY INBOX]\n");
        rendered.push_str("The following accepted inbox items are pending delivery.\n\n");
    } else {
        rendered.push_str("[OULIPOLY NOTIFICATIONS]\n");
        rendered.push_str(
            "The following background agent-bash workloads completed while this session was inactive.\n\n",
        );
    }
    for (index, row) in rows.iter().enumerate() {
        if row.kind == SUBMITTED_INPUT_KIND {
            render_submitted_input(&mut rendered, index, row)?;
        } else {
            render_notification(&mut rendered, index, row);
        }
    }
    if remaining_count > 0 {
        rendered.push_str(&format!(
            "{remaining_count} additional notification(s) remain queued for the next resume.\n\n"
        ));
    }
    if !contains_input {
        rendered.push_str("Use the paths above if you need details. Do not assume log content unless you inspect it.\n");
    }
    rendered.push_str(DELIVERY_NONCE_PREFIX);
    rendered.push_str(delivery_nonce);
    rendered.push_str(DELIVERY_NONCE_SUFFIX);
    rendered.push('\n');
    rendered.push_str(if contains_input {
        "[END OULIPOLY INBOX]"
    } else {
        "[END OULIPOLY NOTIFICATIONS]"
    });
    Ok(rendered)
}

fn render_notification(rendered: &mut String, index: usize, row: &MailboxRow) {
    rendered.push_str(&format!(
        "{}. kind: {}\n   handle: {}\n   rc: {}\n   state_dir: {}\n   meta: {}\n   log: {}\n   rc_file: {}\n\n",
        index + 1,
        sanitize(&row.kind),
        sanitize(&row.handle),
        row.rc,
        quote_path(&row.state_dir),
        quote_path(&row.meta_path),
        quote_path(&row.log_path),
        quote_path(&row.rc_path),
    ));
}

fn render_submitted_input(
    rendered: &mut String,
    index: usize,
    row: &MailboxRow,
) -> Result<(), String> {
    let path = submitted_input_payload_path(row)?;
    let payload = read_submitted_input_payload(path, row.seq)?;
    rendered.push_str(&format_submitted_input(index, row, &payload));
    Ok(())
}

fn submitted_input_payload_path(row: &MailboxRow) -> Result<&str, String> {
    row.payload_file_path
        .as_deref()
        .ok_or_else(|| format_missing_input_payload_file(row.seq))
}

fn format_missing_input_payload_file(mailbox_seq: i64) -> String {
    format!("Input mailbox row {mailbox_seq} has no payload file")
}

fn read_submitted_input_payload(path: &str, mailbox_seq: i64) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format_input_payload_read_error(mailbox_seq, err))
}

fn format_input_payload_read_error(mailbox_seq: i64, err: std::io::Error) -> String {
    format!("Failed to read input mailbox row {mailbox_seq} payload: {err}")
}

fn format_submitted_input(index: usize, row: &MailboxRow, payload: &str) -> String {
    format!(
        "{}. kind: {}\n   item_id: {}\n   target: {}:{}\n   payload:\n{}\n\n",
        index + 1,
        sanitize(&row.kind),
        sanitize(&row.handle),
        sanitize(row.target_kind.as_deref().unwrap_or("unknown")),
        sanitize(row.target_id.as_deref().unwrap_or("unknown")),
        payload,
    )
}

fn new_delivery_nonce() -> String {
    Uuid::new_v4().to_string()
}

fn compose_answer(prefix: String, answer: Option<String>) -> String {
    match answer {
        Some(answer) => format!("{prefix}\n\n[USER RESUME PAYLOAD]\n{answer}"),
        None => prefix,
    }
}

fn quote_path(path: &str) -> String {
    format!("\"{}\"", sanitize(path))
}

fn sanitize(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DataDirOverride(Option<std::ffi::OsString>);

    impl DataDirOverride {
        fn install(path: &Path) -> Self {
            let prior = std::env::var_os("OULIPOLY_DATA_DIR");
            unsafe { std::env::set_var("OULIPOLY_DATA_DIR", path) };
            Self(prior)
        }
    }

    impl Drop for DataDirOverride {
        fn drop(&mut self) {
            unsafe {
                match self.0.as_ref() {
                    Some(value) => std::env::set_var("OULIPOLY_DATA_DIR", value),
                    None => std::env::remove_var("OULIPOLY_DATA_DIR"),
                }
            }
        }
    }

    #[test]
    fn compose_without_pending_preserves_answer_by_caller_contract() {
        let original = Some("byte-identical".to_string());
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27".to_string();
        let prepared = PreparedMailboxDelivery {
            answer: original.clone(),
            session_id,
            seqs: Vec::new(),
            delivery_nonce: None,
            requires_turn_confirmation: false,
        };

        assert_eq!(prepared.answer, original);
    }

    #[test]
    fn notification_prefix_includes_delivery_nonce_near_end() {
        let prefix = render_mailbox_prefix(&[], 0, "nonce-123").unwrap();

        assert!(
            prefix.contains("[OULIPOLY-DELIVERY nonce-123]\n[END OULIPOLY NOTIFICATIONS]"),
            "{prefix}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_submission_state_is_unreadable_not_not_started() {
        let directory = tempfile::tempdir().unwrap();
        let mailbox = MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();

        assert!(matches!(
            delivery_attempt_submission_read(&mailbox, "missing-attempt"),
            DeliverySubmissionRead::Unreadable(_)
        ));
    }

    #[test]
    fn concurrent_exact_evidence_reconciliation_is_idempotent_after_readback() {
        let _env_lock = DATA_DIR_ENV_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let _data_dir = DataDirOverride::install(directory.path());
        drop(StateDb::open_default().unwrap());
        let mut mailbox = MailboxDb::open_default().unwrap();
        let EnqueueResult::Inserted(row) = mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "session-a",
                handle: "handle-a",
                payload_json: "{}",
                owner_invocation_uuid: Some("invocation-a"),
                matched_os_pid: Some(1),
                matched_os_boot_id: Some("boot-a"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: "/tmp/state",
                meta_path: "/tmp/meta",
                log_path: "/tmp/log",
                rc_path: "/tmp/rc",
                rc: 0,
            })
            .unwrap()
        else {
            panic!("expected inserted mailbox row");
        };
        mailbox
            .register_or_reuse_delivery_attempt(
                "concurrent-attempt",
                "session-a",
                "invocation-a",
                "generation-a",
                &[row.seq],
                0,
            )
            .unwrap();
        mailbox
            .begin_delivery_attempt_submission("concurrent-attempt")
            .unwrap();
        mailbox
            .confirm_delivery_attempt("concurrent-attempt")
            .unwrap();
        drop(mailbox);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        *evidence_clear_barrier_slot().lock().unwrap() = Some((
            "concurrent-attempt".to_string(),
            std::sync::Arc::clone(&barrier),
        ));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let worker_barrier = std::sync::Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                reset_evidence_clear_barrier_reached();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut mailbox = MailboxDb::open_default()?;
                    reconcile_pty_transport_evidence(
                        &mut mailbox,
                        "session-a",
                        Some("concurrent-attempt"),
                    )
                }));
                if !evidence_clear_barrier_was_reached() {
                    worker_barrier.wait();
                }
                result
            }));
        }
        let results = threads
            .into_iter()
            .map(|thread| match thread.join().unwrap() {
                Ok(result) => result,
                Err(payload) => std::panic::resume_unwind(payload),
            })
            .collect::<Vec<_>>();
        *evidence_clear_barrier_slot().lock().unwrap() = None;

        assert_eq!(results, vec![Ok(()), Ok(())]);
        let mailbox = MailboxDb::open_default().unwrap();
        assert!(
            mailbox
                .pending_delivery_evidence_obligations("session-a")
                .unwrap()
                .is_empty()
        );
        let state = StateDb::open_default().unwrap();
        assert!(
            state
                .delivery_evidence("pty_transport_ack:concurrent-attempt")
                .unwrap()
                .is_some()
        );
        drop(state);
        drop(mailbox);
        let sidecar = rusqlite::Connection::open(directory.path().join("pid-identity.db")).unwrap();
        sidecar
            .execute_batch(
                "DELETE FROM mailbox_delivery_attempt_items
                 WHERE attempt_id = 'concurrent-attempt';
                 DELETE FROM mailbox_delivery_attempts
                 WHERE attempt_id = 'concurrent-attempt';",
            )
            .unwrap();
        drop(sidecar);

        let mut mailbox = MailboxDb::open_default().unwrap();
        reconcile_pty_transport_evidence(&mut mailbox, "session-a", Some("concurrent-attempt"))
            .unwrap();
    }

    #[test]
    fn v4_migration_adopts_existing_exact_state_evidence_timestamp() {
        let _env_lock = DATA_DIR_ENV_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let _data_dir = DataDirOverride::install(directory.path());
        let mut mailbox = MailboxDb::open_default().unwrap();
        let generation_id =
            RuntimeGenerationId::parse("97777777-7777-4777-8777-777777777777").unwrap();
        mailbox
            .runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "legacy-invocation",
                session_id: Some("legacy-session"),
                runtime_mode: "pty_interactive",
                provider_name: "legacy-provider",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        let EnqueueResult::Inserted(row) = mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "legacy-session",
                handle: "legacy-handle",
                payload_json: "{}",
                owner_invocation_uuid: Some("legacy-invocation"),
                matched_os_pid: Some(1),
                matched_os_boot_id: Some("legacy-boot"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: "/tmp/state",
                meta_path: "/tmp/meta",
                log_path: "/tmp/log",
                rc_path: "/tmp/rc",
                rc: 0,
            })
            .unwrap()
        else {
            panic!("expected inserted mailbox row");
        };
        mailbox
            .register_delivery_attempt(
                "legacy-attempt",
                "legacy-session",
                "legacy-invocation",
                &[row.seq],
                0,
            )
            .unwrap();
        mailbox
            .begin_delivery_attempt_submission("legacy-attempt")
            .unwrap();
        mailbox.confirm_delivery_attempt("legacy-attempt").unwrap();

        let exact_observed_at = 123_456;
        let exact_evidence = PtyTransportAcknowledgementEvidence {
            evidence_id: "pty_transport_ack:legacy-attempt".to_string(),
            delivery_attempt_id: "legacy-attempt".to_string(),
            session_id: "legacy-session".to_string(),
            turn_generation_id: generation_id.to_string(),
            observed_at: exact_observed_at,
        };
        let mut state = StateDb::open_default().unwrap();
        exact_evidence.record(&mut state).unwrap();
        drop(state);
        drop(mailbox);

        let sidecar_path = directory.path().join("pid-identity.db");
        let connection = rusqlite::Connection::open(&sidecar_path).unwrap();
        for column in [
            "evidence_reconciled_at",
            "evidence_observed_at",
            "evidence_turn_generation_id",
            "submission_started_at",
            "evidence_disposition",
        ] {
            connection
                .execute_batch(&format!(
                    "ALTER TABLE mailbox_delivery_attempts DROP COLUMN {column};"
                ))
                .unwrap();
        }
        connection.pragma_update(None, "user_version", 4).unwrap();
        drop(connection);

        let mut mailbox = MailboxDb::open_default().unwrap();
        let migrated = mailbox
            .pending_delivery_evidence_obligations("legacy-session")
            .unwrap();
        assert_eq!(migrated.len(), 1);
        assert!(migrated[0].legacy);
        assert_ne!(migrated[0].observed_at, exact_observed_at);

        reconcile_pending_pty_delivery_evidence(&mut mailbox, "legacy-session").unwrap();
        assert!(
            mailbox
                .pending_delivery_evidence_obligations("legacy-session")
                .unwrap()
                .is_empty()
        );
        let state = StateDb::open_default().unwrap();
        assert_eq!(
            state
                .delivery_evidence("pty_transport_ack:legacy-attempt")
                .unwrap()
                .unwrap()
                .observed_at,
            exact_observed_at
        );
    }

    #[test]
    fn v4_unresolved_ack_handoff_requires_exact_migrated_generation_evidence() {
        fn enqueue(
            mailbox: &mut MailboxDb,
            session_id: &str,
            handle: &str,
            invocation_uuid: &str,
        ) -> MailboxRow {
            let EnqueueResult::Inserted(row) = mailbox
                .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                    session_id,
                    handle,
                    payload_json: "{}",
                    owner_invocation_uuid: Some(invocation_uuid),
                    matched_os_pid: Some(1),
                    matched_os_boot_id: Some("legacy-boot"),
                    matched_os_pid_starttime_ticks: Some(1),
                    matched_chain_index: Some(0),
                    state_dir: "/tmp/state",
                    meta_path: "/tmp/meta",
                    log_path: "/tmp/log",
                    rc_path: "/tmp/rc",
                    rc: 0,
                })
                .unwrap()
            else {
                panic!("expected inserted mailbox row");
            };
            row
        }

        fn create_generation(
            mailbox: &mut MailboxDb,
            generation_uuid: &str,
            session_id: &str,
            invocation_uuid: &str,
        ) -> RuntimeGenerationId {
            let generation_id = RuntimeGenerationId::parse(generation_uuid).unwrap();
            mailbox
                .runtime_lifecycle()
                .create_runtime_generation(CreateRuntimeGeneration {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: invocation_uuid,
                    session_id: Some(session_id),
                    runtime_mode: "pty_interactive",
                    provider_name: "legacy-provider",
                    model_name: None,
                    pty_control_path: None,
                    models_dir: None,
                    effective_cwd: None,
                })
                .unwrap();
            generation_id
        }

        let _env_lock = DATA_DIR_ENV_LOCK.lock().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let _data_dir = DataDirOverride::install(directory.path());
        drop(StateDb::open_default().unwrap());
        let mut mailbox = MailboxDb::open_default().unwrap();
        let unique = (
            "legacy-unique-session",
            "legacy-unique-invocation",
            "legacy-unique-attempt",
        );
        let unmatched = (
            "legacy-unmatched-session",
            "legacy-unmatched-invocation",
            "legacy-unmatched-attempt",
        );
        let ambiguous = (
            "legacy-ambiguous-session",
            "legacy-ambiguous-invocation",
            "legacy-ambiguous-attempt",
        );
        let unique_generation = create_generation(
            &mut mailbox,
            "98888888-8888-4888-8888-888888888888",
            unique.0,
            unique.1,
        );
        for generation_uuid in [
            "99999999-9999-4999-8999-999999999999",
            "9aaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        ] {
            create_generation(&mut mailbox, generation_uuid, ambiguous.0, ambiguous.1);
        }
        for (session_id, invocation_uuid, attempt_id) in [unique, unmatched, ambiguous] {
            let row = enqueue(&mut mailbox, session_id, attempt_id, invocation_uuid);
            mailbox
                .register_delivery_attempt(attempt_id, session_id, invocation_uuid, &[row.seq], 0)
                .unwrap();
            mailbox
                .record_delivery_attempt_transport_ack(attempt_id)
                .unwrap();
        }
        drop(mailbox);

        let sidecar_path = directory.path().join("pid-identity.db");
        let connection = rusqlite::Connection::open(&sidecar_path).unwrap();
        connection
            .execute(
                "UPDATE runtime_generation
                 SET lifecycle_state = 'exited',
                     exited_at = created_at,
                     terminal_reason = 'startup_failed',
                     exit_code = 0
                 WHERE generation_uuid = ?1",
                rusqlite::params![unique_generation.to_string()],
            )
            .unwrap();
        for column in [
            "evidence_reconciled_at",
            "evidence_observed_at",
            "evidence_turn_generation_id",
            "submission_started_at",
            "evidence_disposition",
        ] {
            connection
                .execute_batch(&format!(
                    "ALTER TABLE mailbox_delivery_attempts DROP COLUMN {column};"
                ))
                .unwrap();
        }
        connection.pragma_update(None, "user_version", 4).unwrap();
        drop(connection);

        let mailbox = MailboxDb::open_default().unwrap();
        let obligation = mailbox
            .pending_delivery_evidence_obligations(unique.0)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(obligation.attempt_id, unique.2);
        assert_eq!(obligation.turn_generation_id, unique_generation.to_string());
        assert!(obligation.legacy);
        drop(mailbox);

        assert!(finalize_pty_mailbox_delivery_handoff(Some(unique.0), unique.1, 0).unwrap());
        let mailbox = MailboxDb::open_default().unwrap();
        let unique_attempt = mailbox.delivery_attempt_window(unique.2).unwrap().unwrap();
        assert!(unique_attempt.resolved_at.is_some());
        let unique_rows = mailbox.list_mailbox(unique.0, true).unwrap();
        assert_eq!(unique_rows.len(), 1);
        assert!(unique_rows[0].delivered_at.is_some());
        assert!(
            mailbox
                .pending_delivery_evidence_obligations(unique.0)
                .unwrap()
                .is_empty()
        );
        drop(mailbox);
        let state = StateDb::open_default().unwrap();
        assert_eq!(
            state
                .delivery_evidence(&format!("pty_transport_ack:{}", unique.2))
                .unwrap(),
            Some(expected_pty_transport_evidence(&pty_transport_evidence(
                &obligation
            )))
        );
        drop(state);

        for (attempt, expected_disposition) in [
            (unmatched, "legacy_unmatched_generation"),
            (ambiguous, "legacy_ambiguous_generation"),
        ] {
            let error =
                finalize_pty_mailbox_delivery_handoff(Some(attempt.0), attempt.1, 0).unwrap_err();
            assert!(error.contains("exact generation-bound State evidence obligation"));
            let mailbox = MailboxDb::open_default().unwrap();
            let window = mailbox.delivery_attempt_window(attempt.2).unwrap().unwrap();
            assert!(window.resolved_at.is_none());
            assert!(window.rows[0].delivered_at.is_none());
            let disposition = rusqlite::Connection::open(&sidecar_path)
                .unwrap()
                .query_row(
                    "SELECT evidence_disposition
                     FROM mailbox_delivery_attempts
                     WHERE attempt_id = ?1",
                    rusqlite::params![attempt.2],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            assert_eq!(disposition, expected_disposition);
            drop(mailbox);
            assert!(
                StateDb::open_default()
                    .unwrap()
                    .delivery_evidence(&format!("pty_transport_ack:{}", attempt.2))
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn harness_startup_nack_remains_retryable_and_precise() {
        assert_eq!(
            pty_nack_status("unsafe_provider_starting"),
            "unsafe_provider_starting"
        );
        assert_eq!(pty_nack_status("broker_rejected"), "protocol_error");
    }

    #[test]
    fn stale_generation_trace_uses_normalized_connect_error_reason() {
        let diagnostic = PtyMailboxDeliveryDiagnostic {
            attempted: true,
            status: "stale_generation".to_string(),
            control_path: None,
            submitted: false,
            delivered_seqs: Vec::new(),
            remaining_pending: None,
            message: None,
        };
        let trace_status = notify_trace_status(&diagnostic);
        let record = format_notify_trace_record(
            "early_wake",
            "session-id",
            &diagnostic,
            notify_trace_base_decision(&diagnostic),
            trace_status,
            trace_status,
        );

        assert!(record.contains("inject_status=connect_error"), "{record}");
        assert!(record.contains("reason=connect_error"), "{record}");
    }
}
