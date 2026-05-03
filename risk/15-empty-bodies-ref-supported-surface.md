# WU-15-01 — Phase 4 Supported-Surface Risk

## 1. Termination signal

`NONE`.

### Assumption invalidation check

- A1 (user override supersedes proposals 01/06 for body storage): not
  invalidated. Per the gate constraints, this is the WU's value
  statement, not contradiction-grounds for termination. The override
  is recorded in the ticket "Decision binding" clause and is to land
  in `DECISIONS.md` per AC-8 (`proposals/15-empty-bodies-ref.md:85`).
- A2 (WU-13-01 quota topology coexistence; idempotent column-presence
  migrations, no `user_version` bump): not invalidated. The problem
  map confirms `StateDb::open` runs ensure helpers without managing
  `PRAGMA user_version` and that the topology migration at
  `src-tauri/src/state/db.rs:1099-1137` already uses column-presence
  checks (`research/15-empty-bodies-ref-problem-map.md:11-13`,
  `:45`). The proposed body migration follows the same shape. No
  ordering constraint discovered.
- A3 (canonical-record byte stability when JSONL is present): not
  invalidated. The problem map shows `read_canonical_transcript`
  starts with `fs::read(&metadata.jsonl_path)` and the proposal keeps
  JSONL as the first source, only falling back to DB when JSONL is
  absent or unreadable (`proposals/15-empty-bodies-ref.md:34-36`;
  `research/15-empty-bodies-ref-problem-map.md:22`). DB-derived
  records re-use `canonical_jsonl_bytes`, preserving the wire shape
  from `proposals/06-export.md`.
- A4 (import-replace transaction can absorb one more bound column):
  not invalidated. The problem map confirms `replace_db_turns`
  already owns the entire delete/insert/update transaction in one
  function (`research/15-empty-bodies-ref-problem-map.md:29`); no
  interleaving read depends on the missing-body shape.
- A5 (TEXT sufficient for v1): not invalidated. Existing export and
  import-replace already enforce UTF-8 on JSONL/canonical inputs and
  reference adapters open files with `encoding="utf-8"`
  (`research/15-empty-bodies-ref-problem-map.md:81`). D-002 still
  defers multimodal/binary expansion.
- A8 (no schema-probe bump): not invalidated. `StateDb::open` does
  not read or write `PRAGMA user_version`; `schema_probe` owns
  version 3 separately and its existing fixture's `session_turns`
  shape does not include a body column today, so an additive
  nullable column is compatible with the v3 contract
  (`research/15-empty-bodies-ref-problem-map.md:45`,
  `research/15-empty-bodies-ref-problem-map.md:67`).

### Net-value check

Positive. The proposal closes four concrete current-state failures
on the supported CLI surface:

- RC-1 schema lacks any direct body column
  (`research/12-empty-bodies-ref-rca.md:131-148`).
- RC-2 ingest discards adapter-emitted bodies before insert
  (`research/12-empty-bodies-ref-rca.md:149-169`).
- RC-3 `agents session export` exits 1 when JSONL is missing even if
  bodies could be reconstructed (`research/12-empty-bodies-ref-rca.md:171-190`).
- RC-4 `agents trace --json --inline-transcript` always serializes
  `null` (`research/12-empty-bodies-ref-rca.md:192-207`).

Burden is bounded: nullable additive column, narrow ingest/import
insert changes, a DB-fallback reader that reuses the existing
canonical serializer, adapter and doc updates. Migration is
idempotent and column-presence-driven. Rollback leaves the unused
nullable column in place. Burden is clearly outweighed by closing
the four failures on the supported surface.

## 2. Verdict

`LOW`.

## 3. Findings

### Migration on a hot-path table

`session_turns` is the busy table that contains the live ingest
rows (~900K observed in `research/12-empty-bodies-ref-rca.md:6`).
The proposal's migration is a single `ALTER TABLE session_turns ADD
COLUMN body TEXT` gated on `PRAGMA table_info` column-presence
(`proposals/15-empty-bodies-ref.md:9-15`). On SQLite, an additive
nullable column with no default is an O(1) metadata-only operation,
not a row rewrite, so the size of the table does not translate into
migration cost. Coexistence with the WU-13-01 topology migration is
column-presence-checked from the same site and follows the same
idempotent shape. Acceptable for `LOW`.

### Trace JSON shape change (RC-4)

`TraceNode.transcript` flips from `Option<()>` (always `null`) to a
real per-turn array shape with `body_state` discriminator
(`proposals/15-empty-bodies-ref.md:42-65`). The proposal does
explicitly say no consumer relies on `null` and treats this as the
A6 invalidator condition (`proposals/15-empty-bodies-ref.md:127`).
The RCA further notes README documented the field as `null in this
version`, signalling placeholder intent
(`research/12-empty-bodies-ref-rca.md:27-28`,
`research/15-empty-bodies-ref-problem-map.md:42`). No external
contract or schema commits the trace JSON to a `null` value. The
proposal also handles the `body_state: "missing" | "stored" |
"invalid"` taxonomy explicitly so legacy rows do not silently
masquerade as empty content. Acceptable for `LOW`.

### Ingest serialization cost per turn

The proposal serializes the raw JSON body value to compact JSON text
once during `scan_provider`, then binds it via the existing bulk
insert (`proposals/15-empty-bodies-ref.md:26-28`). That is one
`serde_json::to_string` per turn plus one extra bound parameter;
no double-pass and no N+1. The bulk insert continues to use a single
SQLite transaction with `INSERT OR IGNORE`. Aggregate-only
malformed-body diagnostics avoid per-row log spam
(`proposals/15-empty-bodies-ref.md:30`,
`proposals/15-empty-bodies-ref.md:116`). Bounded.

### Rollback asymmetry honesty

Section 3 rollback path explicitly calls out the asymmetry: the
nullable column survives binary revert and dropping it is a
destructive operator action with understood data loss
(`proposals/15-empty-bodies-ref.md:114`). This honestly names the
"reverting reads but data persists" tradeoff rather than implying a
clean rollback. Honest.

### Adjacent surfaces explicitly preserved

Blast-radius notes confirm `session_chains`,
`session_chain_segments`, `session locate`, `session_metadata`,
quota, routing, and frontend remain unchanged
(`proposals/15-empty-bodies-ref.md:110`). The export contract keeps
JSONL as the preferred source and only falls back when JSONL is
unreadable, preserving canonical-record byte identity for the
supported path. `proposals/06-export.md` canonical-record wire shape
remains authoritative for the on-the-wire bytes; only the byte
source changes.

### Residual concerns flagged for Phase 5, not blockers

Open questions in §7 of the proposal flag (a) the exact
`RecordSource` representation for DB-fallback rows, (b) whether
`session_replace` needs preimage diffs over body bytes, (c)
behavior when an export sees a `NULL` body row (proposal chooses
"error" over "skip"), and (d) whether existing tests that manually
insert into `session_turns` use explicit column lists. Each of
these is a concrete Phase 5 verification target, not an unaddressed
blast-radius issue.

### Schema-probe boundary not silently widened

The proposal explicitly does not edit `src-tauri/src/schema_probe/mod.rs`
and keeps `CURRENT_SCHEMA_VERSION = 3`
(`proposals/15-empty-bodies-ref.md:15`,
`proposals/15-empty-bodies-ref.md:158`). Schema-probe fixtures
already model `session_turns` without a body column at v3
(`research/15-empty-bodies-ref-problem-map.md:67`), so an additive
nullable column is forward-compatible with existing v3 schema-probe
expectations. If a later gate decides probe must require `body`,
that is a scoped follow-up rather than a hidden change here.

## 4. LOW + NONE justification

The proposal restores the four user-visible body-handling failures
on the Tauri/CLI supported surface using an idempotent additive
nullable column, JSONL-first export preservation, a single bound
parameter into the existing import-replace transaction, and an
explicit `body_state`-tagged trace shape — all with honest rollback
semantics and no migration touching adjacent schemas.
