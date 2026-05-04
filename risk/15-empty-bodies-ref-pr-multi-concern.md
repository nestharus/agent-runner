# WU-15-01 — PR multi-concern review

Reviewer: Phase 8 multi-concern check (`claude-opus`)
Branch: `impl/wu-15-01` vs `main`
Commits in scope:
- `242cb87` — `rca(state): reproduce missing-body-storage regression`
- `8c35a6d` — `docs(empty-bodies-ref): WU-15-01 planning artifacts (research, proposal, contract, risk reports)`
- `5f14c22` — `fix(state): store turn bodies directly in state.db` (was `15d6ee1` pre-CodeRabbit)

## 1. Verdict

**KEEP_AS_ONE.**

The diff is one body-storage concern delivered through the standard
implementation-pipeline lifecycle (RCA → planning → fix), with every
file mapped to a contract-named code surface and no unrelated WU
touched.

## 2. Why this is a single concern

### 2.1 Every code/doc change maps to a contract-named surface

The contract `product-strategy/contracts/wu-15-01-empty-bodies-ref.md`
§2 enumerates the in-scope code surfaces. Each touched file in the
fix commit lines up with one bullet:

- Schema migration + ingest helpers — `src-tauri/src/state/db.rs`
  (contract §2 "Schema migration"). New nullable `body TEXT`,
  column-presence migration in `ensure_session_turns_schema`, and
  `SessionTurnIngest.body` in single + bulk inserts.
- Ingest body persistence — `src-tauri/src/sessions/mod.rs` (contract
  §2 "Ingest"). `ScriptTurn.body: Option<Value>` plus canonical-shape
  validation before serializing to compact JSON for DB.
- Export DB-fallback — `src-tauri/src/session_export/mod.rs` (contract
  §2 "Export"). JSONL-first read, `state.db` fallback with
  `RecordSource.storage_type = "state_db"` + `db://session_turns/<id>`
  sentinel exactly as specified.
- CLI export-loop dedup — `src-tauri/src/main.rs:757-776` (contract
  §2 R4-N04). Replacing the manual serializer with
  `session_export::canonical_jsonl_bytes` is required by the contract
  for byte-stability between JSONL and DB-fallback paths; not a
  separate concern.
- Import-replace body column — `src-tauri/src/session_replace/mod.rs`
  (contract §2 "Import-replace"). New `body` value bound inside the
  same atomic transaction; preflight `probe_state_schema_compatible`
  now requires `body`.
- Trace inline transcript — `src-tauri/src/trace/mod.rs` (contract §2
  "Trace inline transcript"). `TraceTranscriptTurn` + `TraceBodyState`
  enum with `Stored` / `Missing` / `Invalid` exactly as specified.
- Adapters + adapter docs — `scripts/claude-code-turns`,
  `scripts/codex-turns`, `scripts/README.md` (contract §2 "Adapter
  scripts" + "Documentation").
- README — `README.md` AC-7 sections only (contract §2
  "Documentation"; ticket AC-7 verified for §Session Ingestion,
  §Inspecting a Run, §Exporting a Session, §Replacing a Session
  Transcript; §Load Balancing untouched).
- DECISIONS — `DECISIONS.md` D-012/D-013/D-014 (contract §2
  "DECISIONS.md" / ticket AC-8). Three entries match the three
  required AC-8 records.

### 2.2 Compile-surface sweep is mechanical propagation

`SessionTurnIngest` gained a `body: Option<String>` field, so every
constructor must add `body: None`. The contract §2 "Compile-surface
sweep" enumerates these sites; the diff hits exactly that list:

- `src-tauri/src/balancer/mod.rs:972` — test helper.
- `src-tauri/tests/initiative_05_migration.rs:257`.
- `src-tauri/tests/routing_fanout_rca/mod.rs:68`.
- `src-tauri/tests/pr_f_resume_integration.rs:320`.
- `src-tauri/src/state/db.rs:3498`, `:6004`, `:6039` inline tests.
- `src-tauri/src/trace/mod.rs` test fixtures.

This is required field propagation, not a refactor of unrelated tests.
Per the constraints, the body-storage fix legitimately spans these
six surfaces and the `body: None` sites are part of the same concern.

### 2.3 RCA harnesses + planning artifacts belong with the fix

The three commits implement the standard implementation-pipeline arc
for one work unit:

- `242cb87` — Phase 0 RCA report (`research/12-empty-bodies-ref-rca.md`)
  and four RED reproduction harnesses
  (`tests/empty_bodies_ref_rca/rc{1..4}_*.rs`). The ticket
  (`tmp/scratch/wu-15-01/ticket.md:10-16`) names this commit as the
  inherited RCA branch the impl branch is built on. Without these
  harnesses there is no RED→GREEN signal for AC-1..AC-4; without the
  RCA there is no shared problem statement for Phase 1+ artifacts.
- `8c35a6d` — Phase 1-5 planning artifacts: proposal, problem map,
  hookpoints, contract, plus risk audits (audit, scope, shortcut,
  supported-surface, two process-tree phase audits). All filenames
  carry the `15-empty-bodies-ref` slug; none reference unrelated WUs.
- `5f14c22` — Phase 6c product code + Phase 6b tests + the four
  harnesses flipped GREEN.

Per the implementation pipeline these are not three separable
concerns — they are three lifecycle phases of one concern (each
phase's output is an input to the next). Splitting would either
strand RED harnesses on `main` or land a fix without the contract /
RCA they reference.

### 2.4 Documentation, RCA, and harness changes are scoped to WU-15-01

No artifact in the diff references a non-WU-15-01 slug:

- `proposals/15-empty-bodies-ref.md` (new, Phase 1-4).
- `product-strategy/contracts/wu-15-01-empty-bodies-ref.md` (new,
  Phase 6a).
- `research/12-empty-bodies-ref-rca.md`,
  `research/15-empty-bodies-ref-hookpoints.md`,
  `research/15-empty-bodies-ref-problem-map.md` (new).
- `risk/15-empty-bodies-ref-{audit,scope,shortcut,supported-surface}.md`,
  `risk/15-empty-bodies-ref-process-tree-audit-phase{4,6}.md` (new).

`DECISIONS.md` only appends D-012/D-013/D-014 — no edits to existing
entries. `README.md` only modifies the four AC-7-named sections.
`scripts/README.md` only adds the `body` field paragraph. The PR does
not touch `proposals/{01,06}-*.md` despite "superseding" them — that
is correct per the contract (D-012 records the override; the prior
proposals stay as-is).

### 2.5 New `lib.rs` test_support helper is contained

`src-tauri/src/lib.rs` adds a tiny `#[cfg(test)] pub(crate) mod
test_support` exposing `env_lock()`. The `env_lock` helper was
previously inlined in `src-tauri/src/state/db.rs` tests; it's
extracted so the new `session_replace` integration test module can
share it. This is a 10-line scaffold extraction directly serving the
new test introduced for this WU (the import-replace round-trip with
bodies in `session_replace/mod.rs:1184-1413`). Not a separable
refactor.

## 3. Findings

### F1 — All file additions/modifications cite a contract surface

Every code surface in the fix commit (`5f14c22`) maps to a §2 bullet
in `product-strategy/contracts/wu-15-01-empty-bodies-ref.md`. The
compile-surface sweep matches §2's enumerated sites verbatim.

### F2 — The CLI export-loop dedup is contract-mandated, not bonus refactor

Per the constraint reminder, the replacement of the manual
`for record in records { serde_json::to_string ... }` block at
`src-tauri/src/main.rs:759-770` with
`session_export::canonical_jsonl_bytes(&records)` is specified in
contract §2 "Export" (R4-N04) to eliminate byte-stability drift
between JSONL-present and DB-fallback paths, both of which now flow
through the canonical serializer. It is part of the body-storage
concern, not a tagalong cleanup.

### F3 — Trace test renamed to assert new contract, not removed

`src-tauri/tests/pr_b_trace_integration.rs` renames
`inline_transcript_with_json_is_accepted_and_returns_null_payloads`
→ `..._returns_empty_arrays_without_turn_rows` and updates the
assertion from `is_null()` to `len() == 0`. This matches the contract
§2 "Trace inline transcript" change to `Option<Vec<TraceTranscriptTurn>>`
(empty array, not `null`, when there are zero rows). Required test
update, not unrelated rework.

### F4 — Three DECISIONS entries match three AC-8 records

`DECISIONS.md` appends D-012 (design intent override), D-013 (Phase
0 done), D-014 (Phase 2.5 human-gate skip). Each maps 1:1 to ticket
AC-8 / contract §2 "DECISIONS.md".

### F5 — No cross-WU code or artifact touched

The PR does not touch:

- routing/balancer/quota production code beyond the `body: None`
  test-helper sweep (WU-11-01 / WU-13-01 / WU-14-01 territory).
- `tests/release_yml_contract.rs`, `tests/session_lock_cross_platform.rs`,
  `tests/session_migration_rca/*` (out-of-scope per ticket AC-5).
- `src/` frontend (out-of-scope per ticket Code Boundary).
- `proposals/{01,06}-*.md` files that are "superseded" by D-012 (correct
  — the override is recorded in DECISIONS, not by editing the older
  proposals).

### F6 — Anti-scope respected

Spot-checked: no `session_chains`/`session_chain_segments` schema
edits, no retroactive backfill, no `source_file` removal, no
compression/encryption/dedup, no canonical-record wire schema change
beyond the additive `state_db` storage_type variant which is itself
a contract requirement.

## 4. Split boundaries (only relevant if SPLIT_REQUIRED — not applicable here)

None proposed. For completeness, the conceivable split lines are all
rejected by the contract or the constraint reminder:

- Schema/migration vs. ingest vs. export vs. replace vs. trace —
  each surface depends on the schema column existing and on the
  shared `ContentChunk` type already used by export. AC-1..AC-4
  cannot be exercised independently because each RC harness (RC-1
  schema, RC-2 ingest, RC-3 export, RC-4 trace) touches the shared
  body column and shared `SessionTurnIngest` field.
- CLI export-loop dedup — explicitly part of the contract per F2; not
  separable.
- Documentation (README AC-7, scripts/README, DECISIONS) — part of the
  contract per the constraint reminder; not separable.
- Compile-surface sweep — required because the production struct
  gained a non-optional struct field; mechanical propagation per the
  constraint reminder.
- RCA / planning artifacts vs. fix — landed as one PR by design of
  the implementation pipeline; the harnesses must merge with the fix
  that flips them GREEN, otherwise `main` carries known-RED tests.

## 5. Citations

- Contract: `product-strategy/contracts/wu-15-01-empty-bodies-ref.md:80-255`
  (in-scope surfaces) and `:256-269` (anti-scope reaffirmed).
- Ticket: `tmp/scratch/wu-15-01/ticket.md:10-16` (RCA inherited),
  `:67-126` (AC-1..AC-8), `:127-159` (Code Boundary in/out scope),
  `:190-207` (Anti-scope).
- Proposal: `proposals/15-empty-bodies-ref.md:1-7` (scope), `:87-101`
  (anti-scope).
- Fix commit code surfaces:
  `src-tauri/src/state/db.rs:198-202,637-642,1042-1046,2576-2585,2614-2647`,
  `src-tauri/src/sessions/mod.rs:42-69,128-160`,
  `src-tauri/src/session_export/mod.rs:88-200`,
  `src-tauri/src/session_replace/mod.rs:378-385,884-905`,
  `src-tauri/src/trace/mod.rs:33-95,159-285`,
  `src-tauri/src/main.rs:757-776`,
  `src-tauri/src/lib.rs:18-26`.
- Compile-surface sweep:
  `src-tauri/src/balancer/mod.rs:974`,
  `src-tauri/tests/initiative_05_migration.rs:257`,
  `src-tauri/tests/routing_fanout_rca/mod.rs:68`,
  `src-tauri/tests/pr_f_resume_integration.rs:320`,
  `src-tauri/src/state/db.rs:3498,6004,6039` (inline tests).
- DECISIONS: `DECISIONS.md:284-329` (D-012/D-013/D-014).
- README: `README.md:380-394,450-453,513-528,561-564`.
