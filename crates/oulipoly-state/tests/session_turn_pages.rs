use chrono::{DateTime, Duration, Utc};
use oulipoly_state::{
    SessionTurnIngestStreamKey, SessionTurnPageApply, SessionTurnPageBodyState,
    SessionTurnPageTurnIngest, SessionTurnStreamProjection, StateDb,
};
use std::path::Path;

const REQUEST_DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PAGE_DIGEST: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const LEASE_OWNER: &str = "worker-a";

fn db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

fn key() -> SessionTurnIngestStreamKey {
    SessionTurnIngestStreamKey {
        provider_name: "provider-a".to_string(),
        provider_instance_id: "provider-instance-a".to_string(),
        settings_id: "settings-a".to_string(),
        session_id: "session-a".to_string(),
        projection: SessionTurnStreamProjection::CanonicalIngest,
    }
}

fn turn(id: &str, sequence: u64, role: &str, boundary: bool) -> SessionTurnPageTurnIngest {
    SessionTurnPageTurnIngest {
        session_id: key().session_id,
        turn_id: id.to_string(),
        snapshot_sequence: sequence,
        timestamp: "2026-08-30T12:00:00Z".parse::<DateTime<Utc>>().unwrap(),
        role: role.to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: boundary,
        body_state: SessionTurnPageBodyState::Absent,
        body: None,
        body_bytes: None,
        body_sha256: None,
        canonical_text_sha256: None,
        canonical_text_digest_verified: false,
    }
}

fn page(
    generation: u64,
    index: u64,
    start_sequence: u64,
    turns: Vec<SessionTurnPageTurnIngest>,
    complete: bool,
) -> SessionTurnPageApply {
    SessionTurnPageApply {
        key: key(),
        lease_owner: LEASE_OWNER.to_string(),
        expected_generation: generation,
        request_token_sha256: REQUEST_DIGEST.to_string(),
        snapshot_id: "snapshot-a".to_string(),
        page_index: index,
        page_start_sequence: start_sequence,
        page_turn_count: turns.len() as u64,
        scan_progress: false,
        snapshot_complete: complete,
        next_page_token: (!complete).then(|| format!("page-{}", index + 1)),
        resume_token: complete.then(|| "resume-a".to_string()),
        page_digest: PAGE_DIGEST.to_string(),
        turns,
    }
}

fn lease(db: &StateDb) {
    let now = Utc::now();
    let leased = db
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            LEASE_OWNER,
            now,
            now + Duration::seconds(60),
        )
        .unwrap()
        .expect("stream should be leaseable");
    assert_eq!(leased.key, key());
    assert_eq!(leased.lease_owner.as_deref(), Some(LEASE_OWNER));
}

#[test]
fn first_page_commits_turn_chain_boundary_effect_and_checkpoint_together() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);

    let outcome = db
        .apply_session_turn_page(&page(0, 0, 0, vec![turn("turn-a", 0, "user", true)], false))
        .unwrap();

    assert_eq!(outcome.inserted_turns, 1);
    assert_eq!(outcome.duplicate_turns, 0);
    assert!(!outcome.replayed);
    assert_eq!(outcome.checkpoint_generation, 1);
    let stream = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    assert_eq!(stream.checkpoint_generation, 1);
    assert_eq!(stream.snapshot_id.as_deref(), Some("snapshot-a"));
    assert_eq!(stream.next_page_token.as_deref(), Some("page-1"));
    assert_eq!(stream.expected_page_index, 1);
    assert_eq!(stream.expected_turn_sequence, 1);
    assert_eq!(stream.committed_page_count, 1);
    assert_eq!(stream.committed_turn_count, 1);
    assert_eq!(
        db.count_session_turns("provider-a", "session-a")
            .unwrap()
            .total,
        1
    );
    assert!(
        db.session_chain_segment_exists_for_provider_session("provider-a", "session-a")
            .unwrap()
    );
    let events = db.owned_turn_event_rows_for_session("session-a").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].turn_uuid, "turn-a");
    assert!(events[0].is_compaction_boundary);
}

#[test]
fn empty_scan_progress_advances_only_page_progress() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);
    let mut progress = page(0, 0, 0, Vec::new(), false);
    progress.scan_progress = true;

    db.apply_session_turn_page(&progress).unwrap();

    let stream = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    assert_eq!(stream.checkpoint_generation, 1);
    assert_eq!(stream.expected_page_index, 1);
    assert_eq!(stream.expected_turn_sequence, 0);
    assert_eq!(stream.committed_page_count, 1);
    assert_eq!(stream.committed_turn_count, 0);
    assert_eq!(
        db.count_session_turns("provider-a", "session-a")
            .unwrap()
            .total,
        0
    );
}

#[test]
fn canonical_freshness_requires_a_tracked_completed_snapshot() {
    let db = db();
    let untracked = db
        .canonical_session_turn_ingest_freshness("provider-a", "session-a")
        .unwrap();
    assert_eq!(untracked.tracked_streams, 0);
    assert!(!untracked.is_caught_up());

    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    let ready = db
        .canonical_session_turn_ingest_freshness("provider-a", "session-a")
        .unwrap();
    assert_eq!(ready.tracked_streams, 1);
    assert_eq!(ready.caught_up_streams, 0);
    assert!(!ready.is_caught_up());

    lease(&db);
    db.apply_session_turn_page(&page(0, 0, 0, Vec::new(), true))
        .unwrap();
    let caught_up = db
        .canonical_session_turn_ingest_freshness("provider-a", "session-a")
        .unwrap();
    assert_eq!(caught_up.tracked_streams, 1);
    assert_eq!(caught_up.caught_up_streams, 1);
    assert!(caught_up.latest_success_at.is_some());
    assert!(caught_up.latest_updated_at.is_some());
    assert!(caught_up.is_caught_up());

    let provider_caught_up = db
        .canonical_provider_turn_ingest_freshness("provider-a")
        .unwrap();
    assert_eq!(provider_caught_up.tracked_streams, 1);
    assert!(provider_caught_up.is_caught_up());

    let mut second_session = key();
    second_session.session_id = "session-b".to_string();
    db.enqueue_session_turn_ingest_stream(&second_session)
        .unwrap();
    let provider_lagging = db
        .canonical_provider_turn_ingest_freshness("provider-a")
        .unwrap();
    assert_eq!(provider_lagging.tracked_streams, 2);
    assert_eq!(provider_lagging.caught_up_streams, 1);
    assert!(!provider_lagging.is_caught_up());
}

#[test]
fn identical_page_replay_has_no_duplicate_effects() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);
    let first = page(0, 0, 0, vec![turn("turn-a", 0, "user", false)], true);
    db.apply_session_turn_page(&first).unwrap();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);

    let replay = db.apply_session_turn_page(&first).unwrap();

    assert!(replay.replayed);
    assert_eq!(replay.inserted_turns, 0);
    assert_eq!(replay.duplicate_turns, 1);
    assert_eq!(replay.checkpoint_generation, 1);
    assert_eq!(
        db.count_session_turns("provider-a", "session-a")
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        db.owned_turn_event_rows_for_session("session-a")
            .unwrap()
            .len(),
        1
    );
    let stream = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    assert_eq!(stream.committed_page_count, 1);
    assert_eq!(stream.committed_turn_count, 1);
}

#[test]
fn changed_page_replay_quarantines_without_advancing_checkpoint() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);
    let first = page(0, 0, 0, vec![turn("turn-a", 0, "user", false)], true);
    db.apply_session_turn_page(&first).unwrap();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);
    let mut changed = first;
    changed.page_digest =
        "3333333333333333333333333333333333333333333333333333333333333333".to_string();

    let error = db.apply_session_turn_page(&changed).unwrap_err();

    assert_eq!(error, "page_replay_mismatch");
    let stream = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    assert_eq!(stream.status, "quarantined");
    assert_eq!(stream.checkpoint_generation, 1);
    assert_eq!(stream.committed_page_count, 1);
    assert_eq!(
        db.count_session_turns("provider-a", "session-a")
            .unwrap()
            .total,
        1
    );
}

#[test]
fn later_stable_id_conflict_rolls_back_new_turn_and_checkpoint() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);
    db.apply_session_turn_page(&page(
        0,
        0,
        0,
        vec![turn("turn-a", 0, "user", false)],
        false,
    ))
    .unwrap();
    lease(&db);
    let conflicting = page(
        1,
        1,
        1,
        vec![
            turn("turn-b", 1, "assistant", false),
            turn("turn-a", 2, "assistant", false),
        ],
        true,
    );

    let error = db.apply_session_turn_page(&conflicting).unwrap_err();

    assert!(error.contains("turn_content_conflict"));
    let stream = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    assert_eq!(stream.status, "quarantined");
    assert_eq!(stream.checkpoint_generation, 1);
    assert_eq!(stream.expected_page_index, 1);
    assert_eq!(stream.expected_turn_sequence, 1);
    assert_eq!(stream.committed_turn_count, 1);
    assert_eq!(
        db.count_session_turns("provider-a", "session-a")
            .unwrap()
            .total,
        1
    );
}

#[test]
fn lease_acquisition_is_exclusive_and_expired_owner_cannot_commit() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    let now = Utc::now();
    let first = db
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            LEASE_OWNER,
            now,
            now + Duration::seconds(1),
        )
        .unwrap()
        .unwrap();
    assert_eq!(first.lease_owner.as_deref(), Some(LEASE_OWNER));
    assert!(
        db.lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            "worker-b",
            now,
            now + Duration::seconds(60),
        )
        .unwrap()
        .is_none()
    );

    let replacement = db
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            "worker-b",
            now + Duration::seconds(2),
            now + Duration::seconds(60),
        )
        .unwrap()
        .unwrap();
    assert_eq!(replacement.lease_owner.as_deref(), Some("worker-b"));
    let error = db
        .apply_session_turn_page(&page(0, 0, 0, Vec::new(), true))
        .unwrap_err();
    assert_eq!(error, "session_turn_stream_lease_lost");
    assert_eq!(
        db.session_turn_ingest_stream(&key())
            .unwrap()
            .unwrap()
            .checkpoint_generation,
        0
    );
}

#[test]
fn lease_selection_is_projection_scoped() {
    let db = db();
    let mut observation_key = key();
    observation_key.provider_name = "provider-0".to_string();
    observation_key.projection = SessionTurnStreamProjection::UserObservation;
    db.enqueue_session_turn_ingest_stream(&observation_key)
        .unwrap();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    let now = Utc::now();

    let leased = db
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            LEASE_OWNER,
            now,
            now + Duration::seconds(60),
        )
        .unwrap()
        .unwrap();

    assert_eq!(leased.key, key());
    assert_eq!(
        db.session_turn_ingest_stream(&observation_key)
            .unwrap()
            .unwrap()
            .status,
        "ready"
    );
}

#[test]
fn retry_releases_lease_and_honors_per_stream_backoff() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    let now = Utc::now();
    db.lease_ready_session_turn_ingest_stream(
        SessionTurnStreamProjection::CanonicalIngest,
        LEASE_OWNER,
        now,
        now + Duration::seconds(60),
    )
    .unwrap()
    .unwrap();
    db.retry_session_turn_ingest_stream(
        &key(),
        LEASE_OWNER,
        0,
        now + Duration::seconds(30),
        "bounded provider failure",
    )
    .unwrap();

    assert!(
        db.lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            "worker-b",
            now + Duration::seconds(20),
            now + Duration::seconds(80),
        )
        .unwrap()
        .is_none()
    );
    let retried = db
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            "worker-b",
            now + Duration::seconds(31),
            now + Duration::seconds(90),
        )
        .unwrap()
        .unwrap();
    assert_eq!(retried.status, "active");
    assert_eq!(retried.lease_owner.as_deref(), Some("worker-b"));
}

#[test]
fn unsupported_stream_is_not_scheduled_again() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    let now = Utc::now();
    db.lease_ready_session_turn_ingest_stream(
        SessionTurnStreamProjection::CanonicalIngest,
        LEASE_OWNER,
        now,
        now + Duration::seconds(60),
    )
    .unwrap()
    .unwrap();
    db.mark_session_turn_ingest_unsupported(&key(), LEASE_OWNER, 0, "paging_capability_missing")
        .unwrap();

    assert_eq!(
        db.session_turn_ingest_stream(&key())
            .unwrap()
            .unwrap()
            .status,
        "unsupported"
    );
    assert!(
        db.lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            "worker-b",
            now + Duration::hours(1),
            now + Duration::hours(2),
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn age343_fixed_capacity_requires_explicit_fenced_rearm_without_checkpoint_reset() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);
    db.apply_session_turn_page(&page(
        0,
        0,
        0,
        vec![turn("prefix", 0, "user", false)],
        false,
    ))
    .unwrap();
    lease(&db);
    let before = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    db.mark_session_turn_ingest_unsupported(
        &key(),
        LEASE_OWNER,
        before.checkpoint_generation,
        "session_turn_paging_paused",
    )
    .unwrap();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    let stopped = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    assert_eq!(stopped.status, "unsupported");
    assert!(
        !db.canonical_session_turn_ingest_freshness(&key().provider_name, &key().session_id)
            .unwrap()
            .is_caught_up()
    );
    assert!(
        !db.rearm_session_turn_ingest_after_capacity_resolution(
            &key(),
            before.checkpoint_generation + 1,
            "session_turn_paging_paused"
        )
        .unwrap()
    );
    assert!(
        !db.rearm_session_turn_ingest_after_capacity_resolution(
            &key(),
            before.checkpoint_generation,
            "session_turn_staging_capacity_exceeded"
        )
        .unwrap()
    );
    assert!(
        db.rearm_session_turn_ingest_after_capacity_resolution(
            &key(),
            before.checkpoint_generation,
            "session_turn_paging_paused"
        )
        .unwrap()
    );
    assert!(
        !db.rearm_session_turn_ingest_after_capacity_resolution(
            &key(),
            before.checkpoint_generation,
            "session_turn_paging_paused"
        )
        .unwrap()
    );
    let resumed = db.session_turn_ingest_stream(&key()).unwrap().unwrap();
    assert_eq!(resumed.next_page_token, before.next_page_token);
    assert_eq!(resumed.checkpoint_generation, before.checkpoint_generation);
    assert_eq!(resumed.committed_turn_count, 1);
    assert_eq!(resumed.expected_page_index, before.expected_page_index);
    lease(&db);
    db.apply_session_turn_page(&page(
        1,
        1,
        1,
        vec![turn("after", 1, "assistant", false)],
        true,
    ))
    .unwrap();
    assert_eq!(
        db.count_session_turns("provider-a", "session-a")
            .unwrap()
            .total,
        2
    );
    assert!(
        db.canonical_session_turn_ingest_freshness("provider-a", "session-a")
            .unwrap()
            .is_caught_up()
    );
}

#[test]
fn age343_rearm_rejects_other_authorities_and_preserves_missing_capability_behavior() {
    let db = db();
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db);
    db.mark_session_turn_ingest_unsupported(&key(), LEASE_OWNER, 0, "session_turn_paging_paused")
        .unwrap();
    let mut other = key();
    other.provider_instance_id = "different-provider".into();
    assert!(
        !db.rearm_session_turn_ingest_after_capacity_resolution(
            &other,
            0,
            "session_turn_paging_paused"
        )
        .unwrap()
    );
    assert!(
        db.rearm_session_turn_ingest_after_capacity_resolution(
            &key(),
            0,
            "session_turn_paging_paused"
        )
        .unwrap()
    );
    lease(&db);
    db.mark_session_turn_ingest_unsupported(&key(), LEASE_OWNER, 0, "session_capability_missing")
        .unwrap();
    assert!(
        !db.rearm_session_turn_ingest_after_capacity_resolution(
            &key(),
            0,
            "session_capability_missing"
        )
        .unwrap()
    );
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    lease(&db); // unrelated unsupported capability still has its old enqueue behavior
    db.quarantine_session_turn_ingest_stream(&key(), LEASE_OWNER, 0, "session_turn_paging_paused")
        .unwrap();
    assert!(
        !db.rearm_session_turn_ingest_after_capacity_resolution(
            &key(),
            0,
            "session_turn_paging_paused"
        )
        .unwrap()
    );
    db.enqueue_session_turn_ingest_stream(&key()).unwrap();
    assert_eq!(
        db.session_turn_ingest_stream(&key())
            .unwrap()
            .unwrap()
            .status,
        "quarantined"
    );
}
