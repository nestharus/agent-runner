# WU-15-01 — Phase 4 Scope Risk

## 1. Verdict

`LOW`.

## 2. Findings

The proposal stays inside the ticket's stated Code Boundary,
respects the ticket's Anti-scope, respects the Test Boundary,
and bounds `schema_probe` and `session_metadata` exactly as the
ticket directs. Deferrals are named explicitly. Specifically:

### Code Boundary

Every file the proposal proposes to modify is in the ticket's
in-scope list:

- `src-tauri/src/state/db.rs` — schema migration and ingest/insert
  helpers (proposal §1, §2, §6 import-replace).
- `src-tauri/src/sessions/mod.rs` — `ScriptTurn.body` and
  `scan_provider` serialization (proposal §2).
- `src-tauri/src/session_export/mod.rs` — DB fallback reader
  (proposal §3).
- `src-tauri/src/session_replace/mod.rs` — body included in the
  existing replace transaction (proposal §6 "Import-replace
  transaction").
- `src-tauri/src/session_metadata/mod.rs` — verification-only
  (proposal §3 supported-surface "session locate and
  session_metadata path resolution continue to report transcript
  availability and workspace roots but do not become body
  readers"). No edits proposed.
- `src-tauri/src/trace/mod.rs` — `TraceNode.transcript` becomes a
  real serializable type (proposal §4).
- `scripts/claude-code-turns`, `scripts/codex-turns` — emit `body`
  raw JSON (proposal §6 "Turn-script adapters").
- `scripts/README.md`, `README.md`, `DECISIONS.md` (proposal §7
  "README and decisions"; ticket Code Boundary explicitly
  in-scope; AC-7/AC-8).

No proposed change references files outside this list. `main.rs`,
`schema_probe/mod.rs`, frontend `src/`, routing/balancer/quota
modules, release-restore, session-migration, pause-handshake, and
session-lock surfaces all remain untouched in the proposal text.

### Anti-scope

The proposal §2 "Anti-scope" enumerates exactly the exclusions the
ticket's "Out of scope" and "Anti-scope" sections require: no
chains-table schema change, no retroactive JSONL backfill, no
deletion of existing metadata columns, no body compression, no
encryption-at-rest, no multimodal expansion (D-002 stays in
force), no BLOB v1, no metadata-only compatibility shims, no
cross-CLI canonical conversion, no canonical-record wire schema
change, no routing/balancer/quota/release-restore/pause-handshake/
session-lock changes, and no `src/` frontend changes.

The §3 supported-surface track adds the matching adjacent-paths
note: quota and routing keep counting assistant turns and do not
inspect `body`; frontend surfaces do not change. None of the
named anti-scope categories leak into proposed changes.

### Test Boundary

The proposal's test-intent table (§5, T1–T12) places fixtures
either in `src-tauri/tests/empty_bodies_ref_rca/` (the new RCA
tree introduced by Phase 0 and named in-scope by the ticket Test
Boundary), in `state::db` inline tests (in-scope), in
`session_export` / `session_replace` / `trace` inline or
particular-integration fixtures (in-scope), or in adapter
fixtures alongside the scripts (in-scope).

No test in the proposal touches `routing_fanout_rca/`,
`release_yml_contract.rs`, `session_lock_cross_platform.rs`,
`session_migration_rca/`, or `e2e/`. T5 explicitly verifies the
WU-13 quota topology coexistence inside `state::db` inline tests,
which is in-scope and does not modify quota tests.

### Schema-probe boundary

Properly bounded. Proposal §1 states explicitly:
"`src-tauri/src/schema_probe/mod.rs` does not join the code
boundary; `CURRENT_SCHEMA_VERSION` remains `3`." The migration is
column-presence-checked via `PRAGMA table_info(session_turns)`,
the column is nullable and additive, and `StateDb::open` does not
read or write `PRAGMA user_version`. Assumption A8 records the
predicate. Open question §7-Q1 keeps "could a risk gate require
schema_probe to expose `body`?" as an explicitly-named open
question with a default-no answer rather than silently expanding
the boundary. No proposed change implicitly requires editing
`src-tauri/src/schema_probe/mod.rs`.

### session_metadata as verification-only

Properly bounded. The only proposal references to
`session_metadata` are §3's statement that it "continue[s] to
report transcript availability and workspace roots but do[es] not
become body readers," which is verification language, not a
modification proposal. No edits to
`src-tauri/src/session_metadata/mod.rs` are described in §1–§7.

### Deferrals named explicitly

Per `~/ai/conventions/no-deferred-stubs.md`:

- **Multimodal / BLOB.** Named-and-deferred against an existing
  scheduled item: "BLOB is deferred; if D-002 reopens, valid
  UTF-8 TEXT values can be migrated into BLOB storage without
  inventing a compatibility shim now" (§1). The follow-up is the
  checked-in `DECISIONS.md` D-002 entry, not an inline TODO.
- **Cross-CLI canonical conversion.** Anti-scope, not a deferral:
  "No cross-CLI canonical conversion. Bodies are captured and
  replayed through the existing per-provider parser/adapter
  semantics" (§2). The proposal explicitly forecloses, not
  defers.
- **Retroactive backfill from provider JSONL.** Anti-scope, not a
  deferral: "No retroactive backfill of legacy rows from provider
  JSONL" (§2). Combined with §1's "legacy rows remain `NULL`;
  new ingest/import-replace rows write `Some(...)`" and with §3's
  decision that export errors when JSONL is missing AND DB body
  is `NULL` (rather than silently emitting empty content), the
  legacy-data behavior is specified end-to-end rather than
  stubbed.

No "TODO," `NotImplementedError`, silent-`None`, or
"will-finish-later" placeholder appears in the proposal text.

## 3. Justification

The proposal touches only files listed in the ticket's Code
Boundary, names every excluded surface that the ticket's
Anti-scope names, places all tests inside in-scope test trees,
keeps `schema_probe` and `session_metadata` properly out of the
edit set, and converts each deferred concern into either a
named-and-scheduled deferral against `DECISIONS.md` D-002 or a
forward anti-scope clause.
