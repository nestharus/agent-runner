# Multi-Concern Check: PR 2

## Verdict: single-concern

PR 2 is one coherent unit of work: *close every path by which a
provider quota row can exist without window rows.* The two
production-code changes are belt-and-suspenders halves of a single
defect class, the new audit column exists only to serve the new
write-path branch, and the test/feat commit pair is a standard
red/green TDD split. Each of the three candidate seams fails the
independence test — splitting any of them produces either a
half-fixed bug, a dead-letter column, or a failing-CI intermediate
state.

The human gate already scoped this pairing explicitly:
`proposals/03-load-balancing-tiers.md:62` names the PR
"`is_stale` empty-windows fix + `upsert_quota_refresh`
reject-empty" as one deliverable, and
`proposals/03-load-balancing-tiers.md:478` places migration
`M_03_01_provider_quotas_last_empty_refresh_at` inside PR 2.

## Evaluation of each candidate concern

### 1. `is_stale` guard vs `upsert_quota_refresh` empty-write reject — **same concern**

The two edits attack the same bug class ("a provider row with zero
window rows is an inconsistent state that silently breaks scoring")
from opposite ends, and each one is individually insufficient.

- **`is_stale` guard alone** (`src-tauri/src/quota/mod.rs:140-142`):
  the wipe still happens whenever a refresh returns empty windows;
  `is_stale` then triggers another refresh on the next selection.
  That "self-heal on next tick" still exposes at least one scoring
  cycle to the zero-windows row, and the scoring math is where the
  symptom surfaces. The comment on line 132 even states the
  invariant it is enforcing ("A provider row with zero windows is
  inconsistent state") — an invariant that only holds because of
  the other half of this PR.
- **Empty-write reject alone** (`src-tauri/src/state/db.rs:1198-1249`):
  no *new* zero-windows rows are created, but any pre-existing row
  in that shape (legacy DB, earlier bug, manual tinkering) still
  feeds a fresh refresh timestamp into `is_stale`, which then
  returns `false` under the old TTL logic and lets the broken row
  sit in scoring for a full TTL window.

They close the same hole, and they are mutually load-bearing even
at the *test* level: the `is_stale_forces_refresh_when_windows_empty`
test can only construct the zero-windows state via the test-only
backdoor `insert_quota_row_without_windows_for_test`
(`src-tauri/src/state/db.rs:1343-1372`). That helper exists
*because* the production empty-write path added in this same PR no
longer produces that state. Split the PR, and the test either
lands before the backdoor it needs or the backdoor lands before
the test that motivates it — neither half makes sense alone.

Proposal support is explicit:
`proposals/03-load-balancing-tiers.md:81-82` specifies both
branches of the empty-write reject (prior windows present vs
absent) and `:100-102` specifies the three `is_stale` tests, all
inside the PR 2 scope block.

### 2. `last_empty_refresh_at` audit column vs the write path — **same concern**

The column is write-only in this PR. Its sole writer is the new
empty-input branch of `upsert_quota_refresh`
(`src-tauri/src/state/db.rs:1215-1246`); nothing in this diff
reads it. Could it land first as "add audit surface"? Technically
the schema-ensure helper (`src-tauri/src/state/db.rs:566-592`) is
idempotent and safe to land alone — but doing so would be
precisely the "speculative scaffolding with no consumer" shape
that `AGENTS.md` and the PR 1 review call out as worth splitting
*only* when a consumer is already in flight on a separate track.
Here the consumer *is* this PR.

The inverse split is worse: landing the write path first without
the column would either fail at INSERT time (the `last_empty_refresh_at`
binding in the empty-branch SQL has nowhere to write) or force a
throwaway fallback that is rewritten on the follow-up. Neither is
a real intermediate state anyone wants to merge.

The column is also not dead metadata after PR 2 — the proposal
scopes it as the audit surface that observes empty refreshes
across both CLI and Tauri sinks (`proposals/03-load-balancing-tiers.md:115`),
which is the reason it had to be a DB column rather than a log
line in the first place. It is load-bearing *for the write
branch's observability*, and removing that observability from
this PR would strip the only user-visible evidence that an empty
refresh occurred.

### 3. Test commit (`31aac6a`) vs feat commit (`273fce8`) — **same concern**

Same shape as PR 1 seam 3. The test commit encodes the five-test
contract (`is_stale_forces_refresh_when_windows_empty`,
`is_stale_honors_ttl_when_windows_present`,
`is_stale_treats_missing_quota_row_as_stale`,
`upsert_quota_refresh_preserves_windows_on_empty_input`,
`upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced`,
`upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input`,
`upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row`,
`upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist`)
alongside the test-only DB backdoor it needs. The feat commit is
the minimum implementation that turns that contract green.
Authored five minutes apart against a consistent slate. Merging
the test commit alone ships failing CI; merging the feat commit
alone ships untested behavior. Two commits inside one PR
preserves the test-first narrative for review without the
two-PR overhead.

## Cross-checks against the `AGENTS.md` split rules

- **"Large deletion is its own PR."** N/A. The only removal is
  the in-function move of `let longest_new = windows.iter().max_by_key(...)`
  and the `unchecked_transaction()` call site
  (`src-tauri/src/state/db.rs:1192-1275`) — that is an in-place
  reordering inside the same function to hoist the transaction
  ahead of the empty-input fast-path, not a standalone deletion.
- **"Additive changes go before behavioral changes."** Already
  satisfied *within* this PR: the schema-ensure helper and the
  `last_empty_refresh_at` column are additive and land as the
  substrate for the write-path change. Hoisting the additive
  piece into its own PR would only be useful if a *different*
  consumer existed; none does.
- **"Dependency order matters."**
  `proposals/03-load-balancing-tiers.md:472` records that PR 3
  (scoring redesign) depends on PR 2 so empty-window rows
  self-heal before scoring tests validate real pools. Splitting
  PR 2 into "is_stale only" and "reject-empty + audit" would
  fracture that dependency into two half-fixes and re-open the
  ordering decision against PR 3.

## Why the files/commits belong together

Two production files, one set of tests, one schema migration, one
audit column, two commits arranged test-then-feat. 293 insertions
across `src-tauri/src/quota/mod.rs` and `src-tauri/src/state/db.rs`.
Every piece is load-bearing for at least one other: the
`is_stale` guard enforces an invariant the empty-write reject
creates; the empty-write reject needs the new column to record
its audit timestamp; the test-only backdoor exists because
the prod write-path no longer produces the state under test; the
feat commit satisfies the contract the test commit encoded.
Splitting any seam produces a half-fixed bug, an empty
scaffold PR, or a failing-CI intermediate. Ship as one.
