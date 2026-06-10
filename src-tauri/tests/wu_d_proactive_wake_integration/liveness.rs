//! ## Declared roles
//!
//! Roles: predicate, filter.
//!
//! TEST: liveness polling, sidecar-session filters, and file-delivery
//! predicates for proactive wake integration cases.

use crate::fixtures::Fixture;
use oulipoly_state::mailbox::WAKE_SWEEP_ABANDONED_ERROR;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) fn wait_for_file(path: &Path) -> String {
    wait_until(&format!("{} exists", path.display()), || path.exists());
    fs::read_to_string(path).unwrap()
}

pub(crate) fn wait_for_mailbox_session(fixture: &Fixture) -> String {
    wait_for_sidecar_session(fixture, "mailbox")
}

pub(crate) fn wait_for_runtime_session(fixture: &Fixture) -> String {
    wait_for_sidecar_session(fixture, "session_runtime")
}

pub(crate) fn wait_for_sidecar_session(fixture: &Fixture, table: &str) -> String {
    let mut found = None;
    wait_until(&format!("{table} session id"), || {
        found = sidecar_session_id(fixture, table);
        found.is_some()
    });
    found.unwrap()
}

pub(crate) fn sidecar_session_id(fixture: &Fixture, table: &str) -> Option<String> {
    let conn = fixture.sidecar_conn();
    conn.query_row(
        &format!("SELECT session_id FROM {table} ORDER BY session_id LIMIT 1"),
        [],
        |row| row.get(0),
    )
    .ok()
}

pub(crate) fn wait_until(label: &str, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {label}");
}

pub(crate) fn settle_wake_sweep() {
    std::thread::sleep(Duration::from_millis(300));
}

pub(crate) fn runtime_is_idle(fixture: &Fixture, session_id: &str) -> bool {
    fixture
        .mailbox()
        .session_runtime(session_id)
        .unwrap()
        .is_some_and(|row| row.run_state == "idle")
}

pub(crate) fn delivered_rows_without_claim(
    fixture: &Fixture,
    session_id: &str,
    expected_len: usize,
) -> bool {
    let db = fixture.mailbox();
    let rows = db.list_mailbox(session_id, true).unwrap();
    rows.len() == expected_len
        && rows.iter().all(|row| row.delivered_at.is_some())
        && db.wake_claim(session_id).unwrap().is_none()
}

pub(crate) fn delivered_rows_without_pending_or_claim(
    fixture: &Fixture,
    session_id: &str,
    expected_len: usize,
) -> bool {
    let db = fixture.mailbox();
    let rows = db.list_mailbox(session_id, true).unwrap();
    rows.len() == expected_len
        && rows.iter().all(|row| row.delivered_at.is_some())
        && db.list_pending(session_id).unwrap().is_empty()
        && db.wake_claim(session_id).unwrap().is_none()
}

pub(crate) fn delivered_single_row_without_error_or_claim(
    fixture: &Fixture,
    session_id: &str,
) -> bool {
    let db = fixture.mailbox();
    let rows = db.list_mailbox(session_id, true).unwrap();
    rows.len() == 1
        && rows[0].delivered_at.is_some()
        && rows[0].delivery_error.is_none()
        && db.wake_claim(session_id).unwrap().is_none()
}

pub(crate) fn auto_wake_cap_left_pending(fixture: &Fixture, session_id: &str) -> bool {
    let db = fixture.mailbox();
    let pending = db.list_pending(session_id).unwrap();
    pending.len() == 1
        && pending[0].handle == "h-auto-2"
        && db.wake_claim(session_id).unwrap().is_none()
        && db
            .session_runtime(session_id)
            .unwrap()
            .is_some_and(|row| row.auto_wake_count == 2)
}

pub(crate) fn captured_opencode_mailbox_delivered(fixture: &Fixture, session_id: &str) -> bool {
    let db = fixture.mailbox();
    let rows = db.list_mailbox(session_id, true).unwrap();
    rows.len() == 1
        && rows[0].handle == "h-capture-midturn"
        && rows[0].delivered_at.is_some()
        && rows[0].owner_invocation_uuid.is_some()
        && rows[0].matched_os_pid.is_some()
        && rows[0].matched_chain_index == Some(0)
        && db.wake_claim(session_id).unwrap().is_none()
}

pub(crate) fn shadow_xdg_mailbox_delivered(fixture: &Fixture, session_id: &str) -> bool {
    let db = fixture.mailbox();
    let rows = db.list_mailbox(session_id, true).unwrap();
    rows.len() == 1
        && rows[0].handle == "h-shadow-xdg"
        && rows[0].delivered_at.is_some()
        && db.wake_claim(session_id).unwrap().is_none()
}

pub(crate) fn newer_mailbox_delivered_with_exhausted_old_pending(fixture: &Fixture) -> bool {
    let rows = fixture
        .mailbox()
        .list_mailbox(crate::SESSION, true)
        .unwrap();
    let old = rows.iter().find(|row| row.handle == "h-unconfirmed-old");
    let newer = rows.iter().find(|row| row.handle == "h-newer");
    old.is_some_and(|row| row.delivered_at.is_none() && row.delivery_attempts == 2)
        && newer.is_some_and(|row| row.delivered_at.is_some())
}

pub(crate) fn backlog_recovered_and_debris_reaped(
    fixture: &Fixture,
    idle_session: &str,
    recent_session: &str,
    dead_sessions: &[String],
) -> bool {
    let db = fixture.mailbox();
    let idle_rows = db.list_mailbox(idle_session, true).unwrap();
    let recent_rows = db.list_mailbox(recent_session, true).unwrap();
    let recovered = idle_rows.len() == 1
        && idle_rows[0].delivered_at.is_some()
        && recent_rows.len() == 1
        && recent_rows[0].delivered_at.is_some();
    let debris_reaped = dead_sessions.iter().all(|session_id| {
        let rows = db.list_mailbox(session_id, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_none()
            && rows[0].delivery_error.as_deref() == Some(WAKE_SWEEP_ABANDONED_ERROR)
            && db.wake_claim(session_id).unwrap().is_none()
    });
    recovered && debris_reaped
}
