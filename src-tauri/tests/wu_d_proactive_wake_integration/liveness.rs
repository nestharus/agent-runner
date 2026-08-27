//! ## Declared roles
//!
//! Roles: predicate, filter.
//!
//! TEST: liveness polling, sidecar-session filters, and file-delivery
//! predicates for proactive wake integration cases.

use crate::fixtures::Fixture;
use oulipoly_state::mailbox::{MailboxRow, SessionGenerationProjection, WakeClaimRow};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

pub(crate) fn wait_for_file(path: &Path) -> String {
    wait_until(&format!("{} exists", path.display()), || path.exists());
    fs::read_to_string(path).unwrap()
}

pub(crate) fn wait_for_runtime_session(fixture: &Fixture) -> String {
    wait_for_sidecar_session(fixture, "session_runtime")
}

pub(crate) fn wait_for_sidecar_session(fixture: &Fixture, table: &str) -> String {
    wait_until(&format!("{table} session id"), || {
        sidecar_session_id_exists(fixture, table)
    });
    existing_sidecar_session_id(fixture, table)
}

pub(crate) fn sidecar_session_id(fixture: &Fixture, table: &str) -> Option<String> {
    let conn = fixture.sidecar_conn();
    conn.query_row(&sidecar_session_id_query(table), [], |row| row.get(0))
        .ok()
}

fn sidecar_session_id_query(table: &str) -> String {
    format!("SELECT session_id FROM {table} ORDER BY session_id LIMIT 1")
}

fn sidecar_session_id_exists(fixture: &Fixture, table: &str) -> bool {
    sidecar_session_id(fixture, table).is_some()
}

fn existing_sidecar_session_id(fixture: &Fixture, table: &str) -> String {
    sidecar_session_id(fixture, table).unwrap()
}

pub(crate) fn wait_until(label: &str, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    fail_wait_until(label);
}

fn fail_wait_until(label: &str) -> ! {
    panic!("timed out waiting for {label}");
}

pub(crate) fn settle_wake_sweep() {
    std::thread::sleep(Duration::from_millis(300));
}

pub(crate) fn runtime_is_idle(fixture: &Fixture, session_id: &str) -> bool {
    fixture
        .mailbox()
        .runtime_lifecycle_reader()
        .session_generation_projection(session_id)
        .is_ok_and(|projection| matches!(projection, SessionGenerationProjection::None))
}

pub(crate) fn delivered_rows_without_claim(
    fixture: &Fixture,
    session_id: &str,
    expected_len: usize,
) -> bool {
    mailbox_rows_are_delivered(&mailbox_rows(fixture, session_id), expected_len)
        && wake_claim_is_absent(&wake_claim(fixture, session_id))
}

pub(crate) fn delivered_rows_without_pending_or_claim(
    fixture: &Fixture,
    session_id: &str,
    expected_len: usize,
) -> bool {
    mailbox_rows_are_delivered(&mailbox_rows(fixture, session_id), expected_len)
        && rows_are_empty(&pending_rows(fixture, session_id))
        && wake_claim_is_absent(&wake_claim(fixture, session_id))
}

pub(crate) fn delivered_single_row_without_error_or_claim(
    fixture: &Fixture,
    session_id: &str,
) -> bool {
    single_row_is_delivered_without_error(&mailbox_rows(fixture, session_id))
        && wake_claim_is_absent(&wake_claim(fixture, session_id))
}

pub(crate) fn backlog_recovered_and_debris_retained(
    fixture: &Fixture,
    idle_session: &str,
    recent_session: &str,
    dead_sessions: &[String],
) -> bool {
    recovered_backlog_is_delivered(
        &mailbox_rows(fixture, idle_session),
        &mailbox_rows(fixture, recent_session),
    ) && wake_claim_is_absent(&wake_claim(fixture, idle_session))
        && wake_claim_is_absent(&wake_claim(fixture, recent_session))
        && dead_sessions
            .iter()
            .all(|session_id| dead_session_debris_is_retained(fixture, session_id))
}

fn mailbox_rows(fixture: &Fixture, session_id: &str) -> Vec<MailboxRow> {
    fixture.mailbox().list_mailbox(session_id, true).unwrap()
}

fn pending_rows(fixture: &Fixture, session_id: &str) -> Vec<MailboxRow> {
    fixture.mailbox().list_pending(session_id).unwrap()
}

fn wake_claim(fixture: &Fixture, session_id: &str) -> Option<WakeClaimRow> {
    fixture
        .mailbox()
        .wake_session_reader()
        .wake_claim(session_id)
        .unwrap()
}

fn mailbox_rows_are_delivered(rows: &[MailboxRow], expected_len: usize) -> bool {
    rows.len() == expected_len && rows.iter().all(mailbox_row_has_delivery)
}

fn mailbox_row_has_delivery(row: &MailboxRow) -> bool {
    row.delivered_at.is_some()
}

fn wake_claim_is_absent(claim: &Option<WakeClaimRow>) -> bool {
    claim.is_none()
}

fn rows_are_empty(rows: &[MailboxRow]) -> bool {
    rows.is_empty()
}

fn single_row_is_delivered_without_error(rows: &[MailboxRow]) -> bool {
    rows.len() == 1 && mailbox_row_has_delivery(&rows[0]) && rows[0].delivery_error.is_none()
}

fn recovered_backlog_is_delivered(idle_rows: &[MailboxRow], recent_rows: &[MailboxRow]) -> bool {
    single_mailbox_row_is_delivered(idle_rows) && single_mailbox_row_is_delivered(recent_rows)
}

fn single_mailbox_row_is_delivered(rows: &[MailboxRow]) -> bool {
    rows.len() == 1 && mailbox_row_has_delivery(&rows[0])
}

pub(crate) fn assert_dead_owner_debris_retained(fixture: &Fixture, session_id: &str) {
    assert!(
        dead_session_debris_is_retained(fixture, session_id),
        "expected non-resumable dead-owner session {session_id} to remain pending for explicit authority"
    );
}

fn dead_session_debris_is_retained(fixture: &Fixture, session_id: &str) -> bool {
    dead_session_rows_are_retained(&mailbox_rows(fixture, session_id))
}

fn dead_session_rows_are_retained(rows: &[MailboxRow]) -> bool {
    rows.len() == 1 && rows[0].delivered_at.is_none() && rows[0].delivery_error.is_none()
}
