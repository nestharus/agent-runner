//! Per-root durable intent: one private directory per root, owned by that
//! root's supervisor. See the crate docs for the labels it backs.
//!
//! * `owner.lock` is held with an exclusive `flock` for the life of the
//!   owning process, so a second live instance is refused before it opens
//!   the database. The kernel drops the lock when the owner dies.
//! * `intent.sqlite3` holds the intent, the minted message keys, every
//!   attempt and its outcome, closures, acknowledgements and the owner
//!   generation. WAL journal with `synchronous=FULL`: each commit syncs
//!   the WAL before returning.
//! * Every write runs in one `BEGIN IMMEDIATE` transaction that first
//!   checks that this instance's generation is still the latest claimed
//!   one. A stale instance (for example after its lock file was replaced)
//!   is refused before it changes anything.
//! * `root` holds this root's identity: 128 random bits minted with the
//!   intent. Agent identity is a harness position within this root, so it
//!   belongs to this one root lineage and outlives owners and root PID 1
//!   incarnations.
//! * `incarnation` records each root PID 1 this lineage started: its
//!   attestation token and exact process identity (host pid, start time,
//!   boot id), and how it was later found ended. `work` records each harness
//!   launch under an incarnation, written before the launch is requested,
//!   and the actual waiter's report of its end.
//! * A `work` of kind `bash` is a Bash run accepted through this root's
//!   Bash ingress; `bash_run` keeps who asked for it (the requesting
//!   harness, as positively attributed, and the owner inputs open on that
//!   harness then) and what it runs. It is committed before the requester
//!   is told `accepted` and before the launch is requested.
//! * A `message` is either part of the intent (`origin` `intent`) or a
//!   caller's further input to the live conversation (`follow-up`),
//!   committed with its minted key, the control line that carried it and
//!   the caller's reference before it is reported admitted. Once admitted
//!   it is owed like any intent message, and a recovery resubmits it the
//!   same way. Version 5 added this; a version 4 store is refused like any
//!   other version (no migration).
//! * A `harness` of kind `child` is a registered child an intent harness
//!   asked for through the ingress; `child` keeps the durable lineage: the
//!   requesting (parent) harness and the exact parent **work** it was
//!   current under at commit, the site route, the requester and the parent
//!   inputs open then, and how the child ended (`outcome`). Its prompt is
//!   its one message. A `bash_run` also keeps the requesting harness work.
//!   Version 7 added both (no migration: no legacy data is kept).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oulipoly_acp::{MessageKey, OutboundMessage};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::Intent;

pub(crate) const LOCK_FILE: &str = "owner.lock";
pub(crate) const DB_FILE: &str = "intent.sqlite3";
/// Version of this new per-root lineage. There is no migration chain: a
/// store of any other version is refused.
const SCHEMA_VERSION: i64 = 7;
/// How long a write waits for a foreign SQLite lock before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The successor's label for an earlier owner's attempt with no recorded
/// outcome. The attempt may or may not have been sent or inserted.
pub(crate) const UNKNOWN_PRIOR_OWNER: &str = "unknown-prior-owner";
/// Durable stop: the message reached its observed-closure cap.
pub(crate) const OUTAGE: &str = "outage";
/// Durable stop: the message used its delivery-attempt budget, counted over
/// every generation, without a recorded acknowledgement. Unresolved
/// attempts among them stay unknown; this is not an outage or a closure.
pub(crate) const ATTEMPTS_EXHAUSTED: &str = "attempts-exhausted";

const SCHEMA: &str = "
CREATE TABLE owner (
    generation INTEGER PRIMARY KEY,
    pid INTEGER NOT NULL,
    claimed_unix INTEGER NOT NULL
);
CREATE TABLE intent (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    outage_closure_cap INTEGER NOT NULL,
    delivery_attempt_cap INTEGER NOT NULL,
    cwd TEXT NOT NULL,
    workload TEXT NOT NULL,
    children TEXT,
    generation INTEGER NOT NULL
);
CREATE TABLE harness (
    position INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    argv TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    session TEXT,
    kind TEXT NOT NULL DEFAULT 'intent' CHECK (kind IN ('intent', 'child'))
);
CREATE TABLE child (
    harness INTEGER PRIMARY KEY REFERENCES harness(position),
    parent INTEGER NOT NULL REFERENCES harness(position),
    parent_work INTEGER NOT NULL REFERENCES work(id),
    route TEXT NOT NULL,
    requester_pid INTEGER NOT NULL,
    inputs_open TEXT NOT NULL,
    admitted_generation INTEGER NOT NULL,
    outcome TEXT,
    resolved_generation INTEGER
);
CREATE TABLE message (
    harness INTEGER NOT NULL REFERENCES harness(position),
    idx INTEGER NOT NULL,
    key TEXT NOT NULL UNIQUE,
    text TEXT NOT NULL,
    closures INTEGER NOT NULL DEFAULT 0,
    stop TEXT,
    ack_label TEXT,
    ack_basis TEXT,
    ack_recovered INTEGER,
    ack_generation INTEGER,
    ack_message_id TEXT,
    origin TEXT NOT NULL DEFAULT 'intent' CHECK (origin IN ('intent', 'follow-up')),
    control INTEGER,
    caller_ref TEXT,
    admitted_generation INTEGER,
    PRIMARY KEY (harness, idx)
);
CREATE TABLE root (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    root_id TEXT NOT NULL
);
CREATE TABLE incarnation (
    id INTEGER PRIMARY KEY,
    token TEXT NOT NULL,
    isolation TEXT NOT NULL,
    generation INTEGER NOT NULL,
    host_pid INTEGER,
    start_time INTEGER,
    boot_id TEXT,
    ended TEXT,
    ended_generation INTEGER
);
CREATE TABLE work (
    id INTEGER PRIMARY KEY,
    harness INTEGER NOT NULL REFERENCES harness(position),
    incarnation INTEGER NOT NULL REFERENCES incarnation(id),
    generation INTEGER NOT NULL,
    harness_host_pid INTEGER,
    outcome TEXT,
    observer TEXT,
    resolved_generation INTEGER,
    kind TEXT NOT NULL DEFAULT 'harness' CHECK (kind IN ('harness', 'bash'))
);
CREATE TABLE bash_run (
    work INTEGER PRIMARY KEY REFERENCES work(id),
    requester_work INTEGER REFERENCES work(id),
    requester_pid INTEGER NOT NULL,
    inputs_open TEXT NOT NULL,
    argv TEXT NOT NULL,
    cwd TEXT NOT NULL
);
CREATE TABLE attempt (
    id INTEGER PRIMARY KEY,
    harness INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    outcome TEXT,
    resolved_generation INTEGER,
    FOREIGN KEY (harness, idx) REFERENCES message(harness, idx)
);
";

#[derive(Debug)]
pub(crate) enum ClaimError {
    /// Another live instance holds this root's owner lock. Nothing written.
    OwnerLive,
    NotPrivate(String),
    /// A recover request found no committed intent.
    NoIntent,
    /// A create request found intent already committed.
    IntentExists,
    Store(String),
}

impl ClaimError {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::OwnerLive => "owner-live".to_owned(),
            Self::NotPrivate(why) => format!("store-not-private: {why}"),
            Self::NoIntent => "no-durable-intent".to_owned(),
            Self::IntentExists => "intent-exists".to_owned(),
            Self::Store(why) => format!("store-error: {why}"),
        }
    }
}

impl From<rusqlite::Error> for ClaimError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.to_string())
    }
}

impl From<io::Error> for ClaimError {
    fn from(error: io::Error) -> Self {
        Self::Store(error.to_string())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// A later generation has claimed this root. Nothing was written.
    FenceLost {
        current: i64,
    },
    Failed(String),
}

impl StoreError {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::FenceLost { .. } => "authority-lost",
            Self::Failed(_) => "store-failed",
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

/// An earlier durable acknowledgement of a message.
#[derive(Debug, Clone)]
pub(crate) struct DurableAck {
    pub(crate) label: String,
    pub(crate) basis: Option<String>,
    pub(crate) recovered: bool,
    pub(crate) generation: i64,
    /// The harness's `messageId` for the inserted user message.
    pub(crate) message_id: Option<String>,
}

pub(crate) struct DurableMessage {
    /// Origin messages keep complete history; restored ones have unknown
    /// history (see [`OutboundMessage::fresh_recorded`]).
    pub(crate) message: OutboundMessage,
    /// Whether it came with the intent or as a caller's follow-up.
    pub(crate) follow_up: bool,
    pub(crate) closures: u32,
    pub(crate) stop: Option<String>,
    pub(crate) ack: Option<DurableAck>,
    /// Attempts recorded by any generation, before this instance.
    pub(crate) attempts: u32,
    /// Earlier attempts this instance classified as unknown.
    pub(crate) prior_unknown: u32,
}

pub(crate) struct DurableHarness {
    pub(crate) id: String,
    pub(crate) argv: Vec<String>,
    pub(crate) endpoint: crate::Endpoint,
    pub(crate) session: Option<String>,
    pub(crate) messages: Vec<DurableMessage>,
    /// Launches with no recorded end, as `(work, incarnation)`.
    pub(crate) open_works: Vec<(i64, i64)>,
}

/// A Bash run with no recorded end.
#[derive(Debug, Clone)]
pub(crate) struct OpenBash {
    pub(crate) work: i64,
    pub(crate) incarnation: i64,
    /// Position of the harness that asked for it.
    pub(crate) harness: usize,
}

/// A registered child with a launch whose end is not recorded.
#[derive(Debug, Clone)]
pub(crate) struct OpenChild {
    pub(crate) position: usize,
    pub(crate) id: String,
    pub(crate) work: i64,
    pub(crate) incarnation: i64,
}

/// A root PID 1 incarnation not yet recorded as ended.
#[derive(Debug, Clone)]
pub(crate) struct IncarnationRow {
    pub(crate) id: i64,
    pub(crate) token: String,
    pub(crate) host_pid: Option<i32>,
    pub(crate) start_time: Option<u64>,
    pub(crate) boot_id: Option<String>,
}

pub(crate) struct Claimed {
    pub(crate) store: Store,
    pub(crate) created: bool,
    pub(crate) outage_closure_cap: u32,
    pub(crate) delivery_attempt_cap: u32,
    pub(crate) cwd: String,
    /// The intent's declared work identity, as committed.
    pub(crate) workload: crate::Workload,
    pub(crate) harnesses: Vec<DurableHarness>,
    /// Earlier-generation attempts this claim classified as unknown.
    pub(crate) classified_unknown: u32,
    pub(crate) root_id: String,
    /// The latest root PID 1 incarnation not recorded as ended, if any.
    pub(crate) live_incarnation: Option<IncarnationRow>,
    pub(crate) open_bash: Vec<OpenBash>,
    /// The intent's child policy, as committed.
    pub(crate) children: Option<crate::children::ChildPolicy>,
    /// Every harness row, intent and child: the next position is this.
    pub(crate) positions: usize,
    /// Children admitted by every generation (the start budget's use).
    pub(crate) child_starts: u32,
    pub(crate) open_children: Vec<OpenChild>,
    /// Children an earlier owner admitted whose end it never recorded and
    /// which have no open launch: this claim labelled them lost.
    pub(crate) children_lost: u32,
}

/// The current owner's handle on its root's store.
pub(crate) struct Store {
    conn: Connection,
    generation: i64,
    _lock: File,
}

impl Store {
    /// Takes the owner lock, opens the store, and in one transaction bumps
    /// the owner generation, classifies earlier unresolved attempts as
    /// unknown, stops messages whose attempt budget is already used, and
    /// either commits `intent` (create) or loads it (recover). Returns only
    /// after that transaction has committed.
    pub(crate) fn claim(dir: &Path, intent: Option<&Intent>) -> Result<Claimed, ClaimError> {
        private_dir(dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join(LOCK_FILE))?;
        // SAFETY: flock on a descriptor we own.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            return Err(if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
                ClaimError::OwnerLive
            } else {
                ClaimError::Store(error.to_string())
            });
        }
        let mut conn = Connection::open(dir.join(DB_FILE))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(ClaimError::Store(format!(
                "journal_mode is {mode}, not wal"
            )));
        }
        conn.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
        let synchronous: i64 = conn.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        if synchronous != 2 {
            return Err(ClaimError::Store(format!(
                "synchronous is {synchronous}, not FULL"
            )));
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match version {
            0 => tx.execute_batch(&format!("{SCHEMA} PRAGMA user_version = {SCHEMA_VERSION};"))?,
            SCHEMA_VERSION => {}
            other => {
                return Err(ClaimError::Store(format!("unknown store version {other}")));
            }
        }
        let previous: Option<i64> =
            tx.query_row("SELECT max(generation) FROM owner", [], |row| row.get(0))?;
        let generation = previous.unwrap_or(0) + 1;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| int(elapsed.as_secs()));
        tx.execute(
            "INSERT INTO owner (generation, pid, claimed_unix) VALUES (?1, ?2, ?3)",
            params![generation, std::process::id(), now],
        )?;
        let classified_unknown = tx.execute(
            "UPDATE attempt SET outcome = ?1, resolved_generation = ?2
             WHERE outcome IS NULL AND generation < ?2",
            params![UNKNOWN_PRIOR_OWNER, generation],
        )?;
        let existing: Option<(u32, u32, String, String, Option<String>)> = tx
            .query_row(
                "SELECT outage_closure_cap, delivery_attempt_cap, cwd, workload, children FROM intent",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?;
        let (created, cap, attempt_cap, cwd, workload, children, harnesses) =
            match (intent, existing) {
                (Some(_), Some(_)) => return Err(ClaimError::IntentExists),
                (None, None) => return Err(ClaimError::NoIntent),
                (Some(intent), None) => {
                    tx.execute(
                        "INSERT INTO root (singleton, root_id) VALUES (1, ?1)",
                        params![crate::sys::random_hex()?],
                    )?;
                    let harnesses = create_intent(&tx, intent, generation)?;
                    (
                        true,
                        intent.outage_closure_cap,
                        intent.delivery_attempt_cap,
                        intent.cwd.clone(),
                        intent.workload.clone(),
                        intent.children.clone(),
                        harnesses,
                    )
                }
                (None, Some((cap, attempt_cap, cwd, workload, children))) => {
                    let workload: crate::Workload = serde_json::from_str(&workload)
                        .map_err(|error| ClaimError::Store(format!("stored workload: {error}")))?;
                    let children = children
                        .map(|text| serde_json::from_str(&text))
                        .transpose()
                        .map_err(|error| ClaimError::Store(format!("stored children: {error}")))?;
                    // Persisted attempts from every generation count: a restart
                    // loop cannot buy more attempts than the intent allows.
                    tx.execute(
                        "UPDATE message SET stop = ?1
                     WHERE stop IS NULL AND ack_label IS NULL
                       AND (SELECT count(*) FROM attempt a
                            WHERE a.harness = message.harness AND a.idx = message.idx) >= ?2",
                        params![ATTEMPTS_EXHAUSTED, attempt_cap],
                    )?;
                    (
                        false,
                        cap,
                        attempt_cap,
                        cwd,
                        workload,
                        children,
                        load_intent(&tx, generation)?,
                    )
                }
            };
        // A child an earlier owner admitted and never resolved, with no
        // launch left open, is lost with that owner: it is never relaunched
        // or delivered to again (its requester's connection died too).
        let children_lost = tx.execute(
            "UPDATE child SET outcome = 'lost-with-prior-owner', resolved_generation = ?1
             WHERE outcome IS NULL AND NOT EXISTS (
                 SELECT 1 FROM work w WHERE w.harness = child.harness AND w.outcome IS NULL)",
            params![generation],
        )?;
        let positions: i64 = tx.query_row("SELECT count(*) FROM harness", [], |row| row.get(0))?;
        let child_starts: i64 = tx.query_row("SELECT count(*) FROM child", [], |row| row.get(0))?;
        let open_children = tx
            .prepare(
                "SELECT w.id, w.incarnation, w.harness, h.id FROM work w
                 JOIN harness h ON h.position = w.harness
                 WHERE h.kind = 'child' AND w.kind = 'harness' AND w.outcome IS NULL ORDER BY w.id",
            )?
            .query_map([], |row| {
                Ok(OpenChild {
                    work: row.get(0)?,
                    incarnation: row.get(1)?,
                    position: usize::try_from(row.get::<_, i64>(2)?).unwrap_or(usize::MAX),
                    id: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let root_id: String = tx.query_row("SELECT root_id FROM root", [], |row| row.get(0))?;
        let live_incarnation = tx
            .query_row(
                "SELECT id, token, host_pid, start_time, boot_id FROM incarnation
                 WHERE ended IS NULL ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok(IncarnationRow {
                        id: row.get(0)?,
                        token: row.get(1)?,
                        host_pid: row.get(2)?,
                        start_time: row.get::<_, Option<i64>>(3)?.map(|time| time as u64),
                        boot_id: row.get(4)?,
                    })
                },
            )
            .optional()?;
        let open_bash = tx
            .prepare(
                "SELECT id, incarnation, harness FROM work
                 WHERE kind = 'bash' AND outcome IS NULL ORDER BY id",
            )?
            .query_map([], |row| {
                Ok(OpenBash {
                    work: row.get(0)?,
                    incarnation: row.get(1)?,
                    harness: usize::try_from(row.get::<_, i64>(2)?).unwrap_or(usize::MAX),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        tx.commit()?;
        // Make the directory entries of the store files durable too.
        File::open(dir)?.sync_all()?;
        // Persist the root directory's own entry before acknowledging the
        // claim. Also sync on recovery/retry: a prior claim may have died or
        // failed after creating the directory but before syncing its parent.
        if let Some(parent) = dir.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(Claimed {
            store: Store {
                conn,
                generation,
                _lock: lock,
            },
            created,
            outage_closure_cap: cap,
            delivery_attempt_cap: attempt_cap,
            cwd,
            workload,
            harnesses,
            classified_unknown: u32::try_from(classified_unknown).unwrap_or(u32::MAX),
            root_id,
            live_incarnation,
            open_bash,
            children,
            positions: usize::try_from(positions).unwrap_or(usize::MAX),
            child_starts: u32::try_from(child_starts).unwrap_or(u32::MAX),
            open_children,
            children_lost: u32::try_from(children_lost).unwrap_or(u32::MAX),
        })
    }

    pub(crate) fn generation(&self) -> i64 {
        self.generation
    }

    /// Runs `write` in one transaction only while this instance is still
    /// the latest owner generation. Commits before returning.
    fn write<T>(
        &mut self,
        write: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: i64 =
            tx.query_row("SELECT max(generation) FROM owner", [], |row| row.get(0))?;
        if current != self.generation {
            return Err(StoreError::FenceLost { current });
        }
        let value = write(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    pub(crate) fn set_session(&mut self, harness: usize, session: &str) -> Result<(), StoreError> {
        self.write(|tx| {
            tx.execute(
                "UPDATE harness SET session = ?2 WHERE position = ?1",
                params![int(harness), session],
            )
            .map(drop)
        })
    }

    /// Records that an attempt is about to be sent. Must commit before the
    /// send, so a successor can never miss an attempt that was sent.
    pub(crate) fn begin_attempt(&mut self, harness: usize, idx: usize) -> Result<i64, StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO attempt (harness, idx, generation) VALUES (?1, ?2, ?3)",
                params![int(harness), int(idx), generation],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }

    pub(crate) fn resolve_attempt(
        &mut self,
        attempt: i64,
        outcome: &str,
    ) -> Result<(), StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "UPDATE attempt SET outcome = ?2, resolved_generation = ?3 WHERE id = ?1",
                params![attempt, outcome, generation],
            )
            .map(drop)
        })
    }

    /// Records an insertion acknowledgement and resolves its attempt.
    pub(crate) fn record_ack(
        &mut self,
        harness: usize,
        idx: usize,
        attempt: i64,
        ack: &DurableAck,
    ) -> Result<(), StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "UPDATE attempt SET outcome = ?2, resolved_generation = ?3 WHERE id = ?1",
                params![attempt, format!("ack:{}", ack.label), generation],
            )?;
            tx.execute(
                "UPDATE message SET ack_label = ?3, ack_basis = ?4, ack_recovered = ?5,
                 ack_generation = ?6, ack_message_id = ?7 WHERE harness = ?1 AND idx = ?2",
                params![
                    int(harness),
                    int(idx),
                    ack.label,
                    ack.basis,
                    ack.recovered,
                    generation,
                    ack.message_id
                ],
            )
            .map(drop)
        })
    }

    /// Commits a caller's follow-up as the harness's next owed message,
    /// with a freshly minted key, before anything reports it admitted.
    /// Returns its index and the message to deliver.
    pub(crate) fn admit_follow_up(
        &mut self,
        harness: usize,
        control: u64,
        caller_ref: Option<&str>,
        text: &str,
    ) -> Result<(usize, OutboundMessage), StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            let idx: i64 = tx.query_row(
                "SELECT coalesce(max(idx) + 1, 0) FROM message WHERE harness = ?1",
                params![int(harness)],
                |row| row.get(0),
            )?;
            let message = OutboundMessage::fresh_recorded(text, |key: &MessageKey| {
                tx.execute(
                    "INSERT INTO message (harness, idx, key, text, origin, control, caller_ref,
                                          admitted_generation)
                     VALUES (?1, ?2, ?3, ?4, 'follow-up', ?5, ?6, ?7)",
                    params![
                        int(harness),
                        idx,
                        key.as_str(),
                        text,
                        int(control),
                        caller_ref,
                        generation
                    ],
                )
                .map(drop)
                .map_err(|error| io::Error::other(error.to_string()))
            })
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            Ok((usize::try_from(idx).unwrap_or(usize::MAX), message))
        })
    }

    /// Records a root PID 1 incarnation before it is started, so that a
    /// successor knows of it even if this owner dies while starting it.
    pub(crate) fn begin_incarnation(
        &mut self,
        token: &str,
        isolation: &str,
    ) -> Result<i64, StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO incarnation (token, isolation, generation) VALUES (?1, ?2, ?3)",
                params![token, isolation, generation],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }

    /// Records the started root PID 1's exact process identity.
    pub(crate) fn record_incarnation_identity(
        &mut self,
        id: i64,
        host_pid: i32,
        start_time: u64,
        boot_id: &str,
    ) -> Result<(), StoreError> {
        self.write(|tx| {
            tx.execute(
                "UPDATE incarnation SET host_pid = ?2, start_time = ?3, boot_id = ?4 WHERE id = ?1",
                params![id, host_pid, int(start_time), boot_id],
            )
            .map(drop)
        })
    }

    /// Records how an incarnation was found ended; `label` says what was
    /// actually observed, never more.
    pub(crate) fn end_incarnation(&mut self, id: i64, label: &str) -> Result<(), StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "UPDATE incarnation SET ended = ?2, ended_generation = ?3
                 WHERE id = ?1 AND ended IS NULL",
                params![id, label, generation],
            )
            .map(drop)
        })
    }

    /// Records a harness launch before it is requested, so a successor
    /// knows of every launch that may have happened.
    pub(crate) fn begin_work(
        &mut self,
        harness: usize,
        incarnation: i64,
    ) -> Result<i64, StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO work (harness, incarnation, generation) VALUES (?1, ?2, ?3)",
                params![int(harness), incarnation, generation],
            )?;
            Ok(tx.last_insert_rowid())
        })
    }

    /// Records an accepted Bash run and its launch under `incarnation`
    /// before the launch is requested, in one transaction.
    pub(crate) fn begin_bash(
        &mut self,
        harness: usize,
        requester_work: i64,
        incarnation: i64,
        requester_pid: i32,
        inputs_open: &str,
        argv: &[String],
        cwd: &str,
    ) -> Result<i64, StoreError> {
        let generation = self.generation;
        let argv = serde_json::to_string(argv).expect("argv serializes");
        self.write(|tx| {
            tx.execute(
                "INSERT INTO work (harness, incarnation, generation, kind)
                 VALUES (?1, ?2, ?3, 'bash')",
                params![int(harness), incarnation, generation],
            )?;
            let work = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO bash_run (work, requester_work, requester_pid, inputs_open, argv, cwd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![work, requester_work, requester_pid, inputs_open, argv, cwd],
            )?;
            Ok(work)
        })
    }

    /// Commits a registered child in one transaction: its harness row (the
    /// next position), its one owed message (`text`, a freshly minted key)
    /// and its lineage to the exact parent work. Returns the position and
    /// the message. Nothing is reported admitted before this commits.
    pub(crate) fn admit_child(
        &mut self,
        child: &ChildAdmission<'_>,
    ) -> Result<(usize, OutboundMessage), StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            let position: i64 =
                tx.query_row("SELECT count(*) FROM harness", [], |row| row.get(0))?;
            tx.execute(
                "INSERT INTO harness (position, id, argv, endpoint, session, kind)
                 VALUES (?1, ?2, '[]', ?3, NULL, 'child')",
                params![position, child.id, child.endpoint.label()],
            )?;
            tx.execute(
                "INSERT INTO child (harness, parent, parent_work, route, requester_pid, inputs_open,
                                    admitted_generation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    position,
                    int(child.parent),
                    child.parent_work,
                    child.route,
                    child.requester_pid,
                    child.inputs_open,
                    generation
                ],
            )?;
            let message = OutboundMessage::fresh_recorded(child.text, |key: &MessageKey| {
                tx.execute(
                    "INSERT INTO message (harness, idx, key, text) VALUES (?1, 0, ?2, ?3)",
                    params![position, key.as_str(), child.text],
                )
                .map(drop)
                .map_err(|error| io::Error::other(error.to_string()))
            })
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            Ok((usize::try_from(position).unwrap_or(usize::MAX), message))
        })
    }

    /// Records the argv a child's launch was provisioned with.
    pub(crate) fn set_harness_argv(
        &mut self,
        harness: usize,
        argv: &[String],
    ) -> Result<(), StoreError> {
        let argv = serde_json::to_string(argv).expect("argv serializes");
        self.write(|tx| {
            tx.execute(
                "UPDATE harness SET argv = ?2 WHERE position = ?1",
                params![int(harness), argv],
            )
            .map(drop)
        })
    }

    /// Records how a child ended for its parent (never more than observed).
    pub(crate) fn resolve_child(
        &mut self,
        harness: usize,
        outcome: &str,
    ) -> Result<(), StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "UPDATE child SET outcome = ?2, resolved_generation = ?3
                 WHERE harness = ?1 AND outcome IS NULL",
                params![int(harness), outcome, generation],
            )
            .map(drop)
        })
    }

    pub(crate) fn record_work_spawned(
        &mut self,
        work: i64,
        host_pid: Option<i32>,
    ) -> Result<(), StoreError> {
        self.write(|tx| {
            tx.execute(
                "UPDATE work SET harness_host_pid = ?2 WHERE id = ?1",
                params![work, host_pid],
            )
            .map(drop)
        })
    }

    /// Records a launch's end as reported by `observer`, or why it is unknown.
    pub(crate) fn resolve_work(
        &mut self,
        work: i64,
        outcome: &str,
        observer: Option<&str>,
    ) -> Result<(), StoreError> {
        let generation = self.generation;
        self.write(|tx| {
            tx.execute(
                "UPDATE work SET outcome = ?2, observer = ?3, resolved_generation = ?4
                 WHERE id = ?1 AND outcome IS NULL",
                params![work, outcome, observer, generation],
            )
            .map(drop)
        })
    }

    /// Adds one observed closure and, in the same transaction, decides from
    /// the persisted counts whether a relaunch is still authorized: the
    /// durable `outage` stop once closures reach `closure_cap`, otherwise
    /// the `attempts-exhausted` stop once the message's attempts over every
    /// generation reach `attempt_cap`. Returns the durable closure total and
    /// the stop, if any.
    pub(crate) fn record_closure(
        &mut self,
        harness: usize,
        idx: usize,
        closure_cap: u32,
        attempt_cap: u32,
    ) -> Result<(u32, Option<&'static str>), StoreError> {
        self.write(|tx| {
            let closures: u32 = tx.query_row(
                "UPDATE message SET closures = closures + 1 WHERE harness = ?1 AND idx = ?2
                 RETURNING closures",
                params![int(harness), int(idx)],
                |row| row.get(0),
            )?;
            let attempts: u32 = tx.query_row(
                "SELECT count(*) FROM attempt WHERE harness = ?1 AND idx = ?2",
                params![int(harness), int(idx)],
                |row| row.get(0),
            )?;
            let stop = if closures >= closure_cap {
                Some(OUTAGE)
            } else if attempts >= attempt_cap {
                Some(ATTEMPTS_EXHAUSTED)
            } else {
                None
            };
            if let Some(stop) = stop {
                tx.execute(
                    "UPDATE message SET stop = ?3 WHERE harness = ?1 AND idx = ?2",
                    params![int(harness), int(idx), stop],
                )?;
            }
            Ok((closures, stop))
        })
    }
}

/// What [`Store::admit_child`] commits.
pub(crate) struct ChildAdmission<'a> {
    pub(crate) id: &'a str,
    pub(crate) endpoint: crate::Endpoint,
    pub(crate) parent: usize,
    pub(crate) parent_work: i64,
    pub(crate) route: &'a str,
    pub(crate) requester_pid: i32,
    pub(crate) inputs_open: &'a str,
    pub(crate) text: &'a str,
}

/// SQLite integers are `i64`; positions, indexes and times fit.
fn int<N: TryInto<i64>>(value: N) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

/// Creates `dir` owner-private, or checks that an existing one is.
fn private_dir(dir: &Path) -> Result<(), ClaimError> {
    if !dir.is_absolute() {
        return Err(ClaimError::NotPrivate(
            "store path is not absolute".to_owned(),
        ));
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(dir)?;
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if !meta.is_dir() {
        return Err(ClaimError::NotPrivate("not a directory".to_owned()));
    }
    if meta.uid() != euid {
        return Err(ClaimError::NotPrivate("owned by another user".to_owned()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(ClaimError::NotPrivate(format!(
            "mode {:o} is group/other accessible",
            meta.permissions().mode() & 0o777
        )));
    }
    Ok(())
}

fn create_intent(
    tx: &Transaction<'_>,
    intent: &Intent,
    generation: i64,
) -> Result<Vec<DurableHarness>, ClaimError> {
    tx.execute(
        "INSERT INTO intent (singleton, outage_closure_cap, delivery_attempt_cap, cwd, workload,
                             children, generation)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            intent.outage_closure_cap,
            intent.delivery_attempt_cap,
            intent.cwd,
            serde_json::to_string(&intent.workload)
                .map_err(|error| ClaimError::Store(error.to_string()))?,
            intent
                .children
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| ClaimError::Store(error.to_string()))?,
            generation
        ],
    )?;
    let mut harnesses = Vec::new();
    for (position, spec) in intent.harnesses.iter().enumerate() {
        let argv =
            serde_json::to_string(&spec.argv).map_err(|e| ClaimError::Store(e.to_string()))?;
        tx.execute(
            "INSERT INTO harness (position, id, argv, endpoint, session)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                int(position),
                spec.id,
                argv,
                spec.endpoint.label(),
                spec.session
            ],
        )?;
        let mut messages = Vec::new();
        for (idx, text) in spec.messages.iter().enumerate() {
            let message = OutboundMessage::fresh_recorded(text.clone(), |key: &MessageKey| {
                tx.execute(
                    "INSERT INTO message (harness, idx, key, text) VALUES (?1, ?2, ?3, ?4)",
                    params![int(position), int(idx), key.as_str(), text],
                )
                .map(drop)
                .map_err(|error| io::Error::other(error.to_string()))
            })?;
            messages.push(DurableMessage {
                message,
                follow_up: false,
                closures: 0,
                stop: None,
                ack: None,
                attempts: 0,
                prior_unknown: 0,
            });
        }
        harnesses.push(DurableHarness {
            id: spec.id.clone(),
            argv: spec.argv.clone(),
            endpoint: spec.endpoint,
            session: spec.session.clone(),
            messages,
            open_works: Vec::new(),
        });
    }
    Ok(harnesses)
}

fn load_intent(tx: &Transaction<'_>, generation: i64) -> Result<Vec<DurableHarness>, ClaimError> {
    let mut harness_rows = tx.prepare(
        "SELECT position, id, argv, endpoint, session FROM harness
             WHERE kind = 'intent' ORDER BY position",
    )?;
    let rows = harness_rows
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut message_rows = tx.prepare(
        "SELECT m.key, m.text, m.closures, m.stop, m.ack_label, m.ack_basis,
                m.ack_recovered, m.ack_generation, m.ack_message_id,
                (SELECT count(*) FROM attempt a WHERE a.harness = m.harness AND a.idx = m.idx),
                (SELECT count(*) FROM attempt a WHERE a.harness = m.harness AND a.idx = m.idx
                    AND a.outcome = ?2 AND a.resolved_generation = ?3),
                m.origin
         FROM message m WHERE m.harness = ?1 ORDER BY m.idx",
    )?;
    let mut work_rows = tx.prepare(
        "SELECT id, incarnation FROM work
         WHERE harness = ?1 AND kind = 'harness' AND outcome IS NULL ORDER BY id",
    )?;
    let mut harnesses = Vec::new();
    for (position, id, argv, endpoint, session) in rows {
        let endpoint = crate::Endpoint::parse(&endpoint)
            .ok_or_else(|| ClaimError::Store(format!("unknown endpoint {endpoint}")))?;
        let open_works = work_rows
            .query_map(params![position], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(i64, i64)>>>()?;
        let argv: Vec<String> =
            serde_json::from_str(&argv).map_err(|e| ClaimError::Store(e.to_string()))?;
        let messages = message_rows
            .query_map(params![position, UNKNOWN_PRIOR_OWNER, generation], |row| {
                let key: String = row.get(0)?;
                let text: String = row.get(1)?;
                let ack_label: Option<String> = row.get(4)?;
                Ok(DurableMessage {
                    // A restored key: supplied identity with unknown history.
                    message: OutboundMessage::new(
                        MessageKey::new(key).ok_or(rusqlite::Error::InvalidQuery)?,
                        text,
                    ),
                    follow_up: row.get::<_, String>(11)? == "follow-up",
                    closures: row.get(2)?,
                    stop: row.get(3)?,
                    ack: match ack_label {
                        Some(label) => Some(DurableAck {
                            label,
                            basis: row.get(5)?,
                            recovered: row.get::<_, Option<bool>>(6)?.unwrap_or(false),
                            generation: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                            message_id: row.get(8)?,
                        }),
                        None => None,
                    },
                    attempts: row.get(9)?,
                    prior_unknown: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        harnesses.push(DurableHarness {
            id,
            argv,
            endpoint,
            session,
            messages,
            open_works,
        });
    }
    Ok(harnesses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HarnessSpec;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "root-store-{name}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            eprintln!("owned-fixture: {}", dir.display());
            Self(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn intent() -> Intent {
        Intent {
            outage_closure_cap: 2,
            delivery_attempt_cap: 3,
            cwd: "/".into(),
            harnesses: vec![HarnessSpec {
                id: "h".into(),
                argv: vec!["peer".into()],
                endpoint: crate::Endpoint::Stdio,
                session: None,
                messages: vec!["one".into()],
            }],
            workload: crate::Workload::UnprivilegedUserns {},
            children: None,
        }
    }

    fn attempts(dir: &Path) -> Vec<(i64, Option<String>)> {
        let conn = Connection::open(dir.join(DB_FILE)).unwrap();
        let mut stmt = conn
            .prepare("SELECT generation, outcome FROM attempt ORDER BY id")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    /// A stale instance whose lock was lost (here: lock file replaced) is
    /// refused before it writes anything, and its write is absent.
    #[test]
    fn stale_generation_is_fenced_before_mutation() {
        let dir = Dir::new("fence");
        let mut stale = Store::claim(&dir.0, Some(&intent())).unwrap().store;
        assert_eq!(stale.generation(), 1);
        stale.begin_attempt(0, 0).unwrap();
        fs::remove_file(dir.0.join(LOCK_FILE)).unwrap();
        let successor = Store::claim(&dir.0, None).unwrap();
        assert_eq!(successor.store.generation(), 2);
        assert_eq!(successor.classified_unknown, 1);

        assert_eq!(
            stale.begin_attempt(0, 0),
            Err(StoreError::FenceLost { current: 2 })
        );
        assert_eq!(
            stale.record_closure(0, 0, 2, 3),
            Err(StoreError::FenceLost { current: 2 })
        );
        let rows = attempts(&dir.0);
        assert_eq!(rows, vec![(1, Some(UNKNOWN_PRIOR_OWNER.to_owned()))]);
        let reloaded = Store::claim(&dir.0, None);
        // The successor still holds the lock.
        assert!(matches!(reloaded, Err(ClaimError::OwnerLive)));
        let conn = Connection::open(dir.0.join(DB_FILE)).unwrap();
        let closures: u32 = conn
            .query_row("SELECT closures FROM message", [], |row| row.get(0))
            .unwrap();
        assert_eq!(closures, 0, "stale closure must not be recorded");
    }

    /// A live owner refuses a duplicate claim before any write, and a
    /// separate root's store is independent.
    #[test]
    fn duplicate_live_claim_is_refused_and_roots_are_separate() {
        let a = Dir::new("a");
        let b = Dir::new("b");
        let owner = Store::claim(&a.0, Some(&intent())).unwrap();
        assert!(matches!(
            Store::claim(&a.0, None),
            Err(ClaimError::OwnerLive)
        ));
        let conn = Connection::open(a.0.join(DB_FILE)).unwrap();
        let owners: i64 = conn
            .query_row("SELECT count(*) FROM owner", [], |row| row.get(0))
            .unwrap();
        assert_eq!(owners, 1, "refused duplicate wrote nothing");
        let other = Store::claim(&b.0, Some(&intent())).unwrap();
        assert_eq!(other.store.generation(), 1);
        drop(owner);
    }

    #[test]
    fn create_and_recover_requests_are_not_interchangeable() {
        let dir = Dir::new("forms");
        assert!(matches!(
            Store::claim(&dir.0, None),
            Err(ClaimError::NoIntent)
        ));
        drop(Store::claim(&dir.0, Some(&intent())).unwrap());
        assert!(matches!(
            Store::claim(&dir.0, Some(&intent())),
            Err(ClaimError::IntentExists)
        ));
    }

    /// A follow-up is the harness's next owed message, durable with its
    /// origin, and a recovery loads it like an intent message.
    #[test]
    fn follow_up_is_durable_owed_debt_with_its_origin() {
        let dir = Dir::new("follow");
        let mut store = Store::claim(&dir.0, Some(&intent())).unwrap().store;
        let (idx, mut message) = store.admit_follow_up(0, 4, Some("r1"), "again").unwrap();
        assert_eq!(idx, 1);
        assert!(message.is_owed());
        let key = message.key().as_str().to_owned();
        drop(store);
        // Loaded as a recovery loads it, without a second claim: a forked
        // child of a parallel test can briefly hold the dropped lock.
        let mut conn = Connection::open(dir.0.join(DB_FILE)).unwrap();
        let tx = conn.transaction().unwrap();
        let harnesses = load_intent(&tx, 2).unwrap();
        drop(tx);
        let messages = &harnesses[0].messages;
        assert_eq!(messages.len(), 2);
        assert!(!messages[0].follow_up);
        assert!(messages[1].follow_up);
        assert!(messages[1].ack.is_none());
        let row: (String, String, i64, String, i64) = conn
            .query_row(
                "SELECT key, origin, control, caller_ref, admitted_generation
                 FROM message WHERE idx = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row, (key, "follow-up".into(), 4, "r1".into(), 1));
    }

    /// Fresh-only schema: a store of the previous version is refused, not
    /// migrated, and no current-schema tables are added. Claim may create
    /// a lock file and configure WAL before it checks the version.
    #[test]
    fn previous_store_version_is_refused() {
        let dir = Dir::new("v4");
        fs::DirBuilder::new().mode(0o700).create(&dir.0).unwrap();
        let conn = Connection::open(dir.0.join(DB_FILE)).unwrap();
        conn.execute_batch("CREATE TABLE marker (x INTEGER); PRAGMA user_version = 4;")
            .unwrap();
        drop(conn);
        let refused = Store::claim(&dir.0, None).err().unwrap().reason();
        assert_eq!(refused, "store-error: unknown store version 4");
        let conn = Connection::open(dir.0.join(DB_FILE)).unwrap();
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            tables, 1,
            "no current-schema tables added to a refused store"
        );
    }

    #[test]
    fn group_accessible_directory_is_refused() {
        let dir = Dir::new("mode");
        fs::create_dir(&dir.0).unwrap();
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(matches!(
            Store::claim(&dir.0, Some(&intent())),
            Err(ClaimError::NotPrivate(_))
        ));
        assert!(!dir.0.join(DB_FILE).exists());
    }
}
