//! Concurrent-scoped root admission accounting.
//!
//! Earlier stages admitted a root only after every prior entry was closed and
//! settled (single flight). Concurrent roots replace that with accounting: an
//! unfinished entry is admitted as an in-flight sibling only while an exact
//! owner still holds it, and every other root, work, grant or source must
//! belong to a known entry. Closed history is joined once per Broker
//! incarnation and then carried by an exact record fingerprint, so admission
//! cost does not repeat the retained State/physical joins for every old root.
use crate::entry_registry::EntryRecord;
use oulipoly_state::mailbox::BrokerStateCloseCursor;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Physical state of the root an entry reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootState {
    /// E reserved the entry; J has not yet published its root.
    Absent,
    /// The exact root PID1 still verifies.
    Live,
    /// The root PID1 is gone. Only a durable admission fence places it in the
    /// Broker-owned close progression.
    Exited { fenced: bool },
}

#[derive(Clone, Copy, Debug)]
pub struct EntryFacts<'a> {
    pub root_id: &'a str,
    /// Caller result settled or offline close marked.
    pub settled: bool,
    /// For an unsettled entry: its exact reserving process (and any prepared
    /// guardian or driver) is still live on this boot.
    pub owner_live: bool,
    pub root: RootState,
    /// The exact record already passed the full close join in this incarnation.
    pub closed_cached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Previously joined closed history; no repeated join.
    Closed,
    /// Accounted sibling: reserved without a root yet, active, or settled and
    /// still in the Broker-owned close progression.
    InFlight,
    /// Fenced exited root. The caller must run the full close join; an absent
    /// owner-close proof means the close is still in flight.
    CloseCandidate,
}

/// Classify every entry and refuse orphan or unaccounted debt. `artifact_roots`
/// yields the root ID of every live or debt root, live or debt work, grant,
/// native grant and physical source.
pub fn account_entries<'a>(
    entries: &[EntryFacts<'_>],
    artifact_roots: impl IntoIterator<Item = &'a str>,
) -> io::Result<Vec<Disposition>> {
    let known: HashSet<&str> = entries.iter().map(|entry| entry.root_id).collect();
    let mut artifacts = artifact_roots.into_iter().peekable();
    if entries.is_empty() {
        if artifacts.peek().is_some() {
            return Err(io::Error::other("root or work exists without an entry"));
        }
        return Ok(Vec::new());
    }
    if known.len() != entries.len() || artifacts.any(|root_id| !known.contains(root_id)) {
        return Err(io::Error::other("unaccounted prior root or work debt"));
    }
    entries
        .iter()
        .map(|entry| {
            if entry.closed_cached {
                return Ok(Disposition::Closed);
            }
            match entry.root {
                RootState::Absent if entry.settled => {
                    Err(io::Error::other("prior entry has no exact root"))
                }
                RootState::Absent | RootState::Live if entry.settled || entry.owner_live => {
                    Ok(Disposition::InFlight)
                }
                RootState::Exited { fenced: true } => Ok(Disposition::CloseCandidate),
                _ => Err(io::Error::other("unaccounted prior root or work debt")),
            }
        })
        .collect()
}

/// Account the closed entries' State close cursors against the current head.
/// Each entry contributes at most one cursor, so equal cursors are distinct
/// roots closed at one unchanged head: the same immutable history row. Equal
/// ordinals with different content are aliasing or a source swap. Concurrent
/// roots share the head; progress past the newest covering close is accounted
/// only when `advances` attributes every step to an `in_flight` entry. A
/// closed, unknown or unattributed advance stays an unclosed generation.
/// The serial writer loop sees no close between its read and joins; the
/// read-only fresh reader may see a later close only as a validated cursor.
pub fn account_close_cursors(
    mut closed: Vec<BrokerStateCloseCursor>,
    current: Option<BrokerStateCloseCursor>,
    read_only: bool,
    in_flight: &HashSet<String>,
    advances: impl FnOnce(&BrokerStateCloseCursor, &BrokerStateCloseCursor) -> io::Result<Vec<String>>,
) -> io::Result<()> {
    closed.sort_by_key(|cursor| cursor.authority_ordinal);
    if closed.windows(2).any(|pair| {
        (pair[0].authority_ordinal == pair[1].authority_ordinal && pair[0] != pair[1])
            || pair[0].file != pair[1].file
            || pair[0].sidecar_generation != pair[1].sidecar_generation
    }) {
        return Err(io::Error::other("prior entry close cursor order changed"));
    }
    let Some(last) = closed.last() else {
        return Ok(());
    };
    let current = current.ok_or_else(|| io::Error::other("prior entry sidecar absent"))?;
    if !read_only && last.authority_ordinal > current.authority_ordinal {
        return Err(io::Error::other("unclosed State continuity generation"));
    }
    let base = closed
        .iter()
        .rev()
        .find(|cursor| cursor.authority_ordinal <= current.authority_ordinal)
        .ok_or_else(|| io::Error::other("unclosed State continuity generation"))?;
    if *base == current {
        return Ok(());
    }
    let roots = advances(base, &current).map_err(|error| {
        io::Error::other(format!("unclosed State continuity generation: {error}"))
    })?;
    if roots.iter().any(|root_id| !in_flight.contains(root_id)) {
        return Err(io::Error::other("unclosed State continuity generation"));
    }
    Ok(())
}

/// One closed entry joined in this Broker incarnation.
#[derive(Clone, Debug)]
pub struct ClosedEntry<C = BrokerStateCloseCursor> {
    pub record: EntryRecord,
    pub cursor: C,
}

#[derive(Debug)]
pub struct ClosedHistory<C = BrokerStateCloseCursor> {
    entries: HashMap<String, ClosedEntry<C>>,
}

impl<C> Default for ClosedHistory<C> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

impl<C> ClosedHistory<C> {
    /// A cached entry counts only while its durable record is byte-for-byte
    /// the record that passed the join.
    pub fn exact(&self, record: &EntryRecord) -> Option<&ClosedEntry<C>> {
        self.entries
            .get(&record.root_id)
            .filter(|closed| closed.record == *record)
    }

    pub fn insert(&mut self, record: EntryRecord, cursor: C) {
        self.entries
            .insert(record.root_id.clone(), ClosedEntry { record, cursor });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Process-local closed history shared by the old writer loop and the fresh
/// endpoint. It is keyed by State root and is never persisted: a restarted
/// Broker joins each closed entry once again before trusting it.
pub fn closed_history(state_root: &Path) -> io::Result<MutexGuard<'static, ClosedHistory>> {
    static CACHES: OnceLock<Mutex<HashMap<PathBuf, &'static Mutex<ClosedHistory>>>> =
        OnceLock::new();
    let cache = {
        let mut caches = CACHES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| io::Error::other("closed history poisoned"))?;
        *caches
            .entry(state_root.to_path_buf())
            .or_insert_with(|| Box::leak(Box::default()))
    };
    // An uncontended lock records nothing; a contended one records its wait
    // against the request on this thread.
    let locked = match cache.try_lock() {
        Ok(guard) => Ok(guard),
        Err(std::sync::TryLockError::WouldBlock) => {
            crate::phase_record::timed(crate::phase_record::Sub::History, || cache.lock())
        }
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Err(poisoned),
    };
    locked.map_err(|_| io::Error::other("closed history poisoned"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry_registry::ProcessStamp;
    use oulipoly_state::mailbox::BoundStateFileIdentity;

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    const C: &str = "33333333-3333-4333-8333-333333333333";

    fn facts(root_id: &str, root: RootState, settled: bool, owner_live: bool) -> EntryFacts<'_> {
        EntryFacts {
            root_id,
            settled,
            owner_live,
            root,
            closed_cached: false,
        }
    }

    fn record(root_id: &str) -> EntryRecord {
        EntryRecord {
            version: 1,
            root_id: root_id.into(),
            owner_uid: 1000,
            entry: ProcessStamp {
                host_pid: 1,
                boot_id: "boot".into(),
                starttime_ticks: 1,
                pidns_dev: 1,
                pidns_ino: 1,
            },
            prepared_guardian: None,
            domain_id: None,
            supervisor_authority_id: None,
            guardian: None,
            join_consumed: false,
            joined_child: None,
            prepared_driver: None,
            terminal_settlement: None,
            offline_close_sha256: None,
        }
    }

    fn message(result: io::Result<Vec<Disposition>>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn reserved_sibling_without_root_is_an_accounted_in_flight_entry() {
        // The observed L1 refusal: sibling E reserved, J not yet run, so the
        // entry count exceeds the root count.
        let entries = [
            facts(A, RootState::Live, false, true),
            facts(B, RootState::Absent, false, true),
        ];
        assert_eq!(
            account_entries(&entries, [A]).unwrap(),
            [Disposition::InFlight, Disposition::InFlight]
        );
    }

    #[test]
    fn fully_active_and_settled_closing_siblings_are_in_flight() {
        let entries = [
            facts(A, RootState::Live, false, true),
            // Settled caller result; Broker-owned fence/close still pending.
            facts(B, RootState::Live, true, false),
            facts(C, RootState::Exited { fenced: true }, true, false),
        ];
        assert_eq!(
            account_entries(&entries, [A, A, B, C]).unwrap(),
            [
                Disposition::InFlight,
                Disposition::InFlight,
                Disposition::CloseCandidate
            ]
        );
    }

    #[test]
    fn orphan_root_work_grant_or_source_is_refused() {
        let entries = [facts(A, RootState::Live, false, true)];
        assert_eq!(
            message(account_entries(&entries, [A, B])),
            "unaccounted prior root or work debt"
        );
        assert_eq!(
            message(account_entries(&[], [A])),
            "root or work exists without an entry"
        );
        assert!(account_entries(&[], []).unwrap().is_empty());
    }

    #[test]
    fn unsettled_entry_without_a_live_owner_is_unaccounted_debt() {
        for root in [RootState::Absent, RootState::Live] {
            let entries = [facts(A, root, false, false)];
            assert_eq!(
                message(account_entries(&entries, [A])),
                "unaccounted prior root or work debt"
            );
        }
    }

    #[test]
    fn unfenced_exited_root_is_debt_even_with_a_live_owner() {
        for (settled, owner_live) in [(false, true), (true, false)] {
            let entries = [facts(
                A,
                RootState::Exited { fenced: false },
                settled,
                owner_live,
            )];
            assert_eq!(
                message(account_entries(&entries, [A])),
                "unaccounted prior root or work debt"
            );
        }
    }

    #[test]
    fn settled_entry_without_its_root_is_a_mismatch() {
        let entries = [facts(A, RootState::Absent, true, false)];
        assert_eq!(
            message(account_entries(&entries, [])),
            "prior entry has no exact root"
        );
    }

    #[test]
    fn exact_cached_closed_entry_skips_the_join_and_a_changed_record_does_not() {
        let closed = record(A);
        let mut history = ClosedHistory::<u8>::default();
        history.insert(closed.clone(), 1);
        assert!(history.exact(&closed).is_some());
        let mut changed = closed.clone();
        changed.offline_close_sha256 = Some("f".repeat(64));
        assert!(history.exact(&changed).is_none());

        let mut cached = facts(A, RootState::Exited { fenced: false }, false, false);
        cached.closed_cached = true;
        assert_eq!(
            account_entries(&[cached], [A]).unwrap(),
            [Disposition::Closed]
        );
    }

    #[test]
    fn classification_of_large_closed_history_needs_no_join() {
        // Closed history is carried by the cache: only the in-flight sibling
        // and the one new close candidate need any retained-authority work.
        let ids: Vec<String> = (0..10_000)
            .map(|n| format!("{n:08x}-0000-4000-8000-000000000000"))
            .collect();
        let mut entries: Vec<_> = ids
            .iter()
            .map(|id| {
                let mut closed = facts(id, RootState::Exited { fenced: true }, true, false);
                closed.closed_cached = true;
                closed
            })
            .collect();
        entries.push(facts(A, RootState::Live, false, true));
        entries.push(facts(B, RootState::Exited { fenced: true }, true, false));
        let dispositions =
            account_entries(&entries, ids.iter().map(String::as_str).chain([A, B])).unwrap();
        assert_eq!(
            dispositions
                .iter()
                .filter(|d| **d != Disposition::Closed)
                .count(),
            2
        );
    }

    fn cursor(ordinal: i64, tag: &str) -> BrokerStateCloseCursor {
        BrokerStateCloseCursor {
            file: BoundStateFileIdentity {
                device: 1,
                inode: 2,
            },
            authority_ordinal: ordinal,
            admission_id: format!("admission-{ordinal}-{tag}"),
            sidecar_generation: "g".into(),
            continuity_digest: format!("{ordinal:064}"),
        }
    }

    fn roots(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    fn unattributed(
        _: &BrokerStateCloseCursor,
        _: &BrokerStateCloseCursor,
    ) -> io::Result<Vec<String>> {
        panic!("no advance expected")
    }

    #[test]
    fn distinct_roots_closed_at_one_unchanged_head_are_accounted() {
        // B2: two roots closed one after another at the same nonzero head.
        for read_only in [false, true] {
            account_close_cursors(
                vec![cursor(7, "a"), cursor(7, "a")],
                Some(cursor(7, "a")),
                read_only,
                &roots(&[]),
                unattributed,
            )
            .unwrap();
        }
        // Equal ordinals with different content are aliasing or a swap.
        let mut swapped = cursor(7, "a");
        swapped.continuity_digest = "f".repeat(64);
        for pair in [
            vec![cursor(7, "a"), cursor(7, "b")],
            vec![cursor(7, "a"), swapped],
        ] {
            assert_eq!(
                account_close_cursors(pair, Some(cursor(7, "a")), false, &roots(&[]), unattributed)
                    .unwrap_err()
                    .to_string(),
                "prior entry close cursor order changed"
            );
        }
        let mut moved = cursor(8, "a");
        moved.sidecar_generation = "other".into();
        assert!(
            account_close_cursors(
                vec![cursor(7, "a"), moved],
                Some(cursor(8, "a")),
                false,
                &roots(&[]),
                unattributed,
            )
            .is_err()
        );
    }

    #[test]
    fn sibling_progress_past_every_closed_cursor_is_accounted_only_for_in_flight_roots() {
        // B3: A closed at 7; in-flight sibling B advanced the head to 9.
        let advances = |base: &BrokerStateCloseCursor, current: &BrokerStateCloseCursor| {
            assert_eq!((base.authority_ordinal, current.authority_ordinal), (7, 9));
            Ok(vec![B.to_owned(), B.to_owned()])
        };
        for read_only in [false, true] {
            account_close_cursors(
                vec![cursor(7, "a")],
                Some(cursor(9, "b")),
                read_only,
                &roots(&[B]),
                advances,
            )
            .unwrap();
        }
        // The advance belongs to a closed or unknown root: still unclosed.
        for in_flight in [roots(&[]), roots(&[C])] {
            assert_eq!(
                account_close_cursors(
                    vec![cursor(7, "a")],
                    Some(cursor(9, "b")),
                    false,
                    &in_flight,
                    advances,
                )
                .unwrap_err()
                .to_string(),
                "unclosed State continuity generation"
            );
        }
        // An unattributed advance (the attribution read refuses) stays unclosed.
        let refused = account_close_cursors(
            vec![cursor(7, "a")],
            Some(cursor(9, "b")),
            false,
            &roots(&[B]),
            |_, _| Err(io::Error::other("unattributed")),
        )
        .unwrap_err()
        .to_string();
        assert!(refused.starts_with("unclosed State continuity generation"));
        // The base is the newest close not past the head.
        let base_seen = std::cell::Cell::new(0);
        account_close_cursors(
            vec![cursor(3, "c"), cursor(7, "a"), cursor(5, "x")],
            Some(cursor(9, "b")),
            false,
            &roots(&[B]),
            |base, _| {
                base_seen.set(base.authority_ordinal);
                Ok(vec![B.to_owned()])
            },
        )
        .unwrap();
        assert_eq!(base_seen.get(), 7);
    }

    #[test]
    fn later_closes_are_visible_only_to_the_read_only_reader() {
        // The writer loop never sees a close after its head read.
        assert!(
            account_close_cursors(
                vec![cursor(7, "a"), cursor(9, "b")],
                Some(cursor(7, "a")),
                false,
                &roots(&[]),
                unattributed,
            )
            .is_err()
        );
        account_close_cursors(
            vec![cursor(7, "a"), cursor(9, "b")],
            Some(cursor(7, "a")),
            true,
            &roots(&[]),
            unattributed,
        )
        .unwrap();
        // A head with the newest ordinal but other content is not that close.
        assert!(
            account_close_cursors(
                vec![cursor(7, "a")],
                Some(cursor(7, "z")),
                false,
                &roots(&[]),
                |_, _| Err(io::Error::other("not forward")),
            )
            .is_err()
        );
        // No closed history: nothing to account against the head.
        account_close_cursors(Vec::new(), None, false, &roots(&[]), unattributed).unwrap();
        assert!(
            account_close_cursors(vec![cursor(7, "a")], None, true, &roots(&[]), unattributed)
                .is_err()
        );
    }

    #[test]
    fn closed_history_is_shared_per_state_root() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        assert!(closed_history(first.path()).unwrap().is_empty());
        closed_history(first.path()).unwrap().insert(
            record(B),
            BrokerStateCloseCursor {
                file: BoundStateFileIdentity {
                    device: 1,
                    inode: 2,
                },
                authority_ordinal: 0,
                admission_id: "no-completion-continuity".into(),
                sidecar_generation: "g".into(),
                continuity_digest: "0".repeat(64),
            },
        );
        assert_eq!(closed_history(first.path()).unwrap().len(), 1);
        assert!(closed_history(second.path()).unwrap().is_empty());
    }
}
