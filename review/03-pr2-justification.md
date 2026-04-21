# Justification: PR 2

## Verdict: JUSTIFIED

Every hunk in `feat/03-pr2-empty-windows` (commits `31aac6a` test + `273fce8`
feat; computed against merge-base `90f433d` since main has since advanced
with unrelated docs commits on the initiative-03 artifacts and a
`.gitignore` tweak) maps directly to §3.2, §3.3, §3.4, or §3.5 of
`proposals/03-load-balancing-tiers.md`. Change surface is exactly
`src-tauri/src/quota/mod.rs` and `src-tauri/src/state/db.rs`, matching
§3.7's scope constraint ("quota/mod.rs, state/db.rs, and tests, with no
scoring redesign"). No unrelated defect fixes, no cross-cutting cleanups,
no code touching `balancer/mod.rs`, `main.rs`, `lib.rs`, TOML parser, or
anything reserved for PR 3.

## Hunks kept

### `src-tauri/src/quota/mod.rs` — §3.2 is_stale guard

- **Doc comment added at `quota/mod.rs:132`** ("A provider row with zero
  windows is inconsistent state; force stale.") — satisfies §3.2's
  "revise the doc comments that currently describe empty windows as a
  max-TTL first-fetch fallback".
- **Empty-windows guard at `quota/mod.rs:141-143`** — inserts
  `if windows.is_empty() { return true; }` immediately after
  `let windows = state.get_windows(provider_name).unwrap_or_default();`.
  This is §3.2 verbatim ("insert an empty-window guard in `is_stale`")
  and is the one-line semantic fix demanded by
  `research/03-load-balancing-tiers-needs.md:271-274`.
  `dynamic_ttl_secs` is left as a pure TTL helper for non-empty lists,
  matching §3.2's placement choice.
- **Three tests at `quota/mod.rs:326-354`** — name and behavior match
  §3.5 bullets 1-3:
  - `is_stale_forces_refresh_when_windows_empty` (§3.5 bullet 1) —
    uses the test-only seeder `insert_quota_row_without_windows_for_test`
    because `upsert_quota_refresh` no longer produces this shape via
    any public path.
  - `is_stale_honors_ttl_when_windows_present` (§3.5 bullet 2).
  - `is_stale_treats_missing_quota_row_as_stale` (§3.5 bullet 3).

### `src-tauri/src/state/db.rs` — §3.4 schema migration

- **`CREATE TABLE IF NOT EXISTS provider_quotas` gains
  `last_empty_refresh_at TEXT` at `state/db.rs:358`** — §3.4 explicitly
  calls for updating the CREATE TABLE declaration so new databases come
  up with the column in place.
- **`Self::ensure_provider_quotas_schema(&conn)?;` at `state/db.rs:470`**
  — invokes the idempotent migration helper inline with the existing
  `ensure_session_turns_schema` pattern (§3.4: "implement it in the
  same schema-ensure style").
- **`ensure_provider_quotas_schema` helper at `state/db.rs:566-578`** —
  performs `ALTER TABLE provider_quotas ADD COLUMN
  last_empty_refresh_at TEXT` when the column is absent. SQL matches
  §3.4's "Exact SQL" block (the `NULL` keyword is dropped, which is
  semantically identical in SQLite — columns without `NOT NULL` are
  nullable by default).
- **`provider_quotas_columns` helper at `state/db.rs:580-593`** —
  `PRAGMA table_info` reader used to make the ALTER idempotent. Mirrors
  the existing `session_turns_columns` helper and is the minimum
  plumbing needed for the §3.4 migration.

### `src-tauri/src/state/db.rs` — §3.3 upsert_quota_refresh empty-input branches

- **Hoisted `prior` / `prior_windows` loads above the transaction at
  `state/db.rs:1193-1194`** — required because the empty-input branch
  now needs to know `prior_windows.is_empty()` before issuing any
  write. Not a behavior change for the non-empty path; only restructuring.
- **Transaction opened earlier at `state/db.rs:1195-1198`** — tx
  scope now covers both the empty-input INSERT/UPDATE and the existing
  non-empty DELETE+INSERT block. Necessary because §3.3 requires the
  empty-input write to still be atomic.
- **Empty-input branch at `state/db.rs:1200-1245`** with two sub-cases:
  - `prior_windows.is_empty()` → INSERT-or-UPDATE both `refreshed_at`
    and `last_empty_refresh_at`, matching §3.3 bullet 3: "upsert a
    `provider_quotas` row with `refreshed_at` and
    `last_empty_refresh_at` so `is_stale` sees the provider row plus
    empty windows and forces another refresh".
  - prior windows exist → UPDATE only `last_empty_refresh_at` (NOT
    `refreshed_at`). This preserves prior `refreshed_at` for PR 3's
    per-window delta learner. The inline comment at
    `state/db.rs:1207-1228` documents this as the phase-7 refinement
    called out in the task prompt. See the Observations section below
    for why this is JUSTIFIED as a phase-7 refinement of §3.3 text.
  - Early `return Ok(());` after the empty-input commit at
    `state/db.rs:1243-1245` — required because the non-empty delta
    math and DELETE+INSERT block must not run on empty input. §3.3
    bullets 1-2 demand this branch-and-return shape explicitly.
- **Non-empty path at `state/db.rs:1248-1337`** — `longest_new`
  lookup, `(delta_percent, delta_calls)` match, INSERT-or-UPDATE of
  `provider_quotas`, DELETE of `provider_quota_windows`, and window
  INSERTs are byte-identical to pre-PR behavior except for the lines
  moved above the tx. §3.3 bullet 3: "retain the current wholesale
  replacement behavior".
- **`insert_quota_row_without_windows_for_test` at `state/db.rs:1342-1370`**
  — `#[cfg(test)]` test-only helper. After the §3.3 rewrite, no public
  code path produces the "quota row present, zero windows" shape, so
  `is_stale_forces_refresh_when_windows_empty` (§3.5 bullet 1) needs a
  direct seeder. Scoped behind `cfg(test)` and `pub(crate)` — never
  reachable in production.

### `src-tauri/src/state/db.rs` — §3.5 named test functions

- **Four test helpers at `state/db.rs:1984-2029`** (`quota_input`,
  `quota_window_rows`, `last_empty_refresh_at`, `calls_since_refresh`)
  — minimum fixtures needed to express the §3.5 assertions. None are
  used outside the five §3.5 tests.
- **Five tests at `state/db.rs:2474-2564`** — names and behaviors match
  §3.5 bullets 4-8:
  - `upsert_quota_refresh_preserves_windows_on_empty_input`
    (§3.5 bullet 4) — asserts `quota_window_rows` returned the same
    tuples before and after an empty-input call.
  - `upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced`
    (§3.5 bullet 5) — non-empty replacement still deletes old windows.
  - `upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input`
    (§3.5 bullet 6) — audit timestamp is written with a `[before, after]`
    sanity check.
  - `upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row`
    (§3.5 bullet 7) — ties §3.3 and §3.2 together: empty first refresh
    creates the quota row and `quota::is_stale` returns true on it.
  - `upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist`
    (§3.5 bullet 8) — five increments survive a subsequent empty refresh.

## Hunks that should move elsewhere

None.

## Non-blocking observations

- **Migration helper is named `ensure_provider_quotas_schema`, not
  `M_03_01_provider_quotas_last_empty_refresh_at`.** §3.4 suggests the
  numbered proposal-level identifier, but the PR matches the existing
  `ensure_session_turns_schema` convention already present in the file.
  Following the in-file convention reads more consistently with
  surrounding code; renaming later is cheap if the naming scheme is
  formalized in a cross-cutting doc pass.
- **§3.3 text still says "Update only `provider_quotas.refreshed_at`
  and `provider_quotas.last_empty_refresh_at`" when prior count > 0,
  but the PR preserves prior `refreshed_at` in that branch.** The task
  prompt explicitly names this as "the refreshed_at-preservation
  subtlety added in phase 7", so treating it as in-scope is correct.
  The code comment at `state/db.rs:1207-1228` documents the reasoning
  (PR 3's per-window delta learner computes `delta_calls =
  count_assistant_turns_since(prior.refreshed_at)`, so advancing
  `refreshed_at` on an empty refresh would undercount the turns window
  on the next successful refresh). `is_stale` still forces stale via
  the §3.2 guard regardless of `refreshed_at`, so user-visible
  freshness semantics are unchanged. Proposal §3.3 prose will benefit
  from a reconciliation pass to match the shipped code, but that's a
  docs fix, not a PR 2 scope issue.
- **No test directly pins the "prior `refreshed_at` preserved" invariant.**
  The five §3.5 tests cover windows preservation, non-empty wipe, audit
  timestamp, forced-stale shape, and calls counter, but the phase-7
  subtlety itself (that `provider_quotas.refreshed_at` does NOT advance
  when prior windows exist and empty input arrives) is load-bearing for
  PR 3 and would benefit from a dedicated assertion. This is a
  test-coverage observation for the companion test-audit report — not a
  justification concern, since the hunk itself maps to §3.3.
- **Transaction scope change.** The non-empty path now opens the tx
  sooner (before `longest_new`/`longest_prior`/delta computation). The
  pre-PR code opened it immediately before the first `tx.execute`. This
  is inert from a correctness standpoint — no SQL-state-dependent read
  sits between the new tx-open point and the first write — but it is
  worth naming so a reviewer doesn't mistake it for an unrelated
  refactor.
- **`last_empty_refresh_at` is read via a bespoke `query_row` in the
  test helper at `state/db.rs:2005-2020`** rather than through a new
  getter on `StateDb`. Appropriate for this PR: no production code
  needs to read the audit timestamp yet (PR 3's error surfacing will
  expose it to the CLI). Adding a getter prematurely would pull surface
  area into PR 2 that the proposal does not ask for.
