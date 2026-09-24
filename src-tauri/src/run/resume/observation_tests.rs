//! Offline headless observation and semantic-submission custody experiments.
use super::*;
use crate::mailbox_delivery::{deliverable_pending_count_on, prepare_headless_resume_delivery_on};
use oulipoly_runtime::session_provider::{SessionProviderPageTurn, SessionProviderReadPageResult};
use oulipoly_state::SessionLifecycleRepository;
use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, EnqueueResult};

fn register_receipt_runtime(f: &mut Fixture) {
    let config_root = f.root.path().join("config");
    f.db.wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: SESSION,
            mode: "headless",
            invocation_uuid: Some("native-invocation"),
            provider_name: Some("account"),
            model_name: Some("offline"),
            models_dir: config_root.to_str(),
            effective_cwd: f.root.path().to_str(),
        })
        .unwrap();
}

#[test]
fn detached_receipt_page_requires_exact_pending_anchor_and_checkpoint() {
    use crate::native_receipt::broker_scan::{
        ScanRequest, ScanResult, finish_completed_checkpoint, integrate_result, prepare_request,
    };
    let result_for = |request: &ScanRequest, matches: &[&str]| -> ScanResult {
        serde_json::from_value(serde_json::json!({
            "protocol": "receipt-scan-page-v1",
            "request_sha256": request.digest().unwrap(),
            "page": {
                "provider_instance_id": "instance", "settings_id": "settings",
                "session_id": SESSION, "reader_identity": "fixture-reader",
                "snapshot_id": "snapshot-1", "page_index": 0,
                "page_start_sequence": 0, "page_turn_count": matches.len(),
                "snapshot_complete": true, "next_page_token": null,
                "resume_token": "after-1", "matching_turn_ids": matches,
            }
        }))
        .unwrap()
    };
    let mut f = Fixture::new();
    assert!(prepare_request(&f.db, &f.attempt).unwrap().is_none());
    f.anchored_submit();
    register_receipt_runtime(&mut f);
    let request = prepare_request(&f.db, &f.attempt).unwrap().unwrap();
    let positive = result_for(&request, &["turn-1"]);
    let mut wrong: serde_json::Value = serde_json::to_value(&positive).unwrap();
    wrong["request_sha256"] = serde_json::json!("0".repeat(64));
    assert!(integrate_result(&f.db, &request, &serde_json::from_value(wrong).unwrap()).is_err());
    assert!(
        f.db.delivery_observation_progress(&f.attempt)
            .unwrap()
            .is_none()
    );
    f.restart(); // Lost worker reply: retained request/result can be read back and CASed once.
    assert!(integrate_result(&f.db, &request, &positive).unwrap());
    assert!(
        f.db.delivery_observation_confirmation(&f.attempt)
            .unwrap()
            .is_some()
    );
    assert!(integrate_result(&f.db, &request, &positive).is_err());

    let mut changed = Fixture::new();
    changed.anchored_submit();
    register_receipt_runtime(&mut changed);
    let request = prepare_request(&changed.db, &changed.attempt)
        .unwrap()
        .unwrap();
    rusqlite::Connection::open(changed.db.path())
        .unwrap()
        .execute(
            "UPDATE session_runtime SET effective_cwd='/changed' WHERE session_id=?1",
            [SESSION],
        )
        .unwrap();
    assert!(integrate_result(&changed.db, &request, &result_for(&request, &["turn-1"])).is_err());
    assert!(
        changed
            .db
            .delivery_observation_progress(&changed.attempt)
            .unwrap()
            .is_none()
    );
    changed.assert_pending_without_replay();

    let mut absent = Fixture::new();
    absent.anchored_submit();
    register_receipt_runtime(&mut absent);
    let request = prepare_request(&absent.db, &absent.attempt)
        .unwrap()
        .unwrap();
    assert!(!integrate_result(&absent.db, &request, &result_for(&request, &[])).unwrap());
    absent.restart();
    absent.assert_pending_without_replay();

    let mut ambiguous = Fixture::new();
    ambiguous.anchored_submit();
    register_receipt_runtime(&mut ambiguous);
    let request = prepare_request(&ambiguous.db, &ambiguous.attempt)
        .unwrap()
        .unwrap();
    assert!(
        !integrate_result(
            &ambiguous.db,
            &request,
            &result_for(&request, &["turn-1", "turn-2"])
        )
        .unwrap()
    );
    ambiguous.restart();
    ambiguous.assert_pending_without_replay();

    let mut interrupted = Fixture::new();
    interrupted.anchored_submit();
    register_receipt_runtime(&mut interrupted);
    let request = prepare_request(&interrupted.db, &interrupted.attempt)
        .unwrap()
        .unwrap();
    let conn = rusqlite::Connection::open(interrupted.db.path()).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER receipt_confirmation_fault
        BEFORE UPDATE OF observation_confirmed_at ON mailbox_delivery_attempts
        BEGIN SELECT RAISE(ABORT,'private receipt confirmation fault'); END;",
    )
    .unwrap();
    assert!(
        integrate_result(
            &interrupted.db,
            &request,
            &result_for(&request, &["turn-1"])
        )
        .is_err()
    );
    assert!(
        interrupted
            .db
            .delivery_observation_progress(&interrupted.attempt)
            .unwrap()
            .is_some()
    );
    assert!(
        interrupted
            .db
            .delivery_observation_confirmation(&interrupted.attempt)
            .unwrap()
            .is_none()
    );
    conn.execute_batch("DROP TRIGGER receipt_confirmation_fault")
        .unwrap();
    interrupted.restart();
    let retained = prepare_request(&interrupted.db, &interrupted.attempt)
        .unwrap()
        .unwrap();
    assert!(finish_completed_checkpoint(&interrupted.db, &retained).unwrap());
    assert!(
        interrupted
            .db
            .delivery_observation_confirmation(&interrupted.attempt)
            .unwrap()
            .is_some()
    );
}

#[test]
fn detached_receipt_page_reads_actual_provider_and_keeps_uncertain_work_pending() {
    use crate::native_receipt::broker_scan::{
        integrate_result, prepare_request, scan_with_registry,
    };
    use oulipoly_config::{ProviderEndpointConfig, ProviderEntry, ProvidersConfig};
    use oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;

    for mode in ["positive", "nonzero", "cancel", "ambiguous"] {
        let mut f = Fixture::new();
        f.anchor.provider_instance_id = "receipt-scan-fixture-instance".into();
        let script = f.root.path().join("receipt-scan-fixture.py");
        let source = include_str!("../../native_receipt/scan_fixture.py").replace(
            "__FIXTURE_ROOT__",
            &serde_json::to_string(f.root.path()).unwrap(),
        );
        std::fs::write(&script, source).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let hashes = if mode == "ambiguous" {
            vec![
                f.anchor.expected_sha256.clone(),
                f.anchor.expected_sha256.clone(),
            ]
        } else {
            vec![f.anchor.expected_sha256.clone()]
        };
        std::fs::write(
            f.root.path().join("turn-hashes.json"),
            serde_json::to_vec(&hashes).unwrap(),
        )
        .unwrap();
        if mode == "nonzero" {
            std::fs::write(f.root.path().join("nonzero"), b"").unwrap();
        }
        if mode == "cancel" {
            std::fs::write(f.root.path().join("hold"), b"").unwrap();
        }
        f.anchored_submit();
        register_receipt_runtime(&mut f);
        let providers = ProvidersConfig {
            entries: HashMap::from([(
                "account".into(),
                ProviderEntry {
                    implementation: Some(ProviderEndpointConfig {
                        family: "receipt-scan-fixture".into(),
                        executable: script.display().to_string(),
                    }),
                    settings_id: Some("settings".into()),
                    ..ProviderEntry::default()
                },
            )]),
        };
        let registry = ProviderRegistry::from_configs(
            &[],
            &providers,
            ProviderRegistryOptions::default()
                .with_config_root(f.root.path().join("config"))
                .with_data_root(f.root.path().join("data")),
        )
        .unwrap();
        let request = prepare_request(&f.db, &f.attempt).unwrap().unwrap();
        let cancellation = CancellationToken::new();
        if mode == "cancel" {
            cancellation.cancel_after(Duration::from_millis(100));
        }
        let result = scan_with_registry(&request, &registry, &cancellation, Duration::from_secs(5));
        if mode == "nonzero" || mode == "cancel" {
            assert!(result.is_err());
            assert!(
                f.db.delivery_observation_progress(&f.attempt)
                    .unwrap()
                    .is_none()
            );
            f.restart();
            f.assert_pending_without_replay();
            continue;
        }
        let result = result.unwrap_or_else(|error| {
            panic!(
                "{error}; provider={}; calls={}",
                std::fs::read_to_string(f.root.path().join("provider-error")).unwrap_or_default(),
                std::fs::read_to_string(f.root.path().join("provider-calls")).unwrap_or_default()
            )
        });
        // The reply may be lost after the provider ran. Integration uses the
        // retained request and result once; it never invokes that provider again.
        f.restart();
        assert_eq!(
            integrate_result(&f.db, &request, &result).unwrap(),
            mode == "positive"
        );
        let calls = std::fs::read_to_string(f.root.path().join("provider-calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| *line == "session.read_turns")
                .count(),
            1
        );
        if mode == "ambiguous" {
            f.assert_pending_without_replay();
        } else {
            assert!(
                f.db.delivery_observation_confirmation(&f.attempt)
                    .unwrap()
                    .is_some()
            );
            assert!(integrate_result(&f.db, &request, &result).is_err());
        }
    }
}

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
struct Fixture {
    root: tempfile::TempDir,
    db: MailboxDb,
    state: oulipoly_state::StateDb,
    attempt: String,
    anchor: MailboxDeliveryObservationAnchor,
    seq: i64,
    submissions: usize,
    envelope: String,
}
impl Fixture {
    fn new() -> Self {
        Self::with_legacy(false)
    }
    fn with_legacy(legacy: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut db = MailboxDb::open_historical(&root.path().join("pid-identity.db")).unwrap();
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
            envelope,
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
        self.db = MailboxDb::open_historical(&self.root.path().join("pid-identity.db")).unwrap();
    }
    fn assert_pending_without_replay(&mut self) {
        let stopped = self.db.mailbox_observation_stop(SESSION).unwrap().is_some();
        assert_eq!(
            deliverable_pending_count_on(&mut self.db, &self.state, SESSION).unwrap(),
            usize::from(!stopped)
        );
        let evidence = rusqlite::Connection::open(self.db.path()).unwrap();
        let pending: i64 = evidence
            .query_row(
                "SELECT COUNT(*) FROM mailbox WHERE session_id=?1 AND delivered_at IS NULL",
                [SESSION],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1, "the stopped row remains physically pending");
        let selected: i64 = evidence
            .query_row(
                "SELECT COUNT(*) FROM mailbox_delivery_attempt_items WHERE attempt_id=?1 AND mailbox_seq=?2",
                rusqlite::params![self.attempt, self.seq],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            selected, 1,
            "the exact attempt-to-row selection is retained"
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
fn age355_unanchored_legacy_recovery_remains_unknown_without_invented_newness() {
    let mut f = Fixture::with_legacy(true);
    f.submissions = 1;
    f.anchor.resume_token = None;
    f.db.record_legacy_delivery_observation_identity(&f.attempt, SESSION, &f.anchor)
        .unwrap();
    for _ in 0..2 {
        assert!(
            !observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, _, _, _| {
                panic!("unanchored legacy history cannot prove new receipt")
            })
            .unwrap()
        );
        f.restart();
        f.assert_pending_without_replay();
    }
}

#[cfg(unix)]
#[path = "observation_paired_tests.rs"]
mod paired_tests;

#[test]
fn fixed_anchor_stop_survives_restart_demand_and_rearms_without_ack_or_replay() {
    for reason in [
        "session_turn_staging_capacity_exceeded",
        "session_turn_paging_paused",
    ] {
        let mut f = Fixture::new();
        f.db.record_delivery_observation_anchor_failure(&f.attempt, SESSION, reason)
            .unwrap();
        f.db.stop_mailbox_observation(SESSION, &f.attempt, reason, reason)
            .unwrap();
        let stop = f.db.mailbox_observation_stop(SESSION).unwrap().unwrap();
        assert_eq!(stop.attempt_id, f.attempt);
        assert_eq!(stop.reason, reason);
        assert_eq!(stop.error, reason);
        let EnqueueResult::Inserted(new_row) =
            f.db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle: "new-demand-while-stopped",
                payload_json: "{}",
                owner_invocation_uuid: Some("owner"),
                matched_os_pid: Some(1),
                matched_os_boot_id: Some("boot"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: "/offline/new",
                meta_path: "/offline/new/meta",
                log_path: "/offline/new/log",
                rc_path: "/offline/new/rc",
                rc: 0,
            })
            .unwrap()
        else {
            panic!("new notification missing")
        };
        assert!(new_row.seq > f.seq);
        for _ in 0..3 {
            f.restart();
            // Routine pause/resume is not intervention-resolution authority.
            f.db.set_notifications_paused(SESSION, true).unwrap();
            f.db.set_notifications_paused(SESSION, false).unwrap();
            assert_eq!(
                deliverable_pending_count_on(&mut f.db, &f.state, SESSION).unwrap(),
                0
            );
            assert!(
                prepare_headless_resume_delivery_on(&mut f.db, SESSION, "chain", None, None)
                    .is_err()
            );
            assert!(f.submit().is_err());
            assert_eq!(f.submissions, 0);
            let rows = f.db.list_pending(SESSION).unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].delivery_attempts, 0);
            assert_eq!(rows[0].delivered_at, None);
            assert_eq!(
                f.db.mailbox_observation_stop(SESSION)
                    .unwrap()
                    .unwrap()
                    .stop_id,
                stop.stop_id
            );
            assert!(
                f.db.rearm_mailbox_observation(SESSION, "stale-stop", "resolved")
                    .is_err()
            );
            assert!(
                f.db.rearm_mailbox_observation(SESSION, &stop.stop_id, "  ")
                    .is_err()
            );
        }
        // Direct state admission is also fenced; preparation cannot replace the evidence.
        assert!(
            f.db.register_headless_delivery_attempt(
                "replacement",
                SESSION,
                None,
                "other",
                &[f.seq],
                0
            )
            .is_err()
        );
        f.db.rearm_mailbox_observation(
            SESSION,
            &stop.stop_id,
            "fixture capacity restored / containment disabled",
        )
        .unwrap();
        assert_eq!(f.db.list_pending(SESSION).unwrap().len(), 2);
        assert_eq!(f.submissions, 0);
        let prepared =
            prepare_headless_resume_delivery_on(&mut f.db, SESSION, "chain", None, None).unwrap();
        f.attempt = prepared.delivery_nonce.unwrap();
        f.anchor.expected_sha256 = normalized_text_sha256(prepared.answer.as_deref().unwrap());
        f.db.bind_delivery_attempt_invocation(&f.attempt, SESSION, "native-invocation")
            .unwrap();
        f.anchored_submit();
        assert!(f.submit().is_err());
        assert!(
            observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, index, seq, _| {
                Ok(page(&f.anchor, index, seq, true, 1))
            })
            .unwrap()
        );
        for _ in 0..3 {
            f.restart();
            assert_eq!(
                deliverable_pending_count_on(&mut f.db, &f.state, SESSION).unwrap(),
                0
            );
            let rows = f.db.list_mailbox(SESSION, true).unwrap();
            assert_eq!(rows[0].delivery_attempts, 1);
            assert!(rows[0].delivered_at.is_some());
            assert_eq!(f.submissions, 1);
        }
    }
}

#[test]
fn fixed_post_submission_stop_retains_checkpoint_and_never_resubmits() {
    let mut f = Fixture::new();
    f.anchored_submit();
    assert!(
        !observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, index, seq, _| {
            Ok(page(&f.anchor, index, seq, false, usize::from(index == 0)))
        })
        .unwrap()
    );
    let checkpoint = f.db.delivery_observation_progress(&f.attempt).unwrap();
    f.db.stop_mailbox_observation(SESSION, &f.attempt, "session_turn_paging_paused", "paused")
        .unwrap();
    f.restart();
    assert!(
        observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, _, _, _| {
            panic!("stopped observer must not read provider")
        })
        .is_err()
    );
    assert_eq!(
        f.db.delivery_observation_progress(&f.attempt).unwrap(),
        checkpoint
    );
    let stop = f.db.mailbox_observation_stop(SESSION).unwrap().unwrap();
    f.db.rearm_mailbox_observation(SESSION, &stop.stop_id, "containment disabled")
        .unwrap();
    assert!(prepare_headless_resume_delivery_on(&mut f.db, SESSION, "chain", None, None).is_err());
    assert!(
        observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, index, seq, _| {
            assert_eq!((index, seq), (OBSERVATION_MAX_PAGES as u64, 1));
            Ok(page(&f.anchor, index, seq, true, 0))
        })
        .unwrap()
    );
    assert_eq!(
        deliverable_pending_count_on(&mut f.db, &f.state, SESSION).unwrap(),
        0
    );
    assert_eq!(f.submissions, 1);
}

#[test]
fn early_ack_settled_anchor_is_historical_not_a_stop_or_replay_candidate() {
    let mut f = Fixture::new();
    f.anchored_submit();
    f.db.acknowledge_range(SESSION, f.seq, f.seq, "consumer")
        .unwrap();
    f.restart();
    assert!(
        f.db.delivery_attempt_fully_settled(&f.attempt, SESSION, Some("chain"), &[f.seq])
            .unwrap()
    );
    assert!(
        f.db.delivery_observation_anchor(&f.attempt)
            .unwrap()
            .is_none()
    );
    assert!(
        f.db.delivery_observation_confirmation(&f.attempt)
            .unwrap()
            .is_none()
    );
    assert!(
        f.db.pending_delivery_observations(SESSION, 4)
            .unwrap()
            .is_empty()
    );
    assert!(f.db.mailbox_observation_stop(SESSION).unwrap().is_none());
    assert!(f.submit().is_err());
    assert_eq!(f.submissions, 1);
    assert_eq!(
        deliverable_pending_count_on(&mut f.db, &f.state, SESSION).unwrap(),
        0
    );
    assert!(f.state.acknowledgement(&f.attempt).unwrap().is_none());
}

#[test]
fn age355_active_no_output_receipt_never_terminalizes_running_invocation() {
    let mut f = Fixture::new();
    f.state
        .start_invocation(&oulipoly_state::InvocationStart {
            invocation_uuid: "native-invocation".into(),
            model_name: "offline".into(),
            provider_name: "account".into(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    f.db.wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: SESSION,
            mode: "headless",
            invocation_uuid: Some("native-invocation"),
            provider_name: Some("account"),
            model_name: Some("offline"),
            models_dir: None,
            effective_cwd: Some("/offline"),
        })
        .unwrap();
    f.anchored_submit();
    assert_eq!(
        f.db.next_headless_receipt_attempt().unwrap(),
        Some(f.attempt.clone())
    );
    let before =
        f.db.wake_session_reader()
            .session_metadata(SESSION)
            .unwrap()
            .unwrap();
    let mut reads = 0;
    assert!(
        observe_delivery_bounded_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            |_, index, seq, remaining| {
                reads += 1;
                assert!(remaining <= Duration::from_secs(2));
                let p = page(&f.anchor, index, seq, true, 1);
                assert!(!p.source_final); // native history has accepted input, no assistant event
                Ok(p)
            }
        )
        .unwrap()
    );
    assert_eq!(reads, 1);
    assert!(f.db.list_pending(SESSION).unwrap().is_empty());
    assert_eq!(
        f.state
            .get_invocation_by_uuid("native-invocation")
            .unwrap()
            .unwrap()
            .status,
        oulipoly_state::InvocationStatus::Running
    );
    assert_eq!(
        f.db.wake_session_reader()
            .session_metadata(SESSION)
            .unwrap()
            .unwrap(),
        before
    );
    // A subsequent native failure is independent, cannot undo or resend receipt.
    assert!(
        f.db.mark_delivery_attempt_failed(&f.attempt, SESSION, None, &[f.seq], "native_exit_1")
            .unwrap()
    );
    f.restart();
    assert!(
        f.db.delivery_observation_confirmation(&f.attempt)
            .unwrap()
            .is_some()
    );
    assert_eq!(f.db.next_headless_receipt_attempt().unwrap(), None);
    assert_eq!(
        f.db.list_mailbox(SESSION, true).unwrap()[0].delivery_attempts,
        1
    );
    assert_eq!(f.submissions, 1);
}

#[test]
fn age355_old_cached_match_is_reobserved_from_original_anchor_not_promoted() {
    let mut f = Fixture::new();
    f.anchored_submit();
    let old = serde_json::json!({"snapshot_id":null,"page_token":null,"after_token":"old-end",
        "page_index":0,"turn_sequence":1,"matching_turn_id":"weaker-match",
        "matching_turns":1,"complete":true})
    .to_string();
    f.db.advance_delivery_observation_progress(&f.attempt, None, &old)
        .unwrap();
    f.restart();
    assert!(
        !observe_delivery_bounded_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            |cursor, index, seq, _| {
                assert_eq!(
                    cursor,
                    SessionProviderPageCursor::Beginning {
                        after_token: f.anchor.resume_token.clone()
                    }
                );
                Ok(page(&f.anchor, index, seq, true, 0))
            }
        )
        .unwrap()
    );
    assert_eq!(
        f.db.delivery_observation_anchor(&f.attempt)
            .unwrap()
            .unwrap(),
        f.anchor
    );
    f.assert_pending_without_replay();
}

#[test]
fn age355_one_page_ticks_resume_finite_uniqueness_after_restart() {
    for duplicates in [false, true] {
        let mut f = Fixture::new();
        f.anchored_submit();
        assert!(
            !observe_delivery_bounded_with(
                &f.db,
                &f.attempt,
                &f.anchor,
                1,
                Duration::from_secs(2),
                |_, index, seq, _| Ok(page(&f.anchor, index, seq, false, 1))
            )
            .unwrap()
        );
        f.restart();
        let confirmed = observe_delivery_bounded_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            |cursor, index, seq, _| {
                assert!(matches!(
                    cursor,
                    SessionProviderPageCursor::Continuation { .. }
                ));
                assert_eq!((index, seq), (1, 1));
                Ok(page(&f.anchor, index, seq, true, usize::from(duplicates)))
            },
        )
        .unwrap();
        assert_eq!(confirmed, !duplicates);
        if duplicates {
            f.assert_pending_without_replay();
        }
    }
}

#[test]
fn age355_final_confirmation_rechecks_stop_owner_checkpoint_and_full_ack() {
    for race in ["stop", "owner", "checkpoint", "full_ack", "pause"] {
        let mut f = Fixture::new();
        f.anchored_submit();
        let checkpoint = "unique finite snapshot";
        f.db.advance_delivery_observation_progress(&f.attempt, None, checkpoint)
            .unwrap();
        // Separate physical handle represents a writer between scan and commit.
        let conn = rusqlite::Connection::open(f.root.path().join("pid-identity.db")).unwrap();
        match race {
            "stop" => {
                f.db.stop_mailbox_observation(
                    SESSION,
                    &f.attempt,
                    "session_turn_paging_paused",
                    "fixed",
                )
                .unwrap()
            }
            "pause" => f.db.set_notifications_paused(SESSION, true).unwrap(),
            "owner" => {
                conn.execute("UPDATE mailbox_delivery_attempts SET delivery_invocation_uuid = 'successor' WHERE attempt_id = ?1", [&f.attempt]).unwrap();
            }
            "checkpoint" => {
                f.db.advance_delivery_observation_progress(&f.attempt, Some(checkpoint), "newer")
                    .unwrap()
            }
            "full_ack" => {
                f.db.acknowledge_range(SESSION, f.seq, f.seq, "consumer")
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            !f.db
                .confirm_native_delivery_receipt(
                    &f.attempt,
                    "native-invocation",
                    &f.anchor,
                    checkpoint,
                    "exact-native-user"
                )
                .unwrap(),
            "{race}"
        );
        assert!(
            f.db.delivery_observation_confirmation(&f.attempt)
                .unwrap()
                .is_none()
        );
        let row = &f.db.list_mailbox(SESSION, true).unwrap()[0];
        assert_eq!(row.delivered_at.is_some(), race == "full_ack");
    }
}

fn seed_derived_observation_checkpoint(f: &Fixture) {
    assert!(
        !observe_delivery_bounded_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            |cursor, index, sequence, _| {
                assert_eq!(
                    cursor,
                    SessionProviderPageCursor::Beginning {
                        after_token: Some("opaque-tail".into())
                    }
                );
                Ok(page(&f.anchor, index, sequence, false, 0))
            },
        )
        .unwrap()
    );
}

#[test]
fn age360_stale_continuation_refreshes_once_from_original_anchor_without_replay_or_ack() {
    let mut f = Fixture::new();
    f.anchored_submit();
    seed_derived_observation_checkpoint(&f);
    let mut calls = 0;
    assert!(
        observe_delivery_stale_fixture_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            |cursor, index, sequence, _| {
                calls += 1;
                if calls == 1 {
                    assert!(matches!(
                        cursor,
                        SessionProviderPageCursor::Continuation { .. }
                    ));
                    return Err(true);
                }
                assert_eq!(
                    cursor,
                    SessionProviderPageCursor::Beginning {
                        after_token: Some("opaque-tail".into())
                    }
                );
                assert_eq!((index, sequence), (0, 0));
                Ok(page(&f.anchor, index, sequence, true, 1))
            },
        )
        .unwrap()
    );
    assert_eq!(calls, 2);
    assert_eq!(f.submissions, 1);
    let evidence = rusqlite::Connection::open(f.db.path()).unwrap();
    let (attempts, acknowledged, confirmation): (i64, Option<String>, Option<String>) = evidence
        .query_row(
            "SELECT m.delivery_attempts,a.acknowledged_at,a.observation_confirmed_turn_id
             FROM mailbox_delivery_attempts a
             JOIN mailbox_delivery_attempt_items i ON i.attempt_id=a.attempt_id
             JOIN mailbox m ON m.seq=i.mailbox_seq
             WHERE a.attempt_id=?1",
            [&f.attempt],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(attempts, 1);
    assert_eq!(acknowledged, None);
    assert_eq!(confirmation.as_deref(), Some("native-user-0"));
}

#[test]
fn age360_repeated_stale_stops_durably_and_leaves_other_attempts_fair() {
    let mut f = Fixture::new();
    f.anchored_submit();
    seed_derived_observation_checkpoint(&f);
    let error =
        observe_delivery_stale_fixture_with(&f.db, &f.attempt, &f.anchor, |cursor, _, _, _| {
            assert!(matches!(
                cursor,
                SessionProviderPageCursor::Continuation { .. }
                    | SessionProviderPageCursor::Beginning { .. }
            ));
            Err(true)
        })
        .unwrap_err();
    assert!(error.contains("stale_after_anchor_refresh"), "{error}");
    let stop = f.db.mailbox_observation_stop(SESSION).unwrap().unwrap();
    assert_eq!(stop.attempt_id, f.attempt);
    assert_eq!(stop.reason, "session_turn_page_token_stale");
    assert!(stop.error.contains("replay=forbidden"));
    assert!(
        f.db.delivery_observation_confirmation(&f.attempt)
            .unwrap()
            .is_none()
    );
    let evidence = rusqlite::Connection::open(f.db.path()).unwrap();
    let acknowledged: Option<String> = evidence
        .query_row(
            "SELECT acknowledged_at FROM mailbox_delivery_attempts WHERE attempt_id=?1",
            [&f.attempt],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(acknowledged, None);
    f.restart();
    assert!(
        observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, _, _, _| {
            panic!("durably stopped work must not be retried")
        })
        .unwrap_err()
        .contains("mailbox_observation_stopped")
    );
    f.assert_pending_without_replay();

    const OTHER_SESSION: &str = "22222222-2222-4222-8222-222222222222";
    let EnqueueResult::Inserted(_other_row) =
        f.db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: OTHER_SESSION,
            handle: "unrelated-fair-work",
            payload_json: "{}",
            owner_invocation_uuid: Some("other-owner"),
            matched_os_pid: Some(1),
            matched_os_boot_id: Some("boot"),
            matched_os_pid_starttime_ticks: Some(1),
            matched_chain_index: Some(0),
            state_dir: "/offline/other-state",
            meta_path: "/offline/other-meta",
            log_path: "/offline/other-log",
            rc_path: "/offline/other-rc",
            rc: 0,
        })
        .unwrap()
    else {
        panic!("missing unrelated row")
    };
    let prepared =
        prepare_headless_resume_delivery_on(&mut f.db, OTHER_SESSION, "other-chain", None, None)
            .unwrap();
    let other_attempt = prepared.delivery_nonce.unwrap();
    f.db.bind_delivery_attempt_invocation(&other_attempt, OTHER_SESSION, "other-invocation")
        .unwrap();
    let other_anchor = MailboxDeliveryObservationAnchor {
        provider_session_id: OTHER_SESSION.into(),
        expected_sha256: normalized_text_sha256(&prepared.answer.unwrap()),
        ..f.anchor.clone()
    };
    f.db.record_delivery_observation_anchor(&other_attempt, OTHER_SESSION, &other_anchor)
        .unwrap();
    f.db.wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: OTHER_SESSION,
            mode: "headless",
            invocation_uuid: Some("other-invocation"),
            provider_name: Some("account"),
            model_name: Some("offline"),
            models_dir: None,
            effective_cwd: Some("/offline"),
        })
        .unwrap();
    f.db.begin_headless_delivery_submission(
        &other_attempt,
        OTHER_SESSION,
        "other-invocation",
        true,
    )
    .unwrap();
    let visits = [
        f.db.next_headless_receipt_attempt().unwrap(),
        f.db.next_headless_receipt_attempt().unwrap(),
    ];
    assert!(
        visits
            .iter()
            .any(|candidate| candidate.as_ref() == Some(&other_attempt)),
        "the fair keyset cursor must move past the stopped slot to unrelated work: {visits:?}"
    );
}

#[test]
fn age360_stale_without_anchor_or_after_cas_loss_stops_without_replay() {
    for case in ["missing-anchor", "cas-loss"] {
        let mut f = Fixture::new();
        f.anchored_submit();
        seed_derived_observation_checkpoint(&f);
        if case == "missing-anchor" {
            f.anchor.resume_token = None;
        }
        let competing = MailboxDb::open(f.db.path()).unwrap();
        let attempt = f.attempt.clone();
        let error =
            observe_delivery_stale_fixture_with(&f.db, &f.attempt, &f.anchor, |_, _, _, _| {
                if case == "cas-loss" {
                    let previous = competing
                        .delivery_observation_progress(&attempt)
                        .unwrap()
                        .unwrap();
                    let mut changed: ObservationProgress = serde_json::from_str(&previous).unwrap();
                    changed.page_index += 1;
                    let changed = serde_json::to_string(&changed).unwrap();
                    competing
                        .advance_delivery_observation_progress(&attempt, Some(&previous), &changed)
                        .unwrap();
                }
                Err(true)
            })
            .unwrap_err();
        assert!(
            error.contains(if case == "missing-anchor" {
                "immutable_anchor_unavailable"
            } else {
                "anchor_refresh_cas_lost"
            }),
            "{case}: {error}"
        );
        assert!(f.db.mailbox_observation_stop(SESSION).unwrap().is_some());
        f.assert_pending_without_replay();
    }
}

#[test]
fn age355_stop_during_page_io_cannot_publish_receipt() {
    let mut f = Fixture::new();
    f.anchored_submit();
    assert!(
        !observe_delivery_bounded_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            |_, index, seq, _| {
                f.db.stop_mailbox_observation(
                    SESSION,
                    &f.attempt,
                    "session_turn_paging_paused",
                    "stopped during IO",
                )
                .unwrap();
                Ok(page(&f.anchor, index, seq, true, 1))
            }
        )
        .unwrap()
    );
    assert!(
        f.db.delivery_observation_confirmation(&f.attempt)
            .unwrap()
            .is_none()
    );
    f.restart();
    assert!(
        observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, _, _, _| panic!("stopped"))
            .is_err()
    );
    assert_eq!(f.submissions, 1);
}

fn enqueue_additional(f: &mut Fixture, handle: &str) -> i64 {
    let EnqueueResult::Inserted(row) =
        f.db.enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
            session_id: SESSION,
            handle,
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
        panic!("expected new fixture item");
    };
    row.seq
}

#[test]
fn age355_partial_ack_race_keeps_consumer_authority_and_settles_only_remainder() {
    let mut f = Fixture::new();
    let second = enqueue_additional(&mut f, "second");
    let prepared =
        prepare_headless_resume_delivery_on(&mut f.db, SESSION, "chain", None, None).unwrap();
    f.attempt = prepared.delivery_nonce.unwrap();
    f.envelope = prepared.answer.unwrap();
    f.anchor.expected_sha256 = normalized_text_sha256(&f.envelope);
    f.db.bind_delivery_attempt_invocation(&f.attempt, SESSION, "native-invocation")
        .unwrap();
    f.anchored_submit();
    let mut competing = MailboxDb::open(&f.root.path().join("pid-identity.db")).unwrap();
    assert!(
        observe_delivery_bounded_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            |_, index, seq, _| {
                competing
                    .acknowledge_range(SESSION, f.seq, f.seq, "consumer")
                    .unwrap();
                Ok(page(&f.anchor, index, seq, true, 1))
            }
        )
        .unwrap()
    );
    let rows = f.db.list_mailbox(SESSION, true).unwrap();
    assert_eq!(
        rows.iter()
            .find(|r| r.seq == f.seq)
            .unwrap()
            .delivered_by_invocation_uuid
            .as_deref(),
        Some("consumer")
    );
    assert_eq!(
        rows.iter()
            .find(|r| r.seq == second)
            .unwrap()
            .delivered_by_invocation_uuid
            .as_deref(),
        Some("native-invocation")
    );
    assert!(f.db.list_pending(SESSION).unwrap().is_empty());
    assert!(
        f.db.delivery_observation_confirmation(&f.attempt)
            .unwrap()
            .is_some()
    );
}

#[test]
fn age355_fair_cursor_survives_restart_and_excludes_prepared_stopped_and_pty() {
    let mut f = Fixture::new();
    f.db.wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: SESSION,
            mode: "headless",
            invocation_uuid: Some("native-invocation"),
            provider_name: Some("account"),
            model_name: None,
            models_dir: None,
            effective_cwd: Some("/offline"),
        })
        .unwrap();
    // Prepared-but-not-submitted attempts never consume a receipt slot.
    f.db.record_delivery_observation_anchor(&f.attempt, SESSION, &f.anchor)
        .unwrap();
    assert_eq!(f.db.next_headless_receipt_attempt().unwrap(), None);
    f.submit().unwrap();
    let mut expected = vec![f.attempt.clone()];
    for id in ["a-fair", "z-fair"] {
        let seq = enqueue_additional(&mut f, id);
        f.db.register_headless_delivery_attempt(id, SESSION, None, id, &[seq], 0)
            .unwrap();
        f.db.record_delivery_observation_anchor(id, SESSION, &f.anchor)
            .unwrap();
        f.db.begin_headless_delivery_submission(id, SESSION, id, true)
            .unwrap();
        expected.push(id.to_string());
    }
    expected.sort();
    for id in expected.iter().cycle().take(6) {
        assert_eq!(
            f.db.next_headless_receipt_attempt().unwrap().as_ref(),
            Some(id)
        );
        // No match or unavailable provider does not starve later candidates.
        f.restart();
    }
    f.db.stop_mailbox_observation(SESSION, &f.attempt, "session_turn_paging_paused", "fixed")
        .unwrap();
    assert_eq!(f.db.next_headless_receipt_attempt().unwrap(), None);
    let stop = f.db.mailbox_observation_stop(SESSION).unwrap().unwrap();
    f.db.rearm_mailbox_observation(SESSION, &stop.stop_id, "fixed cause resolved")
        .unwrap();
    f.db.wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: SESSION,
            mode: "pty_interactive",
            invocation_uuid: Some("native-invocation"),
            provider_name: Some("account"),
            model_name: None,
            models_dir: None,
            effective_cwd: Some("/offline"),
        })
        .unwrap();
    assert_eq!(f.db.next_headless_receipt_attempt().unwrap(), None);
}

#[test]
fn age355_changed_reader_reobserves_original_anchor_instead_of_carrying_matches() {
    let mut f = Fixture::new();
    f.anchored_submit();
    assert!(
        !observe_delivery_for_revision_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            "older-adapter",
            |_, i, s, _| Ok(page(&f.anchor, i, s, false, 1))
        )
        .unwrap()
    );
    f.restart();
    assert!(
        !observe_delivery_for_revision_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::from_secs(2),
            "new-adapter",
            |cursor, i, s, _| {
                assert_eq!(
                    cursor,
                    SessionProviderPageCursor::Beginning {
                        after_token: f.anchor.resume_token.clone()
                    }
                );
                assert_eq!((i, s), (0, 0));
                Ok(page(&f.anchor, i, s, true, 0))
            }
        )
        .unwrap()
    );
    f.assert_pending_without_replay();
}

#[test]
fn age355_expired_tick_budget_never_starts_io() {
    let mut f = Fixture::new();
    f.anchored_submit();
    assert!(
        !observe_delivery_bounded_with(
            &f.db,
            &f.attempt,
            &f.anchor,
            1,
            Duration::ZERO,
            |_, _, _, _| panic!("no IO after exhausted budget")
        )
        .unwrap()
    );
    f.assert_pending_without_replay();
}

#[test]
fn age355_correction_global_selection_does_not_overlap() {
    let mut f = Fixture::new();
    f.anchored_submit();
    f.db.wake_sessions()
        .upsert_session_metadata(oulipoly_state::mailbox::SessionMetadataUpsert {
            session_id: SESSION,
            mode: "headless",
            invocation_uuid: Some("native-invocation"),
            provider_name: Some("account"),
            model_name: Some("offline"),
            models_dir: None,
            effective_cwd: Some("/offline"),
        })
        .unwrap();
    let root = f.root.path().to_path_buf();
    let (entered, observed) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let first_entered = entered.clone();
    let first = std::thread::spawn(move || {
        let mut db = MailboxDb::open(&root.join("pid-identity.db")).unwrap();
        crate::native_receipt::poll_headless_receipt_tick_with(&mut db, |_| {
            first_entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            Err::<oulipoly_runtime::provider_registry::ProviderRegistry, _>(
                "private unavailable registry".into(),
            )
        })
    });
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    // The first selection is still in flight. A second physical DB connection
    // must not perform duplicate global inspection of this sole candidate.
    let result = crate::native_receipt::poll_headless_receipt_tick_with(&mut f.db, |_| {
        entered.send(()).unwrap();
        Err::<oulipoly_runtime::provider_registry::ProviderRegistry, _>(
            "private unavailable registry".into(),
        )
    });
    let duplicated = observed.try_recv().is_ok();
    release.send(()).unwrap();
    assert!(first.join().unwrap().is_err());
    assert!(result.is_ok() || result.as_ref().unwrap_err() == "private unavailable registry");
    f.assert_pending_without_replay();
    assert!(
        !duplicated,
        "both observers entered registry preparation for the sole attempt"
    );
}

#[test]
fn native_receipt_survives_projection_fault_and_recovers_without_replay() {
    let mut f = Fixture::new();
    f.anchored_submit();
    let connection = rusqlite::Connection::open(f.db.path()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_receipt_projection BEFORE UPDATE OF delivered_at ON mailbox
         WHEN NEW.delivered_at IS NOT NULL
         BEGIN SELECT RAISE(ABORT, 'projection fault'); END;",
        )
        .unwrap();
    let error = observe_delivery_with(&f.db, &f.attempt, &f.anchor, |_, index, sequence, _| {
        Ok(page(&f.anchor, index, sequence, true, 1))
    })
    .unwrap_err();
    assert!(error.contains("projection fault"), "{error}");
    f.restart();
    assert_eq!(
        f.db.delivery_observation_confirmation(&f.attempt)
            .unwrap()
            .as_deref(),
        Some("native-user-0")
    );
    assert_eq!(f.db.list_pending(SESSION).unwrap().len(), 1);
    let diagnostic: String = connection
        .query_row(
            "SELECT observation_error FROM mailbox_delivery_attempts WHERE attempt_id = ?1",
            [&f.attempt],
            |row| row.get(0),
        )
        .unwrap();
    assert!(diagnostic.contains("projection fault"));
    assert!(f.submit().is_err(), "admitted work must not be replayed");
    assert!(prepare_headless_resume_delivery_on(&mut f.db, SESSION, "chain", None, None).is_err());
    connection
        .execute_batch("DROP TRIGGER reject_receipt_projection")
        .unwrap();
    assert_eq!(
        deliverable_pending_count_on(&mut f.db, &f.state, SESSION).unwrap(),
        0
    );
    assert_eq!(f.submissions, 1);
    let row = f.db.list_mailbox(SESSION, true).unwrap().remove(0);
    assert_eq!(
        row.delivered_by_invocation_uuid.as_deref(),
        Some("native-invocation")
    );
    assert!(row.delivered_at.is_some());
}
