# WU-15-01 — PR Justification Review (Phase 8)

## Verdict: **LOW**

Every material change in `git diff main..HEAD` is either (a) explicitly
required by `product-strategy/contracts/wu-15-01-empty-bodies-ref.md`
§2 / AC-1..AC-8, (b) explicitly carved out as not-to-be-flagged by the
review constraints (compile-surface body-None sweep, repro-harness
updates, doc updates, DECISIONS entries, CLI export dedup at
`main.rs:759-770`, null-placeholder trace test updates), or (c) a small
justified deviation with a documented review-finding citation in the
test source.

## Findings

### 1. `src-tauri/src/lib.rs:18-25` — `test_support::env_lock` module — **JUSTIFIED**

A new `#[cfg(test)] pub(crate)` module exposes a process-wide
`Mutex<()>` for tests that mutate `XDG_CONFIG_HOME` /
`XDG_DATA_HOME`. The previous incarnation lived inline in
`state/db.rs::tests` (`env_lock`); this PR moves it to `lib.rs` so
the new env-mutating tests in
`src-tauri/src/session_export/mod.rs` (`with_data_home`),
`src-tauri/src/session_replace/mod.rs` (`with_homes`), and
`src-tauri/src/trace/mod.rs::tests` can share *the same* lock.
Duplicating the helper per module would silently break mutual
exclusion (each module would get its own lock, not synchronizing
cross-module env writes), so the central placement is necessary
for correctness, not stylistic. No production-surface impact:
gated behind `#[cfg(test)]` and `pub(crate)`.

Justification needed: none beyond what the test sites already
demonstrate. Keep as-is.

### 2. `src-tauri/src/sessions/mod.rs:58-69` and `:131-148` — `is_canonical_body_shape` validator — **JUSTIFIED (extends contract)**

Contract §2 ingest only specifies "On serialization failure: log
scan diagnostic naming provider/line; treat the row as having NULL
body; do NOT error the whole scan." The PR adds an *upstream*
shape-validation gate that rejects bodies which are not
`Array<Object{type: string, text?: string}>` and writes NULL +
diagnostic in those cases. The new test
`scan_provider_rejects_non_canonical_body_shape`
(`src-tauri/src/sessions/mod.rs:472-507`) cites
**CodeRabbit R4-F05**: an invalid adapter body would otherwise
poison the DB-fallback export path, where
`read_canonical_transcript_from_state_db`
(`src-tauri/src/session_export/mod.rs:170-189`) hard-fails with
`ExportError::Operational` if `serde_json::from_str::<Vec<ContentChunk>>`
returns Err. Catching at ingest preserves the contract's "missing
body is accepted data, not an ingest error" stance for legacy
adapters while preventing structurally-bad bodies from reaching
readers that *will* error.

Justification needed: none beyond the existing R4-F05 citation in
the test comment. Keep as-is.

### 3. `src-tauri/src/session_replace/mod.rs:381` — `probe_state_schema_compatible` adds `body` to required column list — **JUSTIFIED DEVIATION FROM CONTRACT**

Contract §2 import-replace explicitly states:
"`probe_state_schema_compatible` at `:336-382` is a read-only
preflight; it does NOT require `body` (schema-probe v3 unchanged)."
The PR adds `"body"` to the `require_columns(..., "session_turns", ...)`
list, which contradicts that contract sentence.

The deviation is internally justified:

- The new column is added unconditionally by
  `ensure_session_turns_schema`
  (`src-tauri/src/state/db.rs:1045-1048`) on every `StateDb::open`.
  Any DB the binary itself produced or opened will have the column,
  so the stricter probe does not regress real-world callers.
- `replace_db_turns` (`src-tauri/src/session_replace/mod.rs:884-905`)
  now binds `body` as a required INSERT param; without the probe
  check, a DB missing the column would fail mid-transaction with a
  less actionable SQLite error instead of the documented exit-14
  `schema-incompatible` path.
- The associated test
  `t_schema_incompatible_missing_body_column_exit_14`
  (`src-tauri/tests/initiative_06_import_replace.rs:464-490`)
  cites **CodeRabbit R1-F02**.
- The schema-probe constraint quoted by the contract referred to
  `src-tauri/src/schema_probe/mod.rs:217-231` ("schema-probe v3
  unchanged"), which the PR does **not** modify; the
  `probe_state_schema_compatible` helper here is a separate,
  import-replace-local preflight.

Justification needed: none — the test comment + R1-F02 citation
are sufficient. Recommend keeping. (Minor follow-up: a one-line
note in the contract or a Round-N changelog entry acknowledging
the deviation would close the audit loop, but is not blocking.)

### 4. `src-tauri/tests/initiative_06_import_replace.rs:464-490` — new `t_schema_incompatible_missing_body_column_exit_14` — **JUSTIFIED**

Coupled with finding 3. Verifies the new probe column requirement
through the CLI exit-code surface. Fixture, structure, and
`assert_json_error` usage match neighboring `t_schema_incompatible_*`
tests. Keep.

### Non-findings (verified, no action)

- `src-tauri/src/state/db.rs` schema/migration/insert/test changes —
  contract-required (AC-1, §2 ingest).
- `src-tauri/src/session_export/mod.rs` — `read_canonical_transcript`
  branch, `read_canonical_transcript_from_state_db`, and the two
  new `read_canonical_transcript_*` tests — contract §2 export
  + §4 T7/T8.
- `src-tauri/src/session_replace/mod.rs:884-905` body INSERT
  binding + new `import_replace_round_trips_*` test — contract §2
  + §4 T9.
- `src-tauri/src/trace/mod.rs` — `TraceTranscriptTurn`,
  `TraceBodyState`, `read_inline_transcript`, mixed-state test —
  contract §2 trace + §4 T10 + R4-N02.
- `src-tauri/src/main.rs:759-770` — canonical_jsonl_bytes dedup —
  carved out by review constraints (R4-N04).
- 6 compile-surface `body: None` test-fixture additions
  (balancer, initiative_05, routing_fanout, pr_f_resume, plus
  inline tests in db.rs and trace/mod.rs) — carved out as
  mechanical propagation.
- `src-tauri/tests/empty_bodies_ref_rca/*` harness updates and
  the two `pr_b_trace_integration.rs` null-placeholder updates —
  carved out (Phase 6b test-first contract, R4-N02).
- `scripts/claude-code-turns`, `scripts/codex-turns`,
  `scripts/README.md`, `README.md` — contract §2 adapter +
  AC-7. Two new adapter integration tests
  (`tests/scripts/{claude_code,codex}_turns_body.rs`) cover
  contract §4 T11/T12 and one CodeRabbit follow-up each (R1-F03,
  R1-F04).
- `DECISIONS.md` D-012/D-013/D-014 — carved out (AC-8).
- All `proposals/`, `research/`, `risk/`, and contract markdown
  files — WU-15-01 planning artifacts already committed in
  `8c35a6d`; required by the implementation pipeline.

### Side note (out of scope of justification, flagged for accuracy)

`README.md:453` documents `body_state` values as `"available"` and
`"missing"`, but the production enum
(`src-tauri/src/trace/mod.rs:84-90` with
`#[serde(rename_all = "snake_case")]`) emits `"stored"`,
`"missing"`, `"invalid"`. This is a content defect inside an
otherwise-justified AC-7 doc change, not an unjustified change.
Worth a follow-up edit so the README matches the wire shape the
trace tests assert on.
