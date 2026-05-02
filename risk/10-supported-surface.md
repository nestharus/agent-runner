# Supported-Surface Risk Assessment (round 3): proposals/10-routing-claude-skipped.md

## Termination signal: none

## Verdict: LOW

The round-3 revision keeps the proposal squarely on the supported surface
(single-user desktop Tauri v2 with embedded SQLite, opened through
`StateDb::open` for both normal CLI runs and `agents migrate-db`) and
strengthens it on the two cross-gate items the round-2 shortcut report
flagged (`R2-F01`, `R2-N01`) without introducing any new user-facing
surface or binary-side recovery affordance. The new "Recovery procedure
on unexpected `providers` shape" paragraph (proposal lines 99-106) is
explicitly operator-level — restore from a file-system backup of
`state.db`, or use an external `sqlite3` shell to drop the malformed
table so the "table is missing → create post-fix shape" branch runs on
the next open. It does **not** add a heuristic recovery branch inside
`StateDb::open`, which is the posture `WS-1` was protecting. The new
"Residual: rollback during mid-rebuild failure" paragraph (lines
108-110) records the testability gap explicitly and names the rejected
failure-injection mechanisms (test-only `CHECK`/triggers in product
source, sibling-connection contention, OS fault injection); this
exactly matches `WS-3`'s requirement that the residual not be silently
re-promoted to a runtime claim. The rollback property's risk envelope
on this surface is bounded by the unchanged "migration never mutates
`invocations`" guarantee, so the worst-case mid-rebuild crash on a
single-user desktop is recoverable via the new operator procedure (or
already deterministically retryable on the next open if the rebuild
transaction did roll back). The round-2 broken rollback test is replaced
by an idempotency-across-reopens unit test plus a strengthened
unexpected-shape rejection test that asserts byte-identity pre/post and
post-cleanup recovery — both honest and on-surface. None of A1-A6
shifts; A3 and A5 remain slightly strengthened by the explicit
operator-level recovery contract. The blast radius (fallback usage
scoring + recent-error suppression on the single-user local desktop
cohort) is unchanged. Net value remains clearly positive on the
supported surface.

## Prior finding status

- `R2-F01` (medium → closed): closed (cross-gate)
  - Closure evidence: the round-3 revision retires the broken
    rename-conflict "rollback" test entry and replaces it with an
    "idempotency across reopens" unit test (proposal line 242) that
    actually exercises the post-fix-shape no-op branch. The rollback
    property itself is now documented as a code-review-only residual
    (lines 108-110) rather than as a runtime test claim, which is the
    honest posture for the supported surface and aligns with
    `~/ai/conventions/no-deferred-stubs.md` (no test-only `CHECK`
    constraints or triggers added to product source). On the supported
    surface, this is the right posture: the rollback only matters in a
    rare mid-rebuild failure, the bounded "no mutation of
    `invocations`" guarantee plus the new operator recovery procedure
    cover that path, and falsely advertising runtime coverage would be
    worse than acknowledging the gap. Cross-gate: this finding was
    raised by the shortcut gate; the supported-surface gate concurs
    that the round-3 resolution preserves the supported posture.
- `R2-N01` (low note → closed): closed (cross-gate)
  - Closure evidence: the new "Recovery procedure on unexpected
    `providers` shape" subsection (proposal lines 99-106) documents
    the operator-level recovery: (1) restore `state.db` from a
    file-system backup, then re-run `agents`; (2) if no backup, use
    an external `sqlite3` shell to drop the malformed `providers`
    (and any leftover `providers_legacy_index_keyed`) so the next
    `StateDb::open` takes the "table is missing → create post-fix
    shape" branch and the supported normal-write path repopulates
    aggregate state from subsequent finalizations. The proposal
    correctly characterizes this as operator-level by design,
    because a hybrid/foreign shape can only arise from external
    mutation (the binary's transactional migration never produces
    one). Cross-gate: this finding was raised by the shortcut gate;
    the supported-surface gate concurs that the recovery procedure
    is feasible for the single-user desktop cohort and does not
    elevate `state.db` to a documented external-mutation surface.

## Watch signal status

- `WS-1` (migration helper remains transactional and refuses
  heuristic recovery): **upheld**. The round-3 revision adds no
  new binary-side recovery branch. The recovery procedure is
  external (operator action via filesystem backup restore or
  external `sqlite3` shell), so `StateDb::open` itself still
  rejects unexpected shapes with an error, and the rename + create
  + rebuild + drop sequence still runs in one SQLite transaction.
  The strengthened unexpected-shape test (line 241) now also
  asserts post-cleanup recovery, which binds the procedure to
  test-observable behavior without weakening the rejection
  contract.
- `WS-2` (no index-keyed reader alias on the routing-history
  surface): **upheld**. Round 3 makes no changes to
  `get_provider`, `recent_error_count`, or the `providers` schema.
  The signature remains `(model_name, provider_name)`-only.
- `WS-3` (rollback during mid-rebuild is a code-review-only
  residual; do not silently re-promote to a runtime claim without
  specifying a viable failure-injection mechanism): **upheld and
  strengthened**. The new residual paragraph (lines 108-110)
  names exactly which failure-injection mechanisms were
  considered (test-only `CHECK` constraints, mid-transaction temp
  triggers, sibling-connection lock contention, OS-level fault
  injection) and why each is unsuitable here; the rollback
  property is explicitly recorded as verified by code review of
  the explicit `BEGIN`/`COMMIT` envelope rather than by a runtime
  test. This is precisely the posture `WS-3` was created to
  protect.

## Findings (round 3)

### Recovery procedure preserves forward-only rollout posture

- Severity: low
- Surface concerned: writable DB open (`StateDb::open`) on the
  single-user local desktop cohort; the `state.db` file as an
  operator-managed artifact.
- Net effect: reduces risk.
- Claim from proposal: when `StateDb::open` (or `agents migrate-db`)
  rejects a malformed `providers` shape, the user-facing recovery is
  (1) restore from the most recent file-system backup of `state.db`,
  or (2) drop the malformed `providers` table (and any leftover
  `providers_legacy_index_keyed`) using an external `sqlite3` shell;
  aggregate counts older than the manual drop are not recoverable
  but re-derive from new finalizations.
- Evaluation: this is the correct posture for a supported surface
  that already declared "rollback is binary rollback plus restoring
  the pre-migration database from backup" (proposal §Rollback path).
  Critically, the procedure does **not** introduce an unsupported
  recovery contract: it adds zero binary-side branches, makes zero
  new IPC commands, makes zero UI changes, and characterizes itself
  as operator-level by design with the explicit reasoning that "a
  hybrid or foreign shape can only arise from external mutation (the
  binary itself never produces one because the migration is
  transactional)." The single-user desktop cohort has the
  affordances this requires — direct filesystem access to
  `state.db`, ability to install a standard `sqlite3` shell on every
  Tauri-v2-supported platform, and no fleet/coordination concerns —
  which makes the procedure feasible without inflating the supported
  surface. The "aggregate counts older than the manual drop are not
  recoverable" caveat is consistent with A3 and A4 (aggregate is
  downstream of `invocations` and has no UI consumer dependency on
  exact preserved counts).
- Why this preserves WS-1: the procedure is invoked **after**
  `StateDb::open` has already rejected the unexpected shape and
  returned an error. The binary's recovery posture is unchanged
  ("error out, don't guess"); the operator's external recovery is a
  separate, documented affordance.
- Net effect: reduces risk on supported surface (clarifies operator
  remediation without elevating `state.db` to a documented external
  mutation surface).

### Rollback residual is honestly recorded for the supported cohort

- Severity: low
- Surface concerned: pre-merge verification of the migration's
  transactional-rollback property on the supported surface.
- Net effect: reduces risk (replaces a falsely-advertised test with
  an honest residual plus a real test of an adjacent property).
- Claim from proposal: "the transactional-rollback property … is a
  property of the explicit `BEGIN`/`COMMIT` envelope in
  `ensure_providers_schema`. Verifying it through a runtime test
  requires injecting a failure inside the rebuild step … and the
  only viable injections … either require test-only product-source
  changes (which violates `~/ai/conventions/no-deferred-stubs.md`)
  or are out of scope for unit/particular-integration tests." The
  rollback property is "verified at implementation time by
  code-review of `ensure_providers_schema`'s explicit transaction
  wrapper, with the unexpected-shape rejection test … covering the
  early-exit DDL collision path."
- Evaluation: on this surface, the rollback property only matters
  in an extremely rare failure mode — a mid-rebuild SQLite failure
  after `RENAME` succeeds and `CREATE TABLE` runs but before the
  `INSERT … FROM invocations` completes. The proposal's explicit
  "no mutation of `invocations`" guarantee bounds the worst-case
  outcome: even if the rollback didn't restore the original
  `providers`, the source of truth for re-derivation is intact, and
  the new operator recovery procedure cleans up the residual. The
  decision to verify this via code review rather than by adding
  test-only failure-injection scaffolding to product source is
  appropriate because (i) it preserves the project convention
  against deferred stubs / test-only source, (ii) the
  `BEGIN`/`COMMIT` envelope is small enough that code-review
  verification is reliable, and (iii) the proposal explicitly
  records the gap so future rounds can revisit it without it
  silently disappearing. The replacement idempotency-across-reopens
  test (line 242) covers the genuinely-testable adjacent property
  (post-fix-shape → no-op branch), which is what the round-2 test
  pretended to cover but didn't.
- Net effect: reduces risk on supported surface (honest test track
  matched to honest documentation, with the residual tied to
  `WS-3`).

### Strengthened unexpected-shape rejection test ties recovery to behavior

- Severity: low
- Surface concerned: pre-merge verification of the migration error
  contract and the new recovery procedure.
- Net effect: reduces risk.
- Claim from proposal: the unexpected-shape rejection test now
  asserts that on the first open, `providers` and `invocations` are
  byte-identical to their pre-open state, and that after the
  operator-level cleanup (drop malformed `providers`), the second
  open completes and creates the post-fix table per the migration's
  "table is missing" branch.
- Evaluation: byte-identity pre/post is the strongest possible
  observable for "the failed open did not mutate state," which is
  exactly the property the migration error contract promises and
  which the supported surface needs in order to advertise
  deterministic retries. Asserting that the post-cleanup second
  open succeeds via the "table is missing" branch ties the new
  operator recovery procedure (proposal §Recovery procedure step 2)
  to test-observable behavior, so the recovery contract is no
  longer just documentation. Together with the idempotency-across-
  reopens test, the test track now covers the three reachable
  branches of `ensure_providers_schema` on the supported surface
  (pre-fix shape → migrate; post-fix shape → no-op; table missing
  → create post-fix), which is the right shape for this gate.
- Net effect: reduces risk on supported surface.

### Scope additions stay within the supported-surface boundary

- Severity: low
- Surface concerned: the boundary between in-scope/out-of-scope
  items.
- Net effect: neutral.
- Claim from proposal: the only additions vs. round 2 are the
  operator-level recovery paragraph, the rollback residual
  paragraph, the replacement idempotency test entry, and the
  strengthened unexpected-shape test entry.
- Evaluation: all additions are defensive specifications or test-
  intent refinements. Quota tables, `migrate-config`, pool
  grouping, IPC, UI rendering, and `migrate-db` behavior remain
  explicitly out of scope. The recovery procedure does not add a
  new product-side feature (no `agents recover` command, no UI
  prompt) — it documents existing operator affordances on the
  cohort. The proposal still rebuilds aggregate counts from
  `invocations` rather than introducing any new persistence
  mechanism.
- Net effect: neutral on supported surface.

## Assumption review

- A1 (provider_name is the stable supported provider account
  identity for routing history) — uphold. No revision shift; the
  recovery procedure does not contemplate any path where two
  accounts intentionally share a `provider_name`.
- A2 (provider_index is selection/observability metadata only) —
  uphold. RCA red harness still proves index-keyed identity drift;
  no index-keyed reader has been re-added (`WS-2` holds).
- A3 (`invocations` is sufficient to rebuild aggregate counts) —
  uphold, slightly strengthened. The recovery procedure's "no
  backup" branch explicitly relies on this (post-cleanup, the
  empty post-fix `providers` is repopulated by the supported
  normal-write path from new finalizations); the round-2
  guarantee that the migration never mutates `invocations` is
  unchanged and now reinforces the recovery posture as well.
- A4 (losing exact `providers.last_error` snippets during
  migration is acceptable) — uphold. The recovery procedure's
  acknowledgement that "aggregate counts older than the manual
  drop are not recoverable" extends the same posture to the rare
  recovery path: still no UI/IPC consumer of aggregate counts,
  so loss in this branch is tolerable.
- A5 (shape-based migration is the right local schema mechanism)
  — uphold, slightly strengthened. Round 3 makes the operator
  recovery procedure for the rare external-mutation case
  explicit, which closes the last open question about what
  happens "if the strict shape contract rejects the file." The
  shape-based posture is now load-bearing for both the binary
  side (reject anything but the two known shapes) and the
  operator side (cleanup procedure puts the file back into one
  of those two shapes).
- A6 (hidden direct reads of the `providers` SQLite table are
  unsupported) — uphold. The recovery procedure's reference to
  external `sqlite3` shell access is for operator-level
  remediation, not for a supported integration that reads
  `providers`. Problem map's enumeration of consumers is
  unchanged.

## Notes

- `R2-F01` and `R2-N01` were both raised by the shortcut gate.
  The supported-surface gate concurs with cross-gate closure:
  the operator-level recovery procedure is appropriate for this
  surface, and the rollback residual is a coherent posture for
  this cohort given the bounded "no mutation of `invocations`"
  guarantee.
- `WS-1`/`WS-2`/`WS-3` all hold for round 3. `WS-3` was
  authored specifically to protect against the residual being
  re-promoted to a runtime claim without a viable failure-
  injection mechanism; the round-3 residual paragraph names
  exactly which mechanisms were rejected and why, so the
  watch signal is satisfied at the strongest available level.
- No new supported-surface findings in round 3. The revision
  closes both round-2 cross-gate items (`R2-F01`, `R2-N01`)
  and reinforces the existing low findings without expanding
  the surface or weakening the migration error contract.
- The round-1 closures (`R1-F01`, `R1-N01`, `R1-N02`) remain
  closed: the migration error contract is unchanged and still
  explicit; `last_error_at` semantics are unchanged
  (`MAX(finished_at)` over `success = 0` rows);
  `examples/quota_check.rs` is still listed in the in-scope
  diff.
