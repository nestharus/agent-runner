# Detached maintenance lifecycle

Status: **implemented for AGE-377**

## Scheduling and process boundary

GUI startup and provider-oriented CLI startup offer the current UTC-day
opportunity before the GUI loop, completion-owner bootstrap, provider registry,
or provider execution. Offline diagnostics and maintenance-control commands do
not schedule work. Admission holds only `maintenance-v1/launch/admission.lock`.
For the selected epoch it first syncs a checksummed `intent` record under
`launch/opportunity-<epoch>/`, before any spawn can occur. After spawn it records
the exact child PID/boot/start identity as `admitted`, syncs that transition,
releases the child through a private pipe, and returns. Readiness never waits for
maintenance completion.

The private child argv remains:

```text
--oulipoly-detached-maintenance-worker-v1 <utc-day> <launch-id> <basis>
```

The child accepts the current UTC day, plus the previous day only during a
15-minute midnight allowance. A terminal same-epoch launch or exact live child
suppresses duplicate execution. An incomplete current or immediately prior epoch
is reconciled before a replacement is admitted, including fallback-only child
failure evidence left by a prior process. A dead/reused child may be replaced;
the new record retains the prior terminal evidence. Pre-spawn and admission
failures are parent causes; worker/fallback/stderr failures are child causes.
Missing, unreadable, or conflicting parent or child evidence is retained in its
corresponding explicit gap field rather than relabeled as abrupt child death.
Terminal outcomes distinguish `completed`, `yielded`, `cancelled`,
`preserved_gapped`, and `failed`.

Each epoch has an isolated fallback carrier and stderr file. If normal child
ledger publication fails, the child writes the exact returned cause to its
armed carrier; later admission reconciles that cause before classifying the
launch. This distinction is conditional on at least one launch-directory file
remaining writable; total storage failure cannot provide durable causality.
The root supervisor never runs the loop, holds its writer, or passes a database
handle into provider work.

A checksummed `oldest-epoch.json` marker drives a separate
`historical_compaction` job for `launch/evidence`. It retires only terminal
launch epochs at least 30 days old, in slices of at most four epochs and 64
entries. Current/incomplete/corrupt epochs are preserved. A deterministic
`.retiring-opportunity-<epoch>` namespace makes a crash after the epoch rename
resumable; the oldest marker advances only after the retiring directory is
empty and removed. The job's separate resumable scan frontier advances past a
preserved epoch so later terminal epochs still receive cleanup, then wraps to
the oldest unresolved marker for another bounded repair check.

## Exact singleton jobs, evidence, and control

`maintenance-v1/<sha256(kind,partition)>/` holds an `owner.lock`, alternating
checksummed `status.0/.1` checkpoints, `transition.lock`, optional epoch-fenced
`cancel.json`, and alternating `evidence.0/.1` records. The kernel owner lock is
the ownership mechanism: a slow live worker excludes duplicates, while process
death releases ownership and the next attempt records dead-owner recovery with
the retained nonterminal epoch and cursor. Heartbeat age never steals a lease.

The direct evidence slots are independent of the normal event writer and are
read by both:

```text
diagnostics maintenance --kind <kind> --partition <partition>
maintenance status --kind <kind> --partition <partition>
```

`Appended` means a synced evidence slot is reachable through that reader. A
write failure records `diagnostic_gap` in the authoritative job checkpoint when
possible, including a failed evidence write for a rejected duplicate/terminal
request. The maintenance request path also writes a checksummed request-evidence-gap
sidecar when rejected-request evidence cannot be delivered; status reads expose
that sidecar instead of erasing the request-level loss behind the incumbent job
record. Recurring attempts retain the prior terminal owner, schedule basis,
launch ID, completion time, and terminal result. Evidence never proves or rolls
back maintenance effects. AGE-378 owns longitudinal aggregation and
slow-versus-stalled inference.

Local cancellation requires the exact kind, partition, and currently admitted
epoch:

```text
maintenance cancel --kind <kind> --partition <partition> --epoch <epoch>
```

The cancellation writer and a destructive worker's final read+rename use the
same exact-job transition gate. If cancellation returns first, the source has
not moved. If the worker wins, the later request is truthfully post-move and the
worker first makes the pending state recoverable.

Implemented job kinds are `event_discovery`, `orphan_classification`,
`rotation_follow_up`, `event_retirement`, `index_rebuild`,
`catalog_inspection`, `state_retention`, `mailbox_retention`, and
`historical_compaction`. Lifecycle phases distinguish admitted/active work, bounded yield,
terminal success/failure/cancellation/preservation, and closed or reactivated
records. A later authoritative close reactivates an archived derived discovery
record instead of leaving stale archived state as the current interpretation. Index repair and catalog publication remain explicit
preserved outcomes because AGE-376 exposes inspection/planning but no repair-copy
replacement or authoritative catalog-publication effect API.

## Sharded discovery and publication recovery

There is no maintenance inventory database or other global live-path writer.
Each generation has an isolated checksummed two-slot journal under:

```text
diagnostics/event-store-v1/maintenance-discovery-v1/
  active|pending|archive/<32 one-byte hex trie levels>/status.0|status.1
```

The 32-byte trie key is the exact 16-byte writer ID followed by the 16-byte
generation ID. Each directory has at most 256 children. A cursor records the
last raw trie node, so even corrupt/incomplete leaves consume a bounded node
budget and a one-node slice advances. Pending work is always walked before
active work. Independent producers mutate disjoint leaves and never serialize
through a shared SQLite/WAL or append log. Publication gives mode `0700` only to
directories it creates; it does not chmod shared ancestors on every write.
Discovery never prunes source-trie ancestors. The fixed skeleton is retained
so independent producers can continue publishing into stable ancestors without
a source-prune/create race.

The producer takes its shared exact-generation lease before publishing an
`intent` journal. Intent is durable before the unique staging directory exists
and names that exact staging basename, generation, manifest digest, and native
producer. Later `staging`, `publishing`, `prepared`, `selected`, and `closed`
advances are derived evidence. Once the authoritative generation rename is
durable, a later discovery write failure cannot turn publication success into a
generic error or cause a new generation identity on retry.

The global cursor is checkpointed after every candidate. A candidate-local
manifest/head/semantic failure creates exact preserved gap evidence and does
not pin later healthy leaves. A bounded list of concrete discovery causes is
retained in the global checkpoint rather than collapsed to a boolean.

Every pre-head state has a successor, with classification bound to the exact
actual path rather than merely to a retained staging name:

- intent with no artifact is archived as discharged;
- exact incomplete staging at the journal's named staging path, from a
  proven-dead producer, is moved and unlinked only when it contains the fixed
  known file set;
- an authoritative final generation is destructive only with an exact Prepared
  manifest identity and proven emptiness; a manifest-less, corrupt, or
  unprovable final generation is preserved even if the journal retained a
  staging name;
- before any source move, the worker publishes a create-once disposition bound
  to immutable writer/generation, discovery-operation, prepared-proof,
  producer, source-kind, and reason fields. Retry validates and consumes that
  disposition rather than reclassifying a partially unlinked artifact;
- selected heads are never changed by orphan classification; rotation
  follow-up acquires the exact generation maintenance lease and re-reads the
  authoritative head/state before publishing a dead-head archive disposition;
- a closed, non-head generation routes to retirement; and
- pending retirement/orphan trash remains independently discoverable.

Orphan pending trash has a fixed known file order and a durable unlink cursor.
Missing already-unlinked known files are progress. Each unlink is synced and
checkpointed, cancellation is observed between entries, and restart after the
move or any individual unlink completes archive/discharge without weakening
the exact lease, both-head, producer-death, or source-identity fences.

A bounded detached legacy cursor uses native directory cookies to admit
pre-boundary `writers/<id>/generations` and `staging` entries into exact journals
without sorting or materializing a flat directory. Its writer/child/phase cursor
survives process and UTC-day boundaries. Invalid writer/generation entries and
top-level enumeration failures are terminal preserved outcomes with bounded
concrete issues; a failing entry cannot starve later entries. Exhausting one
pass resets the legacy cursor, and the next daily detached opportunity performs
another bounded rewalk. AGE-377 deliberately publishes no irreversible legacy
cutover marker: a still-live parent-version process can initialize its recorder
and create a legacy-only writer namespace after any earlier directory EOF.
Suppressing future enumeration would therefore require an enforceable
deployment/process-drain barrier that this ticket does not own. Invalid or
unprovable artifacts are preserved with a concrete reason rather than claimed
repaired.

## Exact event-generation retirement

For one exact writer/generation the worker:

1. holds the job singleton and AGE-376 exclusive generation lease;
2. reads both head slots and refuses the selected head;
3. checkpoints the closed WAL and runs the indivisible SQLite `quick_check` in
   the detached child;
4. derives aggregate facts and hashes each immutable durable file once for the
   create-once sealed manifest;
5. validates payload length/digest in resumable local-sequence slices (the byte
   target yields between rows and never permanently rejects a large valid row),
   then performs a second complete immutable-file hash traversal to revalidate
   sealed file identities before destruction;
6. obtains fresh AGE-372 approval, re-reads both head slots, checkpoints
   `destructive_ready`, and moves discovery to pending;
7. under the transition gate, re-reads cancellation, renames the whole directory
   to deterministic same-filesystem pending trash, syncs both parents, and
   records the pending phase;
8. publishes the create-once AGE-376 retirement receipt and equal-content
   derived catalog tombstone; and
9. removes only a bounded number of known flat entries per slice before removing
   the empty pending directory and durably terminalizing the singleton job; and
10. archives discovery as a derived follow-up. If that archive write fails, an
    `already_terminal` retry repeats only the archive step and reports the gap;
    the authoritative retirement is not reopened or relabeled.

Crash before the rename leaves the source. Crash after it resumes from the
exact pending path and retained destructive cursor; receipt publication is
idempotent. Source+pending, receipt+source, unknown trash entries, digest or
identity conflict, missing pre-move evidence, failed head validation, or failed
AGE-372 approval preserve/fail closed. Receipts remain correctness evidence
until a separately designed bounded aggregate proof exists.

`quick_check` and the two immutable-file hash traversals are single-generation
detached operations, not startup gates. The first establishes the sealed proof;
the second revalidates it immediately before destruction. There is no historical
correctness size/time ceiling. Payload rows, discovery nodes, partitions,
retention rows, and trash entries are bounded resumable slices.

## Coordination retention and limits

State and PID-mailbox jobs first checkpoint AGE-372's fresh fixed `as_of` and
empty exact cursor, before opening an authority database or mutating any row.
They then open one historical handle in the child, execute one bounded batch,
drop the handle, and checkpoint the returned cursor. A crash after mutation but
before the second checkpoint therefore resumes with the original `as_of`.
Busy/more-work yields. A candidate-local partial result advances past that
candidate and yields the same exact frozen-snapshot job with durable gap
evidence, so later ordered candidates retain a continuing owner. Missing authority
databases are not created. Delivered-payload compaction uses its existing
bounded deletion API. Every maintenance historical open, including compaction,
uses an observation-disabled handle; ordinary application opens keep the normal
connection observation. Progress/outcome remains in direct maintenance
checkpoint/evidence slots, so a maintenance-only child does not recursively
create a process recorder or normal event-store writer/head.

Default child slice targets are 128 discovery trie nodes, four partitions, 256
payload rows, 8 MiB between payload-row yields, four trash entries, and 64
coordination rows per family. These are progress bounds, not hard correctness
caps. No startup sleep, provider/model request, historical `VACUUM`, production
mutation outside the selected data root, or AGE-353 stress path is part of this
lifecycle.
