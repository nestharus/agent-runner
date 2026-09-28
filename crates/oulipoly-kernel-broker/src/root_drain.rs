//! Bounded broker inventory for an exact fenced root. PID1 terminal proof is
//! narrower than State/sidecar close or caller publication.

use crate::accepted_grant::{GrantRecord, GrantRegistry};
use crate::entry_registry::{EntryRegistry, ProcessStamp};
use crate::identity::{ChildExit, observed_incarnation_gone};
use crate::registry::{OwnerCloseIntent, RootRecord, RootRegistry};
use crate::source_physical::SourcePhysicalRegistry;
use crate::work_registry::{LiveWork, WorkRecord, WorkRegistry};
use oulipoly_state::mailbox::{
    BrokerOwnerCloseInventory, BrokerSidecar, BrokerSourceEffectObligations, FreshV30Lane,
    PreparedProcessStamp,
};
use serde::Serialize;
use std::io;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DrainState {
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct RootDrainInventory {
    pub root_id: String,
    pub fenced: bool,
    pub state: DrainState,
    pub pid1: &'static str,
    /// Pinned live incarnation, independent of a parent wait peek.
    pub pid1_exact_live: bool,
    /// Exact durable PID1 self-report that waitpid(-1) reached ECHILD.
    /// It does not prove the PID1 incarnation has exited.
    pub pid1_echild_receipt: bool,
    /// PID1's exact ECHILD receipt plus absence of its recorded incarnation.
    /// This does not include an authoritative parent wait receipt.
    pub pid1_terminal_proof: bool,
    /// Exact parent wait persisted by PID1's stable parent across Broker
    /// restart.
    pub pid1_parent_wait_proof: bool,
    /// Consumed J with the exact original joined child, guardian and driver
    /// bound to the one retained physical source. Its seal is checked below.
    pub entry_physical_settled: bool,
    /// The exact original Runner entry incarnation has ended. PID1 cannot
    /// reach ECHILD while that direct child remains live.
    pub entry_original_exited: bool,
    pub entry_unsettled: bool,
    pub prepared_grants: usize,
    pub spent_without_work: usize,
    pub live_works: usize,
    pub work_debt: usize,
    /// Total retained child work records, including sealed retirements.
    pub work_records: usize,
    /// Child seals revalidated against Q, selected K, State and both ACKs.
    pub work_retired: usize,
    /// Unsealed or changed child work remains a drain obligation.
    pub work_outstanding: usize,
    pub native_prepared: usize,
    pub native_spent: usize,
    /// Total retained source records, including sealed retirements.
    pub source_physical_records: usize,
    /// Seals whose exact physical Q still revalidates on this readback.
    pub source_physical_retired: usize,
    /// Unsealed or changed source Q records that still block drain.
    pub source_physical_outstanding: usize,
    /// Positive debt from the retained broker sidecar only. `None` means its
    /// exact owner/incarnation could not be read, including an absent sidecar.
    pub source_effect: Option<BrokerSourceEffectObligations>,
    pub source_effect_readback_uncertain: bool,
    /// Exact State/retained-sidecar debt preview for the released owner. The
    /// fresh lane and a State writer fence are still required for close.
    pub owner_close_inventory: Option<BrokerOwnerCloseInventory>,
    pub uncertain_registry_or_incarnation: bool,
    pub state_sidecar_outstanding_unknown: bool,
    /// Read-only close preflight. It binds this root's physical Q and the
    /// observed State/sidecar debt; it does not hold their writer fences.
    pub owner_close_preflight: bool,
    /// Exact durable intent, if issued. It does not close or release the owner.
    pub owner_close_intent: Option<OwnerCloseIntent>,
    /// No joined State/sidecar writer fence or atomic close is supplied by
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
    source_effect_unsettled: usize,
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
        || source_effect_unsettled > 0
    {
        DrainState::Blocked
    } else {
        // State, source-admission and old WAL are not certified here.
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
    sidecar: Option<&BrokerSidecar>,
) -> io::Result<RootDrainInventory> {
    roots.exact_record(expected)?;
    let stamp = ProcessStamp {
        host_pid: expected.init_host_pid,
        boot_id: expected.boot_id.clone(),
        starttime_ticks: expected.init_starttime_ticks,
        pidns_dev: expected.pidns_dev,
        pidns_ino: expected.pidns_ino,
    };
    let source_effect = sidecar.and_then(|sidecar| {
        let root_init = PreparedProcessStamp {
            host_pid: expected.init_host_pid,
            boot_id: expected.boot_id.clone(),
            starttime_ticks: expected.init_starttime_ticks,
            pidns_dev: expected.pidns_dev,
            pidns_ino: expected.pidns_ino,
        };
        sidecar
            .read_root_source_effect_obligations(&expected.root_id, &root_init)
            .ok()
    });
    let source_effect_readback_uncertain = source_effect.is_none();
    let owner_close_inventory = sidecar.and_then(|sidecar| {
        let root_init = PreparedProcessStamp {
            host_pid: expected.init_host_pid,
            boot_id: expected.boot_id.clone(),
            starttime_ticks: expected.init_starttime_ticks,
            pidns_dev: expected.pidns_dev,
            pidns_ino: expected.pidns_ino,
        };
        sidecar
            .read_root_owner_close_inventory(&expected.root_id, &root_init)
            .ok()
    });
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
    let lane = FreshV30Lane::open_at(works.broker_root()?).ok();
    let terminal_dir = works.broker_root()?.join("terminals");
    let mut work_retired = 0;
    let mut work_outstanding = 0;
    let mut live_works = 0;
    let mut work_debt = 0;
    let mut work_retirement_uncertain = false;
    for work in root_live
        .iter()
        .map(|work| &work.record)
        .chain(root_debt.iter().copied())
    {
        let result = root_grants
            .iter()
            .find(|grant| work.accepted_grant_id.as_deref() == Some(grant.grant_id.as_str()))
            .ok_or_else(|| io::Error::other("work grant absent"))
            .and_then(|grant| {
                works.retirement(
                    expected,
                    roots,
                    grants,
                    sources,
                    sidecar,
                    lane.as_ref(),
                    grant,
                    &terminal_dir,
                )
            });
        match result {
            Ok(Some(_)) => work_retired += 1,
            Ok(None) => {
                work_outstanding += 1;
                if root_live
                    .iter()
                    .any(|live| live.record.work_incarnation == work.work_incarnation)
                {
                    live_works += 1;
                }
                if root_debt
                    .iter()
                    .any(|debt| debt.work_incarnation == work.work_incarnation)
                {
                    work_debt += 1;
                }
            }
            Err(_) => {
                work_outstanding += 1;
                work_retirement_uncertain = true;
                if root_live
                    .iter()
                    .any(|live| live.record.work_incarnation == work.work_incarnation)
                {
                    live_works += 1;
                }
                if root_debt
                    .iter()
                    .any(|debt| debt.work_incarnation == work.work_incarnation)
                {
                    work_debt += 1;
                }
            }
        }
    }
    let source_records: Vec<_> = sources
        .records()
        .iter()
        .filter(|source| source.grant.root_id == expected.root_id)
        .collect();
    let mut source_physical_retired = 0;
    let mut source_physical_outstanding = 0;
    let mut source_retirement_uncertain = false;
    for source in &source_records {
        match sources.retirement(&source.grant.grant_id) {
            Ok(Some(_)) => source_physical_retired += 1,
            Ok(None) => source_physical_outstanding += 1,
            Err(_) => {
                source_physical_outstanding += 1;
                source_retirement_uncertain = true;
            }
        }
    }
    let entry = entries.record(&expected.root_id);
    let entry_unsettled = entry.is_none_or(|entry| entry.terminal_settlement.is_none());
    let entry_physical_settled = entry.is_some_and(|entry| {
        entry.join_consumed
            && source_records.len() == 1
            && entry.joined_child.as_ref() == Some(&source_records[0].joined_child)
            && entry.prepared_driver.as_ref() == Some(&source_records[0].driver)
            && entry.guardian.as_ref() == Some(&source_records[0].guardian)
    });
    let entry_original_exited = entry
        .and_then(|entry| entry.joined_child.as_ref())
        .is_some_and(|actor| {
            observed_incarnation_gone(
                actor.host_pid,
                &actor.boot_id,
                actor.starttime_ticks,
                (actor.pidns_dev, actor.pidns_ino),
            )
            .unwrap_or(false)
        });
    let pid1_receipt = roots.pid1_echild_receipt(expected);
    let uncertain_registry_or_incarnation = pid1_receipt.is_err()
        || roots.has_unrelated_debt(expected)
        || works.has_uncertain_write()
        || work_retirement_uncertain
        || grants.has_debt()
        || sources.has_debt()
        || source_retirement_uncertain
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
    let pid1_terminal_proof = roots.pid1_terminal_proof(expected).unwrap_or(false);
    let pid1_echild_receipt = pid1_receipt.unwrap_or(false);
    let pid1_exact_live = roots.pid1_exact_live(expected).unwrap_or(false);
    let pid1_parent_wait_proof = roots.pid1_parent_wait_proof(expected).unwrap_or(false);
    let pid1 = if pid1_terminal_proof {
        "terminal_echild_absent"
    } else {
        match roots.observe_init_exit(expected) {
            Ok(ChildExit::Running) => "live",
            Ok(ChildExit::ExitedZero) => "exited_zero_unreaped",
            Ok(ChildExit::ExitedAbnormally) => "abnormal_exit",
            Err(_) => "unknown",
        }
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
    let mut inventory = RootDrainInventory {
        root_id: expected.root_id.clone(),
        fenced: roots.admission_fenced(&expected.root_id),
        state: classify_counts(
            pid1,
            entry_unsettled,
            prepared_grants,
            spent_without_work,
            live_works,
            work_debt,
            native_prepared,
            native_spent,
            source_physical_outstanding,
            source_effect
                .as_ref()
                .map_or(0, BrokerSourceEffectObligations::unsettled),
        ),
        pid1,
        pid1_exact_live,
        pid1_echild_receipt,
        pid1_terminal_proof,
        pid1_parent_wait_proof,
        entry_physical_settled,
        entry_original_exited,
        entry_unsettled,
        prepared_grants,
        spent_without_work,
        live_works,
        work_debt,
        work_records: root_live.len() + root_debt.len(),
        work_retired,
        work_outstanding,
        native_prepared,
        native_spent,
        source_physical_records: source_records.len(),
        source_physical_retired,
        source_physical_outstanding,
        source_effect,
        source_effect_readback_uncertain,
        owner_close_inventory,
        uncertain_registry_or_incarnation,
        state_sidecar_outstanding_unknown: true,
        owner_close_preflight: false,
        owner_close_intent: roots.read_close_intent(expected)?,
        close_eligible: false,
    };
    if let Some(owner_generation) = inventory
        .owner_close_inventory
        .as_ref()
        .map(|owner| owner.owner_generation.clone())
    {
        inventory.owner_close_preflight =
            ready_for_owner_close_preflight(&inventory, &expected.root_id, &owner_generation)
                .is_ok();
    }
    Ok(inventory)
}

/// A fail-closed, read-only prerequisite for the one expected root and owner.
/// A close writer must hold/revalidate the durable admission intent and
/// repeat every check under joined State/sidecar writer fences; this result
/// never authorizes release.
pub fn ready_for_owner_close_preflight(
    inventory: &RootDrainInventory,
    expected_root_id: &str,
    expected_owner_generation: &str,
) -> io::Result<()> {
    let owner = inventory
        .owner_close_inventory
        .as_ref()
        .ok_or_else(|| io::Error::other("owner close sidecar inventory absent"))?;
    let cursor = owner
        .state_cursor
        .as_ref()
        .ok_or_else(|| io::Error::other("owner close State cursor absent"))?;
    if inventory.root_id != expected_root_id
        || owner.root_id != expected_root_id
        || owner.owner_generation != expected_owner_generation
        || owner.source_generation.is_empty()
        || cursor.authority_ordinal <= 0
        || cursor.admission_id.is_empty()
        || cursor.continuity_digest.len() != 64
        || !cursor
            .continuity_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || cursor.sidecar_generation.is_empty()
        || !inventory.fenced
        || inventory.pid1 != "terminal_echild_absent"
        || inventory.pid1_exact_live
        || !inventory.pid1_echild_receipt
        || !inventory.pid1_terminal_proof
        || !inventory.pid1_parent_wait_proof
        || !inventory.entry_physical_settled
        || !inventory.entry_original_exited
        // Entry terminal_settlement is the later caller-publication identity,
        // not an owner-side physical or notification duty.
        || inventory.prepared_grants != 0
        || inventory.spent_without_work != 0
        || inventory.live_works != 0
        || inventory.work_debt != 0
        || inventory.work_records == 0
        || inventory.work_retired != inventory.work_records
        || inventory.work_outstanding != 0
        || inventory.native_prepared != 0
        || inventory.native_spent != 0
        || inventory.source_physical_records != 1
        || inventory.source_physical_retired != 1
        || inventory.source_physical_outstanding != 0
        || inventory.source_effect_readback_uncertain
        || inventory.uncertain_registry_or_incarnation
        || inventory.source_effect.as_ref() != Some(&owner.source_effect)
        || owner.source_effect.unsettled() != 0
        || owner.source_effect.accepted != 1
        || owner.state_projection_pending
        || owner.state_native_channel_pending
        || owner.state_cancelling_native_attempts != 0
        || owner.registered_sources != 0
        || owner.retiring_listeners != 0
        || owner.deliverable_mailbox_rows != 0
        || owner.unresolved_attempts != 0
        || owner.native_grants_without_q != 0
    {
        return Err(io::Error::other(
            "exact root/owner close preflight evidence absent or changed",
        ));
    }
    Ok(())
}

/// An explicit host-root drain request is allowed only after an exact,
/// fenced readback revalidates every retained source and child work seal.
/// Zero records and an empty count alone cannot authorize PID1 exit.
pub fn ready_for_pid1_request(inventory: &RootDrainInventory) -> io::Result<()> {
    if !inventory.fenced
        || !inventory.entry_physical_settled
        || !inventory.entry_original_exited
        || inventory.prepared_grants != 0
        || inventory.spent_without_work != 0
        || inventory.live_works != 0
        || inventory.work_debt != 0
        || inventory.work_records == 0
        || inventory.work_outstanding != 0
        || inventory.work_retired != inventory.work_records
        || inventory.native_prepared != 0
        || inventory.native_spent != 0
        || inventory.source_physical_records == 0
        || inventory.source_physical_outstanding != 0
        || inventory.source_physical_retired != inventory.source_physical_records
        || inventory.source_effect_readback_uncertain
        || inventory
            .source_effect
            .as_ref()
            .is_none_or(|effect| effect.unsettled() != 0)
        || inventory.uncertain_registry_or_incarnation
        || !inventory.pid1_exact_live
    {
        return Err(io::Error::other(
            "exact root PID1 drain prerequisites absent or changed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::{BoundStateFileIdentity, BrokerStateCloseCursor};

    fn settled_owner_inventory() -> RootDrainInventory {
        let effect = BrokerSourceEffectObligations {
            accepted: 1,
            ..Default::default()
        };
        RootDrainInventory {
            root_id: "root".into(),
            fenced: true,
            state: DrainState::Blocked,
            pid1: "terminal_echild_absent",
            pid1_exact_live: false,
            pid1_echild_receipt: true,
            pid1_terminal_proof: true,
            pid1_parent_wait_proof: true,
            entry_physical_settled: true,
            entry_original_exited: true,
            entry_unsettled: true,
            prepared_grants: 0,
            spent_without_work: 0,
            live_works: 0,
            work_debt: 0,
            work_records: 1,
            work_retired: 1,
            work_outstanding: 0,
            native_prepared: 0,
            native_spent: 0,
            source_physical_records: 1,
            source_physical_retired: 1,
            source_physical_outstanding: 0,
            source_effect: Some(effect.clone()),
            source_effect_readback_uncertain: false,
            owner_close_inventory: Some(BrokerOwnerCloseInventory {
                source_generation: "source".into(),
                root_id: "root".into(),
                owner_generation: "owner".into(),
                state_cursor: Some(BrokerStateCloseCursor {
                    file: BoundStateFileIdentity {
                        device: 1,
                        inode: 2,
                    },
                    authority_ordinal: 2,
                    admission_id: "admission".into(),
                    sidecar_generation: "sidecar".into(),
                    continuity_digest: "a".repeat(64),
                }),
                state_projection_pending: false,
                state_native_channel_pending: false,
                state_cancelling_native_attempts: 0,
                registered_sources: 0,
                retiring_listeners: 0,
                deliverable_mailbox_rows: 0,
                unresolved_attempts: 0,
                native_grants_without_q: 0,
                source_effect: effect,
            }),
            uncertain_registry_or_incarnation: false,
            state_sidecar_outstanding_unknown: true,
            owner_close_preflight: false,
            owner_close_intent: None,
            close_eligible: false,
        }
    }

    #[test]
    fn close_preflight_binds_root_owner_cursor_and_physical_q_without_claiming_close() {
        let settled = settled_owner_inventory();
        assert!(ready_for_owner_close_preflight(&settled, "root", "owner").is_ok());
        assert!(!settled.close_eligible);
        assert!(ready_for_owner_close_preflight(&settled, "other", "owner").is_err());
        assert!(ready_for_owner_close_preflight(&settled, "root", "stale").is_err());

        let mut changed = settled.clone();
        changed.pid1_parent_wait_proof = false;
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        changed = settled.clone();
        changed.work_outstanding = 1;
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        changed = settled.clone();
        changed.owner_close_inventory.as_mut().unwrap().state_cursor = None;
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        changed = settled.clone();
        changed
            .owner_close_inventory
            .as_mut()
            .unwrap()
            .state_projection_pending = true;
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        changed = settled.clone();
        changed
            .owner_close_inventory
            .as_mut()
            .unwrap()
            .deliverable_mailbox_rows = 1;
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        changed = settled.clone();
        changed
            .owner_close_inventory
            .as_mut()
            .unwrap()
            .native_grants_without_q = 1;
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        changed = settled;
        changed
            .owner_close_inventory
            .as_mut()
            .unwrap()
            .owner_generation = "other".into();
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
    }

    #[test]
    fn accepted_pre_fork_and_adopted_work_never_look_empty() {
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 1, 0, 0, 0, 0, 0, 0),
            DrainState::Blocked
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 1, 0, 0, 0, 0, 0),
            DrainState::Blocked
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 0, 1, 0, 0, 0, 0),
            DrainState::Blocked
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 0, 0, 0, 1, 0, 0),
            DrainState::Blocked
        );
    }

    #[test]
    fn zero_visible_records_and_restart_uncertainty_do_not_certify_close() {
        assert_eq!(
            classify_counts("unknown", false, 0, 0, 0, 0, 0, 0, 0, 0),
            DrainState::Unknown
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 0, 0, 0, 0, 0, 0),
            DrainState::Unknown
        );
        assert_eq!(
            classify_counts("live", false, 0, 0, 0, 0, 0, 0, 0, 0),
            DrainState::Blocked
        );
        assert_eq!(
            classify_counts("exited_zero_unreaped", false, 0, 0, 0, 0, 0, 0, 0, 1),
            DrainState::Blocked
        );
    }
}
