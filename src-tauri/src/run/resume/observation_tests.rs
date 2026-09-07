//! Offline headless observation and semantic-submission custody experiments.
use super::*;
use crate::mailbox_delivery::{deliverable_pending_count_on, prepare_headless_resume_delivery_on};
use oulipoly_runtime::session_provider::{SessionProviderPageTurn, SessionProviderReadPageResult};
use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, EnqueueResult};

const SESSION: &str = "observation-session";
struct Fixture {
    root: tempfile::TempDir,
    db: MailboxDb,
    state: oulipoly_state::StateDb,
    attempt: String,
    anchor: MailboxDeliveryObservationAnchor,
    seq: i64,
    submissions: usize,
}
impl Fixture {
    fn new() -> Self {
        Self::with_legacy(false)
    }
    fn with_legacy(legacy: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open(&root.path().join("pid-identity.db")).unwrap();
        let state = oulipoly_state::StateDb::open(&root.path().join("state.db")).unwrap();
        let EnqueueResult::Inserted(row) = db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle: "completed-offline-work",
                payload_json: "{}",
                owner_invocation_uuid: Some("owner"),
                matched_os_pid: Some(1),
                matched_os_boot_id: Some("boot"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: "/offline/state",
                meta_path: "/offline/meta",
                log_path: "/offline/log",
                rc_path: "/offline/rc",
                rc: 0,
            })
            .unwrap()
        else {
            panic!("missing fixture row")
        };
        let (attempt, envelope) = if legacy {
            let attempt = "legacy-exact-nonce".to_string();
            db.register_delivery_attempt(&attempt, SESSION, "native-invocation", &[row.seq], 0)
                .unwrap();
            let window = db
                .legacy_delivery_observation_candidates(SESSION, 1)
                .unwrap()
                .remove(0);
            (
                attempt,
                crate::mailbox_delivery::legacy_notification_envelope(&window)
                    .unwrap()
                    .unwrap(),
            )
        } else {
            let prepared =
                prepare_headless_resume_delivery_on(&mut db, SESSION, "chain", None, None).unwrap();
            let attempt = prepared.delivery_nonce.unwrap();
            db.bind_delivery_attempt_invocation(&attempt, SESSION, "native-invocation")
                .unwrap();
            (attempt, prepared.answer.unwrap())
        };
        let anchor = MailboxDeliveryObservationAnchor {
            provider_name: "account".into(),
            provider_instance_id: "instance".into(),
            settings_id: "settings".into(),
            provider_session_id: SESSION.into(),
            resume_token: Some("opaque-tail".into()),
            expected_sha256: normalized_text_sha256(&envelope),
        };
        Self {
            root,
            db,
            state,
            attempt,
            anchor,
            seq: row.seq,
            submissions: 0,
        }
    }
    fn submit(&mut self) -> Result<(), String> {
        self.db.begin_headless_delivery_submission(
            &self.attempt,
            SESSION,
            "native-invocation",
            true,
        )?;
        self.submissions += 1; // offline stand-in at the production pre-executor fence
        Ok(())
    }
    fn anchored_submit(&mut self) {
        self.db
            .record_delivery_observation_anchor(&self.attempt, SESSION, &self.anchor)
            .unwrap();
        self.submit().unwrap();
    }
    fn restart(&mut self) {
        self.db = MailboxDb::open(&self.root.path().join("pid-identity.db")).unwrap();
    }
    fn assert_pending_without_replay(&mut self) {
        assert_eq!(
            deliverable_pending_count_on(&mut self.db, &self.state, SESSION).unwrap(),
            1
        );
        assert!(
            prepare_headless_resume_delivery_on(&mut self.db, SESSION, "chain", None, None)
                .is_err()
        );
        assert!(
            self.db
                .delivery_observation_confirmation(&self.attempt)
                .unwrap()
                .is_none()
        );
        assert_eq!(self.submissions, 1);
    }
}
fn page(
    anchor: &MailboxDeliveryObservationAnchor,
    index: u64,
    sequence: u64,
    complete: bool,
    matches: usize,
) -> SessionProviderReadPageResult {
    let turns = (0..matches)
        .map(|n| SessionProviderPageTurn {
            session_id: SESSION.into(),
            turn_id: format!("native-user-{}", sequence + n as u64),
            snapshot_sequence: sequence + n as u64,
            timestamp: chrono::Utc::now(),
            role: "user".into(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body_state: oulipoly_state::SessionTurnPageBodyState::OmittedOversize,
            body: None,
            body_bytes: Some(1024),
            body_sha256: None,
            canonical_text_sha256: Some(anchor.expected_sha256.clone()),
            canonical_text_digest_verified: false,
        })
        .collect();
    SessionProviderReadPageResult {
        provider_instance_id: anchor.provider_instance_id.clone(),
        settings_id: anchor.settings_id.clone(),
        session_id: SESSION.into(),
        projection: SessionProviderTurnProjection::UserObservation,
        snapshot_id: "opaque-snapshot".into(),
        page_index: index,
        page_start_sequence: sequence,
        turns,
        page_turn_count: matches as u64,
        source_bytes_examined: OBSERVATION_MAX_SOURCE_BYTES,
        scan_progress: matches == 0 && !complete,
        snapshot_complete: complete,
        next_page_token: (!complete).then(|| format!("opaque-page-{}", index + 1)),
        resume_token: complete.then(|| "opaque-next-tail".into()),
        source_final: false,
        warnings: vec![],
        request_token_sha256: "0".repeat(64),
        page_digest: "1".repeat(64),
    }
}

#[test]
fn age347_capacity_blockage_restart_continuation_and_exactly_once_settlement() {
    let mut f = Fixture::new();
    f.anchored_submit(); // native accepts the exact notification and produces an answer
    for error in [
        "session_turn_staging_capacity_exceeded",
        "transient_observation_error",
    ] {
        f.restart();
        let result =
            observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, _, _, _| Err(error.into()));
        assert_eq!(result.unwrap_err(), error);
        f.assert_pending_without_replay();
    }
    let mut calls = 0;
    assert!(
        !observe_delivery_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            |cursor, index, sequence, _| {
                if calls == 0 {
                    assert_eq!(
                        cursor,
                        SessionProviderPageCursor::Beginning {
                            after_token: Some("opaque-tail".into())
                        }
                    );
                }
                calls += 1;
                Ok(page(
                    &f.anchor,
                    index,
                    sequence,
                    false,
                    usize::from(index == 0),
                ))
            }
        )
        .unwrap()
    );
    assert_eq!(calls, OBSERVATION_MAX_PAGES);
    f.restart();
    f.assert_pending_without_replay(); // one exact match is not complete-snapshot evidence
    assert!(
        observe_delivery_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            |cursor, index, sequence, _| {
                assert_eq!(index, OBSERVATION_MAX_PAGES as u64);
                assert_eq!(sequence, 1);
                assert_eq!(
                    cursor,
                    SessionProviderPageCursor::Continuation {
                        snapshot_id: "opaque-snapshot".into(),
                        page_token: format!("opaque-page-{index}"),
                    }
                );
                Ok(page(&f.anchor, index, sequence, true, 0))
            }
        )
        .unwrap()
    );
    for _ in 0..3 {
        f.restart();
        assert_eq!(
            deliverable_pending_count_on(&mut f.db, &f.state, SESSION).unwrap(),
            0
        );
        let rows = f.db.list_mailbox(SESSION, true).unwrap();
        let row = rows.iter().find(|row| row.seq == f.seq).unwrap();
        assert_eq!(row.delivery_attempts, 1);
        assert_eq!(
            row.delivered_by_invocation_uuid.as_deref(),
            Some("native-invocation")
        );
        assert_eq!(f.submissions, 1);
    }
}

#[test]
fn age347_missing_anchor_fails_closed_but_known_unsubmitted_work_can_deliver() {
    let mut f = Fixture::new();
    f.db.record_delivery_observation_anchor_failure(&f.attempt, SESSION, "capacity")
        .unwrap();
    assert!(f.submit().is_err());
    assert_eq!(f.submissions, 0);
    let prepared =
        prepare_headless_resume_delivery_on(&mut f.db, SESSION, "chain", None, None).unwrap();
    assert!(f.submit().is_err()); // replaced preparation cannot race the new one
    f.attempt = prepared.delivery_nonce.unwrap();
    f.anchor.expected_sha256 = normalized_text_sha256(prepared.answer.as_deref().unwrap());
    f.db.bind_delivery_attempt_invocation(&f.attempt, SESSION, "native-invocation")
        .unwrap();
    f.anchored_submit();
    assert!(f.submit().is_err());
    assert_eq!(f.submissions, 1);
}

#[test]
fn age347_empty_snapshot_is_uncertain_then_scans_new_append_without_replay() {
    let mut f = Fixture::new();
    f.anchored_submit();
    assert!(
        !observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, index, seq, _| Ok(page(
            &f.anchor, index, seq, true, 0
        )))
        .unwrap()
    );
    f.restart();
    f.assert_pending_without_replay();
    assert!(
        observe_delivery_with(&f.db, &f.attempt, &f.anchor, |cursor, index, seq, _| {
            assert_eq!(
                cursor,
                SessionProviderPageCursor::Beginning {
                    after_token: Some("opaque-next-tail".into())
                }
            );
            assert_eq!((index, seq), (0, 0));
            Ok(page(&f.anchor, index, seq, true, 1))
        })
        .unwrap()
    );
}

#[test]
fn age347_mismatched_identity_snapshot_and_prose_never_confirm() {
    for mismatch in [
        "session",
        "account",
        "settings",
        "nonce",
        "assistant",
        "duplicates",
    ] {
        let mut f = Fixture::new();
        f.anchored_submit();
        let _ = observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, index, seq, _| {
            let mut result = page(
                &f.anchor,
                index,
                seq,
                true,
                if mismatch == "duplicates" { 2 } else { 1 },
            );
            match mismatch {
                "session" => result.session_id = "other-session".into(),
                "account" => result.provider_instance_id = "other-account".into(),
                "settings" => result.settings_id = "other-settings".into(),
                "nonce" => {
                    result.turns[0].canonical_text_sha256 =
                        Some(normalized_text_sha256("different exact nonce envelope"))
                }
                "assistant" => result.turns[0].role = "assistant".into(),
                _ => (),
            }
            Ok(result)
        });
        f.assert_pending_without_replay();
    }
}

#[test]
fn age347_checkpoint_cas_prevents_concurrent_reader_regression() {
    let mut f = Fixture::new();
    f.anchored_submit();
    f.db.advance_delivery_observation_progress(&f.attempt, None, "{}")
        .unwrap();
    assert!(
        f.db.advance_delivery_observation_progress(&f.attempt, None, "stale")
            .is_err()
    );
    assert_eq!(
        f.db.delivery_observation_progress(&f.attempt)
            .unwrap()
            .as_deref(),
        Some("{}")
    );
}

#[test]
fn age347_legacy_null_marker_recovery_uses_beginning_without_invented_anchor() {
    let mut f = Fixture::with_legacy(true);
    f.submissions = 1; // retained incident signal: native accepted although marker is null
    f.anchor.resume_token = None;
    f.assert_pending_without_replay();
    assert!(
        !f.db
            .delivery_attempt_submission_started(&f.attempt)
            .unwrap()
    );
    f.db.record_legacy_delivery_observation_identity(&f.attempt, SESSION, &f.anchor)
        .unwrap();
    assert!(
        observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, _, _, _| Err(
            "capacity".into()
        ))
        .is_err()
    );
    f.restart();
    f.assert_pending_without_replay();
    assert!(
        observe_delivery_with(&f.db, &f.attempt, &f.anchor, |cursor, index, seq, _| {
            assert_eq!(
                cursor,
                SessionProviderPageCursor::Beginning { after_token: None }
            );
            Ok(page(&f.anchor, index, seq, true, 1))
        })
        .unwrap()
    );
    assert!(
        !f.db
            .delivery_attempt_submission_started(&f.attempt)
            .unwrap()
    );
    assert_eq!(
        deliverable_pending_count_on(&mut f.db, &f.state, SESSION).unwrap(),
        0
    );
    assert_eq!(f.submissions, 1);
}
