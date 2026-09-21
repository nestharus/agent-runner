use crate::model::Event;
use crate::{CandidateObservation, EvalResult, QueryResult, timed};
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use std::path::Path;
use std::thread;

pub fn embedded_lsm(
    root: &Path,
    events: &[Event],
    producers: usize,
    batch_size: usize,
    time_bounds: (u64, u64),
    trace_id: [u8; 16],
) -> EvalResult<CandidateObservation> {
    std::fs::create_dir_all(root)?;
    let database = Database::builder(root)
        .manual_journal_persist(true)
        .open()?;
    let records = database.keyspace("records", KeyspaceCreateOptions::default)?;
    let time = database.keyspace("time", KeyspaceCreateOptions::default)?;
    let trace = database.keyspace("trace", KeyspaceCreateOptions::default)?;
    let producer_events = split_by_producer(events, producers);
    let (append_ms, ()) = timed(|| {
        let handles: Vec<_> = producer_events
            .into_iter()
            .map(|events| {
                let database = database.clone();
                let records = records.clone();
                let time = time.clone();
                let trace = trace.clone();
                thread::spawn(move || -> EvalResult<()> {
                    for events in events.chunks(batch_size) {
                        let mut batch = database.batch().durability(Some(PersistMode::SyncAll));
                        for event in events {
                            let event_key = record_key(&event.event_id);
                            batch.insert(&records, event_key.clone(), event.encoded()?);
                            batch.insert(
                                &time,
                                time_key(event.timestamp_micros, &event.event_id),
                                event_key.clone(),
                            );
                            batch.insert(
                                &trace,
                                trace_key(&event.trace_id, event.timestamp_micros, &event.event_id),
                                event_key,
                            );
                        }
                        batch.commit()?;
                    }
                    Ok(())
                })
            })
            .collect();
        for handle in handles {
            handle.join().map_err(|_| "LSM producer panicked")??;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })?;
    database.persist(PersistMode::SyncAll)?;
    drop(records);
    drop(time);
    drop(trace);
    drop(database);

    let (reopen_ms, (database, records, time, trace)) = timed(|| open(root))?;
    let (full_scan_ms, full_scan) = timed(|| scan_records(&records))?;
    let (time_query_ms, time_query) = timed(|| {
        query_secondary(
            &records,
            time.range(time_key(time_bounds.0, &[0; 16])..time_key(time_bounds.1, &[0; 16])),
        )
    })?;
    let (trace_query_ms, trace_query) = timed(|| {
        let mut prefix = Vec::with_capacity(17);
        prefix.push(b'r');
        prefix.extend_from_slice(&trace_id);
        query_secondary(&records, trace.prefix(prefix))
    })?;
    database.persist(PersistMode::SyncAll)?;

    Ok(CandidateObservation {
        candidate: "embedded_lsm_fjall".to_string(),
        append_ms,
        reopen_ms,
        full_scan_ms,
        time_query_ms,
        trace_query_ms,
        full_scan,
        time_query,
        trace_query,
        bytes_on_disk: crate::directory_size(root)?,
        notes: vec![
            "Fjall 3.1.10, default compression disabled, atomic multi-keyspace batches".to_string(),
            "SyncAll durability per configured batch".to_string(),
            "single embedded database with internal journal/compaction ownership".to_string(),
        ],
    })
}

fn open(root: &Path) -> EvalResult<(Database, Keyspace, Keyspace, Keyspace)> {
    let database = Database::builder(root)
        .manual_journal_persist(true)
        .open()?;
    let records = database.keyspace("records", KeyspaceCreateOptions::default)?;
    let time = database.keyspace("time", KeyspaceCreateOptions::default)?;
    let trace = database.keyspace("trace", KeyspaceCreateOptions::default)?;
    Ok((database, records, time, trace))
}

fn scan_records(records: &Keyspace) -> EvalResult<QueryResult> {
    let mut result = QueryResult::default();
    for item in records.prefix(b"e") {
        let (_, value) = item.into_inner()?;
        result.observe(&value);
    }
    Ok(result)
}

fn query_secondary(
    records: &Keyspace,
    iterator: impl Iterator<Item = fjall::Guard>,
) -> EvalResult<QueryResult> {
    let mut result = QueryResult::default();
    for item in iterator {
        let (_, event_key) = item.into_inner()?;
        let body = records
            .get(&event_key)?
            .ok_or("secondary index referenced a missing record")?;
        result.observe(&body);
    }
    Ok(result)
}

fn record_key(event_id: &[u8; 16]) -> Vec<u8> {
    let mut key = Vec::with_capacity(17);
    key.push(b'e');
    key.extend_from_slice(event_id);
    key
}

fn time_key(timestamp_micros: u64, event_id: &[u8; 16]) -> Vec<u8> {
    let mut key = Vec::with_capacity(25);
    key.push(b't');
    key.extend_from_slice(&timestamp_micros.to_be_bytes());
    key.extend_from_slice(event_id);
    key
}

fn trace_key(trace_id: &[u8; 16], timestamp_micros: u64, event_id: &[u8; 16]) -> Vec<u8> {
    let mut key = Vec::with_capacity(41);
    key.push(b'r');
    key.extend_from_slice(trace_id);
    key.extend_from_slice(&timestamp_micros.to_be_bytes());
    key.extend_from_slice(event_id);
    key
}

fn split_by_producer(events: &[Event], producers: usize) -> Vec<Vec<Event>> {
    let mut split = vec![Vec::new(); producers];
    for event in events {
        split[event.producer as usize].push(event.clone());
    }
    split
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_keys_remain_one_logical_record_after_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let event = crate::model::fixture_events(1, 1, 32).remove(0);
        let (database, records, time, trace) = open(directory.path()).unwrap();

        for _ in 0..2 {
            let event_key = record_key(&event.event_id);
            let mut batch = database.batch().durability(Some(PersistMode::SyncAll));
            batch.insert(&records, event_key.clone(), event.encoded().unwrap());
            batch.insert(
                &time,
                time_key(event.timestamp_micros, &event.event_id),
                event_key.clone(),
            );
            batch.insert(
                &trace,
                trace_key(&event.trace_id, event.timestamp_micros, &event.event_id),
                event_key,
            );
            batch.commit().unwrap();
        }
        database.persist(PersistMode::SyncAll).unwrap();
        drop(records);
        drop(time);
        drop(trace);
        drop(database);

        let (_, records, _, _) = open(directory.path()).unwrap();
        assert_eq!(scan_records(&records).unwrap().records, 1);
    }
}
