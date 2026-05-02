# Scope Risk Assessment (round 3): proposals/10-routing-claude-skipped.md

## Verdict: LOW

The round-2 → round-3 revision closes `R2-F01` and `R2-N01` with text that
stays inside the round-1/round-2 scope envelope. The new "Recovery procedure
on unexpected `providers` shape" subsection is bounded to operator-level
steps (file-system backup restore + external `sqlite3` shell drop) and
explicitly disclaims any binary-side auto-recovery — it does not introduce a
new heuristic recovery affordance, and the closing paragraph reaffirms that
the unexpected-shape branch remains a hard error in `StateDb::open`. The
"Residual: rollback during mid-rebuild failure" subsection honestly downgrades
the rollback property's verification from "runtime test" (which R2-F01 showed
the round-2 rename-conflict test did not actually cover) to "code-review at
implementation time," and explains the convention barrier (no test-only
product source per `no-deferred-stubs.md`) that prevents an in-scope runtime
test. The replacement idempotency test pins a property of the same in-scope
migration helper (`ensure_providers_schema`); the strengthened unexpected-shape
test adds byte-identity pre/post assertions and a post-cleanup recovery step
that both stay inside the migration error contract boundary. Out-of-scope
surfaces (quotas, `derive_pools`, `provider_name(command)`, UI, IPC,
`migrate-config`, backwards-compatibility shims) remain explicitly excluded;
no surface added, no surface silently dropped. Watch signals `WS-1`, `WS-2`,
and `WS-3` from `risk/10-history.md` are all honored.

## Prior verdict carry-over

Rounds 1 and 2 returned LOW. Round 3 still upholds LOW because the revision
closes `R2-F01` and `R2-N01` without expanding into out-of-scope work and
without quietly shrinking properties the proposal still claims to hold.

## Findings (round 3)

### Recovery procedure is bounded to operator-level steps

- Severity: low
- Direction: in-scope (closes R2-N01)
- Subject: proposal §Migration / Recovery procedure on unexpected `providers`
  shape (lines 99–106)
- Evaluation: The subsection lists exactly two operator-level steps:
  (1) restore `state.db` from a file-system backup, then re-run `agents`;
  (2) if no backup exists, manually drop the malformed `providers` table
  (and any leftover `providers_legacy_index_keyed`) via an external
  `sqlite3` shell, after which the next `StateDb::open` takes the
  already-specified "table is missing → create post-fix shape" branch and
  the supported normal-write path repopulates the aggregate over time.
  No binary-side auto-recovery code is introduced; the closing paragraph
  explicitly states the procedure is "operator-level by design" and that
  hybrid/foreign shapes "can only arise from external mutation (the binary
  itself never produces one because the migration is transactional)." Watch
  signal `WS-1` ("must not acquire heuristic recovery affordances") is
  upheld — the unexpected-shape branch remains a hard error in
  `StateDb::open`, and the recovery procedure is documentation about what
  the operator does outside the binary, not new in-binary behavior. The
  acknowledgement that aggregate counts older than the manual drop are not
  recoverable (because `providers` is downstream of `invocations`) is a
  re-statement of the existing migration's source-of-truth model, not a
  new scope concession.

### Rollback residual is precision, not silent shrink

- Severity: low
- Direction: in-scope (closes R2-F01)
- Subject: proposal §Migration / Residual: rollback during mid-rebuild
  failure (lines 108–110)
- Evaluation: Round 2 carried a "Migration rollback on failed rebuild" test
  that used a pre-existing `providers_legacy_index_keyed` table to force
  the `RENAME TO` step to fail. R2-F01 correctly identified that this test
  does not actually cover transactional rollback because the rename
  conflict happens before any rebuild SQL runs — an implementation that
  omitted the `BEGIN`/`COMMIT` envelope would still pass it. Round 3
  removes that test entry and instead documents the rollback property as
  an explicit residual verified by code review of `ensure_providers_schema`
  at implementation time. Crucially, the proposal still claims the property
  holds (the round-1 paragraph at lines 95–96 is unchanged: "If any step
  fails, the transaction rolls back, leaving the original `providers` table
  intact… the `invocations` source of truth is never modified by the
  migration, so retries are deterministic"). What changed is only the
  verification mechanism — from a runtime test that did not actually verify
  it to code review of the explicit `BEGIN`/`COMMIT` envelope. The residual
  text enumerates the four reasons a runtime test is not viable in scope:
  (a) test-only `CHECK` constraints, (b) mid-transaction temp triggers,
  (c) sibling-connection lock contention, (d) OS-level fault injection —
  the first two require test-only product-source changes (which violates
  `~/ai/conventions/no-deferred-stubs.md`), the latter two are out of scope
  for unit/particular-integration tests. Watch signal `WS-3` is honored:
  the property is recorded as a code-review residual with explicit rationale
  for the verification mechanism, not silently re-promoted to a runtime
  test claim and not silently dropped. The unexpected-shape rejection test
  is named in the residual paragraph as covering the early-exit DDL
  collision path, which is the in-scope sibling concern.

### Idempotency test stays on the migration helper

- Severity: low
- Direction: in-scope (replacement, not expansion)
- Subject: proposal §Test-intent track (line 242)
- Evaluation: The new "Migration `ensure_providers_schema` is idempotent
  across reopens" entry pins a property of the same in-scope migration
  helper that the round-1 / round-2 migration entries already cover: the
  helper observes the post-fix shape on a second open and returns
  immediately (no rebuild re-fires, no aggregate mutation). The fixture
  (in-memory or on-disk `StateDb` opened once on a pre-fix shape and then
  reopened on the same DB file) and the assertion (row count plus
  `invocation_count`/`error_count`/`last_invoked_at` byte-identical across
  the second open) both stay inside `ensure_providers_schema`'s contract.
  No new surface is exercised — this is a complementary branch of the
  same shape-inspection migration, not a probe of crash recovery, density
  scoring, quotas, or any out-of-scope surface. The residual ("Does not
  directly exercise transactional rollback") is honest about what the
  test covers and links to the rollback residual rather than overclaiming.

### Strengthened unexpected-shape test stays inside the error contract

- Severity: low
- Direction: in-scope (precision)
- Subject: proposal §Test-intent track (line 241)
- Evaluation: Round 2's unexpected-shape test asserted that `StateDb::open`
  returned an error on a hand-crafted `providers` table with both
  `provider_index` and `provider_name`. Round 3 strengthens it with two
  additions, both inside the migration error contract boundary:
  (a) "byte-identity pre/post" — the migration error contract already says
  "no source rows are mutated" and "the `invocations` source of truth is
  never modified by the migration"; the byte-identity assertion is the
  observable signal of that already-claimed property, not a new property.
  It is an `sqlite3_db` file-content equality check after a failed open,
  which sits inside the same particular-integration test surface the
  round-2 entry already used; it does not reach into UI, IPC, or any
  adjacent surface.
  (b) Post-cleanup recovery — after the operator-level cleanup (drop the
  malformed `providers`), the second `StateDb::open` is asserted to take
  the "table is missing → create post-fix shape" branch. That branch is
  the round-1 migration's step 4, already in scope; the test exercises an
  existing migration branch, it does not introduce a new branch. This
  also doubles as a runtime check that the operator-level recovery
  procedure (lines 99–106) lands the DB on a supported path. The residual
  ("Does not enumerate every malformed shape") is unchanged from round 2;
  the residual reference to the rollback gap correctly points at the new
  rollback residual subsection rather than at a deleted test entry.

### Out-of-scope surfaces unchanged

- Severity: low
- Direction: in-scope (no overflow)
- Subject: proposal §Out of scope (lines 172–181)
- Evaluation: All round-1/round-2 exclusions remain verbatim: no quota
  table changes, no `provider_quotas`/`provider_quota_windows` changes,
  no `provider_name(command)` parsing changes, no `derive_pools` changes,
  no UI changes, no new IPC commands, no `migrate-config` changes, no
  `migrate-db` behavior changes beyond opening `StateDb`, and no
  backwards-compatibility shim or index-keyed fallback reader kept after
  migration. Watch signal `WS-2` ("no index-keyed reader alias has been
  kept on the routing-history surface") remains upheld — the in-scope
  list at lines 163–170 still names only the post-fix `(model_name,
  provider_name)` aggregate and the name-keyed `recent_error_count`
  signature.

### No silent shrink of round-1/round-2 scope

- Severity: low
- Direction: in-scope (no shrink)
- Subject: cross-check vs. round-2 `risk/10-scope.md` and `risk/10-history.md`
- Evaluation: All round-1 and round-2 in-scope items remain present —
  aggregate key fix at `(model_name, provider_name)`, `recent_error_count`
  identity fix, `ProviderRecord` signature change, shape-based migration
  with explicit error contract, writer/reader updates, balancer call-site
  updates, `examples/quota_check.rs` build-only update, the unexpected-shape
  rejection test (now strengthened, not weakened), the `last_error_at`
  failed-row test, and the existing RCA harness plus reorder/rename
  round-trips. The round-1 rejection of Option B (composite key with
  `provider_index`) and the rebuild-from-`invocations` strategy are both
  retained verbatim. The transactional rollback property of
  `ensure_providers_schema` is still claimed in the round-1 paragraph at
  lines 95–96 — only its verification mechanism moved from runtime test
  to code review, with explicit rationale. That is precision, not shrink.

## Notes

- Watch signals `WS-1`, `WS-2`, and `WS-3` from `risk/10-history.md` are
  all honored in the round-3 text.
- The recovery procedure is documentation-only and operator-level; future
  rounds should re-check that it stays operator-level and does not grow
  into a binary-side `agents repair-providers` or similar auto-recovery
  affordance, which would breach `WS-1`.
- The rollback residual now has a load-bearing dependency on the
  `BEGIN`/`COMMIT` envelope being present in `ensure_providers_schema` at
  implementation time. The implementation diff for Phase 5 must include
  that explicit transaction wrapper; if it doesn't, the residual is
  unverified and `WS-3`'s "code review only" claim becomes vacuous. This
  is a Phase 5 implementation watch, not a round-3 scope deviation.
- The strengthened unexpected-shape test now performs a `state.db` byte
  comparison; this is a particular-integration test concern (file-content
  equality after a failed open). Future rounds should re-check that this
  comparison stays at the migration-error-contract boundary and does not
  expand into a general DB-snapshot regression harness.
