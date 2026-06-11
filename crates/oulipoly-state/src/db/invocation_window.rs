use super::{StateDb, sqlite};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

struct InvocationWindowTurnRow {
    session_id: String,
    timestamp_raw: String,
}

impl StateDb {
    pub fn find_session_for_invocation_window(
        &self,
        provider_name: &str,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<Option<String>, String> {
        Ok(self
            .find_sessions_for_invocation_window(provider_name, started_at, finished_at)?
            .into_iter()
            .next())
    }

    pub fn find_sessions_for_invocation_window(
        &self,
        provider_name: &str,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<Vec<String>, String> {
        let rows = self.load_invocation_window_turn_rows(provider_name)?;
        let mut candidates: HashMap<String, (DateTime<Utc>, u64)> = HashMap::new();
        for row in rows {
            Self::accumulate_invocation_window_candidate(
                &mut candidates,
                row,
                started_at,
                finished_at,
            )?;
        }
        Ok(Self::rank_invocation_window_sessions(candidates))
    }

    fn load_invocation_window_turn_rows(
        &self,
        provider_name: &str,
    ) -> Result<Vec<InvocationWindowTurnRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT session_id, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1",
            )
            .map_err(|e| format!("Failed to prepare invocation session lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![provider_name], |row| {
                Ok(InvocationWindowTurnRow {
                    session_id: row.get(0)?,
                    timestamp_raw: row.get(1)?,
                })
            })
            .map_err(|e| format!("Failed to query invocation session lookup: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read invocation session lookup row: {e}"))
    }

    fn accumulate_invocation_window_candidate(
        candidates: &mut HashMap<String, (DateTime<Utc>, u64)>,
        row: InvocationWindowTurnRow,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> Result<(), String> {
        let timestamp = Self::parse_invocation_window_turn_timestamp(&row)?;
        if !Self::invocation_window_turn_is_candidate(&timestamp, started_at, finished_at) {
            return Ok(());
        }
        Self::aggregate_invocation_window_candidate(candidates, row.session_id, timestamp);
        Ok(())
    }

    fn parse_invocation_window_turn_timestamp(
        row: &InvocationWindowTurnRow,
    ) -> Result<DateTime<Utc>, String> {
        Self::strict_rfc3339_message(&row.timestamp_raw, "session turn timestamp")
    }

    fn invocation_window_turn_is_candidate(
        timestamp: &DateTime<Utc>,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> bool {
        Self::timestamp_is_inside_invocation_window(timestamp, started_at, finished_at)
    }

    fn aggregate_invocation_window_candidate(
        candidates: &mut HashMap<String, (DateTime<Utc>, u64)>,
        session_id: String,
        timestamp: DateTime<Utc>,
    ) {
        candidates
            .entry(session_id)
            .and_modify(|(earliest, in_window)| {
                Self::update_invocation_window_candidate(earliest, in_window, timestamp);
            })
            .or_insert((timestamp, 1));
    }

    fn update_invocation_window_candidate(
        earliest: &mut DateTime<Utc>,
        in_window: &mut u64,
        timestamp: DateTime<Utc>,
    ) {
        if Self::is_candidate_strictly_earlier(&timestamp, earliest) {
            *earliest = timestamp;
        }
        *in_window += 1;
    }

    fn is_candidate_strictly_earlier(timestamp: &DateTime<Utc>, earliest: &DateTime<Utc>) -> bool {
        timestamp < earliest
    }

    fn timestamp_is_inside_invocation_window(
        timestamp: &DateTime<Utc>,
        started_at: &DateTime<Utc>,
        finished_at: &DateTime<Utc>,
    ) -> bool {
        timestamp > started_at && timestamp <= finished_at
    }

    fn rank_invocation_window_sessions(
        candidates: HashMap<String, (DateTime<Utc>, u64)>,
    ) -> Vec<String> {
        let ranked = Self::rank_invocation_window_candidate_pairs(candidates);
        Self::project_invocation_window_session_ids(ranked)
    }

    fn rank_invocation_window_candidate_pairs(
        candidates: HashMap<String, (DateTime<Utc>, u64)>,
    ) -> Vec<(String, (DateTime<Utc>, u64))> {
        let pairs = Self::collect_invocation_window_candidate_pairs(candidates);
        Self::rank_candidate_pairs_by_count_timestamp_session(pairs)
    }

    fn collect_invocation_window_candidate_pairs(
        candidates: HashMap<String, (DateTime<Utc>, u64)>,
    ) -> Vec<(String, (DateTime<Utc>, u64))> {
        candidates.into_iter().collect()
    }

    fn rank_candidate_pairs_by_count_timestamp_session(
        mut ranked: Vec<(String, (DateTime<Utc>, u64))>,
    ) -> Vec<(String, (DateTime<Utc>, u64))> {
        ranked.sort_by(
            |(session_a, (earliest_a, count_a)), (session_b, (earliest_b, count_b))| {
                count_b
                    .cmp(count_a)
                    .then_with(|| earliest_a.cmp(earliest_b))
                    .then_with(|| session_a.cmp(session_b))
            },
        );
        ranked
    }

    fn project_invocation_window_session_ids(
        ranked: Vec<(String, (DateTime<Utc>, u64))>,
    ) -> Vec<String> {
        ranked
            .into_iter()
            .map(|(session_id, _)| session_id)
            .collect()
    }
}
