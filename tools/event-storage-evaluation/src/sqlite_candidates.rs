use crate::model::Event;
use crate::{CandidateObservation, EvalResult, QueryResult, timed};
use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

pub fn partitioned_sqlite(
    root: &Path,
    events: &[Event],
    producers: usize,
    batch_size: usize,
    time_bounds: (u64, u64),
    trace_id: [u8; 16],
) -> EvalResult<CandidateObservation> {
    std::fs::create_dir_all(root)?;
    let paths = sqlite_paths(root, producers);
    for path in &paths {
        create_sqlite(path)?;
    }
    let producer_events = split_by_producer(events, producers);
    let (append_ms, ()) = timed(|| {
        let handles: Vec<_> = paths
            .iter()
            .cloned()
            .zip(producer_events)
            .map(|(path, events)| {
                thread::spawn(move || -> EvalResult<()> {
                    let mut connection = open_sqlite(&path)?;
                    insert_batches(&mut connection, &events, batch_size)
                })
            })
            .collect();
        for handle in handles {
            handle.join().map_err(|_| "SQLite producer panicked")??;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })?;

    let (reopen_ms, connections) = timed(|| {
        paths
            .iter()
            .map(|path| open_sqlite_read_only(path))
            .collect::<EvalResult<Vec<_>>>()
    })?;
    let (full_scan_ms, full_scan) = timed(|| query_sqlite(&connections, Query::All))?;
    let (time_query_ms, time_query) = timed(|| {
        query_sqlite(
            &connections,
            Query::Time {
                start: time_bounds.0,
                end: time_bounds.1,
            },
        )
    })?;
    let (trace_query_ms, trace_query) =
        timed(|| query_sqlite(&connections, Query::Trace(trace_id)))?;
    drop(connections);

    Ok(CandidateObservation {
        candidate: "partitioned_sqlite".to_string(),
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
            "one SQLite/WAL database per producer; FULL synchronous transactions".to_string(),
            "one durability acknowledgement per configured batch".to_string(),
        ],
    })
}

pub fn wal_broker_sqlite(
    root: &Path,
    events: &[Event],
    producers: usize,
    batch_size: usize,
    time_bounds: (u64, u64),
    trace_id: [u8; 16],
) -> EvalResult<CandidateObservation> {
    std::fs::create_dir_all(root)?;
    let path = root.join("broker.sqlite3");
    create_sqlite(&path)?;
    let producer_events = split_by_producer(events, producers);
    let (append_ms, ()) = timed(|| run_broker(&path, producer_events, batch_size))?;

    let (reopen_ms, connections) = timed(|| Ok(vec![open_sqlite_read_only(&path)?]))?;
    let (full_scan_ms, full_scan) = timed(|| query_sqlite(&connections, Query::All))?;
    let (time_query_ms, time_query) = timed(|| {
        query_sqlite(
            &connections,
            Query::Time {
                start: time_bounds.0,
                end: time_bounds.1,
            },
        )
    })?;
    let (trace_query_ms, trace_query) =
        timed(|| query_sqlite(&connections, Query::Trace(trace_id)))?;
    drop(connections);

    Ok(CandidateObservation {
        candidate: "wal_write_broker".to_string(),
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
            "one SQLite/WAL database and one broker writer".to_string(),
            "producers wait for their batch transaction acknowledgement".to_string(),
        ],
    })
}

fn run_broker(path: &Path, producer_events: Vec<Vec<Event>>, batch_size: usize) -> EvalResult<()> {
    struct Request {
        events: Vec<Event>,
        completion: mpsc::Sender<Result<(), String>>,
    }
    let (sender, receiver) = mpsc::channel::<Request>();
    let broker_path = path.to_path_buf();
    let broker = thread::spawn(move || -> EvalResult<()> {
        let mut connection = open_sqlite(&broker_path)?;
        while let Ok(request) = receiver.recv() {
            let result = insert_transaction(&mut connection, &request.events)
                .map_err(|error| error.to_string());
            let failed = result.is_err();
            let _ = request.completion.send(result);
            if failed {
                return Err("broker transaction failed".into());
            }
        }
        Ok(())
    });
    let handles: Vec<_> = producer_events
        .into_iter()
        .map(|events| {
            let sender = sender.clone();
            thread::spawn(move || -> EvalResult<()> {
                for batch in events.chunks(batch_size) {
                    let (completion, result) = mpsc::channel();
                    sender.send(Request {
                        events: batch.to_vec(),
                        completion,
                    })?;
                    result.recv()?.map_err(
                        |error| -> Box<dyn std::error::Error + Send + Sync> { error.into() },
                    )?;
                }
                Ok(())
            })
        })
        .collect();
    drop(sender);
    for handle in handles {
        handle.join().map_err(|_| "broker producer panicked")??;
    }
    broker.join().map_err(|_| "SQLite broker panicked")??;
    Ok(())
}

fn create_sqlite(path: &Path) -> EvalResult<()> {
    let connection = open_sqlite(path)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (
             event_id BLOB PRIMARY KEY,
             recorded_at INTEGER NOT NULL,
             trace_id BLOB NOT NULL,
             family INTEGER NOT NULL,
             producer INTEGER NOT NULL,
             body BLOB NOT NULL
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS events_time
             ON events(recorded_at, event_id);
         CREATE INDEX IF NOT EXISTS events_trace_time
             ON events(trace_id, recorded_at, event_id);",
    )?;
    Ok(())
}

fn open_sqlite(path: &Path) -> EvalResult<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 1_000_i64)?;
    Ok(connection)
}

fn open_sqlite_read_only(path: &Path) -> EvalResult<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}

fn insert_batches(
    connection: &mut Connection,
    events: &[Event],
    batch_size: usize,
) -> EvalResult<()> {
    for batch in events.chunks(batch_size) {
        insert_transaction(connection, batch)?;
    }
    Ok(())
}

fn insert_transaction(connection: &mut Connection, events: &[Event]) -> EvalResult<()> {
    let transaction = connection.transaction()?;
    {
        let mut statement = transaction.prepare_cached(
            "INSERT OR IGNORE INTO events
             (event_id, recorded_at, trace_id, family, producer, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for event in events {
            statement.execute(params![
                event.event_id.as_slice(),
                as_sql_integer(event.timestamp_micros)?,
                event.trace_id.as_slice(),
                event.family,
                event.producer,
                event.encoded()?,
            ])?;
        }
    }
    transaction.commit()?;
    Ok(())
}

enum Query {
    All,
    Time { start: u64, end: u64 },
    Trace([u8; 16]),
}

fn query_sqlite(connections: &[Connection], query: Query) -> EvalResult<QueryResult> {
    let mut result = QueryResult::default();
    for connection in connections {
        let (sql, values): (&str, Vec<rusqlite::types::Value>) = match query {
            Query::All => (
                "SELECT body FROM events ORDER BY recorded_at, event_id",
                vec![],
            ),
            Query::Time { start, end } => (
                "SELECT body FROM events
                 WHERE recorded_at >= ?1 AND recorded_at < ?2
                 ORDER BY recorded_at, event_id",
                vec![as_sql_integer(start)?.into(), as_sql_integer(end)?.into()],
            ),
            Query::Trace(trace_id) => (
                "SELECT body FROM events
                 WHERE trace_id = ?1 ORDER BY recorded_at, event_id",
                vec![trace_id.to_vec().into()],
            ),
        };
        let mut statement = connection.prepare_cached(sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        while let Some(row) = rows.next()? {
            let body: Vec<u8> = row.get(0)?;
            result.observe(&body);
        }
    }
    Ok(result)
}

fn as_sql_integer(value: u64) -> EvalResult<i64> {
    i64::try_from(value).map_err(|_| "fixture integer exceeded SQLite i64".into())
}

fn sqlite_paths(root: &Path, producers: usize) -> Vec<PathBuf> {
    (0..producers)
        .map(|producer| root.join(format!("events-{producer:04}.sqlite3")))
        .collect()
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
    fn rollback_and_duplicate_retry_preserve_one_logical_event() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sqlite3");
        create_sqlite(&path).unwrap();
        let events = crate::model::fixture_events(2, 1, 32);
        let mut connection = open_sqlite(&path).unwrap();
        insert_transaction(&mut connection, &events[..1]).unwrap();
        {
            let transaction = connection.transaction().unwrap();
            let body = events[1].encoded().unwrap();
            transaction
                .execute(
                    "INSERT INTO events
                     (event_id, recorded_at, trace_id, family, producer, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        events[1].event_id.as_slice(),
                        events[1].timestamp_micros as i64,
                        events[1].trace_id.as_slice(),
                        events[1].family,
                        events[1].producer,
                        body,
                    ],
                )
                .unwrap();
        }
        drop(connection);

        let mut reopened = open_sqlite(&path).unwrap();
        insert_transaction(&mut reopened, &events[..1]).unwrap();
        let count: i64 = reopened
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn missing_and_corrupt_partitions_do_not_hide_healthy_partition() {
        let directory = tempfile::tempdir().unwrap();
        let healthy = directory.path().join("healthy.sqlite3");
        let corrupt = directory.path().join("corrupt.sqlite3");
        let missing = directory.path().join("missing.sqlite3");
        create_sqlite(&healthy).unwrap();
        let events = crate::model::fixture_events(1, 1, 32);
        insert_transaction(&mut open_sqlite(&healthy).unwrap(), &events).unwrap();
        std::fs::write(&corrupt, b"not a sqlite database").unwrap();

        let mut readable_records = 0;
        let mut issues = Vec::new();
        for path in [&healthy, &corrupt, &missing] {
            if !path.exists() {
                issues.push("missing_partition");
                continue;
            }
            match open_sqlite_read_only(path)
                .and_then(|connection| query_sqlite(&[connection], Query::All))
            {
                Ok(result) => readable_records += result.records,
                Err(_) => issues.push("corrupt_partition"),
            }
        }
        assert_eq!(readable_records, 1);
        assert_eq!(issues, vec!["corrupt_partition", "missing_partition"]);
    }
}
