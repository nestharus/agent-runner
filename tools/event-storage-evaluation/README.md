# AGE-375 event-storage evaluator

This isolated tool compares four storage shapes with the same bounded,
deterministic event set:

- producer-partitioned SQLite/WAL databases;
- one SQLite/WAL database behind a write broker;
- producer-sharded CRC32C-framed files with fixed-width side indexes; and
- Fjall 3.1.10 as an embedded log-structured store.

Every configured batch is acknowledged only after the candidate's durable sync
operation. Each run then closes/reopens storage and performs a full read, a
10-percent time-range query, and one trace query. Result-set counts and a
commutative body checksum must match before results are emitted. Candidate order
rotates by repetition. This is a smoke fixture, not an AGE-353 stress test or an
SLO benchmark.

The append timing boundary is intentionally disclosed rather than normalized
after the fact: SQLite schema/index creation and Fjall database/keyspace open
occur before timing, while framed timing includes first shard open, empty data
file creation/sync, and initial empty-index publication. Preserve the raw values
as observed intervals; do not use them as a like-for-like steady-state ranking.

Run from the repository root with a new private disposable path:

```bash
cargo run --release \
  --manifest-path tools/event-storage-evaluation/Cargo.toml -- \
  --root /tmp/age375-batch32 \
  --output planning/age-375-event-storage-evaluation/results/batch-32.json \
  --records 4096 --producers 4 --batch 32 --repetitions 5 --payload-bytes 256
```

Repeat with `--batch 1` to expose per-record fsync cost. The executable refuses
to reuse or delete an existing root. Remove disposable roots manually after
inspection.

Verification:

```bash
cargo test --manifest-path tools/event-storage-evaluation/Cargo.toml
```

The tests cover logical duplicate retry, SQLite rollback/reopen, concurrent
candidate result parity, framed torn-tail truncation, missing/partial side-index
rebuild, and corrupt framed-shard isolation. SQLite missing/corrupt partition
coverage is checked while an independent healthy partition remains readable.
Fjall duplicate-key idempotency is checked across reopen.
