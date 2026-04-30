# Initiative 04 — Phase 4 Audit Risk Report (Rev 2)

**Verdict: LOW**

Rev 1 returned MEDIUM on three concerns: REPL stderr capture
undesigned (§4), semantic corollary of the capture gap undiscussed
(§4), and heuristic coverage unverified (§6). Rev 2 addresses all
three via explicit scope decisions in answers §D6, §D7, §D8 and
matching updates to proposal §5 and §11. The underlying technical
claims (transactional clear, DROP COLUMN support, filter ordering,
never-fail guarantee, per-account keying, migration safety) were
VERIFIED in rev 1 and remain correct in rev 2.

## Concern-by-concern findings

### 1. Clear-on-refresh transactional ordering — VERIFIED

Proposal §6 claim is correct. `upsert_quota_refresh` opens the
transaction once at `src-tauri/src/state/db.rs:1298-1301`, BEFORE the
empty/non-empty branch. The non-empty branch runs the provider-quota
upsert (`src-tauri/src/state/db.rs:1370-1381`), the window delete
(`src-tauri/src/state/db.rs:1383-1387`), and the window inserts, then
commits as a single unit at `src-tauri/src/state/db.rs:1439`. Adding
`UPDATE provider_quotas SET exhausted_at = NULL WHERE
provider_name = ?1` anywhere inside that non-empty branch is
same-transaction — a concurrent reader on a default SQLite journal
mode will see either the pre-refresh state or the post-refresh state,
never a mix.

Minor shape note (non-blocking): the cleaner implementation is to
extend the existing `INSERT ... ON CONFLICT DO UPDATE SET` at
`src-tauri/src/state/db.rs:1370-1381` to also set
`exhausted_at = NULL`, rather than a separate UPDATE. Both are
correct; one fewer SQL roundtrip. Phase-4 implementer's choice.

### 2. SQLite DROP COLUMN claim — VERIFIED

The claim that bundled SQLite 3.51.1 supports `ALTER TABLE DROP
COLUMN` is already proven by the repo itself: `ensure_provider_quotas_schema`
already executes `ALTER TABLE provider_quotas DROP COLUMN
last_delta_percent` and `last_delta_calls` at
`src-tauri/src/state/db.rs:623-635`. `Cargo.toml` pins
`rusqlite` with the `bundled` feature, and initiative 03 shipped
this pattern.

SQLite DROP COLUMN has documented restrictions (PRIMARY KEY, UNIQUE,
indexed, CHECK, FK, or generated-column references fail). Verified
`quota_tight_routing` appears in NONE of the four invocation indexes
(`idx_invocations_uuid`, `idx_invocations_parent`,
`idx_invocations_provider_created`, `idx_invocations_provider_session`),
NONE of the CHECK constraints (`status` only), and no FK or PK
reference. DROP COLUMN will succeed.

### 3. Filter-after-refresh ordering — VERIFIED INTENTIONAL

Not a race condition. The refresh loop at
`src-tauri/src/balancer/mod.rs:97-108` commits before the quota read
at `src-tauri/src/balancer/mod.rs:111-121`. If `refresh_provider`
lands non-empty windows during this call, the same transaction
clears `exhausted_at`; the subsequent `get_quota` read sees the
cleared flag and the filter treats the provider as eligible. This
matches answers §D5 exactly: "the refresh is exactly the authorized
clear signal." Pinned by the proposal test
`exhausted_filter_does_not_prevent_refresh_loop_from_clearing`.

Concurrent-writer edge case (noted, not a flag): if process A is
failing and races process B's successful refresh, and B's clear
commits just before A's `mark_exhausted`, A will re-set the flag.
This is correct single-call self-correction — the next call avoids
the account.

### 4. REPL stderr-capture gap — RESOLVED BY SCOPE DECISION (D6)

Rev 1 flagged MEDIUM for implementation risk on the undesigned
capture path AND for the undiscussed semantic corollary (one-extra-
failure-before-flag).

Code confirms the constraint: `execute_interactive` calls
`cmd.stderr(Stdio::inherit())` at `src-tauri/src/executor/cli.rs:387`
and returns only `i32` at line 403. No plumbing exists today to
surface child stderr to `run_repl` for heuristic classification.

Rev 2 answers §D6 and proposal §5/§11 now explicitly:

- Document that REPL stderr capture is **NOT implemented in this PR**.
- Enumerate the three alternative designs considered (tee via
  `os_pipe`, ringbuffer, ptty wrap) and their tradeoffs.
- Commit to option B: accept that REPL quota-exit does NOT set the
  flag.
- Analyze the consequence: one guaranteed extra quota-failed
  invocation after a REPL quota-exit before the flag is set.
- Reconcile with the no-spam invariant: the invariant is about
  post-classification stickiness (flag sticky until refresh clears),
  not about signal acquisition (the first classification event
  itself). One pre-classification failure is not spam.
- Flag REPL stderr capture as explicit future work if the one-extra-
  failure becomes painful.

This is a coherent scope decision backed by written rationale and is
consistent with the user-locked "don't spam reactive" framing.
Implementation risk is removed from this PR by removing the feature
from this PR's scope.

**Residual minor observation (not blocking):** D6's "one guaranteed
extra failure" framing is accurate for mixed REPL-then-CLI usage
patterns. For a user who stays in REPL-only and never invokes
`run_with_balancing`, the exhausted account is never classified, so
the flag is never set, so the provider remains eligible in repeated
REPL calls. The refresh loop cannot rescue this path because there
is nothing to clear. This is a user-initiated retry loop, not
balancer-initiated spam, and fits D6's documented scope — the user
sees the quota stderr in their terminal and naturally stops. Not a
flag.

### 5. "Never fail to return a provider" guarantee — VERIFIED

With §3.3 deleting `BalanceError` entirely and §7 ensuring the
filter's empty-set case falls through to the unfiltered list, the
reverted `select_provider(model, state, ctx) -> usize` signature has
no error path by type. `score_by_invocation_count` already never
errors (`src-tauri/src/balancer/mod.rs:422-453`), and `round_robin_fallback`
always returns a valid index (`src-tauri/src/balancer/mod.rs:455-474`).
Proposal test `all_providers_exhausted_falls_through_to_round_robin`
pins this. After the reverts, no caller needs to catch anything.

### 6. Heuristic classification scope — RESOLVED BY SCOPE DECISION (D7)

Rev 1 flagged MEDIUM for unverified coverage of the three-substring
heuristic (`"quota"`, `"billing"`, `"usage limit"` lowercase-matched)
against real CLI stderr.

Code confirms the heuristic at
`src-tauri/src/diagnostics/mod.rs:110-112`. Proposal §5 extracts this
behavior unchanged into `classify_exhaustion`.

Rev 2 answers §D7 now explicitly:

- Commits to keeping the existing heuristic unchanged.
- Acknowledges that broadening the match terms would affect shared
  one-shot diagnostics code — out of scope for this PR.
- Documents graceful degradation: a missed classification leaves the
  flag unset; the refresh TTL still ticks; the next actual failure
  retries classification.
- Flags broadening as explicit future work against
  `src-tauri/src/diagnostics/mod.rs` in a separate PR that benefits
  both paths.
- Notes phase 6 MAY grep test fixtures to validate the heuristic
  against expected quota text; not a gate.

Known false-negative risk remains (e.g., `rate_limit_exceeded` →
classified as `RateLimit` not `QuotaExhausted`;
`plan_limit_reached` / `monthly_limit` → `Unknown`), but the scope
decision to defer broadening is coherent and the degradation path is
documented.

### 7. Per-account vs per-(model, provider_index) keying — VERIFIED CONSISTENT

All exhausted plumbing keys on `provider_name`:

- `provider_quotas.exhausted_at` — table PK'd by `provider_name`
  (`src-tauri/src/state/db.rs:389-396`).
- `mark_exhausted(provider_name: &str)` — proposal §5.
- `upsert_quota_refresh` clear — keys by `provider_name`.
- Filter in `select_provider` reads `quotas[i]` where `quotas` is
  populated by `state.get_quota(&model.providers[i].name)`
  (`src-tauri/src/balancer/mod.rs:112-116`). Index is pool-local but
  the underlying flag is per-account, so the same flag excludes an
  account from every pool that routes through it.

Recent-error avoidance (kept) still keys by
`(model_name, provider_index)` in `recent_error_count`
(`src-tauri/src/state/db.rs:1188-1208`). Intentional — recent errors
are pool-local, exhausted is account-global. `CliSelection` at
`src-tauri/src/setup/actions.rs:92,171` is an unrelated struct
correctly left untouched. No stray cross-wiring.

### 8. Migration ordering — VERIFIED SAFE

Both statements are idempotent ALTER TABLE inside schema-ensure:

- `ALTER TABLE provider_quotas ADD COLUMN exhausted_at TEXT NULL` —
  add the `exhausted_at` check to `ensure_provider_quotas_schema` at
  `src-tauri/src/state/db.rs:611-638`, following the live
  `last_empty_refresh_at` precedent.
- `ALTER TABLE invocations DROP COLUMN quota_tight_routing` —
  replace the current ADD COLUMN branch at
  `src-tauri/src/state/db.rs:545-550` with a symmetric guarded DROP
  following the `last_delta_percent` / `last_delta_calls` pattern at
  lines 623-635.

Failure modes:

- ADD fails, DROP not yet executed → next startup retries
  (idempotent PRAGMA check). No data loss.
- ADD succeeds, DROP fails → next startup skips ADD (column present)
  and retries DROP. Stale `quota_tight_routing` column is dead weight
  but harmless.
- DROP succeeds on invocations but legacy rebuild migration
  (`migrate_legacy_invocations` at `src-tauri/src/state/db.rs:744-857`)
  still creates the column on old databases — §3.6 correctly calls
  out removing the column from the rebuild's CREATE/INSERT SQL at
  lines 786-825.

Order between the two statements is independent.
`ensure_invocations_schema` and `ensure_provider_quotas_schema`
are called in sequence at `StateDb::open`; either can run first
with correct result.

**Minor observation (not blocking):** Proposal §3.6's phrasing
"Delete the schema add-column branch" could more explicitly pair
with "add the symmetric guarded DROP branch." §2 specifies the DROP
ALTER TABLE and says "Match the PR #6 last_empty_refresh_at style";
the test `quota_tight_routing_column_dropped_after_migration` (§8)
pins the intent; the `last_delta_percent` precedent is in the same
file. Implementer has a clear template.

### 9. Deleted test adequacy — ADDRESSED BY D8 + VERIFIED

Rev 1 flagged two dead `use RiskClass` test-module imports not
enumerated in §3.1 or §8:

- `src-tauri/src/main.rs:865` — `use agent_runner_lib::balancer::RiskClass;`
- `src-tauri/src/lib.rs:812` — `use balancer::RiskClass;`

Rev 2 answers §D8 explicitly flags these as mechanical compile
fallout the implementer handles — Rust's unused-import warning catches
them deterministically after the cascade tests delete. Not a
proposal-level gap.

Re-grepped all remaining deleted types across `src-tauri/`:

- `RiskClass` / `RiskClassArg` / `risk_class` — covered §3.1, §3.5, §8.
- `Selection` / `quota_tight_routing` — covered §3.2, §3.6, §8. All
  18+ `quota_tight_routing: false` literals in state, main, and
  integration tests are correctly mapped as mechanical `InvocationStart`
  cleanup in §10.
- `BalanceError` / `ExhaustedError` / `ExhaustedProviderInfo` — §3.3, §8.
- `BalancerConfig` / `RawBalancerBlock` / `user_threshold` /
  `failure_threshold` / `[balancer]` — §3.4, §8, §9.
- `TestModelError` / `TestModelProviderInfo` /
  `TestModelResult.error` — §3.7, §8.
- `invocations.quota_tight_routing` column sites — §3.6 covers
  add-column branch, fresh schema, legacy rebuild create+insert,
  lookup queries.
- README references at `README.md:117-130` and `README.md:217-234` —
  §3.1, §3.4, §9.

No test survives the revert referencing a deleted type. Compile
fallout is bounded, predictable, and explicitly owned by D8.

**Minor observation (not blocking):** §10 cites
`src-tauri/src/executor/cli.rs:971-1148` for `balancer: Default::default()`
cleanup; actual occurrences extend further. The "broad compile
fallout" prose covers the intent and the compiler enforces
completeness.

## Verdict rationale

Rev 2 successfully converts the three rev-1 MEDIUM findings from
implementation-risk into scoped scope-decisions:

- §4 REPL stderr capture → explicitly deferred (D6) with a rationale
  that reconciles the "one extra failure" consequence with the locked
  no-spam invariant.
- §6 heuristic coverage → explicitly kept unchanged (D7) with a
  rationale that keeps broadening as cleanly-scoped future work
  against the shared diagnostics module.
- §9 dead `use` imports → explicitly acknowledged (D8) as mechanical
  compile fallout.

Everything else checks out. Transactional clear is same-tx. DROP
COLUMN is supported by bundled SQLite and precedented in the same
file. Filter-after-refresh is the intended semantics, not a race.
The never-fail-to-return guarantee holds after `BalanceError` is
deleted. Keying is consistent. Migration is idempotent and
recoverable.

Residual minor observations (none blocking): D6's "one extra
failure" framing understates the REPL-only edge (user-initiated,
not balancer-initiated), and §3.6 / §10 could be slightly more
explicit about symmetric DROP branch and full line coverage — but
the intent is clear in §2 / §8 / §10 and the compiler enforces the
rest.

Verdict: **LOW**.
