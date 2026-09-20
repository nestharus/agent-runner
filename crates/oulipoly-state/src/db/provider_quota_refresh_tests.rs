//! Intent: root storage Act1 commitment 2, transaction-current learning and
//! all-or-nothing persistence. These are fixture SQLite schedules, not remote chronology.
use super::*;
use std::sync::mpsc;
use std::time::Duration;

thread_local! {
    static WRITER_WAIT: std::cell::RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
}
fn wait_for_writer(_count: i32) -> bool {
    WRITER_WAIT.with_borrow_mut(|slot| {
        let Some((entered, released)) = slot.take() else {
            return false;
        };
        entered.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(5)).unwrap();
        true
    })
}
fn window(used: f64) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent: used,
        resets_at: "2099-01-01T00:00:00Z".parse().unwrap(),
    }
}

#[test]
fn storage_refresh_waits_before_reading_learning_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = StateDb::open(&path).unwrap();
    db.upsert_quota_refresh("account", &[window(0.2)]).unwrap();
    db.conn
        .execute("UPDATE provider_quotas SET calls_since_refresh=20", [])
        .unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (released_tx, released_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let other = StateDb::open(&path).unwrap();
        WRITER_WAIT.with_borrow_mut(|slot| *slot = Some((entered_tx, released_rx)));
        other.conn.busy_handler(Some(wait_for_writer)).unwrap();
        ready_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        other.upsert_quota_refresh("account", &[window(0.4)])
    });
    ready_rx.recv().unwrap();
    db.conn
        .execute_batch(
            "BEGIN IMMEDIATE;
        UPDATE provider_quota_windows SET used_percent=0.3;
        UPDATE provider_quotas SET calls_since_refresh=40;",
        )
        .unwrap();
    go_tx.send(()).unwrap();
    let waited = entered_rx.recv_timeout(Duration::from_secs(5));
    db.conn.execute_batch("COMMIT").unwrap();
    let _ = released_tx.send(());
    let result = worker.join().unwrap();
    waited.expect("refresh did not wait for the writer before reading its baseline");
    result.unwrap();
    let rows = db.get_windows("account").unwrap();
    assert!((rows[0].last_delta_percent.unwrap() - 0.1).abs() < 1e-9);
    assert_eq!(rows[0].last_delta_calls, Some(40));
    assert_eq!(
        db.get_quota("account")
            .unwrap()
            .unwrap()
            .calls_since_refresh,
        0
    );
    assert_eq!(rows[0].used_percent, 0.4);
}

#[test]
fn storage_refresh_midwindow_fault_rolls_back_aggregate_counter_and_windows() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    db.upsert_quota_refresh("account", &[window(0.2)]).unwrap();
    db.conn
        .execute("UPDATE provider_quotas SET calls_since_refresh=40", [])
        .unwrap();
    db.conn.execute_batch("CREATE TRIGGER fail_second_window BEFORE INSERT ON provider_quota_windows WHEN NEW.window_id=1 BEGIN SELECT RAISE(ABORT, 'selected window fault'); END;").unwrap();
    let before = db.get_quota("account").unwrap().unwrap().refreshed_at;
    let error = db
        .upsert_quota_refresh("account", &[window(0.4), window(0.5)])
        .unwrap_err();
    assert!(error.contains("selected window fault"), "{error}");
    let rows = db.get_windows("account").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].used_percent, 0.2);
    assert_eq!(rows[0].last_delta_percent, None);
    assert_eq!(rows[0].last_delta_calls, None);
    let aggregate = db.get_quota("account").unwrap().unwrap();
    assert_eq!(aggregate.calls_since_refresh, 40);
    assert_eq!(aggregate.refreshed_at, before);
    assert_eq!(aggregate.topology_peak_live_window_count, 1);
}
