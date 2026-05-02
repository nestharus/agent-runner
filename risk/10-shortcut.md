# Shortcut Risk Assessment (round 3): proposals/10-routing-claude-skipped.md

## Verdict: LOW

The round-2 medium (`R2-F01`) is closed: the inadequate
"Migration rollback on failed rebuild" test that fired only on the
initial `RENAME` step is gone; the replacement test
("`ensure_providers_schema` is idempotent across reopens") tests a
real, separable branch ("post-fix shape → no-op") that an unwrapped
or shape-blind migration could fail; and the rollback property is
recorded as an explicit residual verified by code review of the
`BEGIN`/`COMMIT` envelope at implementation time. The residual is
honest in two ways: it names the four failure-injection mechanisms
considered (test-only `CHECK`, mid-transaction temp triggers,
sibling-connection lock contention, OS-level fault injection) and
gives a reason each was rejected, and it makes the transaction
wrapper an explicit code-review obligation rather than a runtime test
claim. The round-2 low note (`R2-N01`) is closed by a complete
operator-level recovery procedure (backup restore; or manual drop of
the malformed table followed by post-fix-shape recreation on next
open) that names every step and is honest about what cannot be
recovered (aggregate counts older than the drop, since `providers` is
downstream of `invocations`). No new shortcuts are introduced by the
round-3 revision: the idempotency test entry is honestly scoped and
explicitly disclaims the rollback property in its own residual cell,
the unexpected-shape test was strengthened (byte-identity assertion +
post-cleanup recovery branch) without overclaiming, and the watch
signals all hold.

## Prior finding status

- `R2-F01` (medium → closed): closed
  - Closure evidence:
    - The "Migration rollback on failed rebuild" test entry is
      removed from the test-intent table — there is no longer a
      test that names rollback as the property under test, so the
      false-confidence vector named in round 2 is gone.
    - Its replacement,
      "Migration `ensure_providers_schema` is idempotent across
      reopens" (`proposals/10-routing-claude-skipped.md:242`), tests
      a property an unwrapped or shape-blind migration genuinely
      could fail: the helper inspects `PRAGMA table_info(providers)`
      and must take the early-return branch when `provider_name`
      exists; an implementation that did not check shape and re-ran
      the rebuild against `invocations` would either re-trigger
      `RENAME TO providers_legacy_index_keyed` (now failing because
      the legacy table no longer exists, but with arbitrary state
      corruption depending on commit order) or perturb the aggregate
      row counts. The test's own residual cell explicitly says
      "Does not directly exercise transactional rollback;
      complements the unexpected-shape test by proving the
      'post-fix shape → no-op' branch" — it does not lay false
      claim to rollback coverage.
    - The new "Residual: rollback during mid-rebuild failure is
      verified by code review, not runtime test" paragraph
      (`proposals/10-routing-claude-skipped.md:108–110`) is honest
      on three axes: (a) it states the property explicitly
      ("if any step fails, the transaction rolls back, leaving the
      original `providers` table intact"); (b) it names the four
      injection mechanisms considered and gives a reason each is
      rejected; (c) it makes the `BEGIN`/`COMMIT` envelope an
      explicit code-review obligation, which preempts the round-2
      "an implementation could quietly drop the wrapper" risk by
      moving the verification surface from "the test catches it" to
      "the reviewer catches it" and recording why a runtime test
      cannot.
- `R2-N01` (low note → closed): closed
  - Closure evidence: the new
    "Recovery procedure on unexpected `providers` shape" paragraph
    (`proposals/10-routing-claude-skipped.md:99–106`) names two
    complete operator paths with no hand-waved steps:
    (1) restore from file-system backup of `state.db`, then re-run
    `agents` (which exercises the migration on a known-good pre-fix
    shape), and (2) absent a backup, manually drop the malformed
    `providers` table (and any leftover
    `providers_legacy_index_keyed`) via an external `sqlite3`
    shell, after which the next `StateDb::open` takes the
    "table is missing → create post-fix shape" branch and the
    post-fix table is repopulated by the supported normal-write
    path on subsequent invocations. The procedure is also honest
    about its limitation — aggregate counts older than the manual
    drop are not recoverable because `providers` is downstream of
    `invocations` — and it correctly characterizes the event as
    operator-level by design (a hybrid/foreign shape can only arise
    from external mutation, since the binary's migration is
    transactional). The unexpected-shape test entry's fixture
    (`proposals/10-routing-claude-skipped.md:241`) now also exercises
    the post-cleanup recovery path ("the same DB is then mutated to
    drop the malformed `providers` and reopened to confirm
    recovery"), so the documented procedure is bound to an
    observable test signal.
- `R1-N01` (round-1 closed): re-verified
  - The `last_error_at` failure-row semantic prose at
    `proposals/10-routing-claude-skipped.md:85` and the dedicated
    test entry "Migration `last_error_at` reflects most recent
    failed invocation" (`proposals/10-routing-claude-skipped.md:243`)
    are unchanged. The round-3 revision did not perturb either.

## Watch signal status

- `WS-1` (helper remains transactional and rejects unexpected
  shapes): upheld. The transactional envelope is still asserted at
  `proposals/10-routing-claude-skipped.md:95` ("The rename + create
  + rebuild + drop sequence runs inside one SQLite transaction. If
  any step fails, the transaction rolls back, leaving the original
  `providers` table intact"), and the migration error contract still
  rejects every shape that is not the exact pre-fix or post-fix
  layout (`proposals/10-routing-claude-skipped.md:93`). The round-3
  revision moves the rollback property from a runtime test claim to
  a code-review obligation but does not soften either guarantee.
- `WS-2` (no index-keyed reader alias kept on the routing-history
  surface): upheld. The Out-of-scope list at
  `proposals/10-routing-claude-skipped.md:181` still says
  "No backwards-compatibility shim or index-keyed fallback reader
  kept after migration." `get_provider`'s signature remains
  `(model_name, provider_name)` (lines 132–137) and
  `recent_error_count` is re-keyed to `provider_name` (lines
  142–157). No alias surface is reintroduced.
- `WS-3` (rollback residual not silently re-promoted to a runtime
  test claim): upheld. The residual is now stated explicitly inside
  the proposal at `proposals/10-routing-claude-skipped.md:108–110`
  and the test entry that took its place
  (`proposals/10-routing-claude-skipped.md:242`) explicitly
  disclaims rollback coverage in its own residual cell. There is no
  test in the round-3 table whose name or assertion claims
  transactional-rollback verification.

## Findings (round 3)

None.

## Notes

- The residual paragraph cites
  `~/ai/conventions/no-deferred-stubs.md` as the reason for
  rejecting test-only `CHECK` constraints and mid-transaction temp
  triggers, because those would require test-only
  product-source changes. The cited convention is primarily about
  deferred/placeholder code rather than test-conditional product
  code, so the citation is partly by extension; however, the
  underlying judgement (that adding test-only branching to
  `ensure_providers_schema` would pollute the migration helper for
  no proportionate gain) stands on its own and the precise
  convention name is not load-bearing for the residual's honesty.
  Not a finding.
- The idempotency test entry deliberately uses on-disk-or-in-memory
  fixture language ("In-memory or on-disk `StateDb` opened once on a
  pre-fix shape … and then reopened on the same DB file"). On-disk
  is the correct mode here because reopening an in-memory
  `StateDb` typically gets a fresh DB unless the connection is
  shared; this is an implementation detail for the test author and
  does not undermine the test entry. Not a finding.
- The unexpected-shape test now asserts byte-identity of `providers`
  and `invocations` pre/post (round-3 strengthening) and exercises
  the post-cleanup recovery path. Both additions are honest
  expansions of an existing test, not new shortcut surface.
- No regressions in `R1-F01`, `R1-N01`, `R1-N02` were observed; the
  round-3 revision is scoped to the rollback-test replacement, the
  recovery-procedure paragraph, the unexpected-shape test
  strengthening, and the residual paragraph.
