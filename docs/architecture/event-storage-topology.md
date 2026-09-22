# Durable event-storage topology

Status: **accepted for AGE-375**

Decision: **producer-partitioned SQLite/WAL event generations**

Primary implementation owner: **AGE-376**, composed with **AGE-372** retention
policy and the implemented **AGE-377** lifecycle in
[`detached-maintenance.md`](detached-maintenance.md)

Implementation status (AGE-376): the producer-partitioned version-1 envelope,
STRICT SQLite/WAL generations, process-local bounded writer, exact-head
rotation/recovery, checksummed two-slot head, prepared/sealed manifests,
bounded readers/reconciliation, preservation-mode JSONL, bounded legacy import,
bounded union reads, and non-destructive maintenance interfaces are implemented in
`crates/oulipoly-state/src/event_store/` and the diagnostic producer modules.
AGE-372 supplies policy approval and exact receipt inputs. AGE-377 implements
historical scheduling, singleton maintenance jobs, bounded coordination
retention/compaction, and exact event retirement. AGE-378 adds closed-schema,
bounded-cardinality longitudinal metrics, bounded cross-generation queries,
and trace exemplars, and activates the partitioned store as the normal
diagnostic sink with byte-identical JSONL fallback. Unsafe repair-copy or
quarantine requests remain fail-closed; maintenance never edits a corrupt
generation in place.

Evidence: [`planning/age-375-event-storage-evaluation/report.md`](../../planning/age-375-event-storage-evaluation/report.md)

## Decision

High-rate diagnostic events will use SQLite databases that are partitioned by
producer instance and rotated into bounded generations. Each process has one
event-writer worker and one writable head; different processes never append to
the same event database. Threads in one process group commits through that
process-local worker. This preserves SQLite transaction, recovery, and index
semantics without putting event append on the State or PID-mailbox writer and
without introducing one global event writer.

The choice is not “SQLite everywhere.” Correctness SQLite and event SQLite have
different files, connections, owners, schemas, lifetimes, and failure effects.
An event-store failure must not delay, roll back, repair, or grant live
coordination authority.

The measured fixture observed the shortest append interval for
producer-partitioned SQLite in both tested acknowledgement modes. At batch size
32 it reported a median 25,093 events/s, versus 16,994 for framed shards,
14,591 for Fjall, and 12,711 for the single SQLite broker. At one sync per
record it reported 1,235 events/s, versus 854, 711, and 551 respectively. The
framed timer, unlike the SQLite and Fjall timers, included one-time file and
empty-index creation/sync, so these observations are not a like-for-like
steady-state ranking and do not establish that SQLite is intrinsically faster
than framed storage. They are WSL2 smoke results, not an SLO or AGE-353 stress
result. Framed files and Fjall used about 5.6 MiB for the fixture versus about
19.7 MiB for SQLite; the selection rests primarily on transactional recovery,
atomic local indexes, existing portability, and bounded ownership rather than
the timing rank.

SQLite WAL still permits only one writer per database. Partitioning is what
removes the global writer, while WAL retains concurrent readers and a stable
per-reader end mark. `synchronous=FULL` is required for acknowledged event
batches. The SQLite documentation is the external source for these properties:

- <https://www.sqlite.org/wal.html>
- <https://sqlite.org/pragma.html#pragma_synchronous>
- <https://www.sqlite.org/atomiccommit.html>

## Authority boundary: what stays and what moves

No current coordination table moves in the implementation ticket. A record in
the event stream is evidence only. Its presence, absence, age, corruption, or
retirement grants no completion, launch, delivery, wake, process, repair,
retry, cancellation, quota, or session authority.

### Remains transactional in existing SQLite

The following correctness-critical families remain in the existing State and
PID-mailbox databases. Append-only shape does not make a table non-authoritative.

| Family | Tables / state | Why it remains transactional |
|---|---|---|
| Invocation and provider routing | `invocations`, `providers`, `provider_quotas`, `provider_quota_windows`, `model_round_robin_cursor` | Running/terminal status, routing, quota, and retry decisions. |
| Provider launch authority | `provider_logical_launches`, `provider_launch_attempts`, `provider_launch_native_channel_duties`, `provider_launch_transition_replays` | Launch fencing, idempotent transition replay, cancellation, and continuing native custody. |
| Completion authority | `invocation_completion_obligations`, `invocation_completion_continuity`, `invocation_completion_continuity_recovery`, `invocation_completion_v2_identity`, `invocation_completion_authority_summary`, `invocation_completion_materialization_summary`, `completed_turn_selections`, `completed_turns` | Admission, exact continuity, recovery working set, and committed completion results. |
| Mailbox and notification settlement | `mailbox`, `mailbox_delivery_attempts`, `mailbox_delivery_attempt_items`, `completion_event`, `completion_event_listener`, `completion_continuation_notification`, `mailbox_completed_turn_pins`, `mailbox_completed_turn_tails` | Delivery, acknowledgement, unresolved work, payload custody, and retirement predicates. |
| Root/continuation authority | `completion_continuation_domain`, `completion_continuation_owner`, `completion_continuation_source`, `completion_continuation_attempt`, `completion_supervisor_authority`, `completion_supervisor_inheritance` | Root ownership, inherited scope, source acceptance, process custody, and drain proof. |
| Runtime/wake/admission authority | `runtime_generation`, `runtime_generation_custody`, `session_runtime`, `session_wake_claim`, `session_admission_queue`, `wake_sweep_progress`, `mailbox_notification_control` | Exact live process identity, wake single-flight, FIFO admission, and current control settings. |
| Session delivery/lifecycle | `session_supervisor_leases`, `provider_turn_generations`, `session_lifecycle_events`, `session_lifecycle_event_dispositions`, `session_lifecycle_sequences`, `session_external_ingress`, `session_external_ingress_cursors`, `session_delivery_acknowledgements`, `session_delivery_evidence` | Fenced turn state, ingress cursor, and submitted/confirmed evidence. |
| Session and transcript state | `session_turns`, `session_turn_ingest_streams`, `session_chains`, `session_chain_segments`, `session_chain_segment_provider_authority`, `invocation_provider_session_authority`, `owned_turn_events`, `fresh_continuations`, `imported_session_display_metadata` | Resume, transcript ingestion, compaction boundaries, provider-session binding, and chain selection. |
| Durable returned data | `invocation_returned_artifacts`, `invocation_output_deliveries` and referenced payload artifacts | User-visible result identity and delivery state. |
| Configuration/setup/deployment | `memory_nodes`, `memory_edges`, `setup_sessions`, `setup_turns`, `cli_providers`, `accounts`, `discovered_models`, `model_parameters`, deployment metadata and row-version state | Mutable configuration, schema compatibility, and setup recovery. |

Terminal rows in those tables may later be pruned by the dedicated retention
work, but an event mirror is never their authority and is not a prerequisite
for live pruning. In particular, the historical classification of
`provider_launch_transition_replays` or a completion continuity ledger does
not authorize moving it out of the transaction that enforces its replay or
chain invariants.

### Moves to the event stream

The primary durable sink for these non-authoritative record families moves to
the partitioned event store:

1. AGE-369 `DiagnosticEvent` flight-recorder records, including SQLite
   observations, runtime-cap evidence, live/history barrier rejections, and
   diagnostic span phases.
2. Invocation lifecycle observations produced by `lifecycle_log.rs`
   (`invocation.started`, capture, finalization, and their failure records).
3. Runtime traces and diagnostic envelopes that are currently logging/tracing
   side effects rather than coordination rows.
4. AGE-378 bounded longitudinal metric samples and trace exemplars.
5. Structured application/maintenance logs whose loss affects diagnosis but
   not live correctness.

The existing private JSONL flight recorder remains as an emergency fallback
when the event store cannot initialize or append. It is no longer the normal
search sink. A detached importer copies fallback records into event partitions
under the compatibility protocol below. `Queued` continues to mean not yet
durable; only a successful event-partition commit is `Appended`.

Derived mirrors of terminal coordination rows may be emitted for diagnosis,
but the word “mirror” is part of their schema and readers must link back to the
authoritative row. The implementation ticket must not dual-write an authority
transition across databases and call the pair atomic.

## On-disk topology and ownership

The versioned root is private to the user:

```text
diagnostics/event-store-v1/
├── writers/
│   └── <writer-instance-id>/
│       ├── HEAD.0
│       ├── HEAD.1
│       ├── leases/
│       │   └── <generation-id>.lock
│       ├── staging/
│       │   └── <generation-id>.tmp/
│       └── generations/
│           └── <generation-id>/
│               ├── events.sqlite3
│               ├── events.sqlite3-wal   # when retained by SQLite
│               ├── events.sqlite3-shm   # transient; included if present
│               ├── prepared.manifest.json
│               └── sealed.manifest.json # closed generations only
├── catalog/
│   ├── CURRENT.0
│   ├── CURRENT.1
│   └── catalog-<epoch>.sqlite3
├── retirement/
│   ├── trash/<writer-instance-id>/<generation-id>.pending/
│   └── receipts/<writer-instance-id>/<generation-id>.json
└── quarantine/
```

- A native `writer_instance_id` is a random 128-bit process-instance ID bound in
  the manifest to exact PID/boot/start-time identity and its parent/root process
  instance IDs. A reused PID cannot inherit the writer identity. Legacy-import
  partitions use the explicit deterministic, provenance-marked exception below;
  they are never presented as a live process writer.
- Exactly one event-writer worker owns a writer directory and its active SQLite
  connection. The root-supervisor control loop only submits bounded messages;
  it never opens historical generations or performs catalog, checkpoint,
  integrity, compaction, repair, or retention work.
- Each writer instance has one logical head represented by the two fixed
  control-record slots `HEAD.0` and `HEAD.1`. Different writer instances have
  different generation directories and database files, so their commits do not
  share a WAL writer lock.
- A generation directory is the physical publication, quarantine, and
  retirement unit. The database, any WAL, any shared-memory sidecar, and its
  manifests are never moved or deleted individually. SQLite's WAL documentation
  explicitly warns that separating a database from its WAL can lose committed
  transactions or corrupt the database.
- Per-generation lease files live outside generation directories so an
  AGE-377 worker can hold the exclusive lease while renaming the directory on
  platforms that reject renaming a directory containing an open handle.
- `prepared.manifest.json` is immutable. It records schema version, writer and
  generation IDs, predecessor, relative database name, creation/head epoch, and
  the matching immutable metadata-row values; it does not pretend to hash the
  later-mutating database/WAL contents. `sealed.manifest.json` is published only
  after closure and records final durable DB/WAL identities, sizes, digests,
  time bounds, row/high-water counts, and any transient sidecars separately.
- Every temporary directory, final generation directory, and retirement trash
  directory must be on the same filesystem. If atomic same-filesystem directory
  rename or durable file/directory sync is unavailable, event-store
  initialization fails to the explicit JSONL fallback rather than weakening the
  protocol.

Generation, manifest, trash-directory, receipt, and catalog-database final names
are create-once. On resume, an existing object or head epoch is accepted only
when its identity and digest equal the pending operation (`AlreadyPublished`);
unequal content is `PublicationConflict`. Temporary names carry a fresh nonce
and are never treated as committed. Only the inactive bounded control slot is
removed for reuse while the other validated slot remains durable.

There is no mandatory globally writable catalog. Per-writer manifests are the
source of partition discovery. A detached worker publishes the read-optimized
cross-partition catalog; it is derived and rebuildable. `CURRENT.0/.1` use the
same two-slot selection rule as `HEAD.0/.1`, but catalog absence or invalidity is
only a coverage issue because the catalog is not authority.

## Logical record framing

There is no custom byte frame. One row is the logical frame, and one SQLite
transaction containing the row plus all local index changes is the atomic
physical publication. SQLite owns page/WAL checksums and torn-transaction
recovery.

The version-1 `STRICT` event table has these required fields:

| Field | Contract |
|---|---|
| `local_sequence INTEGER PRIMARY KEY` | Monotonic only within one generation; not global chronology. |
| `event_id BLOB UNIQUE NOT NULL` | Exactly 16 bytes, generated once before sink fan-out and reused for shadow, fallback, import, and retry. |
| `schema_version INTEGER NOT NULL` | `1` for this envelope. Unknown versions remain discoverable but are not decoded. |
| `family INTEGER NOT NULL` | Bounded enum: diagnostic, trace, metric, log, maintenance. |
| `kind TEXT NOT NULL` | Registered label, UTF-8, at most 64 bytes; no arbitrary user input. |
| `recorded_at_unix_micros INTEGER NOT NULL` | Producer occurrence wall time for range search. For AGE-371 lifecycle records this is parsed from their authoritative `recorded_at`, not resampled during event-store submission. |
| `ingested_at_unix_micros INTEGER NOT NULL` | Event-writer wall time for retention and lag. |
| `producer_sequence INTEGER NOT NULL` | Monotonic source-local ordering; unique with writer instance. A native writer instance is one producer source; each legacy source is its own import writer. |
| `writer_instance_id`, `process_instance_id`, `process_root_id` | 16-byte stable correlation IDs. |
| `parent_process_instance_id` | Nullable 16-byte explicit tree edge. |
| `supervisor_authority_id` | Nullable stable root authority correlation; never an authority token. |
| `trace_id`, `span_id`, `parent_span_id` | 16-byte trace/span IDs; parent nullable. |
| `invocation_uuid` | Nullable stable runner invocation UUID. |
| `session_correlation_sha256` | Nullable 32-byte digest; raw provider session IDs are not telemetry labels. |
| `payload_codec` | Versioned enum; version 1 permits UTF-8 JSON only. |
| `payload`, `payload_sha256`, `payload_bytes` | Maximum 1 MiB, digest checked on read/import/repair. |

Required checks reject malformed fixed-width IDs, negative timestamps or
sequences, unknown family/codec values, payload-length disagreement, and
payloads over 1 MiB. Sensitive correlations, prompts, credentials, claim
tokens, raw SQL/binds, transcript bodies, and arbitrary paths retain the
AGE-369 exclusion/redaction contract.

`event_id` identifies a logical event. `(writer_instance_id, generation_id,
event_id)` is its original physical address. Wall time plus `event_id` is the
stable display order; it is not causality. Trace parent IDs, process parent IDs,
and producer sequence express causal/local order across clock discontinuities.

AGE-376 must move envelope construction ahead of all sink selection. The
producer assigns the event ID, normalized payload, payload digest, producer
sequence, and correlations exactly once; the event-store append and JSONL
shadow/fallback receive byte-equivalent immutable envelope fields. A sink must
not mint an ID inside `append`. This is what makes shadow comparison and retry
refer to one logical event rather than two observations that merely look alike.

The version-1 logical identity digest includes the event ID, schema/family/kind,
recorded time, stable process-tree/native-process identity, typed correlations,
and normalized payload metadata and bytes. It deliberately excludes physical
placement and import metadata: `writer_instance_id`, `ingested_at`,
generation/retry linkage, and `legacy_provenance`. Producer sequence remains
part of logical identity because it is assigned once before fanout. Native
pre-fanout copies retain those fields byte-for-byte. A detached importer may
replace writer/physical partition placement and attach provenance while
retaining the same logical event ID, producer sequence, and digest; the
original writer value remains explicit in its source evidence rather than
being mistaken for the import partition's fact.

## Append, durability, and unknown outcomes

The process-local writer groups at most 32 records, 1 MiB of encoded payload,
or 10 ms of queue residence, whichever occurs first. A group is one SQLite
transaction under `journal_mode=WAL` and `synchronous=FULL`. Configuration is
verified on every new generation; a silent downgrade is an append failure.

An append ticket fixes the event ID, writer instance, and generation before the
first write. Success is returned only after `COMMIT` succeeds. If the commit
result is unknown (connection loss, process death, or an I/O error whose commit
state cannot be proven), retry follows this protocol:

1. Reopen the exact generation from the ticket and select by `event_id`.
2. If the row exists and `payload_sha256` plus immutable envelope fields match,
   return `AlreadyCommitted`.
3. If the ID exists with different immutable bytes, return `IdentityConflict`.
4. If absence is read from a healthy generation, retry with the same ID. If the
   original generation has closed, append to the current head with
   `retry_of_generation_id`; logical readers deduplicate by ID.
5. If the generation is missing, corrupt, or unreadable, return `Unknown` and
   do not blind-retry. Missing evidence is not absence.

Within one partition, `INSERT ... ON CONFLICT(event_id) DO NOTHING` followed by
exact digest comparison is idempotent. Cross-partition duplicate IDs with equal
digests are one logical event and a repair issue; unequal digests are a hard
conflict. No event retry changes coordination state.

Backpressure is bounded. Best-effort observations may return an explicit
`Dropped`/gap reason. Once admitted, their `PendingAppend` and exact pre-fanout
envelope move through a second bounded nonblocking handoff to the recorder
worker, which waits outside State/PID-mailbox producer threads. A successful
normal-mode commit produces no JSONL copy; an unknown write, identity/append
failure, or lost reply routes that same envelope nonblockingly to emergency
JSONL. Completion-handoff or fallback loss emits an explicit bounded gap.
Required-durable callers use the same nonwaiting bounded queue admission and,
once accepted, wait for the real commit/reconciliation outcome only at the
top-level safe boundary before acquiring a State or PID-mailbox writer. No
synthetic timeout turns unfinished persistence into a fabricated failure.
The process-sink lifecycle owner is used only to take the exact writer once for
shutdown; a required-durable caller does not hold it, the writer admission
mutex, or the shared sender slot while waiting for its reply. Consequently,
other durable callers retain bounded admission/group-commit opportunity and a
best-effort caller reaches the real queue whenever that queue and admission
fence permit it. Shutdown first closes the shared admission fence, removes the
shared sender, drains work accepted before the fence, and closes the taken
writer exactly once. Neither append path may hold the State or PID-mailbox
SQLite writer while waiting for event persistence.

Event-store SQLite operations do not recursively emit per-transaction
`DiagnosticEvent` records into the same sink. They update bounded in-memory
counters that a later safe boundary may flush; event-writer failures use the
emergency JSONL gap path. Top-level AGE-369 `Requested` may wait for the event
writer only before the protected coordination transaction, while later phases
retain the existing nonblocking submission rule.

## Local indexes and query contract

Every generation creates these indexes in its schema transaction:

- unique `event_id`;
- `(recorded_at_unix_micros, event_id)`;
- `(family, kind, recorded_at_unix_micros, event_id)`;
- `(trace_id, recorded_at_unix_micros, event_id)` and `(trace_id, span_id)`;
- `(invocation_uuid, recorded_at_unix_micros, event_id)`;
- `(session_correlation_sha256, recorded_at_unix_micros, event_id)`;
- `(process_root_id, process_instance_id, recorded_at_unix_micros, event_id)`;
- `(supervisor_authority_id, recorded_at_unix_micros, event_id)`.

Index changes commit atomically with rows, so there is no local side-index
publication window. The detached catalog records partition ID, writer,
generation, file identity, schema, min/max recorded and ingested times, family
bitset, row count, and Bloom filters for trace/invocation/process correlation.
Bloom filters only exclude partitions; the local SQLite index decides matches.

Catalog publication builds `catalog-<epoch>.sqlite3.tmp`, validates and syncs
it, renames it to its final absent name, and syncs the catalog directory. It
then publishes the inactive `CURRENT.0/.1` slot by the two-slot protocol. Its
watermark lists every included manifest digest. A stale or missing catalog
causes a bounded manifest fallback or an explicit incomplete-coverage result;
it never turns a trace or range query into an authoritative empty result.

An exact event address needs no catalog. A trace join resolves candidate
partitions using catalog filters, queries each local trace index, merges by
`(recorded_at,event_id)`, and validates parent IDs. Missing parents are reported,
not fabricated. Current heads are always queried directly because detached
catalog coverage may lag them.

Ordinary stable-ID, trace, and metric target discovery also performs a bounded
walk of the discovery archive after pending and active work. That archive walk
returns only `preserved_dead_head` generations (plus readable gap evidence), but
every archive entry examined consumes the same caller-supplied node and entry
budgets. If unrelated terminal records exhaust either budget before all
preserved heads are examined, coverage is explicitly incomplete; a bounded
omission is never reported as complete emptiness. Archived work is not returned
to detached maintenance, and the root supervisor does not enumerate or own the
historical store.

Each SQLite read transaction is a consistent snapshot of one generation.
There is deliberately no false global snapshot across independent writers.
Every response carries per-partition high-water sequence, manifest/catalog
watermark, skipped/missing/corrupt partitions, and limits reached. Closed
generations are immutable; current-head results may legitimately omit records
committed after that head's read transaction began.

## Rotation and atomic head publication

Rotation occurs at the first batch boundary after any of:

- UTC day changes;
- database plus WAL reaches the 64-MiB soft threshold (overshoot is bounded by
  one group);
- producer lifecycle shutdown requests best-effort closure; or
- operator/test requests an explicit rotation.

The event-writer worker performs only current-head rotation. Publication uses
one directory rename plus a two-slot head record; it never attempts to rename a
flat database/WAL pair:

1. Create `staging/<successor>.tmp/` on the event-store filesystem. Create
   `events.sqlite3`, set and read back `journal_mode=WAL` and
   `synchronous=FULL`, install the schema, and commit the immutable generation
   metadata row.
2. Because this successor has not accepted events, run
   `PRAGMA wal_checkpoint(TRUNCATE)` and require a non-busy complete result.
   Close every successor SQLite handle. Sync `events.sqlite3` and an existing
   WAL, publish `prepared.manifest.json` by write-temp/file-sync/rename, then
   sync the staging generation directory. Reopen read-only and validate
   `quick_check`, schema/indexes, and equality between the manifest and
   metadata row; close it again.
3. Rename the whole staging directory to the absent final path
   `generations/<successor>/`, then sync both the staging and generations parent
   directories. Revalidate the prepared manifest and database through the final
   path. At this point the successor is durable and discoverable but not a head.
4. Fence new admissions. Batches already ticketed to the old head either commit
   there or retain an unknown outcome; no ticket is silently retargeted. In one
   `synchronous=FULL` old-head transaction set `state='closed'`, `closed_at`,
   and the exact successor ID, then close the old writer handle. Rotation does
   not request a blocking historical checkpoint.
5. Publish head epoch `N+1` into the inactive head slot. Remove only that stale
   inactive slot and sync the writer directory while the other slot remains
   valid; write a checksummed `HEAD.<slot>.tmp`, sync it, rename it to the now
   absent `HEAD.<slot>`, and sync the writer directory. The record names the
   successor and the SHA-256 of its prepared manifest. This rename never relies
   on overwrite semantics.
6. Open the successor at its final path, set and read back the connection-local
   `synchronous=FULL`, verify persistent WAL mode and metadata once more, swap
   the in-memory connection, release admissions, submit a detached follow-up
   request, and return. No event may enter the successor before step 5 is
   durable.

A head reader reads exactly `HEAD.0` and `HEAD.1`, validates record checksums,
manifest digests, generation-directory identity, and database metadata, and
selects the highest valid epoch. Equal-epoch disagreement is corruption. An
invalid higher slot is reported as `head_publication_incomplete`; the lower
valid slot remains readable, but if its database says `closed` a writer must not
append there. Writer recovery follows that closed generation's exact successor
link, validates the already-durable successor, and republishes the next head
epoch. It does not enumerate history.

A crash before the final generation-directory rename leaves a staging orphan.
A crash after it but before old-head closure leaves an unreferenced prepared
generation. A crash after old-head closure but before head publication leaves a
closed selected head with an exact prepared successor. A crash during inactive
slot replacement leaves either only the previous valid slot or both valid slots;
it never exposes a head whose manifest was not already synced and validated.
Unreferenced candidates and temporary files are not deleted at startup; AGE-377
classifies them through the exact detached orphan-classification job. Actual
location is authoritative for that classification: a retained staging basename
cannot make an artifact at the final generation path “incomplete staging.” A
manifest-less, corrupt, or otherwise unprovable final generation is preserved.

The original acceptance obligation remains interruption discrimination at every
create, sync, checkpoint, close, and rename boundary in this publication path.
Current infrastructure proves the following exact visible boundary states; it
does not convert the remaining obligations into optional work:

| Boundary | Current executable discrimination |
|---|---|
| Durable discovery intent before staging creation; invalid intent slot | `supported_pre_head_interruption_states_have_production_recovery_successors` distinguishes both states and their bounded maintenance successor. The individual intent-record temp create, file sync, rename, and directory sync are not separately fault-injected. |
| Staging-directory creation | The same fixture distinguishes a created empty staging directory from intent-only state. Interruption during `create_dir` and the following permission change is not injectible in the current harness. |
| Successor database creation/schema commit, `wal_checkpoint(TRUNCATE)`, SQLite-handle close, and database sync | The same fixture constructs the state after a complete checkpoint, closes the handle, syncs the database, and verifies bounded recovery. It does not inject inside SQLite database creation/schema commit, inside checkpoint, at handle close, or inside the sync syscall. No fixture forces the optional surviving-WAL sync boundary. |
| Prepared-manifest temp create/write/file sync before rename | `interruption_after_prepared_manifest_temp_sync_is_not_generation_publication` proves the synced temp is not a published manifest or generation. The temp-create and partial-write boundaries are not separately injected. |
| Prepared-manifest rename before staging-directory sync | The pre-head recovery fixture publishes the manifest by rename while deliberately omitting the following directory sync, then verifies bounded cleanup. Interruption during the directory sync itself is not injected. |
| Final generation-directory rename before parent-directory sync | The pre-head recovery fixture renames staging to the final generation path while deliberately omitting both parent syncs and verifies bounded recovery. The staging-parent and generations-parent sync calls are not individually fault-injected. `recovery_does_not_adopt_prepared_successor_before_old_close` separately proves that a complete unselected generation is not adopted. |
| Old-head `synchronous=FULL` close transaction and SQLite-handle close before head publication | `recovery_follows_only_exact_successor_after_old_close_before_head` proves recovery follows only the recorded exact successor after the completed close. It does not inject inside the SQLite commit or the handle-close operation. |
| Stale inactive-head removal and writer-directory sync | The reader fixtures cover an invalid higher inactive slot, but no current fixture injects immediately after removal or during its directory sync. Those boundaries remain unmet. |
| Inactive head-temp create/write/file sync before rename | `interruption_after_inactive_head_temp_sync_keeps_old_head_selected` proves a synced temp is not selected. Temp creation and partial write are not separately injected. |
| Inactive head rename before final writer-directory sync | `interruption_after_inactive_head_rename_selects_only_the_complete_successor` proves the renamed, fully validated successor is selected without relying on the temp name. Interruption during the final directory sync remains unmet. |

These are deterministic filesystem/SQLite state fixtures, not a VFS power-loss
campaign. The explicitly unmet syscall-internal and individual-sync evidence
above remains part of the every-boundary obligation.

Processes alive across many days rotate their own heads by the same protocol.
The process-global sink lifecycle distinguishes `never_installed`, `installed`,
and terminal `shutdown`. Lazy default writer construction and shutdown share
the lifecycle mutex: installation that wins becomes visible and is taken for
closure, while shutdown that wins rejects first or replacement installation
before a writer or head is created. Every ordinary production entrypoint return
terminalizes this lifecycle, removes any installed process-local event sink from
admission, and invokes that exact writer's best-effort shutdown path. Repeated
shutdown is a no-op and cannot reopen installation. Successful shutdown drains
accepted work and performs the lifecycle rotation above; failure is emitted as
an event-store evidence gap without changing the command's exit status. Abrupt
process death cannot run this path: its selected head remains fail-closed and is
neither finalized nor transferred by detached maintenance.
Root-supervisor startup does not enumerate writer directories. Event-sink
initialization reads its own two exact head slots; historical discovery belongs
to explicit readers and detached maintenance.

## Detached maintenance, repair, and corruption

Ticket ownership is fixed as follows:

| Ticket | Owns | Does not own |
|---|---|---|
| AGE-372 | The implemented 30-day retention policy/engine: per-family policy, typed preservation of live/unresolved/unknown authority, and an exact generation approval bound to writer/generation and manifest digests. | Event-store layout, worker scheduling, leases, or filesystem deletion. |
| AGE-376 | Event-store append/read implementation; current-head rotation primitives; generation/head/receipt formats; eligibility metadata; preservation-mode JSONL producer changes; bounded importer/cutover operations and their contracts; and fixtures/interfaces for catalog, repair, rebuild, compaction, and retirement. | Historical job scheduling, singleton maintenance leases, or executing destructive historical maintenance from the root supervisor. |
| AGE-377 | Pre-spawn singleton scheduling, durable per-job/per-generation singleton leases, isolated sharded discovery, orphan classification, checkpoint/seal/retirement, bounded AGE-372 retention and compaction, rotation follow-up, and production admission of AGE-376 catalog/index inspection. | Retention policy decisions, live-head append/rotation, or inventing repair-copy/catalog publication authority absent from AGE-376. |

The three tickets compose rather than duplicate work: AGE-376 exposes immutable
facts and callable boundaries, AGE-372 decides policy eligibility, and AGE-377
executes an eligible operation under its detached lease. AGE-376 may exercise
the operations with isolated fixtures and may expose an explicit bounded/offline
import command, but no root-supervisor path schedules or performs historical
work.

The following are never root-supervisor work: WAL checkpoint/truncation on a
closed generation, `ANALYZE`, cross-partition catalog build, integrity scan,
index rebuild, repair copy, orphan classification, compaction/VACUUM, or retention.
Rotation only publishes a request; it does not wait for maintenance. AGE-377
fails closed when AGE-376 says a repair copy is required or when bounded catalog
inspection lacks a resumable cursor/publication interface; it does not claim an
unsupported rebuild, repair, quarantine, or catalog publication occurred.

A detached AGE-377 worker must hold its per-job singleton and the exact
generation lease. Destructive retirement works only on a closed generation;
orphan classification may discharge an exact unselected empty prepared or
incomplete staging artifact only after proving its producer dead. Neither path
may open or retain the current head writer. Progress is bounded and checkpointed.
The orphan worker publishes a stable create-once disposition before moving the
exact source, then resumes deterministic pending-trash unlink from its durable
cursor; it does not reclassify remaining files after a crash.

- A missing expected generation directory is `retirement_in_progress` when its
  exact pending-trash path exists, `retired` when a valid receipt names its
  manifest digests, and otherwise `missing_partition`.
- SQLite open/query/`quick_check` failure is `corrupt_partition`. Other
  partitions remain readable; the query reports incomplete coverage.
- A future repair owner may copy verifiable rows into a new staging generation,
  verify payload digests, build indexes there, and publish through a dedicated
  replacement protocol. AGE-376 does not currently expose that publication
  authority, so AGE-377 preserves the original and records the exact gap.
- If a current head is corrupt, that writer fails its event sink, emits the
  bounded emergency JSONL gap, and may publish a fresh successor with
  `predecessor_status='corrupt'`. It does not claim the missing head was empty.
- AGE-377 invokes the AGE-376 local-index planner only for a closed, non-head
  generation. `RepairCopyRequired` is a preserved terminal outcome, not an
  in-place `REINDEX`. Catalog inspection is bounded and admitted, but a catalog
  build is not claimed because AGE-376 has no authoritative publication API.

## Thirty-day retirement

Default event retention is 30 days for diagnostic, trace, metric, log, and
maintenance families. AGE-372 considers an event generation eligible only when
AGE-376 metadata proves it is closed, has valid prepared and sealed manifests,
has no hold, is not the selected head, and
`max(closed_at, max_ingested_at) <= as_of - 30 days` (inclusive). A proven
empty generation uses `closed_at`. AGE-377 additionally
requires absence of writer/reader/maintenance leases before execution. Unknown
timestamps, incomplete manifests, live leases, corruption, and unclassified
legacy input fail closed against deletion.

AGE-377 retires one complete generation directory with this resumable protocol:

1. Acquire the exclusive retention lease, re-evaluate the AGE-372 decision, and
   re-read both exact head slots. A reader acquires a shared generation lease
   before opening SQLite and revalidates the generation location after the
   lease; therefore a successful exclusive lease proves no open reader/writer
   races the move.
2. Open the closed database, run `PRAGMA wal_checkpoint(TRUNCATE)`, and require
   a non-busy complete result. Run `quick_check`, validate row/payload digests
   in resumable slices, close every SQLite handle, then sync the database and
   any WAL. SQLite may remove or retain a zero-length WAL on clean close; the
   application never moves or deletes it separately. Payload byte targets yield
   between rows rather than rejecting a large valid row. The sealed digest and
   validation phase are checkpointed so later slices do not repeat these
   operations.
3. Hash each immutable durable file once while deriving
   `sealed.manifest.json`, publish it create-once, and sync the generation
   directory. Retirement later re-reads the manifest and performs a second
   complete hash traversal to independently revalidate its recorded file
   identities immediately before destruction. Both traversals are detached;
   neither has a hard time or correctness-size cutoff.
4. Re-read both head slots, persist the destructive-ready proof, move the exact
   discovery leaf to pending, and re-read epoch-fenced cancellation under the
   same transition gate used by cancellation. Then
   atomically rename the single directory
   `writers/<writer>/generations/<id>/` to the absent same-filesystem path
   `retirement/trash/<writer>/<id>.pending/`. Sync both source and trash parent
   directories. This one rename moves the database, WAL, sidecars, and
   manifests together.
5. Publish `retirement/receipts/<writer>/<id>.json` by
   temp/file-sync/rename/parent-sync. The receipt contains the prepared/sealed
   manifest digests, AGE-372 policy version/cutoff, original and trash paths,
   and retirement epoch. Only after the receipt is durable may AGE-377 update
   the derived catalog and unlink the trash directory in bounded resumable
   batches.

Before step 4, readers use the generation path. After the directory rename but
before its receipt, an exact lookup checks the deterministic pending-trash path
and reports `retirement_in_progress`; it does not return an authoritative empty
result or scan trash. After a valid receipt, absence is `retired`, not
`missing_partition`, even while bounded unlink is incomplete. A crash before
the directory rename leaves the source intact; a crash after it resumes from
the exact trash path; a crash during unlink resumes from the receipt. Invalid
or conflicting source/trash/receipt states are `retirement_inconsistent` and
fail closed for deletion and complete-query claims.

Coordination-table retention uses the AGE-372 policy engine too, but deleting an
event generation never authorizes deleting a coordination row.

## Migration and cutover

AGE-376 owns the producer-side preservation/shadow/fallback changes and the
bounded importer/cutover contract. Before any shadow writing, its explicit
detached/offline preservation activator acquires the legacy retention lock,
requires incompatible legacy writers to be quiesced/restarted, inventories the
then-present sources into a coverage manifest, and publishes an exact, durable
`PRESERVE-LEGACY-V1` marker in the recorder root. A pre-marker deletion cannot
be reconstructed and is reported as baseline coverage uncertainty. Updated
recorder initialization checks that exact path before cleanup and then obeys
these rules:

- `FlightRecorder::open` serializes its exact-marker decision and active-shard
  creation/lease with activation under the same retention lock. Thus activation
  either inventories after detecting/refusing the active legacy writer, or the
  opener observes the durably published marker. Open does not enumerate the
  recorder directory and does not invoke stale-shard cleanup in preservation
  mode;
- rotation closes the exact current shard and creates a uniquely named
  immutable successor; it never reuses/truncates ordinal names or deletes the
  fourth shard. A legacy-mode writer reacquires the retention lock and rechecks
  the exact marker before every ordinal truncate/rename/unlink; present or
  unreadable state permanently switches it to create-once preservation
  rotation; and
- the seven-day/aggregate cleanup and explicit `cleanup_stale_shards` request
  perform no scan or deletion in-process. The request may emit a bounded
  detached-work notification and returns `preservation_deferred`; AGE-377 work
  is gated by an import receipt and AGE-372 policy.

Shadow/cutover must not start unless the running binary recognizes preservation
mode and the marker is synced. This intentionally suspends the old four-shard,
seven-day, and aggregate deletion bounds; disk growth is visible and accepted
during compatibility so importer inputs cannot disappear. Root-supervisor
processes only append/rotate their exact current fallback shard. They never
enumerate, import, or delete historical JSONL.

The AGE-376 importer is an explicit bounded detached/offline operation, not a
root startup task. A closed source gets a stable `legacy_source_id` equal to
SHA-256 over a domain separator, complete-file SHA-256, file length, and parsed
producer identity. Its checkpoint and completion receipt record that ID,
complete-line byte offset/ordinal, prefix digest, supported/unsupported/torn
counts, and imported event IDs. Device/inode and path are change-detection
evidence only; rename does not change identity. Active, changing, torn-tail,
unreadable, or replaced sources remain preserved and explicitly incomplete.
The source read itself is capped at one byte beyond the declared limit, so a
file that grows after its first metadata read cannot turn a bounded operation
into an unbounded allocation.

Legacy field derivation is deterministic and provenance-bearing:

| Legacy input | Version-1 derivation |
|---|---|
| AGE-369 diagnostic event with valid `event_id` | Preserve the ID. Otherwise use the first 16 bytes of SHA-256 over `oulipoly.legacy-event.v1`, `legacy_source_id`, complete-line byte offset, and line SHA-256. |
| Producer/process identity | Preserve non-nil `producer_instance` as `process_instance_id`. Otherwise derive a 16-byte domain-separated hash from `legacy_source_id` and the available PID/boot/start tuple. Route each legacy source to a deterministic import partition whose `writer_instance_id` is a separate domain-separated hash of `legacy_source_id`. |
| Producer sequence and process tree | Preserve an existing version-1 envelope's pre-fanout producer sequence. For older diagnostic/lifecycle inputs, use the zero-based complete-record ordinal within that immutable source. If no stable root identity exists, set `process_root_id=process_instance_id`, leave parent null, and mark provenance `synthetic_self_root`; never infer a parent from PID alone. |
| Trace/span/time | Preserve valid diagnostic/span/parent UUID bytes and parsed RFC3339 record time. Missing/invalid required values make the record unsupported rather than guessed. Import time is `ingested_at`; original time remains `recorded_at`. |
| Payload/correlations | Normalize through the version-1 redactor, calculate length/SHA-256 after normalization, and attach source ID, offset, original schema, and every synthetic/unavailable field as `legacy_provenance`. |
| Lifecycle-log record without an event ID | Use the same source/offset/line-digest event-ID rule. Import only a complete recognized frame with trustworthy source timestamp and valid invocation UUID; otherwise preserve and report unsupported. |

AGE-376 must also replace direct `lifecycle_log.rs` forwarding with a typed
normalizer before pre-fanout envelope construction. It allowlists event names
and bounded scalar fields, parses `invocation_uuid`, converts raw `session_id`
to the domain-separated `session_correlation_sha256`, and never places the raw
session ID in payload or labels. `raw_artifact_paths` becomes artifact-role
presence plus domain-separated path fingerprints, never path text. Error chains,
provider/model/source values, terminal reasons, and other strings pass through
the AGE-369 size/secret/path redactor before payload hashing. Unknown fields are
rejected or recorded as bounded provenance; they are not copied wholesale.
The normalizer validates AGE-371's identical `recorded_at` /
`retention_eligible_at` projection and eligible status. It uses `recorded_at`
as the envelope occurrence time but omits all three record-timestamp fields
from the normalized payload: lifecycle JSONL owns that record projection,
while sealed generation metadata and AGE-372 policy own event-store retention.

Cutover then proceeds in these stages:

1. Add the event store/schema and preservation mode without changing authority
   tables; establish one pre-fanout event identity.
2. Shadow-write eligible normalized envelopes to both sinks and compare IDs,
   counts, digests, gaps, and bounded reader results.
3. Use the bounded union reader, which deduplicates by event ID and exposes
   source/import coverage without performing discovery or import itself.
4. Run the bounded importer against leased closed sources and publish completion
   receipts.
5. Make partitioned SQLite the normal sink after shadow verification. JSONL is
   emergency fallback only; cutover deletes nothing.
6. Retire a legacy source only after it is closed, its completion receipt and
   imported event digests validate, no hold/lease remains, and the later of its
   close time or successful import is at least 30 days old under AGE-372 policy.
   AGE-377 performs the actual detached deletion. Missing provenance or an
   unsupported/torn record fails closed.

No historical State/PID-mailbox backfill is part of this cutover. Optional
derived mirrors start from new transitions unless a separate migration defines
their authority-neutral provenance.

## Rejected alternatives

| Candidate | Decision |
|---|---|
| Sharded framed files + side indexes | Rejected as primary. It had the smallest footprint and good queries in the fixture, but the append interval included one-time initialization that SQLite/Fjall excluded, so its observed rate is not ranking evidence. Durable data-then-index publication still required two syncs, reopen needed a full repair scan in the prototype, and production would own framing, checksums, index formats, resynchronization, and platform-specific atomicity. Retained as emergency JSONL/framed fallback precedent only. |
| One WAL database behind a write broker | Rejected. It centralizes all producers on SQLite's one-WAL-writer rule and was about half the selected append rate at batch 32. A broker is useful only inside each producer partition. |
| Embedded log-structured store (Fjall) | Rejected. It had the fastest reads and compact footprint, but slower durable append than partitioned SQLite here, adds a production dependency/disk format, and brings background flush/compaction ownership that must be isolated from the supervisor. The prototype dependency remains tool-only. Fjall's own documentation describes its LSM and background-maintenance model: <https://github.com/fjall-rs/fjall>. |
| Local/external telemetry service | Rejected as the required local authority/sink. It adds a daemon/service lifecycle, IPC/network unknown outcomes, packaging/configuration/security, and backend retention. The OpenTelemetry Collector is explicitly a separately deployed binary feeding backends: <https://opentelemetry.io/docs/collector/deploy/>. No service was installed or benchmarked under this ticket's limits. A future optional exporter may read committed local partitions; service availability may never gate local correctness or erase local coverage. |

## Accepted residual risks

- Measurements are Linux/WSL2 smoke evidence on one filesystem/cache state;
  macOS, Windows, slow disks, high fan-out, 30-day scale, and AGE-353 load are
  unmeasured. The decision rests on isolation and correctness as well as speed.
- SQLite partitions use more space in the fixture. Retention and optional
  closed-partition compaction must bound that cost without touching live heads.
- Per-generation snapshots are not one global instant. The reader contract
  exposes watermarks and incomplete coverage rather than claiming otherwise.
- Directory/manifests can grow with producer churn. The detached catalog and
  bounded discovery need scale validation; the root supervisor never absorbs
  that work.
- The retained-history rotation observation is a bounded ignored test fixture,
  not a benchmark or SLO. It exercises the same exact-head rotation with small
  and larger retained generation sets and prints raw elapsed samples; broader
  filesystem, platform, fan-out, and AGE-353 stress behavior remains unmeasured.
- On Unix, a focused permission oracle makes every retained generation
  unreadable, proves a deliberately history-dependent neighbor fails, and then
  proves exact-current rotation succeeds. This discriminates retained-history
  traversal from the accepted path, but it is not cross-platform latency or
  scale evidence.
- `synchronous=FULL` depends on the platform VFS/filesystem honoring sync.
  Hardware lying about durability is outside the application guarantee.
- Emergency JSONL fallback retains the current best-effort/loss semantics until
  imported; its existence is observable and is never treated as an event-store
  commit.
- Preservation mode deliberately removes the legacy four-shard/seven-day
  deletion bounds, so fallback/shadow disk growth remains a visible risk until
  AGE-372 eligibility and AGE-377 execution retire imported sources.
- The generation-directory/two-slot protocol requires same-filesystem rename
  and durable file/directory sync. AGE-376 validates those preconditions and
  fails producer initialization to JSONL fallback when they are unavailable;
  platform/filesystem durability still depends on the underlying VFS honoring
  sync operations.
- The 64-MiB, 32-record, 1-MiB, and 10-ms thresholds are accepted version-1
  defaults. Later tuning may change bounds without changing topology, identity,
  authority, or publication semantics.

## Next-ticket contract

The implementation ticket is executable when it delivers:

1. a new event-store module with the exact envelope, `STRICT` schema, indexes,
   PRAGMAs, append-ticket outcomes, and one process-local writer worker;
2. generation-directory publication, two-slot `HEAD`, prepared/sealed
   manifests, eligibility metadata, and interruption fixtures/tests at every
   create, sync, checkpoint, close, and rename boundary. Deterministic visible-
   state fixtures are partial evidence only where the current harness cannot
   interrupt the exact operation; each such boundary remains explicitly unmet
   until discriminating evidence exists;
3. bounded readers returning records plus per-partition watermarks and coverage;
4. stable correlation searches for time, family/kind, trace/span,
   invocation/session digest, process tree, and supervisor authority;
5. unknown-outcome reconciliation and equal-ID/equal-digest idempotency tests;
6. concurrent multi-process partitions proving no shared event SQLite writer;
7. SQLite crash/reopen, missing/corrupt partition isolation, active-head
   replacement, and repair/retirement protocol fixtures without running
   historical maintenance from the root;
8. preservation-mode JSONL rotation, deterministic legacy provenance, the
   bounded importer/cutover operation, and union-reader contracts/tests;
9. catalog/repair/rebuild/compaction/retirement interfaces and fixtures: AGE-372
   supplies the policy decision and AGE-377 supplies scheduling, singleton
   leases, and actual detached execution; and
10. shadow/fallback cutover with pre-fanout identity, lifecycle normalization,
    and no authority-table schema migration.

AGE-376 does not need to wait for AGE-377 to test its formats and interfaces,
but it must not claim that 30-day retirement operates end to end until the
AGE-372 policy engine and AGE-377 worker are composed with them.

Changing to custom framed files, a global broker database, an LSM engine, or a
required service is a new architecture decision, not an implementation detail.

## AGE-377 cycle-4 operational refinements

Detached discovery retains the complete source-trie skeleton; retirement and
archive paths do not prune producer-visible ancestors. Bad candidate identities
consume the raw-node budget, advance the durable discovery cursor, and produce
bounded gap evidence, so restart does not pin healthy later leaves. Derived
discovery archive is ordered after authoritative retirement-job terminalization
and can be retried independently.

Legacy discovery remains a bounded detached daily rewalk. Registration or
top-level enumeration failure is a preserved terminal outcome, and the next UTC
opportunity retries from durable bounded state. AGE-377 does not publish an
irreversible legacy cutover marker because directory EOF cannot prove that a
still-live parent-version process will not lazily create its first legacy-only
writer afterward. A future cutover requires an enforceable process-drain or
deployment barrier owned by the consolidated supervisor architecture.

Detached launch evidence is epoch-partitioned and begins with a durable
pre-spawn intent. Parent admission failures and child execution failures keep
separate causes and evidence-gap flags, including cross-epoch fallback-only
reconciliation. Terminal launch outcomes distinguish completed, yielded,
cancelled, preserved/gapped, and failed. A bounded historical-compaction job
removes only terminal launch epochs at least 30 days old while preserving active,
incomplete, and corrupt evidence.
