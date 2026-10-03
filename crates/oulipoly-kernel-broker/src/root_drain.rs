//! Bounded broker inventory for an exact fenced root. PID1 terminal proof is
//! narrower than State/sidecar close or caller publication.

use crate::accepted_grant::{GrantRecord, GrantRegistry};
use crate::entry_registry::{EntryRegistry, ProcessStamp};
use crate::identity::{ChildExit, observed_incarnation_gone};
use crate::normal_physical::{self, PhysicalReadback, PublicationReadback};
use crate::registry::{OwnerCloseIntent, RootRecord, RootRegistry};
use crate::source_physical::SourcePhysicalRegistry;
use crate::work_registry::{LiveWork, WorkRecord, WorkRegistry};
use oulipoly_state::mailbox::{
    BrokerClosedOwner, BrokerOwnerCloseInventory, BrokerSidecar, BrokerSourceEffectObligations,
    FreshRootEffect, FreshRootEffectState, FreshRootMemberSettlement, FreshRootWorkIntent,
    FreshSuccessorTerminalAck, FreshV30Lane, PreparedProcessStamp,
};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DrainState {
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RootPhysicalCloseProof {
    pub root_id: String,
    pub pid1: String,
    pub pid1_echild_receipt: bool,
    pub pid1_terminal_proof: bool,
    pub pid1_parent_wait_proof: bool,
    pub entry_physical_settled: bool,
    pub entry_original_exited: bool,
    pub work_records: usize,
    pub work_retired: usize,
    pub source_physical_records: usize,
    pub source_physical_retired: usize,
    pub source_effect: BrokerSourceEffectObligations,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_ack: Option<FreshSuccessorTerminalAck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_receipt: Option<oulipoly_state::mailbox::FreshOriginalReceiptIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal: Option<NormalRootEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline: Option<OfflineRootEvidence>,
    /// Each member of a child-set root and its own settlement. A one-child
    /// root keeps its scalar ACK or receipt above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_members: Vec<FreshRootMemberSettlement>,
}

/// The root's current child closure, read again on every inventory. A
/// root without child C has none. An unreadable closure is a refusal.
fn root_child_closure(
    lane: Option<&FreshV30Lane>,
    root_id: &str,
) -> (Option<String>, Vec<FreshRootMemberSettlement>) {
    let Some(lane) = lane else {
        return (None, Vec::new());
    };
    match lane.root_child_closure(root_id) {
        Ok(None) => (None, Vec::new()),
        Ok(Some(closure)) => (closure.refusal, closure.members),
        Err(error) => (Some(format!("child_closure_unknown: {error}")), Vec::new()),
    }
}

pub fn exact_successor_ack_for_root(
    lane: &FreshV30Lane,
    root_id: &str,
) -> io::Result<Option<FreshSuccessorTerminalAck>> {
    if !lane
        .successor_admission_exists_for_root(root_id)
        .map_err(io::Error::other)?
    {
        return Ok(None);
    }
    let (released, actor) = lane
        .released_handoff_for_root(root_id)
        .map_err(io::Error::other)?;
    let session = lane
        .read_session(&released.d_key)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("successor original D session absent"))?;
    let terminal = lane
        .read_private_root_terminal(&released, &actor, &session)
        .map_err(io::Error::other)?;
    if !terminal.children.is_empty() {
        // Per-member ACKs are named by the child-set closure, not here.
        return Ok(None);
    }
    if terminal.notification_origin != "admitted_successor"
        || terminal.notification_state != "acked"
        || terminal.ack_basis.as_deref() != Some("successor_receiver_receipt_ack")
        || terminal.successor_ack.is_none()
    {
        return Err(io::Error::other("successor terminal ACK remains unknown"));
    }
    Ok(terminal.successor_ack)
}

pub fn exact_original_receipt_for_root(
    lane: &FreshV30Lane,
    root_id: &str,
) -> io::Result<Option<oulipoly_state::mailbox::FreshOriginalReceiptIdentity>> {
    if !lane
        .original_manual_ack_exists_for_root(root_id)
        .map_err(io::Error::other)?
    {
        return Ok(None);
    }
    let (released, actor) = lane
        .released_handoff_for_root(root_id)
        .map_err(io::Error::other)?;
    let session = lane
        .read_session(&released.d_key)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("original receipt D session absent"))?;
    let terminal = lane
        .read_private_root_terminal(&released, &actor, &session)
        .map_err(io::Error::other)?;
    if !terminal.children.is_empty() {
        return Ok(None);
    }
    if terminal.notification_state != "acked" || terminal.ack_basis.as_deref() != Some("manual_ack")
    {
        return Err(io::Error::other(
            "original receiver receipt terminal unknown",
        ));
    }
    terminal
        .original_receipt
        .ok_or_else(|| io::Error::other("original receiver receipt absent"))
        .map(Some)
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NormalRootEvidence {
    pub physical: PhysicalReadback,
    pub publication: PublicationReadback,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflineRootEvidence {
    /// State's exact released U/D actor and returned CLI result. There is no
    /// provider K or Q on this route.
    pub effect: FreshRootEffect,
}

pub fn offline_no_effect(inventory: &RootDrainInventory) -> bool {
    inventory.offline_intent
        && inventory.offline.as_ref().is_some_and(|offline| {
            matches!(
                offline.effect.state,
                FreshRootEffectState::ReturnedSuccess | FreshRootEffectState::ReturnedFailure
            ) && matches!(
                offline.effect.intent,
                FreshRootWorkIntent::CliHelp(_) | FreshRootWorkIntent::CliDiagnostics(_)
            )
        })
        && !inventory.offline_uncertain
        && inventory.normal.is_none()
        && !inventory.normal_uncertain
        && inventory.grant_records == 0
        && inventory.native_records == 0
        && inventory.work_records == 0
        && inventory.source_physical_records == 0
        && inventory
            .source_effect
            .as_ref()
            .is_some_and(|effect| *effect == Default::default())
}

/// The registries recheck the exact Q and both native ACK seals on every
/// readback. These selected facts are stable across Broker restart.
pub fn physical_close_proof(inventory: &RootDrainInventory) -> io::Result<RootPhysicalCloseProof> {
    let source_effect = inventory
        .source_effect
        .as_ref()
        .ok_or_else(|| io::Error::other("owner close source effect absent"))?;
    let offline = offline_no_effect(inventory);
    if (inventory.offline_intent && !offline)
        || !inventory.fenced
        || inventory.pid1 != "terminal_echild_absent"
        || inventory.pid1_exact_live
        || !inventory.pid1_echild_receipt
        || !inventory.pid1_terminal_proof
        || !inventory.pid1_parent_wait_proof
        || !inventory.entry_physical_settled
        || !inventory.entry_original_exited
        || inventory.prepared_grants != 0
        || inventory.spent_without_work != 0
        || inventory.live_works != 0
        || inventory.work_debt != 0
        || (inventory.normal.is_none() && !offline && inventory.work_records == 0)
        || inventory.work_retired != inventory.work_records
        || inventory.work_outstanding != 0
        || inventory.native_prepared != 0
        || inventory.native_spent != 0
        || (inventory.normal.is_none() && !offline && inventory.source_physical_records != 1)
        || inventory.source_physical_retired != inventory.source_physical_records
        || inventory.source_physical_outstanding != 0
        || inventory.source_effect_readback_uncertain
        || inventory.uncertain_registry_or_incarnation
        || source_effect.unsettled() != 0
        || (inventory.normal.is_none() && !offline && source_effect.accepted != 1)
        || (inventory.normal.is_some()
            && (source_effect.accepted != 0
                || inventory.work_records != 0
                || inventory.source_physical_records != 0))
        || inventory.normal_uncertain
        || inventory.offline_uncertain
        || inventory.successor_ack_unknown
        || inventory.original_receipt_unknown
        || inventory.child_closure_refusal.is_some()
        || inventory.normal.as_ref().is_some_and(|normal| {
            normal.physical.state != "drained"
                || normal.physical.q.is_none()
                || normal.publication.state != "settled"
                || normal.publication.publication_sha256.is_none()
        })
    {
        return Err(io::Error::other("owner close physical Q/ACK proof changed"));
    }
    Ok(RootPhysicalCloseProof {
        root_id: inventory.root_id.clone(),
        pid1: inventory.pid1.into(),
        pid1_echild_receipt: inventory.pid1_echild_receipt,
        pid1_terminal_proof: inventory.pid1_terminal_proof,
        pid1_parent_wait_proof: inventory.pid1_parent_wait_proof,
        entry_physical_settled: inventory.entry_physical_settled,
        entry_original_exited: inventory.entry_original_exited,
        work_records: inventory.work_records,
        work_retired: inventory.work_retired,
        source_physical_records: inventory.source_physical_records,
        source_physical_retired: inventory.source_physical_retired,
        source_effect: source_effect.clone(),
        successor_ack: inventory.successor_ack.clone(),
        original_receipt: inventory.original_receipt.clone(),
        normal: inventory.normal.clone(),
        offline: inventory.offline.clone(),
        child_members: inventory.child_members.clone(),
    })
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
    pub grant_records: usize,
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
    pub native_records: usize,
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
    pub successor_ack_unknown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_ack: Option<FreshSuccessorTerminalAck>,
    pub original_receipt_unknown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_receipt: Option<oulipoly_state::mailbox::FreshOriginalReceiptIdentity>,
    /// Why the root's current child set does not yet accept closure: an
    /// unresolved or late C, or a notify member not yet settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_closure_refusal: Option<String>,
    /// Carried into the close proof; the readback reply names the refusal.
    #[serde(skip)]
    pub child_members: Vec<FreshRootMemberSettlement>,
    pub normal: Option<NormalRootEvidence>,
    pub normal_uncertain: bool,
    /// The released U intent selects the no-effect route even before its
    /// terminal row exists. Mixed effects cannot fall through to legacy Q.
    pub offline_intent: bool,
    pub offline: Option<OfflineRootEvidence>,
    pub offline_uncertain: bool,
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
    /// The sidecar's atomic closed phase, checked against current State and
    /// the exact retained physical Q/ACK seals on this readback.
    pub owner_close_proof: Option<BrokerClosedOwner>,
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
    let successor_ack_read = lane.as_ref().map_or(Ok(None), |lane| {
        exact_successor_ack_for_root(lane, &expected.root_id)
    });
    let successor_ack_unknown = successor_ack_read.is_err();
    let successor_ack = successor_ack_read.ok().flatten();
    let original_receipt_read = lane.as_ref().map_or(Ok(None), |lane| {
        exact_original_receipt_for_root(lane, &expected.root_id)
    });
    let original_receipt_unknown = original_receipt_read.is_err();
    let original_receipt = original_receipt_read.ok().flatten();
    let (child_closure_refusal, child_members) =
        root_child_closure(lane.as_ref(), &expected.root_id);
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
    let offline_intent = lane.as_ref().is_some_and(|lane| {
        lane.released_handoff_for_root(&expected.root_id)
            .ok()
            .is_some_and(|(handoff, _)| {
                matches!(
                    handoff.root_work_intent,
                    FreshRootWorkIntent::CliHelp(_) | FreshRootWorkIntent::CliDiagnostics(_)
                )
            })
    });
    // A normal --model root has one State K and a broker-owned Q instead of
    // source/work grants. Read it only through the exact released D/actor.
    let normal_read = (|| -> io::Result<Option<NormalRootEvidence>> {
        let Some(lane) = lane.as_ref() else {
            return Ok(None);
        };
        let Ok((handoff, actor)) = lane.released_handoff_for_root(&expected.root_id) else {
            return Ok(None);
        };
        let session = lane
            .read_session(&handoff.d_key)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("normal root session absent"))?;
        if !lane
            .normal_provider_k_present(&handoff, &actor, &session)
            .map_err(io::Error::other)?
        {
            return Ok(None);
        }
        let prepared = &handoff.old_release.prepared;
        let same = |entry: &ProcessStamp, prepared: &PreparedProcessStamp| {
            entry.host_pid == prepared.host_pid
                && entry.boot_id == prepared.boot_id
                && entry.starttime_ticks == prepared.starttime_ticks
                && entry.pidns_dev == prepared.pidns_dev
                && entry.pidns_ino == prepared.pidns_ino
        };
        if handoff.old_release.prepared.root_id != expected.root_id
            || prepared.owner_uid != expected.owner_uid
            || handoff.old_release.prepared.root_init.host_pid != expected.init_host_pid
            || handoff.old_release.prepared.root_init.boot_id != expected.boot_id
            || handoff.old_release.prepared.root_init.starttime_ticks
                != expected.init_starttime_ticks
            || prepared.root_init.pidns_dev != expected.pidns_dev
            || prepared.root_init.pidns_ino != expected.pidns_ino
            || entry.is_none_or(|entry| {
                !same(&entry.entry, &prepared.entry)
                    || entry
                        .guardian
                        .as_ref()
                        .is_none_or(|stamp| !same(stamp, &prepared.guardian))
                    || entry
                        .prepared_driver
                        .as_ref()
                        .is_none_or(|stamp| !same(stamp, &prepared.driver))
            })
            || entry
                .and_then(|entry| entry.joined_child.as_ref())
                .is_none_or(|joined| {
                    joined.host_pid != actor.host_pid
                        || joined.boot_id != actor.boot_id
                        || joined.starttime_ticks != actor.starttime_ticks
                        || joined.pidns_dev != actor.pidns_dev
                        || joined.pidns_ino != actor.pidns_ino
                })
        {
            return Err(io::Error::other("normal root/actor identity changed"));
        }
        let physical =
            normal_physical::observe(lane, &handoff, &actor, &session, works.broker_root()?)?
                .ok_or_else(|| io::Error::other("normal root K absent"))?;
        let publication = normal_physical::observe_publication(
            lane,
            &handoff,
            &actor,
            &session,
            works.broker_root()?,
        )?;
        Ok(Some(NormalRootEvidence {
            physical,
            publication,
        }))
    })();
    let normal_uncertain = normal_read.is_err();
    let normal = normal_read.ok().flatten();
    let offline_read = (|| -> io::Result<Option<OfflineRootEvidence>> {
        let Some(lane) = lane.as_ref() else {
            return Ok(None);
        };
        let Ok((handoff, actor)) = lane.released_handoff_for_root(&expected.root_id) else {
            return Ok(None);
        };
        if !matches!(
            &handoff.root_work_intent,
            FreshRootWorkIntent::CliHelp(_) | FreshRootWorkIntent::CliDiagnostics(_)
        ) {
            return Ok(None);
        }
        let session = lane
            .read_session(&handoff.d_key)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("offline D session absent"))?;
        lane.require_released_invocation(&handoff, &actor, &session)
            .map_err(io::Error::other)?;
        if lane
            .normal_provider_k_recorded(&handoff.handoff_id)
            .map_err(io::Error::other)?
        {
            return Err(io::Error::other("offline root has provider K"));
        }
        if !lane
            .no_root_child_requests(&handoff, &actor, &session)
            .map_err(io::Error::other)?
        {
            return Err(io::Error::other("offline root has State child work"));
        }
        let effect = lane
            .read_root_effect(&handoff, &actor, &session)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("offline State terminal absent"))?;
        if !matches!(
            effect.state,
            FreshRootEffectState::ReturnedSuccess | FreshRootEffectState::ReturnedFailure
        ) {
            return Ok(None);
        }
        let prepared = &handoff.old_release.prepared;
        let same = |stamp: &ProcessStamp, original: &PreparedProcessStamp| {
            stamp.host_pid == original.host_pid
                && stamp.boot_id == original.boot_id
                && stamp.starttime_ticks == original.starttime_ticks
                && stamp.pidns_dev == original.pidns_dev
                && stamp.pidns_ino == original.pidns_ino
        };
        if prepared.root_id != expected.root_id
            || prepared.owner_uid != expected.owner_uid
            || prepared.root_init.host_pid != expected.init_host_pid
            || prepared.root_init.boot_id != expected.boot_id
            || prepared.root_init.starttime_ticks != expected.init_starttime_ticks
            || prepared.root_init.pidns_dev != expected.pidns_dev
            || prepared.root_init.pidns_ino != expected.pidns_ino
            || entry.is_none_or(|entry| {
                !same(&entry.entry, &prepared.entry)
                    || entry
                        .guardian
                        .as_ref()
                        .is_none_or(|stamp| !same(stamp, &prepared.guardian))
                    || entry
                        .prepared_driver
                        .as_ref()
                        .is_none_or(|stamp| !same(stamp, &prepared.driver))
                    || entry.joined_child.as_ref().is_none_or(|joined| {
                        joined.host_pid != actor.host_pid
                            || joined.boot_id != actor.boot_id
                            || joined.starttime_ticks != actor.starttime_ticks
                            || joined.pidns_dev != actor.pidns_dev
                            || joined.pidns_ino != actor.pidns_ino
                    })
            })
        {
            return Err(io::Error::other("offline root/actor identity changed"));
        }
        Ok(Some(OfflineRootEvidence { effect }))
    })();
    let offline_uncertain = offline_read.is_err();
    let offline = offline_read.ok().flatten();
    let entry_unsettled = entry.is_none_or(|entry| entry.terminal_settlement.is_none());
    let entry_physical_settled = entry.is_some_and(|entry| {
        entry.join_consumed
            && ((source_records.len() == 1
                && entry.joined_child.as_ref() == Some(&source_records[0].joined_child)
                && entry.prepared_driver.as_ref() == Some(&source_records[0].driver)
                && entry.guardian.as_ref() == Some(&source_records[0].guardian))
                || ((normal.is_some() || offline.is_some())
                    && source_records.is_empty()
                    && entry.joined_child.is_some()
                    && entry.prepared_driver.is_some()
                    && entry.guardian.is_some()))
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
    // Historical proof stays scoped to this root. Later roots are checked
    // independently by the all-entry E gate; their debt cannot rewrite this
    // root's immutable Q, PID1 receipt, or owner certificate.
    let uncertain_registry_or_incarnation = pid1_receipt.is_err()
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
        grant_records: root_grants.len(),
        spent_without_work,
        live_works,
        work_debt,
        work_records: root_live.len() + root_debt.len(),
        work_retired,
        work_outstanding,
        native_prepared,
        native_spent,
        native_records: root_native.len(),
        source_physical_records: source_records.len(),
        source_physical_retired,
        source_physical_outstanding,
        source_effect,
        source_effect_readback_uncertain,
        successor_ack_unknown,
        successor_ack,
        original_receipt_unknown,
        original_receipt,
        child_closure_refusal,
        child_members,
        normal,
        normal_uncertain,
        offline_intent,
        offline,
        offline_uncertain,
        owner_close_inventory,
        uncertain_registry_or_incarnation,
        state_sidecar_outstanding_unknown: true,
        owner_close_preflight: false,
        owner_close_intent: roots.read_close_intent(expected)?,
        owner_close_proof: None,
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
    if let Some(intent) = &inventory.owner_close_intent {
        if let Some(sidecar) = sidecar {
            if let Some(proof) = sidecar
                .read_closed_root_owner(&expected.root_id, &intent.owner_generation)
                .map_err(io::Error::other)?
            {
                if proof.source_generation != intent.source_generation
                    || proof.state_cursor != intent.state_cursor
                    || serde_json::from_str::<RootRecord>(&proof.root_record_json)? != *expected
                    || serde_json::from_str::<RootPhysicalCloseProof>(&proof.physical_proof_json)?
                        != physical_close_proof(&inventory)?
                {
                    return Err(io::Error::other("closed owner root/physical proof changed"));
                }
                inventory.owner_close_proof = Some(proof);
                inventory.state_sidecar_outstanding_unknown = false;
            }
        }
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
    let normal = inventory.normal.as_ref().is_some_and(|normal| {
        normal.physical.state == "drained"
            && normal.physical.q.is_some()
            && normal.publication.state == "settled"
            && normal.publication.publication_sha256.is_some()
            && inventory.work_records == 0
            && inventory.source_physical_records == 0
            && owner.source_effect.accepted == 0
    });
    let offline = offline_no_effect(inventory);
    if (inventory.offline_intent && !offline)
        || inventory.root_id != expected_root_id
        || owner.root_id != expected_root_id
        || owner.owner_generation != expected_owner_generation
        || owner.source_generation.is_empty()
        || cursor.authority_ordinal < 0
        || (cursor.authority_ordinal == 0
            && (cursor.admission_id != "no-completion-continuity"
                || cursor.continuity_digest
                    != "0000000000000000000000000000000000000000000000000000000000000000"))
        || (cursor.authority_ordinal > 0 && cursor.admission_id.is_empty())
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
        || (!normal && !offline && inventory.work_records == 0)
        || inventory.work_retired != inventory.work_records
        || inventory.work_outstanding != 0
        || inventory.native_prepared != 0
        || inventory.native_spent != 0
        || (!normal && !offline && inventory.source_physical_records != 1)
        || inventory.source_physical_retired != inventory.source_physical_records
        || inventory.source_physical_outstanding != 0
        || inventory.source_effect_readback_uncertain
        || inventory.successor_ack_unknown
        || inventory.original_receipt_unknown
        || inventory.child_closure_refusal.is_some()
        || inventory.normal_uncertain
        || inventory.offline_uncertain
        || inventory.uncertain_registry_or_incarnation
        || inventory.source_effect.as_ref() != Some(&owner.source_effect)
        || owner.source_effect.unsettled() != 0
        || (!normal && !offline && owner.source_effect.accepted != 1)
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
    ready_for_pid1_request_with_fence(inventory, true)
}

/// Before the durable fence, the Broker needs the same exact physical and
/// publication prerequisites as the PID1 request. The original caller's
/// settlement must also be retained before autonomous progression begins.
pub fn ready_for_normal_admission_fence(inventory: &RootDrainInventory) -> io::Result<()> {
    if inventory.normal.is_none() || inventory.entry_unsettled {
        return Err(io::Error::other(
            "normal caller settlement absent before fence",
        ));
    }
    if let Some(refusal) = &inventory.child_closure_refusal {
        return Err(io::Error::other(format!(
            "root child closure refused before fence: {refusal}"
        )));
    }
    ready_for_pid1_request_with_fence(inventory, false)
}

/// No-effect CLI roots have no provider K/Q or source/child grant. Their
/// positive State return and exact empty registries select a separate fence.
pub fn ready_for_offline_admission_fence(inventory: &RootDrainInventory) -> io::Result<()> {
    if !offline_no_effect(inventory) {
        return Err(io::Error::other("offline no-effect evidence absent"));
    }
    ready_for_pid1_request_with_fence(inventory, false)
}

fn ready_for_pid1_request_with_fence(
    inventory: &RootDrainInventory,
    require_fenced: bool,
) -> io::Result<()> {
    let normal = inventory.normal.as_ref().is_some_and(|normal| {
        normal.physical.state == "drained"
            && normal.physical.q.is_some()
            && normal.publication.state == "settled"
            && normal.publication.publication_sha256.is_some()
            && inventory.work_records == 0
            && inventory.source_physical_records == 0
    });
    let offline = offline_no_effect(inventory);
    if (inventory.offline_intent && !offline)
        || (require_fenced && !inventory.fenced)
        || !inventory.entry_physical_settled
        || !inventory.entry_original_exited
        || inventory.prepared_grants != 0
        || inventory.spent_without_work != 0
        || inventory.live_works != 0
        || inventory.work_debt != 0
        || (!normal && !offline && inventory.work_records == 0)
        || inventory.work_outstanding != 0
        || inventory.work_retired != inventory.work_records
        || inventory.native_prepared != 0
        || inventory.native_spent != 0
        || (!normal && !offline && inventory.source_physical_records == 0)
        || inventory.source_physical_outstanding != 0
        || inventory.source_physical_retired != inventory.source_physical_records
        || inventory.source_effect_readback_uncertain
        || inventory.normal_uncertain
        || inventory.offline_uncertain
        || inventory
            .source_effect
            .as_ref()
            .is_none_or(|effect| effect.unsettled() != 0 || (normal && effect.accepted != 0))
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
    use oulipoly_state::mailbox::{
        BoundStateFileIdentity, BrokerStateCloseCursor, FreshRecipientIdentity,
    };

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
            grant_records: 1,
            spent_without_work: 0,
            live_works: 0,
            work_debt: 0,
            work_records: 1,
            work_retired: 1,
            work_outstanding: 0,
            native_prepared: 0,
            native_spent: 0,
            native_records: 0,
            source_physical_records: 1,
            source_physical_retired: 1,
            source_physical_outstanding: 0,
            source_effect: Some(effect.clone()),
            source_effect_readback_uncertain: false,
            successor_ack_unknown: false,
            successor_ack: None,
            original_receipt_unknown: false,
            original_receipt: None,
            child_closure_refusal: None,
            child_members: Vec::new(),
            normal: None,
            normal_uncertain: false,
            offline_intent: false,
            offline: None,
            offline_uncertain: false,
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
            owner_close_proof: None,
            close_eligible: false,
        }
    }

    /// The root's current child closure gates its owner close: an
    /// unresolved, late or unsettled member refuses both the preflight and
    /// the close proof. A child set's members enter the proof; a childless
    /// or one-child root's proof keeps its earlier bytes.
    #[test]
    fn child_closure_refusal_blocks_close_and_members_enter_only_set_proofs() {
        let settled = settled_owner_inventory();
        let proof = physical_close_proof(&settled).unwrap();
        let encoded = serde_json::to_string(&proof).unwrap();
        assert!(!encoded.contains("child_members"));
        assert_eq!(
            serde_json::from_str::<RootPhysicalCloseProof>(&encoded).unwrap(),
            proof
        );
        for refusal in [
            "notification_unsettled",
            "unresolved_child_admission",
            "child_admission_after_freeze",
            "child_closure_unknown: State unreadable",
        ] {
            let mut refused = settled.clone();
            refused.child_closure_refusal = Some(refusal.into());
            assert!(ready_for_owner_close_preflight(&refused, "root", "owner").is_err());
            assert!(physical_close_proof(&refused).is_err(), "{refusal}");
        }
        let member = |id: &str| FreshRootMemberSettlement {
            request_id: id.into(),
            notification_state: "acked".into(),
            evidence_sha256: id.repeat(64),
        };
        let mut set = settled.clone();
        set.child_members = vec![member("a"), member("b")];
        let set_proof = physical_close_proof(&set).unwrap();
        assert_eq!(set_proof.child_members, set.child_members);
        assert_ne!(
            set_proof, proof,
            "a changed member settlement changes the proof"
        );
        let mut changed = set.clone();
        changed.child_members[1].evidence_sha256 = "c".repeat(64);
        assert_ne!(physical_close_proof(&changed).unwrap(), set_proof);
    }

    #[test]
    fn close_preflight_binds_root_owner_cursor_and_physical_q_without_claiming_close() {
        let settled = settled_owner_inventory();
        assert!(ready_for_owner_close_preflight(&settled, "root", "owner").is_ok());
        assert!(!settled.close_eligible);
        assert!(ready_for_owner_close_preflight(&settled, "other", "owner").is_err());
        assert!(ready_for_owner_close_preflight(&settled, "root", "stale").is_err());

        let mut changed = settled.clone();
        changed.successor_ack_unknown = true;
        assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        assert!(physical_close_proof(&changed).is_err());
        changed = settled.clone();
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

    fn returned_offline_inventory() -> RootDrainInventory {
        let mut inventory = settled_owner_inventory();
        inventory.grant_records = 0;
        inventory.work_records = 0;
        inventory.work_retired = 0;
        inventory.source_physical_records = 0;
        inventory.source_physical_retired = 0;
        inventory.source_effect = Some(Default::default());
        inventory.offline_intent = true;
        inventory
            .owner_close_inventory
            .as_mut()
            .unwrap()
            .source_effect = Default::default();
        inventory.offline = Some(OfflineRootEvidence {
            effect: FreshRootEffect {
                handoff_id: "handoff".into(),
                invocation_uuid: "invocation".into(),
                session_id: "session".into(),
                actor: FreshRecipientIdentity {
                    host_pid: 12,
                    boot_id: "boot".into(),
                    starttime_ticks: 3,
                    pidns_dev: 4,
                    pidns_ino: 5,
                },
                intent: FreshRootWorkIntent::CliHelp(vec!["--help".into()]),
                state: FreshRootEffectState::ReturnedSuccess,
            },
        });
        inventory
    }

    #[test]
    fn offline_close_requires_positive_return_and_exact_empty_effect_registries() {
        let settled = returned_offline_inventory();
        assert!(ready_for_owner_close_preflight(&settled, "root", "owner").is_ok());
        let proof = physical_close_proof(&settled).unwrap();
        assert!(proof.normal.is_none());
        assert_eq!(proof.offline, settled.offline);
        let mut diagnostics = settled.clone();
        diagnostics.offline.as_mut().unwrap().effect.intent =
            FreshRootWorkIntent::CliDiagnostics(vec!["diagnostics".into()]);
        assert!(physical_close_proof(&diagnostics).is_ok());
        diagnostics.offline.as_mut().unwrap().effect.state = FreshRootEffectState::ReturnedFailure;
        assert!(physical_close_proof(&diagnostics).is_ok());

        let mut candidate = settled.clone();
        candidate.pid1_exact_live = true;
        candidate.pid1 = "live";
        assert!(ready_for_offline_admission_fence(&candidate).is_ok());
        candidate.offline = None;
        assert!(ready_for_offline_admission_fence(&candidate).is_err());

        for changed in [
            {
                let mut value = settled.clone();
                value.grant_records = 1;
                value
            },
            {
                let mut value = settled.clone();
                value.native_records = 1;
                value
            },
            {
                let mut value = settled.clone();
                value.work_records = 1;
                value
            },
            {
                let mut value = settled.clone();
                value.source_physical_records = 1;
                value
            },
            {
                let mut value = settled.clone();
                value.source_effect.as_mut().unwrap().accepted = 1;
                value
            },
            {
                let mut value = settled.clone();
                value.offline_uncertain = true;
                value
            },
            {
                let mut value = settled.clone();
                value.normal_uncertain = true;
                value
            },
            {
                let mut value = settled.clone();
                value.offline = None;
                value
            },
            {
                let mut value = settled.clone();
                value.offline.as_mut().unwrap().effect.state = FreshRootEffectState::Started;
                value
            },
            {
                let mut value = settled.clone();
                value.offline.as_mut().unwrap().effect.intent =
                    FreshRootWorkIntent::NormalCli(vec!["--model".into()]);
                value
            },
        ] {
            assert!(physical_close_proof(&changed).is_err());
            assert!(ready_for_owner_close_preflight(&changed, "root", "owner").is_err());
        }
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
