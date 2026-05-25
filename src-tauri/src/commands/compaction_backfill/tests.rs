use super::CompactionBackfillReport;
use super::accessor::{backfill_session, segments};
use super::formatter::{
    format_compaction_backfill_report_line, format_compaction_backfill_session_line,
};
use super::orchestration::run_compaction_backfill;
use super::report::{accumulate_compaction_backfill, empty_compaction_backfill_report};
use oulipoly_state::{OwnedTurnEventRow, StateDb};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

#[test]
fn compaction_backfill_report_empty_and_accumulate_math_is_pinned() {
    let mut report = empty_compaction_backfill_report();
    assert_eq!(
        report,
        CompactionBackfillReport {
            turns_flagged: 0,
            sessions_processed: 0,
        }
    );

    accumulate_compaction_backfill(&mut report, 0);
    assert_eq!(
        report,
        CompactionBackfillReport {
            turns_flagged: 0,
            sessions_processed: 1,
        }
    );

    accumulate_compaction_backfill(&mut report, 3);
    accumulate_compaction_backfill(&mut report, 2);
    assert_eq!(
        report,
        CompactionBackfillReport {
            turns_flagged: 5,
            sessions_processed: 3,
        }
    );
}

#[test]
fn private_compaction_backfill_formatter_lines_match_render_contract_bytes() {
    assert_eq!(
        format_compaction_backfill_session_line("fixture-provider", "fixture-session", 7),
        "compaction backfill session: provider=fixture-provider session_id=fixture-session flagged=7"
    );

    let report = CompactionBackfillReport {
        turns_flagged: 42,
        sessions_processed: 3,
    };
    assert_eq!(
        format_compaction_backfill_report_line(&report),
        "compaction backfill: 42 turns flagged across 3 sessions"
    );
}

#[test]
fn run_compaction_backfill_returns_empty_report_for_empty_state() {
    let (_dir, _path, state) = open_temp_state_db();

    let report = run_compaction_backfill(&state).unwrap();

    assert_eq!(
        report,
        CompactionBackfillReport {
            turns_flagged: 0,
            sessions_processed: 0,
        }
    );
}

#[test]
fn real_state_segments_and_backfill_session_round_trip_preserves_report_and_lines() {
    let (_dir, path, state) = open_temp_state_db();
    seed_chain_segment_and_turns(&path);
    state
        .insert_owned_turn_event_rows(&[OwnedTurnEventRow {
            session_id: "fixture-session".to_string(),
            turn_uuid: "compact-turn".to_string(),
            is_compaction_boundary: true,
            summary_metadata_json: None,
        }])
        .unwrap();

    let mut report = empty_compaction_backfill_report();
    let mut session_lines = Vec::new();
    for (provider_name, session_id) in segments(&state).unwrap() {
        let flagged = backfill_session(&state, &provider_name, &session_id).unwrap();
        accumulate_compaction_backfill(&mut report, flagged);
        session_lines.push(format_compaction_backfill_session_line(
            &provider_name,
            &session_id,
            flagged,
        ));
    }

    assert_eq!(
        session_lines,
        [
            "compaction backfill session: provider=fixture-provider session_id=fixture-session flagged=1"
        ]
    );
    assert_eq!(
        report,
        CompactionBackfillReport {
            turns_flagged: 1,
            sessions_processed: 1,
        }
    );
    assert_eq!(
        format_compaction_backfill_report_line(&report),
        "compaction backfill: 1 turns flagged across 1 sessions"
    );
    assert_eq!(
        turn_flags(&path),
        [
            ("compact-turn".to_string(), 1_i64),
            ("ignored-normal".to_string(), 0_i64),
        ]
    );

    assert_eq!(
        backfill_session(&state, "fixture-provider", "fixture-session").unwrap(),
        0
    );
    assert_eq!(
        turn_flags(&path),
        [
            ("compact-turn".to_string(), 1_i64),
            ("ignored-normal".to_string(), 0_i64),
        ]
    );
}

fn open_temp_state_db() -> (tempfile::TempDir, PathBuf, StateDb) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let state = StateDb::open(&path).unwrap();
    (dir, path, state)
}

fn seed_chain_segment_and_turns(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES ('fixture-chain', '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'fixture')",
        [],
    )
    .unwrap();
    conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES ('fixture-chain', 'fixture-provider', 'fixture-session', '2026-04-17T08:00:00Z', 'initial')",
            [],
        )
        .unwrap();
    for (turn_id, is_boundary) in [("ignored-normal", 0_i64), ("compact-turn", 0_i64)] {
        conn.execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, is_compaction_boundary, source_file, ingested_at)
                 VALUES ('fixture-provider', 'fixture-session', ?1, '2026-04-17T08:00:00Z', 'assistant', ?2, '/tmp/source.jsonl', '2026-04-17T08:00:01Z')",
                params![turn_id, is_boundary],
            )
            .unwrap();
    }
}

fn turn_flags(path: &Path) -> Vec<(String, i64)> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT turn_id, is_compaction_boundary
                 FROM session_turns
                 ORDER BY turn_id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}
