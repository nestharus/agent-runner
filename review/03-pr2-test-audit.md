# Test-Audit Gate: PR 2 — `is_stale` empty-windows + `upsert_quota_refresh` reject-empty

## Overall verdict: PASS

The diff is a faithful implementation of `proposals/03-load-balancing-tiers.md`
§3: `is_stale` short-circuits to `true` when the provider row exists but
the window set is empty; `upsert_quota_refresh` no longer wipes prior
windows on `windows: []`, preserves `calls_since_refresh`, preserves
`refreshed_at` in the prior-present branch (the phase-7 CodeRabbit pass
4 finding #3 fix), and records `last_empty_refresh_at` as the audit
column; the `provider_quotas` schema adds the column both in the
new-DB `CREATE TABLE` and via an idempotent `ensure_provider_quotas_schema`
ALTER guard for existing DBs. All 8 named tests compile, run, and
pass, and each pins a behavior that would not hold against the
pre-change baseline. Coverage-delta is PARTIAL and acknowledged
implementation-mode: no test pins the `refreshed_at`-preservation
invariant that the phase-7 fix added, and no test exercises the
column-add ALTER path against a DB that already exists without the
column. Neither is a blocker; both are worth closing before PR 3
ships.

Note on commit SHAs: the prompt cites `80b1b17` for the test commit;
branch HEAD is `31aac6a`. Both SHAs exist in the object database
with identical commit messages, authors, timestamps, and trees —
`80b1b17` is dangling (no branch points to it), `31aac6a` is the
actual HEAD of `feat/03-pr2-empty-windows`. Audit was performed
against the current branch HEAD.

## Sub-audit 1 — Spec alignment

Verdict: PASS

Against `proposals/03-load-balancing-tiers.md` §3.2–§3.4,
`research/03-load-balancing-tiers-answers.md` §Q9/§Q11, and
`research/03-load-balancing-tiers-hookpoints.md` §2:

- **§3.2 `is_stale` forced-stale on empty windows.**
  `src-tauri/src/quota/mod.rs:140-143` inserts the empty-window
  guard exactly where the hookpoints doc §2.1 called for (between
  the existing `get_windows(...).unwrap_or_default()` and the
  `dynamic_ttl_secs` call). `dynamic_ttl_secs` is left unchanged at
  `quota/mod.rs:152-163`, consistent with hookpoints §2.2 ("this
  helper does not change"). The doc comment at
  `quota/mod.rs:129-132` now explicitly documents the
  inconsistent-state invariant.
- **§3.3 three-branch semantics in `upsert_quota_refresh`.** The
  empty-input branch short-circuits before the delta-learn / legacy
  mirror / window-DELETE / window-INSERT code, at
  `src-tauri/src/state/db.rs:1202-1247`:
  - **`windows.is_empty() && prior_windows.len() > 0`** (lines
    1232-1242): the `INSERT … ON CONFLICT … DO UPDATE` only writes
    `last_empty_refresh_at = ?2`. `refreshed_at` is *not* in the
    UPDATE clause, so the prior value is preserved. The INSERT
    clause on this branch would only fire if the `provider_quotas`
    row is absent despite prior windows existing — a degenerate
    state; in practice, when prior windows exist the row exists,
    so the UPDATE path runs and `refreshed_at` stays put. This is
    the phase-7 CodeRabbit pass 4 finding #3 fix: advancing
    `refreshed_at` would poison the PR 3 per-window delta learner
    by measuring the delta against the older sample while counting
    only the turns since the empty refresh. The inline comment
    block at lines 1203-1220 explains the invariant clearly.
  - **`windows.is_empty() && prior_windows.is_empty()`** (lines
    1221-1231): INSERT creates a new `provider_quotas` row with
    `refreshed_at = last_empty_refresh_at = now`; the
    ON-CONFLICT branch (for a row with no prior windows but an
    existing quota row — e.g., a never-refreshed provider whose
    row was created by `increment_calls_since_refresh`) bumps
    both timestamps. The forced-stale §3.2 guard will still fire
    on the next `is_stale` call because windows are still empty,
    satisfying the Q9 "zero-window path 2" self-heal.
  - **`!windows.is_empty()`** (lines 1249-1322): unchanged
    wholesale-replace path — delta-learn against longest window,
    upsert provider quota, `DELETE FROM provider_quota_windows`,
    re-insert the incoming set. This is the existing behavior
    required for "scripts can legitimately add/remove windows"
    per the proposal §3.3 last bullet.
- **§3.4 `last_empty_refresh_at TEXT NULL` schema.** New-DB
  declaration at `src-tauri/src/state/db.rs:358` inside the
  `CREATE TABLE IF NOT EXISTS provider_quotas` block. Existing-DB
  ALTER guard at `state/db.rs:566-579` (`ensure_provider_quotas_schema`),
  dispatched from `StateDb::open` at line 471 alongside the
  existing `ensure_session_turns_schema` pattern that
  hookpoints §2.4 pointed to as precedent. The guard uses
  `PRAGMA table_info(provider_quotas)` to detect absence, then a
  single `ALTER TABLE … ADD COLUMN last_empty_refresh_at TEXT`.
  Idempotent: re-opening a migrated DB is a no-op because the
  column is already present.
- **Q11 audit-trail semantic.** The new column is written only on
  the empty-input path; the non-empty path leaves it alone (the
  `UPDATE SET` clause on lines 1281-1287 of the non-empty branch
  omits it). That matches Q11's "diagnose empty scraper output
  from the DB" framing: a non-zero `last_empty_refresh_at` is
  unambiguous evidence of a prior empty refresh, and the delta
  between `refreshed_at` and `last_empty_refresh_at` tells you how
  long the empty-refresh state has persisted.
- **Q9 zero-window self-heal.** The combination of §3.2 guard +
  §3.3 empty-with-no-prior branch closes Q9 paths 3 and 4 (script
  returns `{"windows":[]}` → empty upsert; empty-vec upsert
  wipes) — both now produce a row that the next `is_stale` call
  will force-refresh rather than deferring for 24h.

No spec gaps observed.

## Sub-audit 2 — Test quality

Verdict: PASS

All 8 named tests from §3.5 are present, compile, and pass:

- `quota/mod.rs:327-333` `is_stale_forces_refresh_when_windows_empty`
- `quota/mod.rs:336-346` `is_stale_honors_ttl_when_windows_present`
- `quota/mod.rs:348-353` `is_stale_treats_missing_quota_row_as_stale`
- `state/db.rs:2474-2488` `upsert_quota_refresh_preserves_windows_on_empty_input`
- `state/db.rs:2490-2507` `upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced`
- `state/db.rs:2509-2529` `upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input`
- `state/db.rs:2531-2543` `upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row`
- `state/db.rs:2545-2563` `upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist`

Verified run:

```
quota::tests::is_stale_forces_refresh_when_windows_empty ... ok
quota::tests::is_stale_honors_ttl_when_windows_present ... ok
quota::tests::is_stale_treats_missing_quota_row_as_stale ... ok
state::db::tests::upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist ... ok
state::db::tests::upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row ... ok
state::db::tests::upsert_quota_refresh_preserves_windows_on_empty_input ... ok
state::db::tests::upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input ... ok
state::db::tests::upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced ... ok
```

Not trivially passing — each test pins a behavior that would
regress against the pre-change baseline (`main`):

- **Test 1** (`is_stale_forces_refresh_when_windows_empty`) — seeds
  a row with `refreshed_at = now()` via the
  `insert_quota_row_without_windows_for_test` helper
  (`state/db.rs:1344-1372`), which directly writes a
  `provider_quotas` row and a matching `DELETE FROM
  provider_quota_windows` to ensure the windows set is empty. On
  baseline: `dynamic_ttl_secs([])` = `MAX_TTL_SECS` (24h), age ≈ 0s,
  so `is_stale` returns false and the `assert!` would fail. Post-
  change: the new guard at `quota/mod.rs:141-143` returns true
  first. Pre/post behavior is meaningfully different.
- **Test 2** (`is_stale_honors_ttl_when_windows_present`) — writes
  one window 24h out via the real `upsert_quota_refresh` path and
  asserts `!is_stale`. 24h / DIVISOR (5) = 4.8h, clamped to
  [5min, 24h] = 4.8h. Age ≈ 0s, so the assertion holds. A
  regression guard that would fail if the new empty-windows guard
  were miswritten (e.g., `windows.len() <= 1` instead of
  `is_empty()`).
- **Test 3** (`is_stale_treats_missing_quota_row_as_stale`) — no
  seed; asserts `is_stale("p")` on an empty DB. Regression guard
  for the existing early-return at `quota/mod.rs:134-136`.
- **Test 4** (`upsert_quota_refresh_preserves_windows_on_empty_input`)
  — seeds two windows via the real path, captures
  `quota_window_rows(&db, provider)` as `before`, calls
  `upsert_quota_refresh(provider, &[])`, and asserts
  `quota_window_rows(&db, provider) == before`. The helper at
  `state/db.rs:1989-2000` returns `(window_id, used_percent,
  resets_at)` tuples sorted by `window_id` (since `get_windows`
  orders by primary key). On baseline, the empty upsert would
  `DELETE FROM provider_quota_windows WHERE provider_name = ?1`
  and insert nothing, so `after` would be empty and the `==`
  assertion would fail. Meaningfully pre/post different.
- **Test 5** (`upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced`)
  — seeds two windows, then writes one replacement with
  `used_percent = 0.30` / `resets_at = 2026-04-23T12:00:00Z`,
  asserting exactly one window with those values. Pins the
  preserved wholesale-replace behavior that the proposal
  explicitly keeps for the non-empty branch.
- **Test 6** (`upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input`)
  — seeds two windows, brackets the empty-upsert call with
  `before = Utc::now()` and `after = Utc::now()`, and asserts
  `before - 1s ≤ last_empty_refresh_at ≤ after + 1s`. The 1s
  tolerance on both ends is generous for the NTP jitter /
  Rust→SQLite RFC3339 roundtrip. On baseline, compile fails
  because the column doesn't exist; post-change, the assertion
  holds. Meaningfully pre/post different.
- **Test 7**
  (`upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row`)
  — single empty upsert on an empty DB, asserts
  `quota.refreshed_at.is_some()`,
  `last_empty_refresh_at(...).is_some()`, windows empty, and
  `is_stale(&db, "p")`. This test is load-bearing: it's the
  combined proof that §3.2 + §3.3's empty-with-no-prior branch
  interact correctly, and it answers the prompt's second coverage
  question ("does any test exercise the `is_stale` fix against an
  actual DB state produced by `upsert_quota_refresh(&[])`?")
  affirmatively. On baseline, the empty upsert would write the
  legacy mirror with `used_percent = 0`, `resets_at = NULL`,
  `refreshed_at = now` and the `last_empty_refresh_at` column
  wouldn't exist — so the test wouldn't compile in the first
  place, and the `is_stale` assertion would fail on the post-
  compile path anyway because `dynamic_ttl_secs([])` = 24h and age
  ≈ 0s.
- **Test 8**
  (`upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist`)
  — seeds two windows, calls `increment_calls_since_refresh` 5
  times, asserts `calls_since_refresh == 5`, calls the empty
  upsert, asserts `calls_since_refresh == 5` again. On baseline,
  the empty upsert would `UPDATE … SET calls_since_refresh = 0`
  as part of its legacy mirror (current
  `state/db.rs:1196-1217`), so the second assertion would read 0
  and fail. Meaningfully pre/post different.

Test seed shapes match the function signature. The
`quota_input` helper at `state/db.rs:1981-1987` builds
`QuotaWindowInput { used_percent, resets_at }` from two strings —
same shape as `QuotaWindowInput` (the actual arg to
`upsert_quota_refresh`). The `ts()` helper parses RFC3339 strings
into `DateTime<Utc>`, matching the field type. `quota_input(0.10,
"2026-04-22T00:00:00Z")` is a 2026 future timestamp relative to
today (2026-04-21), so the seeded windows don't accidentally
land in the past and distort TTL math.

The `insert_quota_row_without_windows_for_test` helper at
`state/db.rs:1344-1372` is `#[cfg(test) pub(crate)` and scoped
to the tests — it writes a minimal `provider_quotas` row and
then explicitly clears the windows table, which is the shape
that test 1 needs to exercise the `is_stale` empty-windows
branch without going through `upsert_quota_refresh`. That
separation is important: test 1 pins the `is_stale` branch
independently of the `upsert_quota_refresh` branch, so if the
upsert path ever stops creating the empty-prior row, test 1
still catches the `is_stale` regression.

All tests use `:memory:` DB via `test_db()` or `StateDb::open(":memory:")`,
so there's no cleanup state across tests and no shared filesystem
fixture to race.

## Sub-audit 3 — Coverage delta

Verdict: PARTIAL (implementation-mode PARTIAL is acknowledged)

Baseline: `main` has no empty-windows-specific unit coverage for
`is_stale` or `upsert_quota_refresh`; the closest existing piece
is `ttl_empty_windows_falls_back_to_max` at
`quota/mod.rs:388-391` which pins the *pure-helper*
`dynamic_ttl_secs([]) == MAX_TTL_SECS` behavior. That test is
correctly preserved — `dynamic_ttl_secs` intentionally still
returns `MAX_TTL_SECS` on empty input because the proposal put
the semantic fix in `is_stale`, not in `dynamic_ttl_secs`, so
the helper's contract is unchanged.

Branches covered (all 8 tests passing):

- `is_stale` zero-windows state → forced stale (test 1, with a
  non-upsert seed path).
- `is_stale` with one 24h window → not stale (test 2).
- `is_stale` missing provider row → stale (test 3).
- `upsert_quota_refresh` empty + prior windows → windows
  preserved byte-for-byte (test 4).
- `upsert_quota_refresh` non-empty replacement → wholesale delete
  + insert (test 5).
- `upsert_quota_refresh` empty input → `last_empty_refresh_at`
  populated within ±1s of wall clock (test 6).
- `upsert_quota_refresh` empty input + no prior → forced-stale
  quota row created (test 7; also the combined `is_stale`
  round-trip assertion against a real upsert-produced state).
- `upsert_quota_refresh` empty input + prior windows →
  `calls_since_refresh` preserved (test 8).

Branches *not* covered (gaps):

- **`refreshed_at`-preservation on empty-with-prior-windows.**
  This is the phase-7 CodeRabbit pass 4 finding #3 fix. The
  implementation at `state/db.rs:1232-1242` is correct: the
  `ON CONFLICT (provider_name) DO UPDATE SET last_empty_refresh_at
  = ?2` clause deliberately omits `refreshed_at`, so the prior
  value is preserved, and the inline comment at
  `state/db.rs:1205-1215` spells out *why* advancing it would
  poison the PR 3 burn-rate learner. But no test asserts this
  invariant directly. Any future refactor that mis-reads the
  comment and adds `refreshed_at = ?2` to the UPDATE set would
  compile, all 8 tests would continue to pass, and the silent
  regression would only surface as skewed delta values in PR 3.
  The test that would close this gap:

  ```rust
  let refreshed_before = db.get_quota(provider).unwrap().unwrap()
      .refreshed_at.unwrap();
  std::thread::sleep(std::time::Duration::from_millis(10));
  db.upsert_quota_refresh(provider, &[]).unwrap();
  let refreshed_after = db.get_quota(provider).unwrap().unwrap()
      .refreshed_at.unwrap();
  assert_eq!(refreshed_before, refreshed_after,
      "refreshed_at must not advance on empty input with prior windows");
  ```

  The 10ms sleep forces wall-clock divergence so a `SET refreshed_at
  = now` regression would produce two observably different values.
  Strongly recommended to add before PR 3 lands (PR 3 is where the
  poisoned-learner symptom would actually manifest, and locking the
  invariant in PR 2's test file makes the intent durable against
  future edits).

- **`ensure_provider_quotas_schema` ALTER path.** All 8 tests use
  `:memory:` / `test_db()` which call `StateDb::open` on a fresh
  file with an empty database. The fresh path hits the
  `CREATE TABLE IF NOT EXISTS` at line 358 (column is in the CREATE
  body), so the `ensure_provider_quotas_schema` guard at line
  566-579 takes its no-op branch every time — the actual `ALTER
  TABLE ADD COLUMN` is never exercised by tests. A regression that
  broke the ALTER branch (e.g., wrong column name in the
  predicate, wrong SQL syntax) would ship undetected. Closing the
  gap requires a test that (a) opens a temp-file DB, (b) drops the
  column or pre-creates the table without it, (c) re-opens the DB
  via `StateDb::open`, (d) asserts the column is present and the
  post-migration upsert paths still work. Out of scope for the
  named §3.5 test plan; worth a one-line follow-up test before
  PR 3 ships.

- **Non-empty input after an empty-only upsert on a fresh row.**
  Sequence `empty → non-empty` with no prior windows. The empty
  call creates the row via the empty-prior branch; the follow-up
  non-empty call takes the wholesale-replace path and must
  compute delta against `longest_prior = None`. The logic is
  covered incidentally by the existing balancer tests that start
  non-empty, but not by a dedicated state transition test.

- **`last_empty_refresh_at` cleared / untouched on subsequent
  successful refresh.** The non-empty branch's `UPDATE SET` at
  `state/db.rs:1281-1287` does not mention `last_empty_refresh_at`
  — so once set, it persists through a subsequent successful
  refresh. That's probably the right semantic (the audit column
  is the *most recent* empty-refresh observation, not a "last
  was empty" flag), but no test pins it either way, and the
  proposal §3.4 doesn't explicitly spec it.

Per the orchestrator rules, implementation-mode coverage-delta
PARTIAL is acknowledged and does not block PR opening. The first
gap (`refreshed_at`-preservation) is the most load-bearing and
should be closed before PR 3; the second (ALTER-path migration
test) is defensive but not production-critical since the change
is a single `ADD COLUMN` of a nullable TEXT with a widely-used
idiom in this repo.

## Blocking issues

None. No FAIL verdicts. The only PARTIAL is the acknowledged
implementation-mode coverage delta on the `refreshed_at`-preservation
invariant + the ALTER-path migration guard.

## Non-blocking observations

- **Add the `refreshed_at`-preservation test before PR 3 ships.** The
  phase-7 CodeRabbit finding #3 fix exists in the implementation
  and comment but not in a test assertion. A 5-line test (shown
  above) would pin it and prevent the PR 3 delta learner from
  being poisoned by a future empty-path refactor.
- **Add an existing-DB migration test.** Open a temp-file DB, open
  once, drop the column manually via raw SQL, re-open, assert
  the column is back. One test closes the coverage hole for the
  existing-DB ALTER branch that production will actually traverse
  when the migration first rolls out.
- **Test 1 uses `Utc::now()` for the seeded `refreshed_at`.** That
  is fine — the `is_stale` guard short-circuits before the TTL
  math — but if the guard were ever removed, the test would then
  depend on TTL behavior for an empty-windows row, which is
  semantically the wrong shape. Consider a deliberately-stale
  timestamp (e.g., `Utc::now() - Duration::hours(48)`) to make
  the test resilient to refactors, since the *forced*-stale
  invariant should hold regardless of age.
- **Test 6's ±1s tolerance is appropriate for wall-clock bracketing
  but loose for microsecond precision.** On a fast machine the
  bracket is usually narrower than 1ms; 1s gives room for
  SQLite RFC3339 second-truncation. No action needed — just
  documenting the rationale for future readers.
- **The `last_empty_refresh_at` column has no index.** Given that
  it's an audit-only column read ad hoc for diagnosis (not
  filtered on in a hot path), no index is correct. Worth
  restating so the PR 3 reviewer doesn't flag the missing index.
- **Commit SHA drift.** Prompt cites `80b1b17` for the test
  commit; branch HEAD is `31aac6a`. Both SHAs exist in the object
  database with identical trees and commit messages; `80b1b17`
  is dangling. Flagging only so the orchestrator's audit log
  isn't surprised.
