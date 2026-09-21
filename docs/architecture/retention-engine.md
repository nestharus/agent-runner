# Policy-driven retention engine

Status: **implemented by AGE-372**

AGE-372 defines the policy and bounded executable operations for terminal
lifecycle history and rotated event generations. It does not create a timer,
detached process, singleton job, or root-supervisor hook. AGE-377 owns those
scheduling and execution concerns.

## Policy and boundary

Policy version `age372.v1` has an explicit rule for every supported family.
The default horizon for every rule is 30 days. The boundary is inclusive:

```text
eligible when authoritative_time <= as_of - 30 days
```

Thus a record one microsecond younger than 30 days is preserved; a record
exactly 30 days old and an older record may proceed to the remaining safety
checks. The policy operates on integer Unix microseconds. SQLite candidate
queries use an indexed RFC3339 upper bound plus the deterministic
`oulipoly_rfc3339_micros` predicate so text formatting cannot weaken the exact
boundary.

| Family | Authoritative root / action |
|---|---|
| invocation | Terminal standalone invocation; children, launches, turns, returned output, delivery, or provider-session authority block deletion. |
| provider logical launch | Terminal aggregate root; attempts and transition replay rows inherit its retirement transaction. Active/uncertain custody, bad return-channel state, native duties, or legacy replay evidence block deletion. |
| provider launch attempt | Inherits its logical-launch root and is never selected independently. |
| completed turn | Committed, non-recovery-pending turn; its derived selection retires in the same transaction. |
| mailbox | Delivered `agent_bash_complete` row with no unacknowledged listener or unresolved/finalizing attempt. A retained payload must have an independent completion-event reference. |
| mailbox delivery attempt | Resolved attempt with no finalizer or unreconciled pending/legacy evidence; item rows inherit the attempt. |
| completion event | The content-addressed payload artifact may retire; the event row remains recovery authority. |
| completion event listener | Retained as recovery/continuity authority. |
| runtime generation | Retained as recovery/runtime authority. |
| diagnostic, trace, metric, log, maintenance event | A mixed closed event generation uses the longest configured horizon among these five families. |

The policy validator rejects missing or duplicate family rules, empty/invalid
versions, non-positive horizons, invalid `as_of`, and arithmetic underflow.
Resume cursors bind the exact policy version, family, and cutoff snapshot.

## Fail-closed eligibility

Age never establishes deletion authority. A record must also be terminal, have
an explicit eligible projection, and carry a parseable authoritative
closure/occurrence time. Active, unresolved, unacknowledged, inherited,
recovery-authoritative, legacy-unknown, clock-anomalous, stale, and
within-horizon records are preserved with typed reasons.

Event-generation approval consumes the AGE-376
`GenerationEligibilityFacts` and `GenerationMaintenanceTarget` contracts. It
requires a closed non-head generation, validated prepared and sealed
manifests, exact writer/generation identity, both manifest digests, complete
row/time watermarks, no hold, no corruption, and no unclassified legacy input.
For a nonempty generation, the authoritative time is
`max(closed_at, max_ingested_at)`; a proven empty generation uses `closed_at`.
Missing age, row count, ingestion watermark, or sealed digest fails closed.

Preservation-mode rotated JSONL uses the separate `RotatedHistoryFacts`
decision contract. It requires a closed source, authoritative close and
occurrence watermarks, complete row evidence, no lease/hold/corruption or
recovery authority, and—for legacy input—a valid completed import receipt. Its
age reference is the latest of close, maximum occurrence, and successful import
time. Active, torn/unsupported, unimported, unknown-age, or leased sources are
preserved with typed reasons. AGE-377 performs any eventual source deletion.

An approval is only a policy result. It binds the exact generation identity,
manifest digests, policy version, authoritative time, and cutoff and can build
the existing AGE-376 `RetirementReceipt`. AGE-377 must still acquire the exact
exclusive generation lease, re-read both HEAD slots, revalidate the approval,
checkpoint and validate the closed generation, move the whole directory to its
deterministic pending-trash identity, and publish the create-once receipt.
AGE-372 never deletes or moves an event generation.

## Bounded execution and restart

`StateDb::run_retention_batch` and `MailboxDb::run_retention_batch` execute one
caller-bounded slice. Candidate selection is an indexed autocommit read with
`LIMIT + 1`. Each coordination-row candidate is revalidated and changed in its
own short `BEGIN IMMEDIATE` transaction with zero busy wait. A competing writer
therefore produces a typed `busy` result instead of allowing maintenance to
hold or wait on the live writer. No operation uses a wall-clock failure cap.

The cursor is the last completed `(authoritative_time, stable_key)` under the
fixed policy snapshot. On interruption, replay is idempotent: already retired
records disappear from selection, a busy/failed candidate is retried because
the cursor did not advance past it, and exact timestamp/status/dependency
predicates prevent a stale snapshot from deleting changed or newly referenced
authority. New writes after candidate discovery are not part of the snapshot;
new references to a selected content-addressed payload are rechecked under the
payload fence and preserve the artifact.

Payload retirement unlinks under the content-addressed publication fence and
then durably marks every eligible event reference. A crash after unlink and
before the marker resumes from the absent file and publishes the marker
idempotently. Unknown, live, continuation-owned, unacknowledged, or younger
shared references block unlink.

## Outcomes and observations

Every bounded operation returns counts, typed preservation reasons, a resume
cursor, gaps, and one of `complete`, `more_work`, `busy`, or `partial`.
Retention emits a separate `historical_maintenance` diagnostic observation
only after live transactions are closed. The observation is not the record
being pruned and is never needed to prove deletion. Recorder disablement or
failure is returned as an explicit `retention_observation` gap; deletion is
not rolled back or silently reclassified.

The compatibility `prune_terminal_history` and its read-only statistics now
use the default 30-day policy. Count pressure is no longer deletion evidence.
VACUUM remains an explicit historical operation and is not part of a bounded
retention batch.
