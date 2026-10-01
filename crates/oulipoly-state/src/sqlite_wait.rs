//! Measured SQLite lock waiting for connections whose callers attribute it.
//!
//! The handler keeps SQLite's default busy back-off and the 5 second budget
//! that `rusqlite` installs on open, so waiting and `SQLITE_BUSY` outcomes are
//! unchanged; it only adds the slept time to a per-thread counter.

use std::cell::Cell;
use std::time::{Duration, Instant};

const DELAYS_MS: [u64; 12] = [1, 2, 5, 10, 15, 20, 25, 25, 25, 50, 50, 100];
const TOTALS_MS: [u64; 12] = [0, 1, 3, 8, 18, 33, 53, 78, 103, 128, 178, 228];
const TIMEOUT_MS: u64 = 5_000;

thread_local! {
    static WAIT_NS: Cell<u64> = const { Cell::new(0) };
}

fn busy(count: i32) -> bool {
    let count = count.max(0) as usize;
    let (delay, prior) = if count < DELAYS_MS.len() {
        (DELAYS_MS[count], TOTALS_MS[count])
    } else {
        (
            DELAYS_MS[DELAYS_MS.len() - 1],
            TOTALS_MS[TOTALS_MS.len() - 1] + (count as u64 - 11) * 100,
        )
    };
    let delay = if prior + delay > TIMEOUT_MS {
        TIMEOUT_MS.saturating_sub(prior)
    } else {
        delay
    };
    if delay == 0 {
        return false;
    }
    let start = Instant::now();
    std::thread::sleep(Duration::from_millis(delay));
    let slept = start.elapsed().as_nanos() as u64;
    WAIT_NS.with(|wait| wait.set(wait.get().saturating_add(slept)));
    true
}

/// Replaces the connection's busy timeout with the measured equivalent.
pub fn install(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    connection.busy_handler(Some(busy))
}

/// Returns and clears the lock wait measured on this thread.
pub fn take_thread_wait_ns() -> u64 {
    WAIT_NS.with(|wait| wait.replace(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_matches_sqlite_default_and_stops_at_budget() {
        let mut total = 0;
        for (index, delay) in DELAYS_MS.iter().enumerate() {
            assert_eq!(TOTALS_MS[index], total);
            total += delay;
        }
        let mut waited = 0;
        let mut count = 0;
        loop {
            let (delay, prior) = if count < 12 {
                (DELAYS_MS[count], TOTALS_MS[count])
            } else {
                (100, 228 + (count as u64 - 11) * 100)
            };
            let delay = if prior + delay > TIMEOUT_MS {
                TIMEOUT_MS.saturating_sub(prior)
            } else {
                delay
            };
            if delay == 0 {
                break;
            }
            waited += delay;
            count += 1;
        }
        assert_eq!(waited, TIMEOUT_MS);
    }

    #[test]
    fn locked_writer_wait_is_measured_and_still_times_out_busy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("busy.db");
        let holder = rusqlite::Connection::open(&path).unwrap();
        holder
            .execute_batch("CREATE TABLE t(x); BEGIN EXCLUSIVE; INSERT INTO t VALUES(1);")
            .unwrap();
        let waiter = rusqlite::Connection::open(&path).unwrap();
        install(&waiter).unwrap();
        let _ = take_thread_wait_ns();
        let started = Instant::now();
        let error = waiter.execute("INSERT INTO t VALUES(2)", []).unwrap_err();
        assert!(error.to_string().contains("locked"), "{error}");
        let waited = take_thread_wait_ns();
        assert!(started.elapsed() >= Duration::from_millis(4_900));
        assert!(waited >= 4_900_000_000, "{waited}");
        assert_eq!(take_thread_wait_ns(), 0);
    }
}
