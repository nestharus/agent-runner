//! Output settlement uses caller-retained authority, independently of provider success.
//! Declared roles: orchestration, validator
use super::settle;
use oulipoly_state::*;
use rusqlite::Connection;
use std::io::{self, Write};
use uuid::Uuid;

fn allocated(db: &StateDb) -> (BeginProviderLaunchRequest, ProviderLaunchLease) {
    let request = BeginProviderLaunchRequest {
        logical_launch_id: Uuid::new_v4(),
        request_identity_sha256: "a".repeat(64),
        model_name: "synthetic".into(),
        start_mode: ProviderLaunchStartMode::Create,
        expected_provider_session_id: None,
        candidates: vec![
            ProviderLaunchCandidate {
                provider_index: 0,
                account_name: "first".into(),
            },
            ProviderLaunchCandidate {
                provider_index: 1,
                account_name: "second".into(),
            },
        ],
        parent_invocation_id: None,
        allocation: ProviderLaunchAttemptAllocation::allocate().unwrap(),
    };
    let lease = db.begin_launch(&request).unwrap();
    db.activate_attempt(&lease, &request.allocation.completion_authority)
        .unwrap();
    db.retain_launch_owner(&lease.owner).unwrap();
    (request, lease)
}

fn output(db: &StateDb, id: i64, uuid: &str) -> InvocationOutputArtifactPaths {
    use sha2::{Digest, Sha256};
    let paths = db.invocation_output_artifact_paths(uuid).unwrap().unwrap();
    std::fs::write(&paths.stdout, b"native resumed\n").unwrap();
    std::fs::write(&paths.stderr, b"").unwrap();
    db.record_invocation_output_pending(
        db.invocation_mutation_scope(id).authority(),
        id,
        uuid,
        &paths,
        15,
        &format!("{:x}", Sha256::digest(b"native resumed\n")),
        0,
        &format!("{:x}", Sha256::digest(b"")),
        1,
    )
    .unwrap();
    paths
}

fn delivery_state(db: &StateDb, id: i64) -> (String, String, Option<String>, Option<String>) {
    Connection::open(db.path()).unwrap().query_row(
        "SELECT provider_outcome_state,delivery_state,delivery_failure_stage,delivery_failure_kind FROM invocation_output_deliveries WHERE invocation_id=?1",
        [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap()
}

fn success_fixture(
    is_allocated: bool,
) -> (
    tempfile::TempDir,
    StateDb,
    i64,
    String,
    InvocationOutputArtifactPaths,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let (id, uuid) = if is_allocated {
        let (_, lease) = allocated(&db);
        (
            lease.owner.invocation_row_id,
            lease.owner.invocation_uuid.to_string(),
        )
    } else {
        let uuid = Uuid::new_v4().to_string();
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid.clone(),
                model_name: "synthetic".into(),
                provider_name: "first".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        (id, uuid)
    };
    let paths = output(&db, id, &uuid);
    db.finalize_invocation(
        db.invocation_mutation_scope(id).authority(),
        id,
        true,
        0,
        None,
        Some("native_completed"),
    )
    .unwrap();
    (dir, db, id, uuid, paths)
}

#[test]
fn allocated_and_standalone_output_settle_after_provider_success() {
    for allocated in [true, false] {
        let (_dir, db, id, uuid, paths) = success_fixture(allocated);
        let mut delivered = Vec::new();
        assert!(
            settle(&db, id, true, || {
                assert_eq!(
                    delivery_state(&db, id),
                    (
                        "settled".into(),
                        "failed".into(),
                        Some("delivery_confirmation".into()),
                        Some("unconfirmed".into())
                    )
                );
                delivered.write_all(&std::fs::read(&paths.stdout)?)
            }),
            "allocated={allocated}"
        );
        assert_eq!(delivered, b"native resumed\n");
        assert_eq!(
            delivery_state(&db, id),
            ("settled".into(), "delivered".into(), None, None)
        );
        let row = db.get_invocation_by_uuid(&uuid).unwrap().unwrap();
        assert_eq!(row.exit_code, Some(0));
        assert_eq!(row.status, InvocationStatus::Succeeded);
    }
}

struct BrokenOutput;
impl Write for BrokenOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn allocated_and_standalone_output_failure_preserves_provider_success() {
    for allocated in [true, false] {
        let (_dir, db, id, uuid, paths) = success_fixture(allocated);
        assert!(!settle(&db, id, true, || BrokenOutput
            .write_all(&std::fs::read(&paths.stdout)?)));
        assert_eq!(
            delivery_state(&db, id),
            (
                "settled".into(),
                "failed".into(),
                Some("payload_or_control".into()),
                Some("BrokenPipe".into())
            )
        );
        let row = db.get_invocation_by_uuid(&uuid).unwrap().unwrap();
        assert_eq!(row.status, InvocationStatus::Succeeded);
        assert_eq!(row.exit_code, Some(0));
        assert_eq!(std::fs::read(paths.stdout).unwrap(), b"native resumed\n");
    }
}

#[test]
fn missing_and_foreign_output_owner_cannot_emit_or_mutate() {
    let (_dir, db, id, _uuid, _paths) = success_fixture(true);
    // Reopening cannot infer owner authority from persisted rows.
    let other_handle = StateDb::open(db.path()).unwrap();
    let (_, foreign) = allocated(&other_handle);
    assert!(
        other_handle
            .mark_invocation_output_delivered(
                InvocationMutationAuthority::ProviderLaunch(&foreign.owner),
                id
            )
            .unwrap_err()
            .contains("owner_fence")
    );
    let mut wrong = foreign.owner.clone();
    wrong.invocation_row_id = id;
    assert!(other_handle.retain_launch_owner(&wrong).is_err());
    let before = delivery_state(&db, id);
    assert!(!settle(&other_handle, id, true, || panic!(
        "unauthorized bytes escaped"
    )));
    assert_eq!(delivery_state(&db, id), before);
}

#[test]
fn transferred_output_owner_is_stale_even_when_retained() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let (request, old) = allocated(&db);
    let id = old.owner.invocation_row_id;
    output(&db, id, &old.owner.invocation_uuid.to_string());
    db.request_transfer(
        &old.owner,
        &ProviderLaunchFailureRecord {
            kind: RotatableLaunchFailureKind::ProviderUnavailable,
            request_id: Uuid::new_v4(),
            code: "provider_unavailable".into(),
            exit_code: 73,
        },
    )
    .unwrap();
    let proof = ProviderLaunchCustodyProof {
        attempt_id: old.owner.attempt_id,
        runtime_generation_uuid: old.runtime_generation_uuid,
        spawn_invocation_uuid: old.owner.invocation_uuid,
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
        return_channel_id: old.return_channel_id.clone(),
        channel: ProviderLaunchChannelSettlement::NotCreated,
        return_channel_settlement_sha256: "c".repeat(64),
    };
    db.certify_effect_incapable(&old.owner, &proof).unwrap();
    db.lease_successor(
        &old.owner,
        &request.allocation.completion_authority,
        &old.candidate_plan_sha256,
        &request.candidates[1],
        &proof,
        &ProviderLaunchAttemptAllocation::allocate().unwrap(),
    )
    .unwrap();
    assert!(matches!(
        db.invocation_mutation_scope(id).authority(),
        InvocationMutationAuthority::ProviderLaunch(_)
    ));
    let before = delivery_state(&db, id);
    assert!(!settle(&db, id, true, || panic!(
        "stale owner emitted output"
    )));
    assert_eq!(delivery_state(&db, id), before);
}

#[test]
fn post_delivery_confirmation_failure_is_not_provider_failure() {
    let (_dir, db, id, uuid, paths) = success_fixture(true);
    let mut bytes = Vec::new();
    assert!(!settle(&db, id, true, || {
        bytes.write_all(&std::fs::read(paths.stdout)?)?;
        Connection::open(db.path()).unwrap().execute_batch("CREATE TRIGGER reject_delivered BEFORE UPDATE OF delivery_state ON invocation_output_deliveries WHEN NEW.delivery_state='delivered' BEGIN SELECT RAISE(ABORT,'injected confirmation failure'); END;").unwrap();
        Ok(())
    }));
    assert_eq!(bytes, b"native resumed\n");
    assert_eq!(
        delivery_state(&db, id),
        (
            "settled".into(),
            "failed".into(),
            Some("delivery_confirmation".into()),
            Some("unconfirmed".into())
        )
    );
    assert_eq!(
        db.get_invocation_by_uuid(&uuid).unwrap().unwrap().status,
        InvocationStatus::Succeeded
    );
}
