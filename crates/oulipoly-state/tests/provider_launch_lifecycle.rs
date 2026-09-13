use oulipoly_state::*;
use rusqlite::{Connection, params};
use uuid::Uuid;

fn fixture() -> (tempfile::TempDir, StateDb, BeginProviderLaunchRequest) {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let request = BeginProviderLaunchRequest {
        logical_launch_id: Uuid::new_v4(),
        request_identity_sha256: "a".repeat(64),
        model_name: "model".into(),
        start_mode: ProviderLaunchStartMode::Create,
        expected_provider_session_id: None,
        candidates: vec![
            ProviderLaunchCandidate {
                provider_index: 1,
                account_name: "first".into(),
            },
            ProviderLaunchCandidate {
                provider_index: 0,
                account_name: "second".into(),
            },
        ],
        parent_invocation_id: None,
        allocation: ProviderLaunchAttemptAllocation::allocate().unwrap(),
    };
    (dir, db, request)
}
fn proof(lease: &ProviderLaunchLease) -> ProviderLaunchCustodyProof {
    ProviderLaunchCustodyProof {
        attempt_id: lease.owner.attempt_id,
        runtime_generation_uuid: lease.runtime_generation_uuid,
        spawn_invocation_uuid: lease.owner.invocation_uuid,
        actors: ["describe", "policy", "launch"]
            .into_iter()
            .map(|operation| ProviderLaunchActorSettlement::NeverSpawned {
                operation: operation.into(),
            })
            .collect(),
        runtime_terminal_code: "startup_failed".into(),
        runtime_never_bound: true,
        runtime_process_identity_sha256: None,
        runtime_exited: true,
        active_delivery_claim: false,
        runtime_settlement_sha256: "b".repeat(64),
        return_channel_id: lease.return_channel_id.clone(),
        channel: ProviderLaunchChannelSettlement::NotCreated,
        return_channel_settlement_sha256: "c".repeat(64),
    }
}
fn failure() -> ProviderLaunchFailureRecord {
    ProviderLaunchFailureRecord {
        kind: RotatableLaunchFailureKind::ProviderUnavailable,
        request_id: Uuid::new_v4(),
        code: "provider_unavailable".into(),
        exit_code: 73,
    }
}
fn transferable(
    db: &StateDb,
    request: &BeginProviderLaunchRequest,
) -> (ProviderLaunchLease, ProviderLaunchCustodyProof) {
    let lease = db.begin_launch(request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    db.request_transfer(&lease.owner, &failure()).unwrap();
    let proof = proof(&lease);
    db.certify_effect_incapable(&lease.owner, &proof).unwrap();
    (lease, proof)
}
fn count(db: &StateDb, table: &str) -> i64 {
    Connection::open(db.path())
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
#[test]
fn fresh_migration_registration_and_partial_current_schema_fail_closed() {
    let (_dir, db, _) = fixture();
    let conn = Connection::open(db.path()).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        i64::from(CURRENT_SCHEMA_VERSION)
    );
    assert!(
        migrations::plan(22, 23)
            .unwrap()
            .iter()
            .any(|m| m.target_version == 23)
    );
    conn.execute_batch("DROP TRIGGER provider_launch_attempt_immutable")
        .unwrap();
    assert!(StateDb::open(db.path()).is_err());
    assert!(StateDb::open_read_only(db.path()).is_err());
}
#[test]
fn schema_22_migrates_provider_ownership_once_to_current() {
    let (dir, db, _) = fixture();
    let path = db.path().to_path_buf();
    drop(db);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF; ALTER TABLE invocation_completion_obligations DROP COLUMN completion_v2_binding; DROP TABLE provider_launch_transition_replays; DROP TABLE provider_logical_launches; DROP TABLE provider_launch_attempts; PRAGMA user_version=22;").unwrap();
    drop(conn);
    let db = StateDb::open(&path).unwrap();
    assert_eq!(count(&db, "provider_logical_launches"), 0);
    drop(db);
    assert!(StateDb::open(&dir.path().join("state.db")).is_ok());
}
#[test]
fn retained_begin_input_replays_without_storing_secret_changed_or_lost_input_conflicts() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    assert_eq!(db.begin_launch(&request).unwrap(), lease);
    let mut changed = request.clone();
    changed.allocation.completion_authority = CompletionRegistrationAuthority::generate().unwrap();
    assert!(db.begin_launch(&changed).is_err());
    changed = request.clone();
    changed.model_name = "different".into();
    assert!(db.begin_launch(&changed).is_err());
    assert_eq!(count(&db, "invocations"), 1);
    let conn = Connection::open(db.path()).unwrap();
    let stored: String = conn
        .query_row(
            "SELECT completion_registration_capability_digest FROM invocations",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(
        stored,
        request
            .allocation
            .completion_authority
            .process_environment_value()
    );
    let json: String = conn
        .query_row(
            "SELECT result_json FROM provider_launch_transition_replays",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        !json.contains(
            request
                .allocation
                .completion_authority
                .process_environment_value()
        )
    );
}
#[test]
fn begin_rolls_back_invocation_and_launch_when_attempt_insert_fails() {
    let (_dir, db, request) = fixture();
    Connection::open(db.path()).unwrap().execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON provider_launch_attempts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(db.begin_launch(&request).is_err());
    assert_eq!(count(&db, "invocations"), 0);
    assert_eq!(count(&db, "provider_logical_launches"), 0);
}
#[test]
fn successor_is_atomic_distinct_idempotent_and_retains_honest_failure() {
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    let allocation = ProviderLaunchAttemptAllocation::allocate().unwrap();
    let next = db
        .lease_successor(
            &old.owner,
            &request.allocation.completion_authority,
            &old.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &allocation,
        )
        .unwrap();
    assert_eq!(
        db.lease_successor(
            &old.owner,
            &request.allocation.completion_authority,
            &old.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &allocation
        )
        .unwrap(),
        next
    );
    assert_eq!(next.owner.owner_epoch, 2);
    assert_ne!(next.owner.invocation_row_id, old.owner.invocation_row_id);
    assert_ne!(next.runtime_generation_uuid, old.runtime_generation_uuid);
    assert_ne!(next.return_channel_id, old.return_channel_id);
    let row = db
        .get_invocation_by_uuid(&old.owner.invocation_uuid.to_string())
        .unwrap()
        .unwrap();
    assert_eq!(row.exit_code, Some(73));
    assert_eq!(row.error_category.as_deref(), Some("provider_unavailable"));
    assert!(
        db.finalize_invocation(
            InvocationMutationAuthority::ProviderLaunch(&old.owner),
            old.owner.invocation_row_id,
            true,
            0,
            None,
            None
        )
        .is_err()
    );
    assert!(
        db.record_promotion(
            &old.owner,
            Uuid::new_v4(),
            ProviderLaunchPromotion::PromptAccepted
        )
        .is_err()
    );
    assert!(
        db.lease_successor(
            &old.owner,
            &request.allocation.completion_authority,
            &old.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &ProviderLaunchAttemptAllocation::allocate().unwrap()
        )
        .is_err()
    );
    assert_eq!(count(&db, "provider_launch_attempts"), 2);
    db.activate_attempt(&next, &allocation.completion_authority)
        .unwrap();
}
#[test]
fn successor_fault_rolls_back_predecessor_finalization_and_epoch() {
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    Connection::open(db.path()).unwrap().execute_batch("CREATE TRIGGER injected_failure BEFORE INSERT ON provider_launch_attempts WHEN NEW.attempt_ordinal=1 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        db.lease_successor(
            &old.owner,
            &request.allocation.completion_authority,
            &old.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &ProviderLaunchAttemptAllocation::allocate().unwrap()
        )
        .is_err()
    );
    assert_eq!(count(&db, "invocations"), 1);
    assert_eq!(
        db.get_invocation_by_uuid(&old.owner.invocation_uuid.to_string())
            .unwrap()
            .unwrap()
            .status,
        InvocationStatus::Running
    );
    let epoch: i64 = Connection::open(db.path())
        .unwrap()
        .query_row(
            "SELECT owner_epoch FROM provider_logical_launches",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(epoch, 1);
}
#[test]
fn cancellation_before_lease_and_between_lease_and_activation_blocks_work() {
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    db.request_cancel(request.logical_launch_id).unwrap();
    assert!(
        db.lease_successor(
            &old.owner,
            &request.allocation.completion_authority,
            &old.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &ProviderLaunchAttemptAllocation::allocate().unwrap()
        )
        .is_err()
    );
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    let allocation = ProviderLaunchAttemptAllocation::allocate().unwrap();
    let next = db
        .lease_successor(
            &old.owner,
            &request.allocation.completion_authority,
            &old.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &allocation,
        )
        .unwrap();
    db.request_cancel(request.logical_launch_id).unwrap();
    assert!(
        db.activate_attempt(&next, &allocation.completion_authority)
            .is_err()
    );
    db.settle_cancel(&next.owner, &self::proof(&next)).unwrap();
}
#[test]
fn each_late_promotion_invalidates_certification_permanently() {
    for promotion in [
        ProviderLaunchPromotion::ProviderSessionObserved,
        ProviderLaunchPromotion::PromptAccepted,
        ProviderLaunchPromotion::AssistantResponseObserved,
        ProviderLaunchPromotion::CapturedChild,
        ProviderLaunchPromotion::ReturnedArtifact,
        ProviderLaunchPromotion::MailboxSubmissionAccepted,
    ] {
        let (_dir, db, request) = fixture();
        let (old, proof) = transferable(&db, &request);
        let id = Uuid::new_v4();
        db.record_promotion(&old.owner, id, promotion).unwrap();
        db.record_promotion(&old.owner, id, promotion).unwrap();
        assert!(
            db.lease_successor(
                &old.owner,
                &request.allocation.completion_authority,
                &old.candidate_plan_sha256,
                &request.candidates[1],
                &proof,
                &ProviderLaunchAttemptAllocation::allocate().unwrap()
            )
            .is_err()
        );
        assert!(db.request_transfer(&old.owner, &failure()).is_err());
        let conn = Connection::open(db.path()).unwrap();
        assert!(conn.execute("UPDATE provider_launch_attempts SET provider_session_observed=0,prompt_accepted=0,assistant_response_observed=0,captured_child_count=0,returned_artifact_count=0,mailbox_submission_accepted=0",[]).is_err());
    }
}
#[test]
fn incomplete_or_changed_custody_is_rejected() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    db.request_transfer(&lease.owner, &failure()).unwrap();
    let original = proof(&lease);
    let mut bad = original.clone();
    bad.actors.pop();
    assert!(db.certify_effect_incapable(&lease.owner, &bad).is_err());
    bad = original.clone();
    bad.active_delivery_claim = true;
    assert!(db.certify_effect_incapable(&lease.owner, &bad).is_err());
    bad = original.clone();
    bad.runtime_generation_uuid = Uuid::new_v4();
    assert!(db.certify_effect_incapable(&lease.owner, &bad).is_err());
    bad = original.clone();
    bad.runtime_exited = false;
    assert!(db.certify_effect_incapable(&lease.owner, &bad).is_err());
    db.certify_effect_incapable(&lease.owner, &original)
        .unwrap();
    bad = original.clone();
    bad.return_channel_settlement_sha256 = "d".repeat(64);
    assert!(db.certify_effect_incapable(&lease.owner, &bad).is_err());
}
#[test]
fn dual_transfer_callers_create_exactly_one_successor() {
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    let allocation = ProviderLaunchAttemptAllocation::allocate().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = db.path().to_path_buf();
            let old = old.clone();
            let proof = proof.clone();
            let allocation = allocation.clone();
            let next = request.candidates[1].clone();
            let predecessor_authority = request.allocation.completion_authority.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let db = StateDb::open(&path).unwrap();
                barrier.wait();
                db.lease_successor(
                    &old.owner,
                    &predecessor_authority,
                    &old.candidate_plan_sha256,
                    &next,
                    &proof,
                    &allocation,
                )
                .unwrap()
            })
        })
        .collect();
    let mut outputs = handles.into_iter().map(|h| h.join().unwrap());
    assert_eq!(outputs.next().unwrap(), outputs.next().unwrap());
    assert_eq!(count(&db, "provider_launch_attempts"), 2);
}
#[test]
fn lost_input_after_restart_can_only_reconcile_same_owner_without_new_attempt() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    let path = db.path().to_path_buf();
    drop(db);
    let db = StateDb::open(&path).unwrap();
    db.reconcile_incomplete(
        &lease.owner,
        ProviderLaunchRecoveryDisposition::RecoveryBlocked,
        None,
        &recovery_join(&db, "d"),
    )
    .unwrap();
    assert!(
        db.activate_attempt(&lease, &request.allocation.completion_authority)
            .is_err()
    );
    db.reconcile_incomplete(
        &lease.owner,
        ProviderLaunchRecoveryDisposition::Failed,
        Some(&proof(&lease)),
        &recovery_join(&db, "e"),
    )
    .unwrap();
    assert_eq!(count(&db, "provider_launch_attempts"), 1);
}
#[test]
fn resume_plan_and_duplicate_candidates_are_rejected() {
    let (_dir, db, mut request) = fixture();
    request.start_mode = ProviderLaunchStartMode::Resume;
    assert!(db.begin_launch(&request).is_err());
    request.start_mode = ProviderLaunchStartMode::Create;
    request.candidates[1] = request.candidates[0].clone();
    assert!(db.begin_launch(&request).is_err());
    assert_eq!(count(&db, "invocations"), 0);
}
#[test]
fn exact_fence_matrix_covers_linked_writers_and_unlinked_standalone_control() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let mut invalid = vec![];
    let mut f = lease.owner.clone();
    f.logical_launch_id = Uuid::new_v4();
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.attempt_id = Uuid::new_v4();
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.owner_epoch += 1;
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.invocation_row_id += 1;
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.invocation_uuid = Uuid::new_v4();
    invalid.push(f);
    for authority in std::iter::once(InvocationMutationAuthority::Standalone).chain(
        invalid
            .iter()
            .map(InvocationMutationAuthority::ProviderLaunch),
    ) {
        denied_writers(&db, &lease, authority);
    }
    let auth = InvocationMutationAuthority::ProviderLaunch(&lease.owner);
    db.update_resume_acceptance(
        auth,
        lease.owner.invocation_row_id,
        "accepted",
        Some("verified"),
    )
    .unwrap();
    let standalone = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "model".into(),
            provider_name: "first".into(),
            provider_index: 1,
            parent_invocation_id: None,
        })
        .unwrap();
    assert!(
        db.update_resume_acceptance(auth, standalone, "accepted", None)
            .is_err()
    );
    db.update_resume_acceptance(
        InvocationMutationAuthority::Standalone,
        standalone,
        "accepted",
        None,
    )
    .unwrap();
}
fn denied_writers(
    db: &StateDb,
    lease: &ProviderLaunchLease,
    auth: InvocationMutationAuthority<'_>,
) {
    let id = lease.owner.invocation_row_id;
    let uuid = lease.owner.invocation_uuid.to_string();
    let binding = ProviderSessionBinding {
        provider_session_id: "session".into(),
        capture_method: "provider_session_capture",
        resume_input_id: None,
        provider_session_resolved_account: Some("first".into()),
    };
    let outputs = InvocationOutputArtifactPaths {
        stdout: "/tmp/unused-stdout".into(),
        stderr: "/tmp/unused-stderr".into(),
    };
    let results = [
        db.finalize_invocation(auth, id, false, 1, None, None),
        db.finalize_invocation_typed(auth, id, false, 1, None, None)
            .map_err(|e| format!("{e:?}")),
        db.update_session_capture(auth, id, Some("session"), "provider_session_capture"),
        db.update_resume_acceptance(auth, id, "accepted", None),
        db.record_legacy_resume_input_session_id(auth, id, "session"),
        db.bind_invocation_provider_session_start(auth, id, &binding),
        db.commit_invocation_provider_session_authority(
            auth,
            id,
            &ProviderSessionAuthorityCommit {
                invocation_uuid: &uuid,
                provider_name: "first",
                provider_instance_id: "instance",
                settings_id: "settings",
                binding: &binding,
            },
        ),
        db.transition_invocation_provider_session_capture_method(auth, id, "session", "old", "new"),
        db.mint_chain_for_invocation_session(auth, id),
        db.record_returned_artifacts(auth, id, &[]),
        db.record_invocation_output_pending(
            auth,
            id,
            &uuid,
            &outputs,
            0,
            &"a".repeat(64),
            0,
            &"b".repeat(64),
            0,
        ),
        db.mark_invocation_output_delivered(auth, id),
        db.mark_invocation_output_delivery_failed(auth, id, "stdout", "io", None),
        db.commit_finalized_provider_session_authority(
            auth,
            id,
            &FinalizedProviderSessionAuthority {
                provider_session_id: "session",
                capture_method: "provider_session_capture",
                provider_instance_id: "instance",
                settings_id: "settings",
            },
        ),
        db.apply_provider_turn_effects(
            auth,
            ProviderTurnEffectInput {
                invocation_row_id: id,
                delivery_ids: &[],
                accept_delivery_if_missing: false,
                session_id: "session",
                turn_generation_id: "generation",
                submitted_evidence: None,
                confirmed_evidence: None,
                observed_at: 0,
                returned_artifacts: &[],
                resume_acceptance_status: None,
                resume_acceptance_evidence: None,
                success: false,
                exit_code: 1,
                error_category: None,
                terminal_reason: None,
            },
        )
        .map(|_| ()),
    ];
    for (i, result) in results.into_iter().enumerate() {
        assert!(result.unwrap_err().contains("owner_fence"), "writer {i}");
    }
}
#[test]
fn sql_identity_constraints_reject_forged_join_and_mutations() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    let conn = Connection::open(db.path()).unwrap();
    for assignment in [
        "attempt_ordinal=1",
        "owner_epoch=2",
        "invocation_id=1000",
        "invocation_uuid='other'",
        "provider_index=99",
        "account_name='other'",
        "runtime_generation_uuid='other'",
        "return_channel_id='other'",
        "endpoint_family='partial'",
    ] {
        assert!(
            conn.execute(
                &format!("UPDATE provider_launch_attempts SET {assignment}"),
                []
            )
            .is_err(),
            "{assignment}"
        );
    }
    assert!(
        conn.execute(
            "UPDATE provider_logical_launches SET candidate_plan_sha256=?1",
            params!["changed"]
        )
        .is_err()
    );
    assert_eq!(lease.owner.owner_epoch, 1);
}

#[test]
fn activation_replay_after_cancellation_is_not_executable_and_endpoint_rebind_conflicts() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let endpoint = ProviderLaunchEndpoint {
        endpoint_family: "family".into(),
        settings_id: "settings".into(),
        provider_instance_id: "instance".into(),
        endpoint_identity_sha256: "d".repeat(64),
    };
    db.bind_launch_endpoint(&lease.owner, &endpoint).unwrap();
    db.bind_launch_endpoint(&lease.owner, &endpoint).unwrap();
    let mut changed = endpoint.clone();
    changed.settings_id = "other".into();
    assert!(db.bind_launch_endpoint(&lease.owner, &changed).is_err());
    db.request_cancel(request.logical_launch_id).unwrap();
    assert!(
        db.activate_attempt(&lease, &request.allocation.completion_authority)
            .is_err()
    );
    assert!(db.bind_launch_endpoint(&lease.owner, &endpoint).is_err());
}
#[test]
fn completion_requires_exact_finalized_outcome_and_rejects_changed_replay() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let result = ProviderLaunchTerminalResult {
        success: true,
        exit_code: 0,
        code: "completed".into(),
    };
    assert!(db.complete_launch(&lease.owner, &result).is_err());
    db.finalize_invocation(
        InvocationMutationAuthority::ProviderLaunch(&lease.owner),
        lease.owner.invocation_row_id,
        true,
        0,
        None,
        Some("completed"),
    )
    .unwrap();
    assert!(db.request_transfer(&lease.owner, &failure()).is_err());
    db.complete_launch(&lease.owner, &result).unwrap();
    db.complete_launch(&lease.owner, &result).unwrap();
    let changed = ProviderLaunchTerminalResult {
        success: false,
        exit_code: 7,
        code: "different".into(),
    };
    assert!(db.complete_launch(&lease.owner, &changed).is_err());
}
#[test]
fn accepted_effect_writes_promote_without_relying_on_callback_order() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    db.update_session_capture(
        InvocationMutationAuthority::ProviderLaunch(&lease.owner),
        lease.owner.invocation_row_id,
        Some("session"),
        "provider_session_capture",
    )
    .unwrap();
    assert!(db.request_transfer(&lease.owner, &failure()).is_err());
    assert_eq!(
        Connection::open(db.path())
            .unwrap()
            .query_row(
                "SELECT provider_session_observed FROM provider_launch_attempts",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
#[test]
fn concurrent_cancellation_and_transfer_always_bar_successor_activation() {
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let path = db.path().to_path_buf();
    let b = barrier.clone();
    let launch = request.logical_launch_id;
    let cancel = std::thread::spawn(move || {
        let db = StateDb::open(&path).unwrap();
        b.wait();
        db.request_cancel(launch).unwrap();
    });
    barrier.wait();
    let allocation = ProviderLaunchAttemptAllocation::allocate().unwrap();
    let next = db.lease_successor(
        &old.owner,
        &request.allocation.completion_authority,
        &old.candidate_plan_sha256,
        &request.candidates[1],
        &proof,
        &allocation,
    );
    cancel.join().unwrap();
    if let Ok(next) = next {
        assert!(
            db.activate_attempt(&next, &allocation.completion_authority)
                .is_err()
        );
    }
    assert!(count(&db, "provider_launch_attempts") <= 2);
}
#[test]
fn concurrent_promotion_and_transfer_have_one_linearized_winner() {
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let path = db.path().to_path_buf();
    let b = barrier.clone();
    let owner = old.owner.clone();
    let promotion = std::thread::spawn(move || {
        let db = StateDb::open(&path).unwrap();
        b.wait();
        db.record_promotion(
            &owner,
            Uuid::new_v4(),
            ProviderLaunchPromotion::PromptAccepted,
        )
    });
    barrier.wait();
    let next = db.lease_successor(
        &old.owner,
        &request.allocation.completion_authority,
        &old.candidate_plan_sha256,
        &request.candidates[1],
        &proof,
        &ProviderLaunchAttemptAllocation::allocate().unwrap(),
    );
    let promoted = promotion.join().unwrap();
    assert_ne!(next.is_ok(), promoted.is_ok());
    assert_eq!(
        count(&db, "provider_launch_attempts"),
        if next.is_ok() { 2 } else { 1 }
    );
}
#[test]
fn duplicate_global_identities_rollback_all_begin_rows() {
    let (_dir, db, request) = fixture();
    db.begin_launch(&request).unwrap();
    for which in 0..3 {
        let mut changed = request.clone();
        changed.logical_launch_id = Uuid::new_v4();
        changed.allocation = ProviderLaunchAttemptAllocation::allocate().unwrap();
        match which {
            0 => changed.allocation.attempt_id = request.allocation.attempt_id,
            1 => changed.allocation.invocation_uuid = request.allocation.invocation_uuid,
            _ => {
                changed.allocation.runtime_generation_uuid =
                    request.allocation.runtime_generation_uuid
            }
        }
        assert!(db.begin_launch(&changed).is_err());
        assert_eq!(count(&db, "invocations"), 1);
        assert_eq!(count(&db, "provider_logical_launches"), 1);
    }
}
#[test]
fn request_identity_digest_is_canonical_and_covers_input_and_plan() {
    let inputs = std::collections::BTreeMap::from([("x".into(), serde_json::json!("one"))]);
    let candidates = [ProviderLaunchCandidate {
        provider_index: 0,
        account_name: "a".into(),
    }];
    let mut request = ProviderLaunchRequestIdentity {
        model_name: "m",
        prompt_sha256: &"a".repeat(64),
        prompt_mode: "file",
        extra_inputs: &inputs,
        effective_cwd: "/project",
        parent_invocation_uuid: None,
        start_mode: ProviderLaunchStartMode::Create,
        expected_provider_session_id: None,
        mailbox_correlation: None,
        candidates: &candidates,
    };
    let original = request.sha256().unwrap();
    assert_eq!(original, request.sha256().unwrap());
    request.mailbox_correlation = Some("delivery");
    assert_ne!(original, request.sha256().unwrap());
}

fn recovery_join(db: &StateDb, byte: &str) -> ProviderLaunchRecoveryJoin {
    ProviderLaunchRecoveryJoin {
        state_db_path: db.path().to_path_buf(),
        sidecar_path: mailbox::MailboxDb::path_for_state_db(db.path()),
        evidence_sha256: byte.repeat(64),
    }
}

#[test]
fn completion_admission_and_repair_require_all_exact_fence_fields() {
    let (_dir, mut db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let mut invalid = Vec::new();
    let mut f = lease.owner.clone();
    f.logical_launch_id = Uuid::new_v4();
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.attempt_id = Uuid::new_v4();
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.owner_epoch += 1;
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.invocation_row_id += 1;
    invalid.push(f);
    let mut f = lease.owner.clone();
    f.invocation_uuid = Uuid::new_v4();
    invalid.push(f);
    let uuid = lease.owner.invocation_uuid.to_string();
    for auth in std::iter::once(InvocationMutationAuthority::Standalone).chain(
        invalid
            .iter()
            .map(InvocationMutationAuthority::ProviderLaunch),
    ) {
        let registration = || mailbox::CompletionEventRegistrationInput {
            event_id: "event",
            delivery_mode: "async",
            owner_session_id: Some("session"),
            owner_invocation_uuid: Some(&uuid),
            state_dir: "/unused",
            meta_path: "/unused/meta",
            log_path: "/unused/log",
            rc_path: "/unused/rc",
        };
        assert!(
            db.register_completion_event_with_authority(
                auth,
                &request.allocation.completion_authority,
                "admission",
                registration()
            )
            .unwrap_err()
            .contains("owner_fence")
        );
        assert!(
            db.repair_admitted_completion_event(auth, "admission", registration())
                .unwrap_err()
                .contains("owner_fence")
        );
    }
    assert_eq!(count(&db, "invocation_completion_obligations"), 0);
}
#[test]
fn stale_finalizer_racing_transfer_cannot_change_successor() {
    let (_dir, db, request) = fixture();
    let (old, proof) = transferable(&db, &request);
    let path = db.path().to_path_buf();
    let owner = old.owner.clone();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let b = barrier.clone();
    let finalizer = std::thread::spawn(move || {
        let db = StateDb::open(&path).unwrap();
        b.wait();
        db.finalize_invocation(
            InvocationMutationAuthority::ProviderLaunch(&owner),
            owner.invocation_row_id,
            true,
            0,
            None,
            None,
        )
    });
    barrier.wait();
    let next = db
        .lease_successor(
            &old.owner,
            &request.allocation.completion_authority,
            &old.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &ProviderLaunchAttemptAllocation::allocate().unwrap(),
        )
        .unwrap();
    assert!(finalizer.join().unwrap().is_err());
    assert_eq!(
        db.get_invocation_by_uuid(&next.owner.invocation_uuid.to_string())
            .unwrap()
            .unwrap()
            .status,
        InvocationStatus::Running
    );
}
#[test]
fn recovery_rejects_a_different_state_or_sidecar_namespace() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    let mut join = recovery_join(&db, "a");
    join.sidecar_path = "/other/sidecar".into();
    assert!(
        db.reconcile_incomplete(
            &lease.owner,
            ProviderLaunchRecoveryDisposition::Failed,
            Some(&proof(&lease)),
            &join
        )
        .is_err()
    );
    assert_eq!(count(&db, "provider_launch_attempts"), 1);
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
}

#[test]
fn public_persisted_lease_identity_cannot_replace_lost_live_secret() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    let lost = CompletionRegistrationAuthority::generate().unwrap();
    assert!(db.activate_attempt(&lease, &lost).is_err());
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    assert!(db.activate_attempt(&lease, &lost).is_err());
    db.request_transfer(&lease.owner, &failure()).unwrap();
    let proof = proof(&lease);
    db.certify_effect_incapable(&lease.owner, &proof).unwrap();
    assert!(
        db.lease_successor(
            &lease.owner,
            &lost,
            &lease.candidate_plan_sha256,
            &request.candidates[1],
            &proof,
            &ProviderLaunchAttemptAllocation::allocate().unwrap()
        )
        .is_err()
    );
    assert_eq!(count(&db, "provider_launch_attempts"), 1);
}

// These are supplied receipt DTOs, not proof of real process or channel cleanup.
fn spawned_proof(lease: &ProviderLaunchLease) -> ProviderLaunchCustodyProof {
    let mut settled = proof(lease);
    settled.actors = ["describe", "policy", "launch"]
        .into_iter()
        .enumerate()
        .map(|(index, operation)| ProviderLaunchActorSettlement::Reaped {
            operation: operation.into(),
            process_identity_sha256: format!("{:064x}", index + 1),
            process_tree_terminated: true,
            leader_reaped: true,
        })
        .collect();
    settled.runtime_never_bound = false;
    settled.runtime_process_identity_sha256 = Some(format!("{:064x}", 3));
    settled.runtime_terminal_code = "provider_unavailable".into();
    settled.channel = ProviderLaunchChannelSettlement::EmptyRemoved;
    settled
}

#[test]
fn reaped_actors_with_matching_runtime_and_empty_removed_channel_allow_successor() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    db.request_transfer(&lease.owner, &failure()).unwrap();
    let settled = spawned_proof(&lease);
    db.certify_effect_incapable(&lease.owner, &settled).unwrap();
    db.certify_effect_incapable(&lease.owner, &settled).unwrap();
    let row: (String, String, bool) = Connection::open(db.path())
        .unwrap()
        .query_row(
            "SELECT actor_custody_state,return_channel_state,effect_incapable_at IS NOT NULL FROM provider_launch_attempts WHERE attempt_id=?1",
            [lease.owner.attempt_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        ("effect_incapable".into(), "empty_removed".into(), true)
    );
    let next = db
        .lease_successor(
            &lease.owner,
            &request.allocation.completion_authority,
            &lease.candidate_plan_sha256,
            &request.candidates[1],
            &settled,
            &ProviderLaunchAttemptAllocation::allocate().unwrap(),
        )
        .unwrap();
    assert_eq!(next.owner.owner_epoch, 2);
    assert_ne!(next.owner.invocation_row_id, lease.owner.invocation_row_id);
}

#[test]
fn spawned_receipt_controls_reject_unsettled_actors_and_runtime_process_mismatch() {
    // Change exactly one receipt fact per case, holding all other joins valid.
    for case in 0..9 {
        let (_dir, db, request) = fixture();
        let lease = db.begin_launch(&request).unwrap();
        db.activate_attempt(&lease, &request.allocation.completion_authority)
            .unwrap();
        db.request_transfer(&lease.owner, &failure()).unwrap();
        let valid = spawned_proof(&lease);
        let mut invalid = valid.clone();
        match case {
            0..=5 => {
                let ProviderLaunchActorSettlement::Reaped {
                    leader_reaped,
                    process_tree_terminated,
                    ..
                } = &mut invalid.actors[case / 2]
                else {
                    panic!("spawned fixture must contain reaped actor receipts");
                };
                if case % 2 == 0 {
                    *leader_reaped = false;
                } else {
                    *process_tree_terminated = false;
                }
            }
            6 => invalid.runtime_process_identity_sha256 = Some("f".repeat(64)),
            7 => invalid.runtime_process_identity_sha256 = None,
            8 => invalid.runtime_never_bound = true,
            _ => unreachable!(),
        }
        assert!(
            db.certify_effect_incapable(&lease.owner, &invalid).is_err(),
            "case {case}"
        );
        let row: (String, Option<String>) = Connection::open(db.path())
            .unwrap()
            .query_row(
                "SELECT status,effect_incapable_at FROM provider_launch_attempts WHERE attempt_id=?1",
                [lease.owner.attempt_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("transfer_requested".into(), None), "case {case}");
        assert!(
            db.lease_successor(
                &lease.owner,
                &request.allocation.completion_authority,
                &lease.candidate_plan_sha256,
                &request.candidates[1],
                &invalid,
                &ProviderLaunchAttemptAllocation::allocate().unwrap(),
            )
            .is_err(),
            "case {case}"
        );
        assert_eq!(count(&db, "invocations"), 1, "case {case}");
        // Failed certification must not poison the replay key or strand the owner.
        db.certify_effect_incapable(&lease.owner, &valid).unwrap();
    }
}

#[test]
fn linked_session_commit_preserves_endpoint_authority_and_rolls_back_rejected_promotion() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let id = lease.owner.invocation_row_id;
    let uuid = lease.owner.invocation_uuid.to_string();
    let binding = ProviderSessionBinding {
        provider_session_id: "authenticated-native-session".into(),
        capture_method: "provider_live_report",
        resume_input_id: None,
        provider_session_resolved_account: Some("/fixture/workspace".into()),
    };
    let commit = ProviderSessionAuthorityCommit {
        invocation_uuid: &uuid,
        provider_name: "first",
        provider_instance_id: "authenticated-instance",
        settings_id: "selected-settings",
        binding: &binding,
    };
    let conn = Connection::open(db.path()).unwrap();
    conn.execute(
        "INSERT INTO invocation_provider_session_authority VALUES (?1, 'other-instance', 'other-settings')",
        [id],
    )
    .unwrap();
    let linked = InvocationMutationAuthority::ProviderLaunch(&lease.owner);
    assert!(
        db.commit_invocation_provider_session_authority(linked, id, &commit)
            .is_err()
    );
    let row = db.get_invocation_by_uuid(&uuid).unwrap().unwrap();
    assert_eq!(row.provider_session_id, None);
    assert_eq!(row.provider_session_resolved_account, None);
    assert_eq!(
        db.chain_id_for_segment("first", &binding.provider_session_id)
            .unwrap(),
        None
    );
    let promoted: i64 = conn
        .query_row(
            "SELECT provider_session_observed FROM provider_launch_attempts WHERE invocation_id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        promoted, 0,
        "endpoint rejection must roll back owner promotion"
    );
    conn.execute(
        "DELETE FROM invocation_provider_session_authority WHERE invocation_id=?1",
        [id],
    )
    .unwrap();
    assert!(
        db.commit_invocation_provider_session_authority(
            InvocationMutationAuthority::Standalone,
            id,
            &commit
        )
        .is_err()
    );
    db.commit_invocation_provider_session_authority(linked, id, &commit)
        .unwrap();
    db.commit_invocation_provider_session_authority(linked, id, &commit)
        .unwrap();
    let row = db.get_invocation_by_uuid(&uuid).unwrap().unwrap();
    assert_eq!(
        row.provider_session_id.as_deref(),
        Some("authenticated-native-session")
    );
    assert_eq!(
        row.provider_session_resolved_account.as_deref(),
        Some("/fixture/workspace")
    );
    let chain = db
        .chain_id_for_segment("first", &binding.provider_session_id)
        .unwrap()
        .unwrap();
    let authority = db
        .active_provider_session_authority(&chain)
        .unwrap()
        .unwrap();
    assert_eq!(authority.provider_instance_id, "authenticated-instance");
    assert_eq!(authority.settings_id, "selected-settings");
    let promoted: i64 = conn
        .query_row(
            "SELECT provider_session_observed FROM provider_launch_attempts WHERE invocation_id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(promoted, 1);
    assert!(db.request_transfer(&lease.owner, &failure()).is_err());
}

#[test]
fn retained_scope_never_reconstructs_or_overrides_a_stale_owner() {
    let (_dir, db, request) = fixture();
    let (lease, proof) = transferable(&db, &request);
    db.retain_launch_owner(&lease.owner).unwrap();
    assert!(matches!(
        db.invocation_mutation_scope(lease.owner.invocation_row_id)
            .authority(),
        InvocationMutationAuthority::ProviderLaunch(_)
    ));
    let reopened = StateDb::open(db.path()).unwrap();
    assert!(matches!(
        reopened
            .invocation_mutation_scope(lease.owner.invocation_row_id)
            .authority(),
        InvocationMutationAuthority::Standalone
    ));
    let next = ProviderLaunchAttemptAllocation::allocate().unwrap();
    db.lease_successor(
        &lease.owner,
        &request.allocation.completion_authority,
        &lease.candidate_plan_sha256,
        &request.candidates[1],
        &proof,
        &next,
    )
    .unwrap();
    assert!(
        db.record_promotion(
            &lease.owner,
            Uuid::new_v4(),
            ProviderLaunchPromotion::ReturnedArtifact
        )
        .is_err()
    );
    assert!(db.retain_launch_owner(&lease.owner).is_err());
}

#[test]
fn cancellation_retains_exact_artifacts_but_does_not_grant_transfer() {
    use oulipoly_agent_messenger::{ReturnedArtifactRef, ReturnedArtifactSource, StoreAddress};
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let id = lease.owner.invocation_uuid;
    let artifact = ReturnedArtifactRef {
        version_id: format!("store://return/{id}/result/1"),
        name: "result".into(),
        store_address: StoreAddress {
            workflow_run_id: format!("return:{id}"),
            artifact_name: "result".into(),
            version: 1,
        },
        sha256: "a".repeat(64),
        content_len: 4,
        format_hint: None,
        verdict_line: None,
        source: ReturnedArtifactSource::Scratchpad {
            name: "result".into(),
            version: 1,
        },
        producer_invocation_uuid: id,
        returned_at: chrono::Utc::now(),
    };
    db.record_returned_artifacts(
        InvocationMutationAuthority::ProviderLaunch(&lease.owner),
        lease.owner.invocation_row_id,
        std::slice::from_ref(&artifact),
    )
    .unwrap();
    let mut custody = proof(&lease);
    custody.channel = ProviderLaunchChannelSettlement::ArtifactsCommitted(vec![artifact.clone()]);
    assert!(db.request_transfer(&lease.owner, &failure()).is_err());
    assert!(db.certify_effect_incapable(&lease.owner, &custody).is_err());
    db.request_cancel(lease.owner.logical_launch_id).unwrap();
    let mut wrong = custody.clone();
    if let ProviderLaunchChannelSettlement::ArtifactsCommitted(ref mut refs) = wrong.channel {
        refs[0].sha256 = "b".repeat(64);
    }
    assert!(db.settle_cancel(&lease.owner, &wrong).is_err());
    db.settle_cancel(&lease.owner, &custody).unwrap();
    assert_eq!(
        db.list_returned_artifacts(lease.owner.invocation_row_id)
            .unwrap(),
        vec![artifact]
    );
    let conn = Connection::open(db.path()).unwrap();
    let row: (String, String) = conn
        .query_row(
            "SELECT status,return_channel_state FROM provider_launch_attempts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("cancelled".into(), "artifacts_committed".into()));
    let retained: String = conn.query_row("SELECT result_json FROM provider_launch_transition_replays WHERE operation_key LIKE '%/terminal-custody'", [], |r| r.get(0)).unwrap();
    assert_eq!(
        serde_json::from_str::<ProviderLaunchCustodyProof>(&retained).unwrap(),
        custody
    );
}

#[test]
fn recovered_native_observation_preserves_original_uncertainty_and_fences() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    // Synthetic SQL fixture tests publication/replay identity, not execution of
    // actor waits. The native private experiment separately exercises real custody.
    let (drain, row) = native_publication_fixture(&db, &lease);
    let original = serde_json::json!({"observation":"original uncertainty"});
    let recovery = serde_json::json!({"lease":lease, "recovery_evidence":{"original_drain":drain}});
    db.retain_native_attempt_custody(&lease.owner, &original)
        .unwrap();
    db.retain_native_recovered_attempt_custody(&lease.owner, &recovery)
        .unwrap();
    assert_eq!(
        db.native_attempt_custody(lease.runtime_generation_uuid, lease.owner.invocation_uuid)
            .unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        db.native_recovered_attempt_custody(
            lease.runtime_generation_uuid,
            lease.owner.invocation_uuid
        )
        .unwrap(),
        Some(recovery.clone())
    );
    assert!(
        db.retain_native_attempt_custody(&lease.owner, &recovery)
            .is_err()
    );
    assert!(
        db.retain_native_recovered_attempt_custody(&lease.owner, &original)
            .is_err()
    );
    let runtime = serde_json::json!({"original_runtime_row":row, "original_drain":drain, "cancellation_terminal_code":"startup_failed"});
    db.retain_native_runtime_cancellation(&lease.owner, &runtime)
        .unwrap();
    db.retain_native_runtime_cancellation(&lease.owner, &runtime)
        .unwrap();
    let preserved: String = db
        .connection()
        .query_row(
            "SELECT result_json FROM provider_launch_transition_replays WHERE operation_key=?1",
            [format!(
                "{}/native-runtime-cancellation-receipts",
                lease.owner.attempt_id
            )],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&preserved).unwrap(),
        runtime
    );
    assert!(
        db.retain_native_runtime_cancellation(&lease.owner, &original)
            .is_err()
    );
    assert_eq!(
        db.native_attempt_custody(lease.runtime_generation_uuid, lease.owner.invocation_uuid)
            .unwrap(),
        Some(original)
    );
    let mut wrong = lease.owner.clone();
    wrong.owner_epoch += 1;
    assert!(
        db.retain_native_runtime_cancellation(&wrong, &runtime)
            .is_err()
    );
    assert!(
        db.retain_native_recovered_attempt_custody(&wrong, &recovery)
            .is_err()
    );
}

// This fixture intentionally seeds published database facts only. It supplies no
// physical-custody execution evidence and is never used to assert actor Q.
fn native_publication_fixture(
    db: &StateDb,
    lease: &ProviderLaunchLease,
) -> (serde_json::Value, serde_json::Value) {
    use oulipoly_state::mailbox::{CompletionDomainOwner, MailboxDb};
    let path = MailboxDb::path_for_state_db(db.path());
    let mut mailbox = MailboxDb::open_completion_continuation_domain(&path).unwrap();
    let identity =
        oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
    let identity = oulipoly_state::completion_continuation::SourceProcessIdentity {
        pid: identity.os_pid,
        boot_id: identity.os_boot_id,
        starttime_ticks: identity.os_pid_starttime_ticks,
    };
    let owner = CompletionDomainOwner {
        protocol: oulipoly_state::completion_continuation::PROTOCOL.into(),
        domain_id: mailbox.completion_continuation_domain().unwrap().unwrap(),
        owner_generation: Uuid::new_v4().to_string(),
        guardian_identity: identity.clone(),
        driver_identity: identity.clone(),
        endpoint: "fixture-only".into(),
    };
    mailbox
        .publish_completion_continuation_owner(&owner)
        .unwrap();
    drop(mailbox);
    let conn = Connection::open(&path).unwrap();
    conn.execute("INSERT INTO runtime_generation(generation_uuid,lifecycle_state,spawn_invocation_uuid,runtime_mode,provider_name,created_at,exited_at,terminal_reason) VALUES(?1,'exited',?2,'headless','fixture','now','now','recovered_dead')",
        params![lease.runtime_generation_uuid.to_string(), lease.owner.invocation_uuid.to_string()]).unwrap();
    conn.execute("INSERT INTO completion_continuation_attempt(attempt_id,domain_id,owner_generation,operation,request_sha256,session_id,claim_token,phase,revision,custodian_identity,adopter_identity,spawn_invocation_uuid,runtime_generation_uuid,result_path,integrated,drain_receipt) VALUES(?1,?2,?3,'activation',?4,'fixture','claim','drained',1,?5,?5,?6,?7,'fixture',1,?8)",
        params![Uuid::new_v4().to_string(),owner.domain_id,owner.owner_generation,"a".repeat(64),serde_json::to_string(&identity).unwrap(),lease.owner.invocation_uuid.to_string(),lease.runtime_generation_uuid.to_string(),r#"{"accepted_cancellation":"fixture"}"#]).unwrap();
    let published = MailboxDb::read_native_publication(
        &path,
        &lease.runtime_generation_uuid.to_string(),
        &lease.owner.invocation_uuid.to_string(),
    )
    .unwrap();
    (
        published.original_drain.unwrap(),
        serde_json::to_value(published.runtime.unwrap()).unwrap(),
    )
}

#[test]
fn native_publication_replay_revalidates_actual_drain_and_runtime_without_overwrite() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let (drain, row) = native_publication_fixture(&db, &lease);
    let recovery = serde_json::json!({"lease":lease,"recovery_evidence":{"original_drain":drain}});
    db.retain_native_recovered_attempt_custody(&lease.owner, &recovery)
        .unwrap();
    let supplement = serde_json::json!({"original_drain":drain,"original_runtime_row":row,"cancellation_terminal_code":"startup_failed"});
    db.retain_native_runtime_cancellation(&lease.owner, &supplement)
        .unwrap();
    let sidecar = Connection::open(mailbox::MailboxDb::path_for_state_db(db.path())).unwrap();
    db.request_cancel(lease.owner.logical_launch_id).unwrap();
    let mut cancellation = proof(&lease);
    cancellation.runtime_settlement_sha256 =
        completion_continuation::sha256(&serde_json::to_vec(&supplement).unwrap());
    sidecar.execute("UPDATE runtime_generation SET active_delivery_claim_uuid=?2,active_delivery_claimed_at='now',active_delivery_seqs_json='[1]' WHERE generation_uuid=?1",
        params![lease.runtime_generation_uuid.to_string(),Uuid::new_v4().to_string()]).unwrap();
    assert_eq!(
        db.settle_cancel(&lease.owner, &cancellation).unwrap_err(),
        "native_publication_runtime_not_settled"
    );
    sidecar.execute("UPDATE runtime_generation SET active_delivery_claim_uuid=NULL,active_delivery_claimed_at=NULL,active_delivery_seqs_json=NULL WHERE generation_uuid=?1",
        [lease.runtime_generation_uuid.to_string()]).unwrap();
    let duty = ProviderLaunchChannelSettlement::ContinuingCustody {
        domain_id: "wrong-domain".into(),
        original_owner: lease.owner.clone(),
        disposition: "cleanup_failed".into(),
        path: "synthetic-test-only".into(),
        artifacts: vec![],
    };
    assert_eq!(
        db.retain_native_channel_duty(&lease.owner, &duty)
            .unwrap_err(),
        "native_publication_channel_owner_conflict"
    );
    let mut duty = duty;
    if let ProviderLaunchChannelSettlement::ContinuingCustody { domain_id, .. } = &mut duty {
        *domain_id = drain["domain_id"].as_str().unwrap().into();
    }
    db.retain_native_channel_duty(&lease.owner, &duty).unwrap();
    sidecar
        .execute(
            "UPDATE runtime_generation SET exit_code=37 WHERE generation_uuid=?1",
            [lease.runtime_generation_uuid.to_string()],
        )
        .unwrap();
    assert_eq!(
        db.retain_native_runtime_cancellation(&lease.owner, &supplement)
            .unwrap_err(),
        "native_publication_runtime_conflict"
    );
    sidecar.execute("UPDATE completion_continuation_attempt SET revision=revision+1,drain_receipt='{}' WHERE attempt_id=?1",[drain["attempt_id"].as_str().unwrap()]).unwrap();
    assert_eq!(
        db.retain_native_recovered_attempt_custody(&lease.owner, &recovery)
            .unwrap_err(),
        "native_publication_original_drain_conflict"
    );
    assert_eq!(
        db.native_recovered_attempt_custody(
            lease.runtime_generation_uuid,
            lease.owner.invocation_uuid
        )
        .unwrap(),
        Some(recovery)
    );
}

#[test]
fn native_publication_missing_storage_never_grandfathers_detached_evidence() {
    let (_dir, db, request) = fixture();
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    let path = mailbox::MailboxDb::path_for_state_db(db.path());
    assert!(!path.exists());
    let recovery = serde_json::json!({"lease":lease,"recovery_evidence":{"original_drain":{"unpublished":"fixture"}}});
    assert!(
        db.retain_native_recovered_attempt_custody(&lease.owner, &recovery)
            .is_err()
    );
    assert!(!path.exists());
    assert!(
        db.native_recovered_attempt_custody(
            lease.runtime_generation_uuid,
            lease.owner.invocation_uuid
        )
        .unwrap()
        .is_none()
    );
}
