# Shortcut Risk Assessment: proposals/04-reactive-routing.md (rev 2)

## Verdict: LOW

The §D6 / §11 decision to NOT implement REPL stderr capture is a
**principled trade-off**, not a shortcut. It is correctly scoped
against the user-locked "no spam" invariant, and the consequence
(one extra quota-failed invocation per REPL quota-exit) is on the
*signal-acquisition* side of the invariant, not the *post-
classification stickiness* side that the invariant actually
governs. All seven other shortcut patterns are clean. One internal
inconsistency surfaces (§8 keeps a test that §5 / §11 explicitly
say is not implemented), but that is a proposal-coherence issue —
not a shortcut hiding behavior — so it remains LOW for shortcut
purposes and is flagged for the audit / scope assessors to triage.

## Findings (severity >= medium)

None.

## Section-D6 deep-dive: principled trade-off vs. masking shortcut

### Why this is the load-bearing question

Audit revision 1 raised "REPL stderr capture undesigned" as a
MEDIUM finding. The author's response (`tmp/init04-risk-rerun-
addendum.md` §1) is to *not* design it — accept the gap and
document the consequence. From a shortcut-risk perspective the
question is whether that acceptance violates the "don't spam
reactive" invariant from `tmp/init04-scope-anchor.md:9-13`.

### What the invariant actually says

`tmp/init04-scope-anchor.md:10-13` — "Once a provider is **known
exhausted**, the balancer skips it without re-checking until the
next successful refresh. No probing, no retry on the next user
call, no 'maybe it came back' timers beyond the refresh itself.
**Refresh is the single event that can clear the flag.**"

The verb tense matters: the invariant governs behavior **after** a
provider is *known exhausted*. It is a constraint on the post-
classification path: once flagged, do not unflag until refresh.

### What §D6 actually defers

`research/04-reactive-routing-answers.md:148-189` defers stderr
capture from `execute_interactive`. Consequence
(`research/04-reactive-routing-answers.md:185-189` and
`proposals/04-reactive-routing.md:161`): if the REPL session
itself fails with quota stderr, that REPL exit does NOT set the
flag. The flag gets set on the **next** balancer-routed call to
the same account, after that call's failure runs through
`run_with_balancing` → `run_diagnostics` → category
`quota_exhausted` → `mark_exhausted`. Net cost: one guaranteed
extra quota-failed CLI invocation per REPL quota-exit.

### Why this is signal acquisition, not post-classification re-probing

The deferred behavior happens **before** any flag is set. In the
REPL-quota-exit-then-CLI scenario:

1. REPL fails with quota stderr. Flag remains NULL (deferred path).
2. User runs CLI. Balancer reads `provider_quotas.exhausted_at` —
   NULL — so the account is eligible.
3. CLI subprocess fails with quota stderr.
4. `run_diagnostics` classifies as `quota_exhausted`.
5. `state.mark_exhausted(provider_name)` writes the timestamp.
6. **From here, the locked invariant applies**: balancer skips
   this account for every subsequent call until next non-empty
   refresh. No "spamming" — exactly one classification event, one
   flag write, sticky thereafter.

Step 3 is the signal acquisition event that the locked invariant
sits on top of. The invariant does not promise that the *first*
classification of a quota event happens on the path that first
encounters quota stderr — it promises that *once classified*, the
balancer respects the flag. §D6 trades a one-call delay in step
3's location (REPL → next CLI call) for not having to design
stderr capture under TTY passthrough constraints. The invariant is
intact.

### Compare to the audit-1 alternative

If the proposal had instead added a `wait_5_min_then_re_probe`
helper, or a "set `exhausted_at` to `Some(now())` then check 5
minutes later if a single test call succeeds" pattern, that would
be a shortcut violation: post-classification re-probing under the
guise of "graceful recovery." §D6 does the opposite — it defers
the *acquisition* of one signal channel and accepts a one-call
delay, not a re-probe.

### What would have made this a shortcut

- A boolean flag like `repl_quota_exhausted` written through some
  side channel (env var, parent env, file marker) without going
  through `provider_quotas.exhausted_at`.
- A scheduled background task that "checks if the REPL crashed" by
  parsing `recent_error_count` and synthesizing a flag write.
- Lowering the existing `ERROR_THRESHOLD = 3` recent-error guard
  to "approximate" the missing classification.
- Removing the `select_provider` exhausted filter "until REPL
  classification is wired" (i.e., feature-flagging the routing
  improvement itself).

None of these appear. The `mark_exhausted` write site list at
`proposals/04-reactive-routing.md:158-162` is exactly: CLI
one-shot post-diagnostics, **deferred** REPL site, and Tauri
test_model post-execute. No alternative side-channels.

### Heuristic-coverage corollary (§D7)

`research/04-reactive-routing-answers.md:191-211` and
`proposals/04-reactive-routing.md:243` decline to broaden the
existing quota heuristic (`"quota"` / `"billing"` /
`"usage limit"` per `src-tauri/src/diagnostics/mod.rs:102-132`).
Same logic applies: if real CLI stderr uses non-matching phrasing,
the flag isn't set; the next call re-attempts; refresh TTL
self-corrects. No silent-degradation hack — the same heuristic
already gates `error_category = "quota_exhausted"` and would be
miscategorizing in `run_diagnostics` today regardless of this PR.
Broadening becomes a separate PR against the shared heuristic
that benefits both surfaces. **Not a shortcut**: extracting
`classify_exhaustion` (`proposals/04-reactive-routing.md:140`)
literally just refactors the existing match to a callable helper
without changing terms.

### One coherence wart (flagged, not blocking)

`proposals/04-reactive-routing.md:203` retains the test
`run_repl_marks_provider_exhausted_on_quota_stderr` in the §8
test plan, which can only pass if REPL stderr capture *is*
implemented. §5 (`proposals/04-reactive-routing.md:161`) and §11
(`proposals/04-reactive-routing.md:241`) explicitly say it is
**not** implemented. This is an internal inconsistency in the
revised proposal — almost certainly a leftover from the rev-1
test plan that should have been deleted alongside the §D6
decision. It does not represent a hidden shortcut (the
consequence is documented, not masked), but it does mean §8 is
out of sync with §5 / §11 and the implementer would either need
to delete the test or re-add the REPL plumbing the deferral
removed. Belongs to audit/scope to triage; flagging here for
visibility.

## Per-pattern evidence (eight required checks)

### 1. Per-invocation re-classification / cache

Forbidden in two places.

- `proposals/04-reactive-routing.md:187` — "Do not cache exhausted
  state across calls. The filter reads
  `provider_quotas.exhausted_at` fresh through `get_quota` every
  `select_provider` call".
- `proposals/04-reactive-routing.md:239` — "the exhausted filter
  must read `provider_quotas.exhausted_at` fresh for every
  `select_provider` call through `get_quota`; no caching, no
  per-invocation memoization, and no background re-probe loop".

The filter is attached to the per-call `quotas` vector at
`src-tauri/src/balancer/mod.rs:112-116`, which already calls
`state.get_quota(...)` per provider per `select_provider` call —
no `OnceCell` / `lazy_static` / `Mutex` / module-level state
introduced. Grep
(`cache|memo|lazy_static|OnceCell|RefCell|Mutex`) on the proposal
returns three matches, all negations or pre-existing per-call
language ("cached quotas and windows" at ln 180 references the
existing per-call reads).

PASS.

### 2. Background re-probe loop

Grep
(`timer|retry|backoff|periodic|sleep|wait|schedule|re-probe|reprobe`)
returns only:

- `proposals/04-reactive-routing.md:156` — "No insert, no retry,
  no error on zero affected rows".
- `proposals/04-reactive-routing.md:239` — "no background
  re-probe loop".

Both negations. No new tokio task / interval handler / oneshot
future appears. The pre-existing opportunistic refresh loop at
`src-tauri/src/balancer/mod.rs:93-108` is kept — that is the
locked clear signal (refresh), not a re-probe of the flag.

PASS.

### 3. Threshold behavior preserved in disguise

Threshold comparisons are deleted, not renamed.

- §3.4 at `proposals/04-reactive-routing.md:88` — `BalancerConfig`,
  `RawBalancerBlock`, `parse_balancer`, `append_balancer_toml`,
  threshold validation, the `[balancer]` TOML block, and threshold
  reads in density scoring all removed.
- §3.8 at `proposals/04-reactive-routing.md:124` — "stop comparing
  projected usage to `model.balancer.failure_threshold` and
  `model.balancer.user_threshold`. Remove `hard_eligible`,
  `user_eligible`, all-threshold-exhausted error construction, and
  the user soft-degrade branch".
- §7 at `proposals/04-reactive-routing.md:184` — "preserve the
  projection formula and binding-score ranking, and remove
  threshold gating". Projection stays as **ranking**, never
  **gating** — matches `tmp/init04-scope-anchor.md:28`.

No `projected > X` / `projected >= X` branch survives. No new
scalar replaces `0.70` / `0.95`. No "if `exhausted_at` is None
but projection > N, also write the flag" logic in the
`mark_exhausted` write sites at `proposals/04-reactive-routing.md:160-162`
— mark is gated only on observed subprocess failure with quota
stderr, never on projection.

The only surviving scalar guard is the pre-existing
`ERROR_THRESHOLD = 3` recent-error count
(`src-tauri/src/balancer/mod.rs:9`), kept by §4 at ln 136 — it is
not projection-derived.

PASS.

### 4. Flag stickiness bypass

Clear path is narrow.

- §6 at `proposals/04-reactive-routing.md:166-176` — "add the
  clear only in the non-empty branch. The empty branch preserves
  existing windows and writes only `last_empty_refresh_at`, so it
  must not clear exhausted state".
- Clear SQL: `UPDATE provider_quotas SET exhausted_at = NULL
  WHERE provider_name = ?1`, inside the same transaction as the
  quota/window replacement. Concurrent readers see (old quota +
  flag) or (new quota + cleared) — never (new quota + stale
  flag).

No time-based clear (grep for `5 min|timeout|expire|ttl` returns
no proposal hits), no public `unmark_exhausted` helper exposed,
no background sweep. Mark path at lines 140-164 is
single-statement (`UPDATE` only, no insert, no retry). Test
`upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh`
at ln 194 pins the empty-branch non-clear behavior.

PASS.

### 5. Backward-compat shim for `RiskClass`

- `proposals/04-reactive-routing.md:62` — "Delete the `RiskClass`
  enum from the balancer module". No re-export, no type alias,
  no `#[deprecated]` keep.
- `proposals/04-reactive-routing.md:98` — `--risk-class`,
  `OULIPOLY_RISK_CLASS`, `RiskClassArg`, `resolve_risk_class`,
  and `with_risk_envs` removed.
- `proposals/04-reactive-routing.md:231` — for `ModelConfig.balancer`:
  "those fixtures should be mechanically updated, not replaced
  with a compatibility field". Explicit refusal of the compat-
  field shortcut.
- `research/04-reactive-routing-answers.md:213-220` (§D8) — the
  two test-module `use RiskClass` imports that would dangle after
  the cascade tests delete are removed mechanically as compile
  fallout, not preserved as `#[allow(unused_imports)]` shims.

Grep (`compat|shim|backward|legacy|transitional|alias`) on the
proposal returns three hits: two reference the pre-existing
`invocations.status = 'legacy'` check-constraint surface that the
proposal is *removing* `quota_tight_routing` from (i.e. the
existing legacy migration is being updated, not paired with a new
shim); one is the negation at line 231.

PASS.

### 6. `quota_tight_routing` ghost

Column drop and field deletion are total.

- `proposals/04-reactive-routing.md:14` — `ALTER TABLE
  invocations DROP COLUMN quota_tight_routing`.
- §2 at lines 33-54 — fresh `invocations` schema and the legacy-
  rebuild migration both updated to stop emitting the column.
- §3.6 at line 108 — `InvocationRecord::quota_tight_routing`,
  `InvocationStart::quota_tight_routing`, and
  `Selection::quota_tight_routing` all removed.
- §3.6 at line 106 — warning emission in `run_repl` /
  `run_with_balancing` removed; quota-tight `InvocationStart`
  literals scrubbed from balancer / state / main / integration
  test fixtures.
- §3.6 at line 110 — `quota_tight_routing_column_persisted_to_invocations`
  is **deleted**, not modified.
- §10 at line 229 — remaining cleanup is described as
  "mechanical: remove `quota_tight_routing: false` from the two
  `InvocationStart` literals" — pure literal scrub, no
  compat-wrapper replacement.

Positive coverage: `quota_tight_routing_column_dropped_after_migration`
at line 200 asserts `PRAGMA table_info(invocations)` no longer
lists the column post-`StateDb::open`.

No data-migration shim back-fills the dropped column into
another table. No `pub(crate) use` re-export of the field.

PASS.

### 7. Dual-write of old threshold logic

`BalancerConfig`, `RawBalancerBlock`, `parse_balancer`,
`append_balancer_toml`, threshold validation, and the threshold
fields are deleted at `proposals/04-reactive-routing.md:88-92`.
Threshold reads in density scoring and `exhausted_error` are
deleted at the same lines. `ProviderEval.hard_blocked` /
`user_blocked` fields removed (§3.8 at line 124). README threshold
prose deleted (§3.4 at line 88). No metric / log / counter
continues emitting `user_threshold` / `failure_threshold` after
deletion — grep finds no surviving threshold read sites outside
the delete list.

PASS.

### 8. `BalanceError` zombie

Type-level proof: §10 at `proposals/04-reactive-routing.md:222` —
`select_provider(model, state, ctx) -> usize`. No `Result`. No
`Err` can escape by construction.

Supporting deletes:

- §3.3 at `proposals/04-reactive-routing.md:80` — `BalanceError`,
  `ExhaustedError`, `ExhaustedProviderInfo`, `Display` / `Error`
  impls, and `exhausted_error` deleted.
- Same line — hard-error branch removed; locked Q4 behavior
  applied: "all-exhausted-by-flag falls through to invocation-
  count selection per answers Q4 rather than returning a balancer
  error".
- Same line — `Err(BalanceError::Exhausted(_))` caller branches
  removed from `run_repl`, `run_with_balancing`, and Tauri
  `test_model_with_db_path` preflight.

Pinning test:
`all_providers_exhausted_falls_through_to_round_robin` at line
198 asserts round-robin fallback (lowest invocation-count) when
every provider is flagged — confirms no `Err` path remains.

PASS.

## Implementation-risk notes (not shortcut violations)

- §8 ↔ §5/§11 inconsistency: the
  `run_repl_marks_provider_exhausted_on_quota_stderr` test
  (proposal ln 203) is incompatible with the §D6 deferral. Two
  resolutions are possible: (a) drop the test and rely on the
  documented next-call self-correction, or (b) reverse §D6 and
  add the REPL stderr-capture plumbing. Either is a coherence
  fix, not a shortcut. **Recommend dropping the test** — that
  matches the §D6 decision and avoids the medium-cost
  `os_pipe`/forwarding-thread plumbing the deferral chose to
  avoid. Flag for audit/scope to triage.
- `mark_exhausted` is intentionally outside the
  `finalize_invocation` transaction
  (`proposals/04-reactive-routing.md:164`). It writes
  `provider_quotas`, not invocation/provider aggregates. Single-
  statement, no dual-write, no symptom-masking.
- §7 wording on the all-exhausted fall-through ("use the
  unfiltered provider list so the balancer always returns a
  provider, matching answers Q4") is slightly looser than Q4's
  explicit "fall through to `round_robin_fallback`". The §8 test
  pins round-robin behavior, so the implementer has the
  unambiguous spec from the test. Audit/scope concern, not
  shortcut.

## Shortcut-indicator grep summary

Queries run against `proposals/04-reactive-routing.md`:

| Pattern | Hits | Interpretation |
|---|---|---|
| `cache\|memo\|lazy_static\|OnceCell\|RefCell\|Mutex` | 3 | All three are negations or refer to pre-existing per-call reads. |
| `timer\|retry\|backoff\|periodic\|sleep\|wait\|schedule\|re-probe\|reprobe` | 2 | Both negations ("no retry"; "no background re-probe loop"). |
| `5 min\|timeout\|expire\|ttl` (case-insensitive) | 0 | No time-based clear. |
| `compat\|shim\|backward\|legacy\|transitional\|dual-write\|feature flag\|for now\|in the future\|TODO\|FIXME\|workaround\|temporary\|graceful\|alias` (case-insensitive) | 3 | Two reference the pre-existing `invocations.status = 'legacy'` rebuild path that the proposal is *updating* to drop `quota_tight_routing`; one is the explicit refusal of a compat field for `ModelConfig.balancer`. |
| `Err\(\|Result<\|BalanceError\|quota_tight_routing` | n | Every hit is a delete / drop / migration statement removing the named item — no surviving producer of these symbols after the PR. |

Verdict: **LOW**.
