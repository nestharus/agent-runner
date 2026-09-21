# spec-event-storage — Durable diagnostic event-storage decision and evaluator

## Source files

- `tools/event-storage-evaluation/Cargo.toml`
- `tools/event-storage-evaluation/Cargo.lock`
- `tools/event-storage-evaluation/README.md`
- `tools/event-storage-evaluation/src/lib.rs`
- `tools/event-storage-evaluation/src/main.rs`
- `tools/event-storage-evaluation/src/model.rs`
- `tools/event-storage-evaluation/src/sqlite_candidates.rs`
- `tools/event-storage-evaluation/src/framed.rs`
- `tools/event-storage-evaluation/src/lsm.rs`
- `docs/architecture/event-storage-topology.md`
- `docs/architecture/live-history-access-inventory.md`
- `planning/age-375-event-storage-evaluation/report.md`
- `planning/age-375-event-storage-evaluation/results/batch-1.json`
- `planning/age-375-event-storage-evaluation/results/batch-32.json`

## Preconditions

- A new private disposable experiment root that does not already exist.
- Bounded synthetic inputs within the evaluator guardrails.
- Local filesystem support for SQLite WAL, durable file/directory sync, and
  atomic same-filesystem directory rename; otherwise the production event store
  fails to JSONL fallback.
- For production implementation, a process-local writer instance with exact
  process identity and no State/PID-mailbox writer held while waiting on event
  persistence.

## Input → Expected output

| Input situation | Expected output |
|---|---|
| Evaluator receives valid record/producer/batch/repetition bounds and a new root. | All four local candidates process the identical fixture, reopen, and return full/range/trace results matching the common count/checksum oracle; JSON retains raw observations and medians. Framed append timing includes first-open initialization that SQLite/Fjall timing excludes, so the result is not represented as a like-for-like steady-state ranking. |
| Evaluator receives an existing root or exceeds smoke guardrails. | It fails without deleting, truncating, or reusing the root. |
| Concurrent producers append to producer-partitioned SQLite. | Each producer uses a distinct SQLite/WAL database; successful batch return follows a `synchronous=FULL` transaction commit. |
| Multiple threads emit production events in one process. | They use that process's bounded event-writer worker and group commit; other process partitions do not share its WAL writer. |
| Commit result is unknown. | Exact generation/event-ID/digest reconciliation precedes retry; missing/corrupt evidence remains `Unknown`, not absence. |
| Reader searches time, family/kind, trace/span, invocation/session digest, process tree, or supervisor authority. | Local SQLite indexes answer candidate partitions; response includes partition watermarks, discovery/catalog coverage, and missing/corrupt/limited work. |
| Current head reaches the day/size/lifecycle rotation boundary. | Writer syncs and validates the prepared successor manifest/database, renames the complete staging directory, drains already-ticketed batches, closes the old generation, and publishes the successor through the inactive two-slot head record. It does not scan, checkpoint, compact, or retain old history synchronously. |
| Detached catalog is absent/stale. | Reader uses bounded manifest fallback or returns incomplete coverage; it never fabricates an empty complete result. |
| Closed event partition passes the 30-day cutoff. | AGE-372 approves policy eligibility from AGE-376 metadata; only an AGE-377 detached leased worker may checkpoint/seal and atomically rename the complete generation directory to pending trash, publish a receipt, and unlink it. |
| Event storage is unavailable. | Live coordination proceeds unchanged; the bounded emergency JSONL recorder records a gap/fallback when possible under preservation mode. |
| JSONL shadow/fallback compatibility is enabled. | A durable preservation marker precedes shadowing; root processes rotate uniquely named exact current shards without directory enumeration or deletion, and import/retirement remain detached. |

## Edge cases

- Equal event ID and equal immutable digest is idempotent; equal ID with unequal
  bytes is an identity conflict.
- One SQLite transaction rolls back entirely after interruption; an uncommitted
  row and its indexes do not become visible on reopen.
- Torn framed prototype tail is removed only to the last complete frame; a
  missing/partial derived index rebuilds from checked data.
- A missing expected partition differs from a valid retirement receipt.
- A corrupt partition does not hide healthy partitions and makes aggregate
  coverage incomplete.
- One valid head slot may name an old generation marked closed while the other
  is absent/invalid after interruption; exact successor linkage allows recovery
  to finish publication without historical enumeration.
- A prepared generation directory without a head is an orphan for detached
  audit, not a visible writable generation.
- SQLite database, WAL, sidecars, and manifests move only as one generation
  directory; separating them is invalid.
- A generation moved to deterministic pending trash without a receipt is
  `retirement_in_progress`; a valid receipt is `retired`; conflicting states
  are incomplete/corrupt, not authoritative absence.
- Legacy four-shard/seven-day cleanup is disabled before shadowing so an
  importer source cannot disappear before its receipt and 30-day eligibility.
- Legacy synthetic root/sequence/identity values carry explicit provenance and
  never masquerade as observed process-tree facts.
- Cross-partition reads are per-generation snapshots with explicit high-water
  marks, not one fabricated global snapshot.
- Clock discontinuity does not define causality; producer sequence and explicit
  trace/process parent IDs do.
- Unknown schema versions remain discoverable and contribute coverage issues;
  they are not decoded as version 1.

## Error conditions

- `RootAlreadyExists` — disposable evaluator root was not new.
- `SmokeBoundsExceeded` — evaluator inputs exceeded bounded guardrails.
- `ResultOracleMismatch` — candidate returned a different set/body checksum.
- `AppendFailed` — serialization, transaction, sync, or writer worker failed.
- `AppendOutcomeUnknown` — exact commit outcome cannot be proved.
- `IdentityConflict` — stable event ID exists with different immutable bytes.
- `MissingPartition` — manifest expects a partition without retirement proof.
- `CorruptPartition` — SQLite open/query/integrity or payload-digest validation failed.
- `CoverageIncomplete` — discovery/catalog/read bounds or partition issues prevent a complete answer.
- `HeadPublicationInvalid` — successor, manifest, checksum, epoch, or predecessor relation failed validation.
- `PublicationConflict` — a create-once generation/manifest/receipt or head epoch exists with different identity/digest.
- `RetirementInProgress` — generation is at its exact pending-trash path without a final receipt.
- `RetirementInconsistent` — source, trash, receipt, or manifest state conflicts.
- `LegacySourceIncomplete` — legacy source is active, torn, replaced, unreadable, or lacks deterministic required fields.

## Boundaries

- The event store is not completion, launch, mailbox, wake, process, quota,
  delivery, session, retry, repair, or retention authority.
- No existing State/PID-mailbox coordination table moves under AGE-375/AGE-376.
- An event commit is not a distributed commit with a coordination transaction.
- The root supervisor does not enumerate, query, checkpoint, catalog, compact,
  import, repair, or retain historical event/JSONL generations.
- The process-local event writer may rotate its exact current head; historical
  follow-up belongs to detached maintenance.
- The derived catalog is not completeness authority and may always be rebuilt.
- Emergency JSONL append/read success is not event-partition durability or live
  coordination success.
- AGE-372 owns retention policy/engine decisions. AGE-376 owns event-store and
  current-head primitives, eligibility metadata, preservation/import/cutover
  contracts, and maintenance fixtures/interfaces. AGE-377 owns detached
  scheduling, singleton leases, and actual retention/repair/rebuild/compaction.
- The evaluator is not an SLO, endurance, 30-day, external-service, or AGE-353
  stress test.
- The tool-only Fjall dependency does not select or add Fjall to production.

## Declared test patterns

`cargo test --manifest-path tools/event-storage-evaluation/Cargo.toml`:

- `tests::bounded_evaluation_returns_identical_result_sets`
- `tests::evaluator_refuses_to_reuse_a_root`
- `sqlite_candidates::tests::rollback_and_duplicate_retry_preserve_one_logical_event`
- `sqlite_candidates::tests::missing_and_corrupt_partitions_do_not_hide_healthy_partition`
- `framed::tests::torn_tail_is_removed_without_losing_acknowledged_frames`
- `framed::tests::missing_and_partial_indexes_rebuild_from_durable_data`
- `framed::tests::duplicate_retry_is_physically_idempotent_within_the_routed_shard`
- `framed::tests::corrupt_shard_is_detected_independently`
- `lsm::tests::duplicate_keys_remain_one_logical_record_after_reopen`

AGE-376 must add subprocess interruption tests for every generation-directory
and two-slot-head boundary, real SQLite crash/reopen, concurrent processes,
unknown outcome reconciliation, bounded readers, corrupt/missing/pending-trash
partitions, preservation-mode JSONL rotation, deterministic import provenance,
lifecycle normalization/redaction, and 30-day eligibility metadata. AGE-377
tests actual detached catalog rebuild, repair, compaction, and retirement
execution under singleton leases using those fixtures/interfaces.

## Cross-references

- `docs/architecture/event-storage-topology.md` — accepted topology and
  implementation contract.
- `docs/architecture/live-history-access-inventory.md` — existing SQLite live
  and historical authority boundary.
- `planning/coverage/spec-diagnostics.md` — AGE-369 recorder and diagnostic
  producer behavior.
- `planning/coverage/spec-state-db.md` — authoritative SQLite schema and
  migrations that remain in place.
- `AGENTS.md` § “State DB Schema Migrations”.
