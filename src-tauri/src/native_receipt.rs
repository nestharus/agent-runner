//! Provider-neutral native receipt observation, shared by resume and wake maintenance.
//! Receipt settlement has no invocation lifecycle authority.
//!
//! ## Declared roles
//! `accessor`, `mapper`, `orchestration`, `predicate`
use oulipoly_provider::client::CancellationToken;
use oulipoly_runtime::session_provider::{
    SessionProviderIdentity, SessionProviderPageCursor, SessionProviderReadPageRequest,
    SessionProviderTurnProjection, read_turn_page,
};
use oulipoly_state::mailbox::{MailboxDb, MailboxDeliveryObservationAnchor};
use std::time::{Duration, Instant};

pub(crate) const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const OBSERVATION_DEADLINE: Duration = Duration::from_secs(30);
pub(crate) const OBSERVATION_MAX_PAGES: usize = 16;
pub(crate) const OBSERVATION_MAX_TURNS: u64 = 64;
pub(crate) const OBSERVATION_MAX_RESPONSE_BYTES: u64 = 128 * 1024;
pub(crate) const OBSERVATION_MAX_SOURCE_BYTES: u64 = 512 * 1024;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ObservationProgress {
    #[serde(default)]
    pub(crate) receipt_policy: u32,
    #[serde(default)]
    pub(crate) reader_identity: Option<String>,
    pub(crate) snapshot_id: Option<String>,
    pub(crate) page_token: Option<String>,
    pub(crate) after_token: Option<String>,
    pub(crate) page_index: u64,
    pub(crate) turn_sequence: u64,
    pub(crate) matching_turn_id: Option<String>,
    pub(crate) matching_turns: u64,
    pub(crate) complete: bool,
}

impl ObservationProgress {
    fn cursor(
        &self,
        anchor: &MailboxDeliveryObservationAnchor,
    ) -> Result<SessionProviderPageCursor, String> {
        match (&self.snapshot_id, &self.page_token) {
            (Some(snapshot_id), Some(page_token)) => Ok(SessionProviderPageCursor::Continuation {
                snapshot_id: snapshot_id.clone(),
                page_token: page_token.clone(),
            }),
            (None, None) => Ok(SessionProviderPageCursor::Beginning {
                after_token: self
                    .after_token
                    .clone()
                    .or_else(|| anchor.resume_token.clone()),
            }),
            _ => Err("mailbox observation checkpoint incomplete".into()),
        }
    }
}

pub(crate) fn confirm_delivery_observation(
    db: &MailboxDb,
    attempt_id: &str,
    registry: &oulipoly_runtime::provider_registry::ProviderRegistry,
    identity: SessionProviderIdentity,
    effective_cwd: &std::path::Path,
    anchor: &MailboxDeliveryObservationAnchor,
) -> Result<bool, String> {
    confirm_delivery_observation_bounded(
        db,
        attempt_id,
        registry,
        identity,
        effective_cwd,
        anchor,
        OBSERVATION_MAX_PAGES,
        OBSERVATION_DEADLINE,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn confirm_delivery_observation_bounded(
    db: &MailboxDb,
    attempt_id: &str,
    registry: &oulipoly_runtime::provider_registry::ProviderRegistry,
    identity: SessionProviderIdentity,
    effective_cwd: &std::path::Path,
    anchor: &MailboxDeliveryObservationAnchor,
    max_pages: usize,
    budget: Duration,
) -> Result<bool, String> {
    ensure_observation_not_stopped(db, &anchor.provider_session_id)?;
    let endpoint = registry
        .preflight_account(&identity.provider_name)
        .map_err(|e| e.to_string())?;
    // Validate live semantic admission even when a completed checkpoint needs
    // no further page. Parsed configuration equality is not receipt authority.
    if endpoint.account_name() != anchor.provider_name
        || format!("{}-instance", endpoint.capabilities().provider_id)
            != anchor.provider_instance_id
        || endpoint.settings_id().map_err(|e| e.to_string())? != anchor.settings_id
        || identity.provider_name != anchor.provider_name
        || identity.provider_instance_id.as_deref() != Some(anchor.provider_instance_id.as_str())
        || identity.settings_id != anchor.settings_id
    {
        return Err("receipt endpoint identity changed".into());
    }
    // Existing pinned-client identity, not a provider version-string claim. A
    // changed adapter cannot inherit weaker cached matches or later cursors.
    let reader_identity = endpoint.client().pinned_executable_identity_sha256()?;
    let cancellation = CancellationToken::new();
    observe_delivery_for_revision_with(
        db,
        attempt_id,
        anchor,
        max_pages,
        // Preparation has its own transport deadline; synchronous identity IO
        // is not page work. A successful slow preparation must still leave a
        // page opportunity. This is a page budget, not an end-to-end deadline.
        budget,
        &reader_identity,
        |cursor, page_index, turn_sequence, remaining| {
            read_turn_page(SessionProviderReadPageRequest {
                registry,
                identity: identity.clone(),
                session_id: &anchor.provider_session_id,
                effective_cwd: Some(effective_cwd),
                projection: SessionProviderTurnProjection::UserObservation,
                expected_delivery_nonce: Some(attempt_id),
                cursor,
                expected_page_index: page_index,
                expected_turn_sequence: turn_sequence,
                max_turns: OBSERVATION_MAX_TURNS,
                max_response_bytes: OBSERVATION_MAX_RESPONSE_BYTES,
                max_source_bytes: OBSERVATION_MAX_SOURCE_BYTES,
                max_inline_body_bytes: 0,
                cancellation: &cancellation,
                timeout: remaining.min(OBSERVATION_TIMEOUT),
            })
            .map_err(|error| {
                retain_observation_failure(db, &anchor.provider_session_id, attempt_id, error)
            })
        },
    )
}

// The production reader validates account/session/nonce-bound opaque pages.
// Keeping the scan driver separate permits deterministic offline fault/restart
// tests without launching a provider or admitting a second semantic prompt.
#[cfg(test)]
pub(crate) fn observe_delivery_with(
    db: &MailboxDb,
    attempt_id: &str,
    anchor: &MailboxDeliveryObservationAnchor,
    read: impl FnMut(
        SessionProviderPageCursor,
        u64,
        u64,
        Duration,
    ) -> Result<
        oulipoly_runtime::session_provider::SessionProviderReadPageResult,
        String,
    >,
) -> Result<bool, String> {
    observe_delivery_bounded_with(
        db,
        attempt_id,
        anchor,
        OBSERVATION_MAX_PAGES,
        OBSERVATION_DEADLINE,
        read,
    )
}

#[cfg(test)]
pub(crate) fn observe_delivery_bounded_with(
    db: &MailboxDb,
    attempt_id: &str,
    anchor: &MailboxDeliveryObservationAnchor,
    max_pages: usize,
    budget: Duration,
    read: impl FnMut(
        SessionProviderPageCursor,
        u64,
        u64,
        Duration,
    ) -> Result<
        oulipoly_runtime::session_provider::SessionProviderReadPageResult,
        String,
    >,
) -> Result<bool, String> {
    observe_delivery_for_revision_with(
        db,
        attempt_id,
        anchor,
        max_pages,
        budget,
        "offline-fixture-v1",
        read,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn observe_delivery_for_revision_with(
    db: &MailboxDb,
    attempt_id: &str,
    anchor: &MailboxDeliveryObservationAnchor,
    max_pages: usize,
    budget: Duration,
    reader_identity: &str,
    mut read: impl FnMut(
        SessionProviderPageCursor,
        u64,
        u64,
        Duration,
    ) -> Result<
        oulipoly_runtime::session_provider::SessionProviderReadPageResult,
        String,
    >,
) -> Result<bool, String> {
    ensure_observation_not_stopped(db, &anchor.provider_session_id)?;
    if db.delivery_observation_confirmation(attempt_id)?.is_some() {
        return Ok(true);
    }
    let Some(owner) = db.delivery_observation_owner(attempt_id, &anchor.provider_session_id)?
    else {
        return Ok(false);
    };
    let deadline = Instant::now() + budget;
    let mut stored = db.delivery_observation_progress(attempt_id)?;
    let mut progress: ObservationProgress = stored
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|err| format!("invalid observation checkpoint: {err}"))?
        .unwrap_or_default();
    // Pre-upgrade cached matches used a weaker provider projection. Discard
    // their interpretation, not the immutable submission anchor. An unanchored
    // legacy window cannot prove newness and remains unknown.
    if progress.receipt_policy != 1 || progress.reader_identity.as_deref() != Some(reader_identity)
    {
        if anchor.resume_token.is_none() {
            return Ok(false);
        }
        progress = ObservationProgress {
            receipt_policy: 1,
            reader_identity: Some(reader_identity.to_string()),
            ..Default::default()
        };
        let next = serde_json::to_string(&progress).map_err(|e| e.to_string())?;
        db.advance_delivery_observation_progress(attempt_id, stored.as_deref(), &next)?;
        stored = Some(next);
    }
    for _ in 0..max_pages {
        if progress.matching_turns > 1 {
            return Ok(false);
        }
        if progress.complete {
            if progress.matching_turns == 1 {
                return db.confirm_native_delivery_receipt(
                    attempt_id,
                    &owner,
                    anchor,
                    stored.as_deref().ok_or("observation checkpoint missing")?,
                    progress
                        .matching_turn_id
                        .as_deref()
                        .ok_or("observation match id missing")?,
                );
            }
            // A completed empty snapshot is not proof of non-submission. Scan
            // future append-only evidence from its opaque resume token, never
            // create another attempt or rescan the old snapshot.
            progress.complete = false;
            progress.snapshot_id = None;
            progress.page_token = None;
            progress.page_index = 0;
            progress.turn_sequence = 0;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let page = match read(
            progress.cursor(anchor)?,
            progress.page_index,
            progress.turn_sequence,
            remaining,
        ) {
            Ok(page) => page,
            Err(error) => {
                db.record_delivery_observation_error(attempt_id, &error)?;
                return Err(error);
            }
        };
        if page.provider_instance_id != anchor.provider_instance_id
            || page.settings_id != anchor.settings_id
            || page.session_id != anchor.provider_session_id
            || page.projection != SessionProviderTurnProjection::UserObservation
            || page.page_index != progress.page_index
            || page.page_start_sequence != progress.turn_sequence
            || progress
                .snapshot_id
                .as_ref()
                .is_some_and(|snapshot| snapshot != &page.snapshot_id)
        {
            return Err("mailbox observation identity/position mismatch".into());
        }
        for turn in page.turns.iter().filter(|turn| turn.role == "user") {
            if turn.canonical_text_sha256.as_deref() == Some(anchor.expected_sha256.as_str()) {
                progress.matching_turns = progress.matching_turns.saturating_add(1);
                progress
                    .matching_turn_id
                    .get_or_insert_with(|| turn.turn_id.clone());
            }
        }
        progress.complete = page.snapshot_complete;
        if page.snapshot_complete {
            progress.after_token = Some(
                page.resume_token
                    .ok_or("mailbox observation resume token missing")?,
            );
            progress.snapshot_id = None;
            progress.page_token = None;
        } else {
            progress.page_index = page
                .page_index
                .checked_add(1)
                .ok_or("observation page overflow")?;
            progress.turn_sequence = page
                .page_start_sequence
                .checked_add(page.page_turn_count)
                .ok_or("observation sequence overflow")?;
            progress.snapshot_id = Some(page.snapshot_id);
            progress.page_token = Some(
                page.next_page_token
                    .ok_or("mailbox observation page token missing")?,
            );
        }
        let next = serde_json::to_string(&progress).map_err(|err| err.to_string())?;
        db.advance_delivery_observation_progress(attempt_id, stored.as_deref(), &next)?;
        stored = Some(next);
        if progress.complete {
            if progress.matching_turns == 1 {
                return db.confirm_native_delivery_receipt(
                    attempt_id,
                    &owner,
                    anchor,
                    stored.as_deref().ok_or("observation checkpoint missing")?,
                    progress
                        .matching_turn_id
                        .as_deref()
                        .ok_or("observation match id missing")?,
                );
            }
            return Ok(false);
        }
    }
    Ok(false)
}

pub(crate) fn ensure_observation_not_stopped(
    db: &MailboxDb,
    session_id: &str,
) -> Result<(), String> {
    if let Some(stop) = db.mailbox_observation_stop(session_id)? {
        return Err(format!(
            "mailbox_observation_stopped stop_id={}: {}",
            stop.stop_id, stop.error
        ));
    }
    Ok(())
}

pub(crate) fn retain_observation_failure(
    db: &MailboxDb,
    session_id: &str,
    attempt_id: &str,
    error: oulipoly_runtime::session_provider::SessionProviderError,
) -> String {
    let message = error.to_string();
    if let Some(reason) = error.fixed_observation_stop_reason() {
        if let Err(storage) = db.stop_mailbox_observation(session_id, attempt_id, reason, &message)
        {
            return format!("{message}; {storage}");
        }
    }
    message
}

/// One periodic slot, one provider page, independent of assistant output and
/// semantic resume. The durable scan cursor advances before any external IO.
pub(crate) fn poll_headless_receipt_tick() -> Result<(), String> {
    helper::run_once()
}

pub(crate) fn poll_headless_receipt_tick_with<
    R: std::borrow::Borrow<oulipoly_runtime::provider_registry::ProviderRegistry>,
>(
    db: &mut MailboxDb,
    make_registry: impl FnOnce(Option<&std::path::Path>) -> Result<R, String>,
) -> Result<(), String> {
    // Independent physical connections/processes share this admission. It is
    // retained across selection, preparation, pages and publication, but is not
    // a SQLite writer lock and does not exclude consumer ACKs.
    let Some(_admission) = helper::try_admit(db.path(), "receipt-scan")? else {
        return Ok(());
    };

    let Some(attempt_id) = db.next_headless_receipt_attempt()? else {
        return Ok(());
    };
    let Some(anchor) = db.delivery_observation_anchor(&attempt_id)? else {
        return Ok(());
    };
    let Some(runtime) = db
        .wake_session_reader()
        .session_metadata(&anchor.provider_session_id)?
    else {
        return Ok(());
    };
    if runtime.session_id != anchor.provider_session_id
        || runtime.provider_name.as_deref() != Some(anchor.provider_name.as_str())
    {
        return Err("receipt runtime identity mismatch".into());
    }
    let cwd = match runtime.effective_cwd {
        Some(cwd) => std::path::PathBuf::from(cwd),
        None => {
            let state = oulipoly_state::StateDb::open_read_only(
                &oulipoly_state::StateDb::default_path()?,
            )
            .map_err(|error| format!("receipt cwd recovery: {error}"))?;
            let Some(cwd) = recover_observation_cwd(&state, &anchor)? else {
                return Ok(());
            };
            cwd
        }
    };
    if !cwd.is_absolute() {
        return Err("receipt cwd must be absolute".into());
    }
    let registry = make_registry(runtime.models_dir.as_deref().map(std::path::Path::new))?;
    // The normal reader performs endpoint/schema/settings admission. No vendor
    // parsing or execution/resume capability is called by this inspection path.
    let identity = SessionProviderIdentity {
        model_name: runtime.model_name.unwrap_or_default(),
        provider_name: anchor.provider_name.clone(),
        provider_instance_id: Some(anchor.provider_instance_id.clone()),
        settings_id: anchor.settings_id.clone(),
    };
    confirm_delivery_observation_bounded(
        db,
        &attempt_id,
        registry.borrow(),
        identity,
        &cwd,
        &anchor,
        1,
        Duration::from_secs(2),
    )?;
    Ok(())
}

// Reuse the same persisted, authority-bound cwd sources as session metadata.
// No current-directory, arbitrary invocation, or filesystem discovery fallback.
fn recover_observation_cwd(
    state: &oulipoly_state::StateDb,
    anchor: &MailboxDeliveryObservationAnchor,
) -> Result<Option<std::path::PathBuf>, String> {
    let authority = oulipoly_state::StoredProviderSessionAuthority {
        provider_instance_id: anchor.provider_instance_id.clone(),
        settings_id: anchor.settings_id.clone(),
    };
    let imported = state.imported_session_cwd_for_authority(
        &anchor.provider_name,
        &anchor.provider_session_id,
        &authority,
    )?;
    let cwd = match imported {
        Some(cwd) => Some(cwd),
        None => state.latest_provider_session_resolved_account_for_authority(
            &anchor.provider_name,
            &anchor.provider_session_id,
            &authority,
        )?,
    };
    cwd.map(|cwd| {
        let path = std::path::PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err("receipt recovered cwd must be absolute".into());
        }
        Ok(path)
    })
    .transpose()
}

/// A headless owner keeps the existing scanner active even when no desktop/REPL
/// maintenance service is present (including auto-wake children). Drop cancels
/// future ticks and joins the single bounded in-flight inspection, never a model.
pub(crate) struct ReceiptPollGuard {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    cancellation: Option<CancellationToken>,
}
impl Drop for ReceiptPollGuard {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(token) = &self.cancellation {
            token.cancel();
        }
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            if let Err(error) = worker.join() {
                tracing::warn!(?error, "receipt observer panicked");
            }
        }
    }
}
pub(crate) fn start_headless_receipt_polling() -> Result<ReceiptPollGuard, String> {
    helper::start()
}

#[cfg(test)]
pub(crate) fn start_receipt_polling_with(
    mut tick: impl FnMut() -> Result<(), String> + Send + 'static,
    interval: Duration,
) -> Result<ReceiptPollGuard, String> {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopping = stop.clone();
    let worker = std::thread::Builder::new()
        .name("headless-native-receipt".into())
        .spawn(move || {
            while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
                if let Err(error) = tick() {
                    tracing::warn!("Bounded active headless receipt tick: {error}");
                }
                // IO may consume the unpark token (for example a channel wait).
                // Do not park for another full interval after stop was requested.
                if stopping.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                std::thread::park_timeout(interval);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(ReceiptPollGuard {
        stop,
        worker: Some(worker),
        cancellation: None,
    })
}

#[cfg(test)]
#[path = "native_receipt_correction_tests.rs"]
mod correction_tests;

#[path = "native_receipt_helper.rs"]
pub(crate) mod helper;
