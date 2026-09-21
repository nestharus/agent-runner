# Record timestamp contract

AGE-371 defines the timestamp authority needed by later 30-day retention work.
It does not schedule retention or introduce an age-based deletion worker.

## Common rules

- Durable time is UTC wall-clock time encoded as RFC 3339. In-process latency is
  measured independently with a monotonic clock (`elapsed_micros` or
  `latency_us`) and is never converted into a retention timestamp.
- Creation time is immutable. Update time is the occurrence time of the latest
  relevant lifecycle transition. Terminal/closed time is written once by the
  transition that establishes terminal state and ordinary replay preserves its
  exact bytes.
- A terminal state is not, by itself, deletion authority. Only
  `retention_status='eligible'` together with non-null
  `retention_eligible_at` may enter a retention candidate query.
- `pending` means the lifecycle is open; `blocked` means a terminal observation
  still owns unresolved custody or recovery; `legacy_unknown` means no precise
  authoritative time can be proved; and `clock_anomaly` means the observed
  close precedes creation. The last two are retention-ineligible.
- Equal timestamps are valid. Backward timestamps are preserved, never clamped.
  Future timestamps remain stored and naturally fail an age cutoff until wall
  time catches up. Once a lifecycle is classified `legacy_unknown` or
  `clock_anomaly`, ordinary close/reopen transitions cannot promote it to
  eligible; only the audited historical repair route can replace that
  evidence.
- The transition and all timestamps/eligibility fields it establishes are one
  SQLite transaction. Process inspection, provider calls, network work,
  recorder flushes, and payload filesystem work remain outside live writer
  spans.
- Established terminal times can change only through the explicit historical
  repair route. The same transaction appends old/new values, actor, reason, and
  repair time to an append-only ledger before updating the record. Live handles
  are rejected by the AGE-373 barrier and attributed through AGE-369.

## Authoritative family inventory

| Record family | Creation | Update / occurrence | Terminal / closed | Retention rule and legacy treatment |
|---|---|---|---|---|
| State `invocations` | `created_at` | `lifecycle_updated_at` | `finished_at` for succeeded/failed | Eligible only when terminal time is parseable and not before creation. Versionless legacy migration no longer fabricates `finished_at=created_at`; legacy/equal-copy evidence remains `legacy_unknown`. |
| State provider logical launches | `created_at` | existing `updated_at` | `finished_at` for succeeded/failed/cancelled | Eligible after a chronological close. `recovery_blocked` is explicitly `blocked`, not silently terminal-aged. |
| State provider launch attempts | `created_at` | lifecycle-specific attempt fields | `finished_at` for superseded/succeeded/failed/cancelled | Same eligibility rule as logical launch. Terminalization samples one UTC value for attempt, logical update, and logical close. |
| Provider transition replay | parent launch creation | immutable `recorded_at` for the replay fact | inherits parent | New replay facts use `retention_status='inherits_parent'`; legacy facts are unknown. Replay never gets an independent deletion clock. |
| State `completed_turns` | `created_at` at immutable admission | `updated_at` at commit/tail transition | first authoritative `closed_at` when recovery tails become complete | Pending while recovery is pending; chronological first close is eligible. Legacy tail JSON never manufactures close time. A closed turn cannot reopen. |
| Sidecar `mailbox` | `enqueued_at` | delivery/error transition | `closed_at` from delivery or a declared terminal error | Eligible only from an authoritative chronological close. Terminal errors now receive a close time atomically. Existing count-based pruning additionally requires explicit eligibility. |
| Sidecar delivery attempts | `created_at` | `updated_at` from submission/ack/evidence/resolution | `resolved_at` | Only chronological resolved attempts are eligible; unresolved or uncertain evidence remains pending. |
| Sidecar completion events | `created_at` | `updated_at` | `triggered_at` / `closed_at` | Eligible only after triggering and after listener retirement dependencies permit it; otherwise blocked/unknown. |
| Sidecar completion listeners | `created_at` | `updated_at` | immutable first `closed_at` when retirement completes | Chronological retirement is eligible. Reactivation clears listener eligibility and blocks the parent; a later retirement uses that new occurrence for update/eligibility while preserving the first close. Parent maintenance reads the exact changed child plus the indexed pending projection, never retained sibling history. |
| Sidecar runtime generations | `created_at` | `updated_at` across running/draining/exited | `exited_at` | Chronological exit is eligible. The historical epoch creation sentinel is explicitly `legacy_unknown`. |
| AGE-369 diagnostic events | append-time `recorded_at` | one immutable event; no lifecycle update | same occurrence | Diagnostic schema v2 copies the same UTC value to `retention_eligible_at` and marks it eligible. Schema-v1 records deserialize as `legacy_unknown`. Monotonic `elapsed_micros` remains separate. |
| Lifecycle log records | append-time `recorded_at` | one immutable observation | same occurrence | `retention_eligible_at` equals `recorded_at`; `latency_us` stays monotonic. These logs observe State transitions and do not replace State terminal authority. |
| Diagnostic cleanup status | `started_at` | bounded cleanup generation | `completed_at` | A completed status snapshot uses its completion as eligibility time. Missing/invalid shard mtime is unknown and retained; aggregate pressure never substitutes for proven age. Shard mtime is operational input, not record authority. |

Every SQL family above has a partial eligibility index where candidate discovery
is currently meaningful. Existing terminal-history prune/reclaim paths require
eligibility but remain count based; AGE-374/AGE-376 own age policy and worker
scheduling.

The invocation contract has one idempotent installer shared by numbered v27,
fresh/current initialization, current drift repair, and the supported current
pre-UUID rebuild. It owns safe projection initialization, repair-ledger guards,
immutability/reopen triggers, and the eligibility index as one contract. A
classifier-accepted versionless database with the exact seven-column pre-UUID
shape first establishes its non-invocation v5 baseline and version marker in
one transaction, then uses the existing through-v26 rebuild and v27 sequence.
Other nonempty pre-UUID shapes fail closed before WAL, DDL, or `user_version`
mutation.

## Inherited and excluded projections

Append-only or derived child records are deleted only with their authoritative
root: completed-turn selections, completion continuity/v2 identity rows,
delivery-attempt items, continuation notifications, supervisor-inheritance
edges, and provider native-channel current-duty rows. The continuation
owner/source/attempt and supervisor-authority tables are custody/revision
projections, not independent 30-day roots; their unresolved phases block the
event/listener/payload root that owns retention.

No independent durable metric store exists in current scope. Runtime trace
reports are projections over invocation and session-turn persistence and inherit
those records' clocks. Pending native runtime-exit journals, `notify-trace.log`,
overlay trace, and opt-in TUI-profile JSON are not selected for automated
30-day retention: their file timestamps, rotation order, or epoch sentinels are
not authoritative age evidence. Selecting any of them later requires a new
family contract and migration; until then unknown artifacts are not age-reaped.

AGE-375 selects, and AGE-376 implements, producer-partitioned SQLite/WAL
generations as the primary store for non-authoritative diagnostic, trace,
metric, log, and maintenance events. That topology preserves every
State/PID-mailbox authority family above in its existing transactional store;
event presence or absence grants no authority. AGE-376 lifecycle envelope
normalization parses this contract's `recorded_at` as event occurrence time,
validates the identical eligibility projection, and does not copy record-level
retention fields into the event payload. Generation metadata is the separate
event-store retention evidence. Schema-v2 JSONL diagnostic and lifecycle
records remain the preservation/shadow/fallback format. See
[`event-storage-topology.md`](event-storage-topology.md) for generation
ownership, timestamp columns, and retention scheduling boundaries.

## Repair and threat boundary

`StateDb::repair_terminal_timestamp` and
`MailboxDb::repair_terminal_timestamp` are available only on explicit
historical handles. Each writes its repair ledger in the same transaction as
the repaired terminal and eligibility projection. Both ledgers are append-only
under normal SQLite access, and only their newest authorization for a record
and field can be consumed; an older repair row cannot be replayed later.

These controls prevent ordinary application replay and accidental direct DML
from silently rewriting history. They do not claim tamper resistance against an
operator with unrestricted offline access to the database files.
