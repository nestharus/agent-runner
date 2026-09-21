# AGE-375 event-storage evaluation report

Decision: producer-partitioned SQLite/WAL generations. The normative contract
is [`docs/architecture/event-storage-topology.md`](../../docs/architecture/event-storage-topology.md).

## Reproduction

The executable is isolated from the production workspace in
`tools/event-storage-evaluation/`. Its independent lock file pins Fjall 3.1.10
and rusqlite 0.39.0; it does not add a production dependency. It refuses to
reuse or delete an existing experiment root.

Commands run from repository root:

```bash
age375_parent=$(mktemp -d /tmp/age375-event-storage.XXXXXX)

cargo test --manifest-path tools/event-storage-evaluation/Cargo.toml

cargo run --release \
  --manifest-path tools/event-storage-evaluation/Cargo.toml -- \
  --root "$age375_parent/batch-32" \
  --output planning/age-375-event-storage-evaluation/results/batch-32.json \
  --records 4096 --producers 4 --batch 32 --repetitions 5 --payload-bytes 256

cargo run --release \
  --manifest-path tools/event-storage-evaluation/Cargo.toml -- \
  --root "$age375_parent/batch-1" \
  --output planning/age-375-event-storage-evaluation/results/batch-1.json \
  --records 4096 --producers 4 --batch 1 --repetitions 3 --payload-bytes 256
```

Raw result digests:

```text
6ad33af98133826cc25df4f93fc162a8bc6cabdd4adbc2074cf073325531a73a  results/batch-32.json
d9114bef23f5168799d7dea3958876056df2f6edf62f9b6bc6c9d1cd8a6b24a8  results/batch-1.json
4297be6320374814ef03109e6ab6e4be91e56261cd36ad61384aa96f99e68699  tools/event-storage-evaluation/Cargo.lock
```

Environment recorded by the fixture: Linux x86-64, WSL2 kernel
5.15.167.4, bundled SQLite 3.51.3. Host CPU was an AMD Ryzen 9 7950X3D
(16 cores/32 threads); experiment root reported an ext2/ext3-family filesystem
with 4-KiB blocks. The paths were private disposable roots and contained only
synthetic fixture data.

## Common workload

Every candidate receives the same deterministic event objects and validation:

- 4,096 records across four concurrent producers;
- four record families, 128 traces, explicit span/parent/process correlations;
- 256 deterministic payload bytes per event (JSON array representation made
  the total stored body set 4,952,007 bytes);
- durable acknowledgement after every configured batch;
- close/reopen, then full read, a middle 10-percent time range (409 records),
  and one trace query (32 records);
- every query reads event bodies, not only counts;
- record counts and a commutative body checksum must equal the common oracle;
- candidate order rotates on each repetition.

Candidate-specific mapping was intentionally minimal:

- **partitioned SQLite**: one WAL database per producer, local time/trace
  indexes, `synchronous=FULL` transactions;
- **WAL broker**: one WAL database and one writer receiving producer batches;
- **framed shards**: one CRC32C data shard plus fixed-width side index per
  producer; data `sync_data` precedes index `sync_data`. Its append timer also
  includes each shard's first open, empty data-file create/sync, and initial
  empty-index file/parent-directory publication;
- **embedded LSM**: one Fjall database with record/time/trace keyspaces and
  atomic `SyncAll` batches.

The broker prototype is in-process. It excludes IPC/network overhead and is
therefore a favorable lower bound for a separate local service using the same
single database. No external service was installed or deployed.

## Measurements

All numbers below are medians from the raw JSON. “Append/s” includes event
serialization, indexed writes, and the candidate's durable sync at the stated
batch boundary. SQLite schema/index creation and Fjall database/keyspace open
occur before their append timers. Framed-shard first-open initialization occurs
inside its timer. The displayed values are therefore the actual observed
intervals but are not a like-for-like steady-state ranking between framed
storage and SQLite/Fjall. Size is the candidate directory after
close/reopen/query.

The raw result files are intentionally unchanged. The independent review found
the timing-boundary asymmetry; this correction narrows the claim instead of
retroactively replacing the measured observations.

### Grouped durability: batch 32, five repetitions

| Candidate | Append ms | Append/s | Reopen ms | Full read ms | Range ms | Trace ms | Bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| Partitioned SQLite | 163.23 | 25,093 | 0.12 | 27.17 | 2.66 | 0.37 | 19,726,336 |
| Framed shards/index | 241.02 | 16,994 | 23.68 | 24.94 | 2.48 | 0.24 | 5,591,047 |
| Embedded Fjall LSM | 280.72 | 14,591 | 18.03 | 4.70 | 0.55 | 0.09 | 5,698,281 |
| One WAL broker | 322.25 | 12,711 | 0.07 | 29.33 | 2.96 | 0.34 | 19,496,960 |

### Per-record durability: batch 1, three repetitions

| Candidate | Append ms | Append/s | Reopen ms | Full read ms | Range ms | Trace ms | Bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| Partitioned SQLite | 3,316.24 | 1,235 | 0.14 | 28.13 | 2.74 | 0.40 | 19,726,336 |
| One WAL broker | 4,793.90 | 854 | 0.08 | 29.76 | 2.97 | 0.41 | 19,566,592 |
| Embedded Fjall LSM | 5,764.49 | 711 | 21.95 | 5.15 | 0.65 | 0.10 | 5,801,449 |
| Framed shards/index | 7,435.23 | 551 | 23.95 | 25.75 | 2.55 | 0.23 | 5,591,047 |

The batch-32 observations were about 20.3x the batch-1 rate for partitioned
SQLite, 14.9x for the broker, 20.5x for Fjall, and 30.8x for the two-sync framed
format. This is consistent with bounded group commit amortizing sync cost, but
the fixture does not isolate every causal component and does not justify
acknowledging before a durable commit.

## Failure and recovery evidence

`cargo test --manifest-path tools/event-storage-evaluation/Cargo.toml` ran nine
tests:

1. all four candidates return identical full/range/trace result sets under
   concurrent append;
2. evaluator refuses an existing root;
3. SQLite drops an uncommitted transaction on reopen and equal-ID retry remains
   one logical row;
4. missing and corrupt SQLite partitions are reported without preventing an
   independently opened healthy partition from returning records;
5. framed torn-tail bytes are truncated to the last acknowledged frame;
6. a missing or partially published framed side index rebuilds from durable
   data;
7. equal-ID framed retry does not append another physical record within its
   routed shard;
8. body corruption in one framed shard is detected while another shard remains
   readable; and
9. duplicate Fjall key batches remain one logical record after reopen.

SQLite atomic index publication and WAL recovery are supplied by SQLite rather
than reimplemented in the prototype. AGE-376 still must run
subprocess interruption tests around head publication and real SQLite commit
outcomes; this evaluation did not claim simulated truncation of arbitrary
SQLite pages was a valid SQLite crash test.

## Candidate comparison

| Concern | Partitioned SQLite | One WAL/write broker | Framed append + side index | Embedded LSM | Required local/external service |
|---|---|---|---|---|---|
| Atomic record + indexes | One transaction | One transaction | Data and index are two ordered publications; repair required | Atomic batch/keyspaces | Depends on protocol/backend; not cross-authority atomic |
| Crash/torn recovery | SQLite WAL per partition | SQLite WAL, but one failure domain | Custom header/body checksum, tail truncate, index rebuild | Engine journal/recovery | Adds IPC/network and service recovery |
| Unknown commit + retry | Exact partition PK/digest lookup | Exact global PK/digest lookup | Stable route plus index/data reconciliation | Exact key lookup | Requires request IDs plus service readback |
| Concurrent append | Parallel across producer DBs | Serialized by broker/one WAL writer | Parallel across producer shards | API accepts threads; journal persistence remains shared | Service can batch, but is another shared dependency |
| Fsync cost | One WAL sync per local batch, parallel partitions | One WAL sync per broker batch | Data sync then index sync | One journal SyncAll per batch | At least local spool plus service durability for strong ACK |
| Index publication | Atomic with event transaction | Atomic with event transaction | Separate publication; rebuildable | Atomic keyspace batch | Backend-specific |
| Search/range/trace | Native indexes; fan-out over candidates | Native global indexes | Custom index formats/readers | Ordered key prefixes/ranges, fastest reads here | Usually strong after ingestion, but remote availability required |
| Trace joins | Local SQL plus derived partition catalog | Global SQL, easy but centralized | Custom fan-out and join | Custom key schema/fan-out | Backend query semantics/versioning |
| Corruption isolation | One producer generation | Whole event store | One bounded shard | Whole embedded DB unless itself partitioned | Service-defined; often larger blast radius |
| Rotation/retention | Publish new DB head; delete closed bundle | Rotating global DB pauses/recoordinates broker | Natural rename/delete, but custom recovery | Engine compaction conflicts with file-level retirement | Backend policies and service operations |
| Portability | Existing bundled SQLite on supported platforms | Same, plus broker lifecycle | Files portable; lock/sync/rename details are platform-sensitive | New crate/disk format/MSRV | Packaging, daemon, network, configuration, upgrades |
| Operational complexity | Moderate; existing dependency and expertise | Higher single point/backpressure | Highest implementation correctness burden | New dependency plus background maintenance | Highest deployment burden |
| Migration | New independent sink; SQLite tooling | Same plus global broker cutover | JSONL precedent, but format/index rewrite | New dependency/disk-format adoption | Dual delivery and external availability |

## Confirmed facts versus inference

Confirmed by this repository and execution:

- Current State/PID-mailbox data mixes live authority, bounded cross-boundary
  rows, and retained history; AGE-373 blocks historical access from live paths.
- AGE-369's recorder already uses per-process JSONL shards, a bounded deferred
  queue, rotation, leases, bounded readers, and fail-open behavior. Its regular
  append path flushes but does not call `sync_data`, has no searchable persisted
  index, and explicitly says queued records may be lost on abrupt exit.
- All candidate query result sets matched the same oracle in every repetition.
- The median timings, sizes, and nine fault/contract tests above are directly
  represented in the committed result files and test output.

Architecture inferences:

- A separate broker process using one SQLite WAL cannot remove SQLite's
  per-database one-writer limit; the in-process broker result is a lower-bound
  warning, not a benchmark of every service implementation.
- Partitioning Fjall could improve isolation/concurrency, but then it would
  need the same head/catalog/retention topology while retaining a new engine and
  compaction model. The measured global LSM did not justify that cost.
- Production framed files could avoid the prototype's full reopen scan with a
  stronger head/index protocol, but that protocol is precisely the custom
  correctness surface the decision avoids. Because framed first-open
  initialization was timed and SQLite/Fjall initialization was not, the
  observed append ordering is not evidence that framed steady-state writes are
  intrinsically slower.
- SQLite's larger footprint may matter at 30-day scale. The correct response is
  bounded generations and detached retention/measurement, not weakening the
  selected atomicity contract before scale evidence exists.

## Limits and residual work

- No real provider/model traffic, production database/configuration, external
  service, AGE-353 stress, network filesystem, or CRW surface was touched.
- AGE-375 adds the decision, evaluator, and implementation contracts only; it
  does not implement the production event store or change recorder behavior.
- Results do not establish p95/p99 latency, fairness, sustained compaction,
  checkpoint starvation, disk-full behavior, SSD wear, or 30-day cardinality.
- WSL2 filesystem and cache behavior are not macOS/Windows evidence.
- The append timing boundary is asymmetric as described above. The raw results
  are preserved, but no “measured fastest” or like-for-like steady-state claim
  is made from them.
- Reopen numbers are not equivalent integrity scans: SQLite validates lazily,
  Fjall performs engine recovery, and the framed prototype deliberately scans
  every frame to reconcile its index.
- SQLite size includes its table/index representation of the JSON body; the
  fixture did not run VACUUM or compression. Fjall default compression was
  disabled for explicitness.
- The external-service alternative was evaluated operationally and rejected,
  not benchmarked. Installing/deploying one was prohibited and not justified
  for an offline desktop default.
- The result oracle is record count plus a commutative 64-bit checksum, not a
  collision-resistant sorted event-ID/body equality proof.

These limits are accepted for the topology decision. AGE-376 has
functional/crash/concurrency smoke gates; AGE-372 supplies retention policy and
AGE-377 executes detached historical maintenance. AGE-353 later supplies scale
and SLO evidence without reopening the fundamental storage choice.
