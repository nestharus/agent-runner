# Commit Hygiene Audit: PR 2

**Branch:** `feat/03-pr2-empty-windows`
**Base:** `main`
**Commits reviewed:** `31aac6a` (test), `273fce8` (feat)
**Mode:** audit-only (read-only; branch not modified)

**Note on the audit brief.** The prompt names the test commit as
`80b1b17`. The actual commit on the branch today is `31aac6a`
("test(pr2): is_stale empty-windows + upsert reject-empty
contract"). The subject matches, `31aac6a` is the only test commit
on the branch, and the feat commit named in the prompt
(`273fce8`) lands on top of it, so `31aac6a` is almost certainly
the commit the prompt meant. I audited `31aac6a` and flag the
hash discrepancy for the orchestrator.

## Verdict: CLEAN

The two-commit red/green TDD structure is appropriate for this PR
and is executed correctly: the test commit precedes the feat
commit, each commit is internally single-concern, the cumulative
diff tells a coherent story (tests assert the contract → code
delivers it), and no drop-then-restore or transient-regression
patterns are present. Per-commit verification confirms a genuine
red → green transition (5/8 new tests fail at `31aac6a`, 8/8 pass
at `273fce8`; no pre-existing tests regress at either commit).
The phase-7 amendment to the feat commit is coherent with the
test commit — the tests that were red stay red for the right
reason against the full amended behavior. The one weakness is
message quality: both commits are subject-only with no body
explaining WHY. This matches the PR 1 pattern and is a
non-blocking observation, not a REORGANIZE trigger — the subjects
themselves are specific and scoped.

## Per-commit evaluation

### `31aac6a` — test(pr2): is_stale empty-windows + upsert reject-empty contract

**Message evaluation**
- **Type/scope:** `test(pr2)` — type is correct. Scope `pr2` is a
  PR-number label rather than a module scope (guide suggests
  something like `quota` or `state/db`), but this is consistent
  with the initiative-03 convention used on sibling branches
  (`feat/03-pr1-*`, `feat/03-pr3-*`) and not a reorg trigger on
  its own.
- **Subject:** specific (names both surfaces under test —
  `is_stale` empty-windows + `upsert_quota_refresh` reject-empty —
  and their contract relationship). Under 72 chars. Minor style
  nit: elliptical noun phrase rather than strict imperative
  ("cover is_stale empty-windows and upsert reject-empty contract"
  would be more imperative).
- **Body:** *missing.* Does not explain why both the `is_stale`
  forcing-stale assertion and the `upsert_quota_refresh`
  empty-input contract are encoded in one commit (answer: they
  are two halves of the same §5.1 empty-windows invariant — if
  upsert accepted empty writes, is_stale alone could not self-heal
  the resulting row), why `last_empty_refresh_at` is an observable
  audit column rather than internal state, or why
  `calls_since_refresh` is asserted to survive an empty refresh
  (answer: it is the denominator PR 3's delta learner uses and
  must measure the full inter-refresh window, not be reset by a
  no-op refresh). The guide calls for "what and why, not how" in
  the body — this commit has neither.

**Scope**
Single-concern. Adds:
- `src-tauri/src/quota/mod.rs` (+29 lines in `mod tests`):
  3 tests covering `is_stale` — the empty-windows forcing case,
  the TTL-honored-with-windows regression guard, and the
  missing-row regression guard.
- `src-tauri/src/state/db.rs` (+173 lines): one test-only helper
  (`insert_quota_row_without_windows_for_test`) + 3 test-file
  helpers (`quota_input`, `quota_window_rows`,
  `last_empty_refresh_at`, `calls_since_refresh`) + 5 tests
  covering `upsert_quota_refresh` empty-input semantics
  (preserve windows, don't-regress non-empty replacement,
  stamp `last_empty_refresh_at`, create forced-stale row on
  first-ever empty refresh, preserve `calls_since_refresh`).

All changes are scaffolding for one invariant (the §5.1
empty-windows contract). No unrelated hitchhikers. The
`insert_quota_row_without_windows_for_test` helper is gated
behind `#[cfg(test)]` so it has no non-test footprint — correct
scoping.

**Red-state verification** (verified via worktree at `31aac6a`)
- `cargo build --tests` → **OK** (per-commit verifiability: test
  commit compiles cleanly).
- `cargo test --lib` → exit **1**, **214 passed; 5 failed**.
- The 5 failing tests are exactly the 5 new PR2 tests that
  encode behavior the feat commit delivers:
  1. `quota::tests::is_stale_forces_refresh_when_windows_empty` —
     assertion fail (the empty-windows guard is absent in `is_stale`).
  2. `state::db::tests::upsert_quota_refresh_preserves_windows_on_empty_input` —
     assertion fail (`get_windows` returns `[]` because upsert
     ran the DELETE unconditionally).
  3. `state::db::tests::upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input` —
     `SqlInputError: no such column: last_empty_refresh_at`
     (the column doesn't exist yet).
  4. `state::db::tests::upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row` —
     same `no such column` SQL error on the
     `last_empty_refresh_at` probe.
  5. `state::db::tests::upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist` —
     assertion `left: 0, right: 5` (upsert unconditionally
     zeroes `calls_since_refresh`).
- The 3 PR2 tests that pass (`is_stale_honors_ttl_when_windows_present`,
  `is_stale_treats_missing_quota_row_as_stale`,
  `upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced`)
  are **regression guards** — they encode existing correct behavior
  that PR 2 must not break. Including them in the test commit is
  correct: green-at-test-commit, green-at-feat-commit is the right
  shape for a regression guard, and omitting them would leave the
  replacement path and missing-row path unprotected during the
  refactor.
- This is a **clean red**: every failure maps to a specific piece
  of behavior the feat commit delivers; no flakes, no
  fail-for-other-reasons. Pre-existing tests are unaffected
  (214/214 pre-existing pass).
- Size: 202 insertions, 0 deletions. Appropriate for the scope.

### `273fce8` — feat(pr2): is_stale self-heals empty windows, reject empty-write wipe

**Message evaluation**
- **Type/scope:** `feat(pr2)` — type is correct. Same scope nit
  as above.
- **Subject:** specific (names both behavior changes — `is_stale`
  self-heals + empty-write rejection — and pairs them). Under 72
  chars. Minor style nit: "self-heals" and "reject" are an
  indicative/imperative mix; strict imperative would be
  "make is_stale self-heal empty windows; reject empty-write
  wipe".
- **Body:** *missing* at the message level. However, an
  unusually thorough 20-line *code comment* inside
  `upsert_quota_refresh` (lines 1196–1215) explains the
  `refreshed_at` preservation rationale in detail (tie to the
  PR 3 delta-learner formula, why advancing `refreshed_at` on an
  empty refresh would inflate burn rate, why
  `last_empty_refresh_at` is separate). That rationale belongs
  in **both** places: the code comment is appropriate because
  it protects the invariant at the line where it could
  accidentally be violated; a matching commit-message body is
  still desirable so a maintainer bisecting does not have to
  open the file. Partial credit; not a reorg trigger.

**Scope**
Single-concern: ships the feature tested in `31aac6a`. Contents:
- `src-tauri/src/quota/mod.rs` (+4/-0): 3-line empty-windows
  guard in `is_stale` + 1 doc-comment line.
- `src-tauri/src/state/db.rs` (+87/-7):
  - Schema: new `last_empty_refresh_at TEXT` column in the
    `provider_quotas` `CREATE TABLE`.
  - Migration: `ensure_provider_quotas_schema` +
    `provider_quotas_columns` helpers, called from `open()` — an
    `ALTER TABLE ... ADD COLUMN` for existing DBs missing the
    new column. Mirrors the existing `ensure_session_turns_schema`
    pattern in the same file (not a new abstraction, extension
    of an established one).
  - `upsert_quota_refresh` rewrite: transaction is now opened
    earlier; new early-return branch for `windows.is_empty()`
    with two sub-branches (prior-windows-exist vs
    no-prior-windows) that control what gets written to the
    `provider_quotas` row while leaving `provider_quota_windows`
    untouched.

The schema column + migration + empty-write rewrite are **one
concern** — the column exists exclusively to support the empty-
refresh audit path. Splitting them would leave either the
migration landing before any code reads the column (unused
state) or the code landing before the column (broken SQL). The
bundling is correct.

The `ensure_provider_quotas_schema` helper is separable in
principle (it is a generic migration shape), but extracting it
into a preceding scaffolding commit would create a commit whose
sole behavior change is "add a column and a migration for a
column nothing reads yet" — a scaffolding commit with no
observable behavior. The guide's single-concern rule targets
*mixing concerns*, not *separating mechanism from use*; keeping
the migration with its sole consumer is the correct call.

**Green-state verification** (verified via worktree at `273fce8`)
- `cargo build --tests` → **OK**.
- `cargo test --lib` → exit **0**, **219 passed; 0 failed**.
- All 5 previously-failing PR2 tests now pass; all 3 PR2
  regression guards continue to pass; no pre-existing tests
  regress (214 → 214 + 5 = 219).
- This is a **clean green**: the feat commit flips every
  assertion the test commit encoded, with no tests left red and
  no tests skipped.
- Size: 91 insertions, 7 deletions across 2 files. Small and
  focused given the migration + transaction-rewrite scope.

## Cross-commit checks

- **Ordering:** test commit precedes feat commit
  (`git log main..feat/03-pr2-empty-windows`:
  `273fce8` → `31aac6a`). Correct for red/green TDD.
- **Drop-then-restore:** none. Both commits are net-additive
  against main; no hunk in `31aac6a` is undone or rewritten in
  `273fce8`.
- **Transient regressions:** none. Main is green pre-branch
  (inferred from the 214 pre-existing tests passing at `31aac6a`);
  test commit is red only on the 5 new tests it introduces;
  feat commit is fully green. At no point does a pre-existing
  test break.
- **Cumulative diff:** 2 files, +293/-7 lines. Matches the PR
  description (quota/is_stale empty-windows guard + state/db
  upsert rewrite + schema migration + tests and helpers).

## Phase-7 amendment check

The feat commit shows `AuthorDate Tue Apr 21 07:24:54` vs
`CommitDate Tue Apr 21 09:42:07`, confirming it was amended
after initial authoring (the phase-7 amendment cited in the
prompt, adding the `refreshed_at` preservation branch).

The amendment split the empty-input write path into two
sub-branches (prior-windows-exist vs no-prior-windows) with the
key behavioral distinction: **when prior windows exist,
`refreshed_at` is NOT advanced** (only `last_empty_refresh_at`
is). This is motivated by the PR 3 delta-learner formula quoted
in the 20-line code comment.

**Does `31aac6a` still fail for the right reasons against the
amended behavior?** Yes. The amendment only added a new code
branch and preserved-field rule; it did not weaken or redefine
any assertion the test commit encoded. The 5 red failures at
`31aac6a` map to:
- 1 missing `is_stale` guard → unaffected by the amendment.
- 1 missing window-preservation → unaffected.
- 2 missing `last_empty_refresh_at` column → unaffected.
- 1 missing `calls_since_refresh` preservation → unaffected.

The amendment did, however, introduce a **behavior branch that
is not directly asserted by any test** — specifically, the
preservation of the `refreshed_at` column value when prior
windows exist during an empty refresh. The test
`upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist`
is adjacent (it asserts `calls_since_refresh` preservation on
the same path) but does not probe `refreshed_at`. This is a
**test-audit concern, not a commit-hygiene concern** — the
commit structure itself (test-first, single-concern, red→green)
is not compromised; what is compromised is test coverage of a
post-authoring refinement. Flag for the test-audit deliverable;
not a REORGANIZE trigger here.

## If REORGANIZE: what to change

Not triggered. If the author wants to volunteer a cleanup pass
anyway (not required to pass this gate), the only material
improvement would be adding message bodies:

```
test(pr2): is_stale empty-windows + upsert reject-empty contract

Encode the §5.1 empty-windows invariant ahead of the
implementation in 273fce8. Two surfaces are under contract:
(a) is_stale returns true when a quota row has zero window
rows — the row is inconsistent state and must self-heal on
the next refresh; (b) upsert_quota_refresh with an empty
input slice must NOT wipe prior windows or reset
calls_since_refresh — it records the empty observation in
last_empty_refresh_at only. Three regression guards cover
the paths the rewrite must not break (TTL-honored with
windows, missing-row stale, non-empty input replaces cleanly).
```

```
feat(pr2): is_stale self-heals empty windows, reject empty-write wipe

is_stale now forces refresh on any quota row with zero
window rows. upsert_quota_refresh branches on empty vs
non-empty input: empty input preserves prior windows and
calls_since_refresh, stamps last_empty_refresh_at, and
preserves the prior refreshed_at when windows exist (so the
PR 3 delta learner's inter-refresh interval remains the
real observation-to-observation span). New schema column
last_empty_refresh_at is added with a forward-compatible
ALTER TABLE migration following the existing
ensure_session_turns_schema pattern.
```

Rewriting these messages would be a pure message-only
reorganize (no hunk re-staging), so `git rebase -i main` with
`reword` on both commits is sufficient. Not required.

## Non-blocking observations

1. **Scope label `pr2` vs module scope.** The guide suggests
   module scope (`quota`, `state/db`) over PR-number scope.
   Sibling branches (`feat/03-pr1-*`, `feat/03-pr3-*`) use the
   same `pr1`/`pr2`/`pr3` convention, so this is initiative-wide
   stylistic consistency, not a per-PR defect. Flag for the
   initiative, not this PR. Same observation as PR 1.

2. **Subject mood.** Both subjects mix indicative
   (`self-heals`) with imperative (`reject`). Minor; does not
   obstruct review.

3. **Message body absence.** As with PR 1, the strongest guide
   signal for REORGANIZE on messages is vagueness (`wip`, `fix`,
   `address feedback`). These subjects are specific, so this
   does not cross the threshold — but the "why" content is
   genuinely absent at the message level. Mitigated on the feat
   commit by the unusually detailed in-code comment (20 lines)
   that survives in the file rather than only in the commit
   message; this is actually *better* for the invariant's
   long-term protection than a commit-message body alone, because
   a maintainer editing `upsert_quota_refresh` will see the
   rationale in the file.

4. **Test-audit gap (cross-reference, not a hygiene defect).**
   The phase-7 amendment added `refreshed_at` preservation logic
   that is not directly asserted by a test — the nearest test
   (`upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist`)
   covers an adjacent column. The commit-hygiene gate passes
   because this is a coverage question, not a structure
   question. Worth surfacing in `03-pr2-test-audit.md`.

5. **Migration safety.** The `ensure_provider_quotas_schema` +
   `provider_quotas_columns` pair matches the existing
   `ensure_session_turns_schema` shape, so the migration story
   is consistent with precedent. This is a quiet strength of
   the feat commit worth preserving in any future message
   rewrite. (Observation, not a hygiene concern.)

6. **Per-commit full-suite verification was performed** (214
   pass + 5 fail at test commit; 219 pass at feat commit). This
   goes beyond the PR-scoped spot check run for PR 1 — at no
   point does a pre-existing test break on either commit, which
   is the strongest signal of per-commit verifiability the
   hygiene gate can produce.
