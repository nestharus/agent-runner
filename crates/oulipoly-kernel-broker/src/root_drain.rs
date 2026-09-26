//! Bounded broker inventory for an exact fenced root. This is deliberately a
//! preparatory readback: no combination of these counts closes PID1 or State.

use crate::accepted_grant::{GrantRecord, GrantRegistry};
use crate::entry_registry::{EntryRegistry, ProcessStamp};
use crate::identity::ChildExit;
use crate::registry::{RootRecord, RootRegistry};
use crate::source_physical::SourcePhysicalRegistry;
use crate::work_registry::{LiveWork, WorkRecord, WorkRegistry};
use serde::Serialize;
use std::io;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DrainState {
    Blocked,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct RootDrainInventory {
    pub root_id: String,
    pub fenced: bool,
    pub state: DrainState,
    pub pid1: &'static str,
    pub entry_unsettled: bool,
    pub prepared_grants: usize,
    pub spent_without_work: usize,
    pub live_works: usize,
    pub work_debt: usize,
    pub native_prepared: usize,
    pub native_spent: usize,
    pub source_physical_records: usize,
    pub uncertain_registry_or_incarnation: bool,
    pub state_sidecar_outstanding_unknown: bool,
    /// No State/sidecar atomic close or source-admission proof is supplied by
    /// this inventory. An empty count is never a drain certificate.
    pub close_eligible: bool,
}

fn classify_counts(
    pid1: &str,
    entry_unsettled: bool,
    prepared_grants: usize,
    spent_without_work: usize,
    live_works: usize,
    work_debt: usize,
    native_prepared: usize,
    native_spent: usize,
    source_physical_records: usize,
) -> DrainState {
    if pid1 == "live"
        || entry_unsettled
        || prepared_grants > 0
        || spent_without_work > 0
        || live_works > 0
        || work_debt > 0
        || native_prepared > 0
        || native_spent > 0
        || source_physical_records > 0
    {
        DrainState::Blocked
    } else {
        // State, source-admission, old WAL and PID1 ECHILD are not certified.
        DrainState::Unknown
    }
}

pub(crate) fn spent_without_work(
    grants: &[&GrantRecord],
    live: &[&LiveWork],
    debt: &[&WorkRecord],
) -> usize {
    grants
        .iter()
        .filter(|grant| {
            grant.consumed
                && !live.iter().any(|work| {
                    work.record.accepted_grant_id.as_deref() == Some(grant.grant_id.as_str())
                })
                && !debt
                    .iter()
                    .any(|work| work.accepted_grant_id.as_deref() == Some(grant.grant_id.as_str()))
        })
        .count()
}

pub fn readback(
    expected: &RootRecord,
    roots: &RootRegistry,
    entries: &EntryRegistry,
    works: &WorkRegistry,
    grants: &GrantRegistry,
    sources: &SourcePhysicalRegistry,
) -> io::Result<RootDrainInventory> {
    roots.exact_record(expected)?;
    let stamp = ProcessStamp {
        host_pid: expected.init_host_pid,
        boot_id: expected.boot_id.clone(),
        starttime_ticks: expected.init_starttime_ticks,
        pidns_dev: expected.pidns_dev,
        pidns_ino: expected.pidns_ino,
    };
    let root_grants: Vec<_> = grants
        .records()
        .iter()
        .filter(|grant| grant.root_id == expected.root_id)
        .collect();
    let root_native: Vec<_> = grants
        .native_records()
        .iter()
        .filter(|grant| grant.root_id == expected.root_id)
        .collect();
    let root_live: Vec<_> = works
        .live_works()
        .filter(|work| work.record.root_id == expected.root_id)
        .collect();
    let root_debt: Vec<_> = works
        .debt_records()
        .iter()
        .filter(|work| work.root_id == expected.root_id)
        .collect();
    let source_records: Vec<_> = sources
        .records()
        .iter()
        .filter(|source| source.grant.root_id == expected.root_id)
        .collect();
    let entry = entries.record(&expected.root_id);
    let entry_unsettled = entry.is_none_or(|entry| entry.terminal_settlement.is_none());
    let uncertain_registry_or_incarnation = roots.has_debt()
        || works.has_debt()
        || grants.has_debt()
        || sources.has_debt()
        || entries.has_uncertain_write()
        || entry.is_none()
        || root_grants.iter().any(|grant| grant.root_init != stamp)
        || root_native.iter().any(|grant| grant.root_init != stamp)
        || root_live.iter().any(|work| {
            work.record.root_init_host_pid != expected.init_host_pid
                || work.record.root_init_starttime_ticks != expected.init_starttime_ticks
                || work.record.root_pidns_dev != expected.pidns_dev
                || work.record.root_pidns_ino != expected.pidns_ino
        })
        || root_debt.iter().any(|work| {
            work.root_init_host_pid != expected.init_host_pid
                || work.root_init_starttime_ticks != expected.init_starttime_ticks
                || work.root_pidns_dev != expected.pidns_dev
                || work.root_pidns_ino != expected.pidns_ino
        })
        || source_records
            .iter()
            .any(|source| source.root_init != stamp);
    let pid1 = match roots.observe_init_exit(expected) {
        Ok(ChildExit::Running) => "live",
        Ok(ChildExit::ExitedZero) => "exited_zero_unreaped",
        Ok(ChildExit::ExitedAbnormally) => "abnormal_exit",
        Err(_) => "unknown",
    };
    let prepared_grants = root_grants.iter().filter(|grant| !grant.consumed).count();
    let spent_without_work = spent_without_work(&root_grants, &root_live, &root_debt);
    let native_prepared = root_native
        .iter()
        .filter(|grant| grant.state == "prepared")
        .count();
    let native_spent = root_native
        .iter()
        .filter(|grant| grant.state == "consumed")
        .count();
    Ok(RootDrainInventory {
        root_id: expected.root_id.clone(),
        fenced: roots.admission_fenced(&expected.root_id),
        state: classify_counts(
            pid1,
            entry_unsettled,
            prepared_grants,
            spent_without_work,
            root_live.len(),
            root_debt.len(),
            native_prepared,
            native_spent,
            source_records.len(),
        ),
        pid1,
        entry_unsettled,
        prepared_grants,
        spent_without_work,
        live_works: root_live.len(),
        work_debt: root_debt.len(),
        native_prepared,
        native_spent,
        source_physical_records: source_records.len(),
        uncertain_registry_or_incarnation,
        state_sidecar_outstanding_unknown: true,
        close_eligible: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_pre_fork_and_adopted_work_never_look_empty() {
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 1, 0, 0, 0, 0, 0),
            DrainState::Blocked
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 1, 0, 0, 0, 0),
            DrainState::Blocked
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 0, 1, 0, 0, 0),
            DrainState::Blocked
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 0, 0, 0, 1, 0),
            DrainState::Blocked
        );
    }

    #[test]
    fn zero_visible_records_and_restart_uncertainty_do_not_certify_close() {
        assert_eq!(
            classify_counts("unknown", false, 0, 0, 0, 0, 0, 0, 0),
            DrainState::Unknown
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 0, 0, 0, 0, 0),
            DrainState::Unknown
        );
        assert_eq!(
            classify_counts("live", false, 0, 0, 0, 0, 0, 0, 0),
            DrainState::Blocked
        );
    }
}
