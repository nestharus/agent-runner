# Initiative 04 — Phase 4 Scope Risk Assessment (revision 2)

**Assessor:** `claude-opus` (scope)
**Verdict:** **LOW.** Coverage matrix, keep list, single-PR framing,
and README scope are correctly bounded by `research/04-reactive-routing-answers.md`
(including new §D6/§D7/§D8) and `tmp/init04-scope-anchor.md`. No scope
creep. One drafting inconsistency between proposal §8 and the newly
added §D6 REPL deferral is flagged below; fix is a one-bullet
deletion in the test list, not a scope reshape.

Revision 1 verdict was PASS (scope). Rerun against revision 2 checks
the same categories plus the new §D6/§D7/§D8 boundaries and
proposal §5/§11 revisions.

---

## 1. Coverage matrix — problem research §1 → proposal §3.x

Every category in problem research §1 maps to a proposal §3 subsection
and to concrete file/line deletions. Walked item-by-item:

| Problem §1 category | Concrete items (#) | Proposal coverage | Gap? |
| --- | --- | --- | --- |
| §1.1 `RiskClass` | 13 | §3.1 + §3.7 (Tauri) + §9 (README) + §10 (signatures) | none |
| §1.2 `Selection` | 7 | §3.2 | none |
| §1.3 `BalanceError` / `ExhaustedError` / `ExhaustedProviderInfo` | 10 | §3.3 + §10 + §3.7 (Tauri preflight mapping) | none |
| §1.4 `BalancerConfig`, thresholds, `[balancer]` | 11 | §3.4 + §3.8 (threshold reads) + §9 (README) | none |
| §1.5 `--risk-class`, env, cascade | 7 | §3.5 + §9 + §10 | none |
| §1.6 `quota_tight_routing` | 16 | §3.6 + §2 (migration) + §10 (InvocationStart literals) | none |
| §1.7 `TestModelResult.error` / `TestModelError` / `TestModelProviderInfo` | 7 | §3.7 | none |
| §1.8 `ProviderEval.{hard,user}_blocked`, eligibility, soft-degrade | 8 | §3.8 + §7 (new filter replaces gating) | minor (see obs #1) |
| §1.9 Tests pinning delete behavior | 6 groups | §3.9 + §8 "Tests to delete" | none |

**9/9 categories mapped, ~70/70 concrete items covered.** Every file
path named in problem research §1 is cited in the proposal with
matching line ranges (e.g. `balancer/mod.rs:19-23` for `Selection`,
`state/db.rs:138-166` for `InvocationRecord`, `state/db.rs:691-718`
for fresh schema, `state/db.rs:786-825` for legacy rebuild,
`lib.rs:902-942` for structured preflight error test).

§D8 confirms the two test-module `use RiskClass` imports
(`main.rs:865`, `lib.rs:812`) are compile fallout the implementer
removes mechanically — not a proposal-level deletion item, correctly
excluded from §3.x.

---

## 2. Keep-list preservation — problem research §2 → proposal §4

| Keep item | Proposal §4 confirms | Accidentally in delete list? |
| --- | --- | --- |
| Projection ranking in `score_by_density` (`balancer/mod.rs:162-196`) | yes | no — §3.8 explicitly "Keep projection and scoring" |
| Bootstrap cascade learned → sibling pool → duration-ratio (`:296-377`) | yes | no |
| Per-window delta learning in `upsert_quota_refresh` (`state/db.rs:1282-1437`) | yes | no — §6 only *adds* the clear inside the non-empty branch |
| Delta-learning guards (`MIN_LEARN_SAMPLE_CALLS`, `NEAR_EXHAUSTED_USED_PERCENT`, `MAX_LEARNABLE_BURN_RATE`) (`state/db.rs:8-45`) | yes | no |
| Fully-unlearned pools reach `round_robin_fallback` (`:455-474`) | yes | no — §7 preserves it as the invocation-count fallback path |
| Recent-error avoidance in density scoring + invocation-count (`:146-160`, `:422-450`) | yes | no — §3.8 explicitly preserves this path |
| `recent_error_count` (`state/db.rs:1188-1208`) | yes | no |

No keep-list drift.

§D7 reconfirms that `diagnose_error` and its existing heuristic
(`"quota"` / `"billing"` / `"usage limit"`) stay unchanged; proposal §5
correctly "extracts the current quota heuristic exactly" without
modifying the diagnostics module's public classification behavior.

---

## 3. Single-PR boundary — evaluated three splits

The proposal's §1 claim ("tightly coupled, one PR") holds under
examination:

### Split A — schema migration as prereq PR
- `ADD COLUMN exhausted_at`: could technically ship alone (backward
  compatible no-op). But the balancer filter in §7 reads it, so the
  migration carries no value without the filter PR immediately
  following.
- `DROP COLUMN quota_tight_routing`: **cannot** ship alone — the
  column is written by `start_invocation` (`state/db.rs:898-929`) and
  by every `InvocationStart` literal in `run_repl` / `run_with_balancing`
  / tests. Dropping it before the Rust struct loses the field is a
  deliberate schema/code mismatch.
- **Verdict: proposal correct.** Migration and code are coupled.

### Split B — `RiskClass` deletion before exhausted-flag add
- Would produce an intermediate state where the balancer has neither
  threshold gating nor reactive filtering — every provider is always
  eligible, a behavioral regression no reviewer would accept.
- Also, `BalanceError::Exhausted(exhausted_error(..., risk_class, ...))`
  at `balancer/mod.rs:225-228` and `:272-289` forces the
  `BalanceError` deletion and the `RiskClass` deletion into the same
  PR regardless.
- **Verdict: proposal correct.** Split creates a dead intermediate
  state explicitly rejected by answers D1–D5.

### Split C — Tauri `TestModelResult` revert as its own PR
- The revert removes `error: Option<TestModelError>` and the
  preflight exhausted mapping. But the mapping only exists because
  `select_provider` returns `Result<Selection, BalanceError>`. As
  long as the balancer still returns `BalanceError::Exhausted`, the
  Tauri caller still needs a code path that maps that error to a
  `TestModelResult` — so deleting the `error` field on its own leaves
  nothing to do with the preflight error.
- Further, the hardcoded `balancer::RiskClass::User` at `lib.rs:525`
  can only be removed once `select_provider` drops the argument.
- Additionally, Tauri `test_model_with_db_path` is one of the
  reactive write sites for `mark_exhausted` per §5 — splitting
  creates a window where the structured-error shim is gone but the
  reactive flag is not yet set from that path.
- **Verdict: proposal correct.** Coupled with balancer API revert.

**Conclusion on PR boundary:** single PR is justified. The answers
doc D1–D5 locks this and the mechanical coupling confirms it.

---

## 4. Test list completeness

Proposal §8 "Tests to delete" and §3.9 together enumerate:

- Balancer threshold/error tests (`mod.rs:732-797`) — 4 tests ✓
- Balancer API tests modified (not deleted) for the `usize` revert —
  all 10 names in problem research §1.9 bucket 2 ✓
- Main risk cascade tests at `main.rs:1198-1325` — 8 tests ✓
- Config balancer tests at `config/model.rs:1180-1278` — 5 tests ✓
- State quota-tight persistence `state/db.rs:2929-2951` — 1 test ✓
- Tauri structured preflight error `lib.rs:902-942` — 1 test ✓
- Test helpers: `with_risk_envs` (`main.rs:906-953`), balancer test
  helper with `quota_tight_routing: false` (`mod.rs:483-501`), state
  test helper (`state/db.rs:2157-2172`), `pr_b_trace_integration.rs`
  literals (`:73-99`), state lifecycle tests (`:2953-3290`, `:3533-3554`),
  main parent-resolution / finalizer-guard tests (`main.rs:1348-1364`,
  `:1441-1525`) — all mechanically updated ✓

All tests named in problem research §1.9 are covered.

### Test-list drift flagged against §D6

Proposal §8 still lists:

> `run_repl_marks_provider_exhausted_on_quota_stderr`: run an
> interactive fixture that exits nonzero with forwarded quota
> stderr, assert the selected provider is marked exhausted.

This contradicts the newly added deferrals:

- Proposal §5 now states for `run_repl`: **"not implemented in this
  PR"** per answers §D6.
- Proposal §11 confirms: **"The implementation does NOT add REPL-side
  classification."**
- Answers §D6 locks Option B (accept-the-gap) with "one guaranteed
  extra quota-failed invocation after a REPL quota-exit" as the
  documented consequence.

A test asserting REPL marks the provider exhausted cannot pass
without REPL-side classification that §D6 explicitly removes from
scope. This is a drafting error in §8 that survived the revision —
the implementer will hit the contradiction immediately. **Fix: delete
the `run_repl_marks_provider_exhausted_on_quota_stderr` bullet from
§8.** One-line correction, not a scope redesign.

The §8 additions that remain correct are:
`mark_exhausted_writes_timestamp_on_existing_quota_row`,
`mark_exhausted_is_noop_when_no_quota_row`,
`upsert_quota_refresh_clears_exhausted_at_on_nonempty_refresh`,
`upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh`,
`classify_exhaustion_matches_quota_billing_usage_limit_stderr`,
`classify_exhaustion_ignores_non_quota_errors`,
`select_provider_filters_exhausted_accounts`,
`all_providers_exhausted_falls_through_to_round_robin`,
`exhausted_filter_does_not_prevent_refresh_loop_from_clearing`,
`quota_tight_routing_column_dropped_after_migration`,
`run_with_balancing_marks_provider_exhausted_on_quota_exhausted_diagnostics`,
`test_model_marks_provider_exhausted_on_quota_stderr`. All justified
by Q1 / Q2 / Q4 / Q5 / D1 / D4 / D7.

**Observation carried from rev 1 (still non-blocking):** §8 adds
`quota_tight_routing_column_dropped_after_migration` but no symmetric
`exhausted_at_column_added_after_migration` test. Recommend adding
the companion test for defensive migration coverage — test-plan
completeness nit, not a scope miss.

---

## 5. README scope

Full grep of README for delete-inventory terms returns exactly the
two regions the proposal names:

- `README.md:117-130` — `--risk-class` flag in CLI options
- `README.md:217-234` — "Risk classes" subsection under Load
  Balancing, including the `[balancer]` TOML block and the
  "Why per-window + risk-class?" closer (line 234, within the
  deletion range)

Proposal §9 covers both. No stray mentions elsewhere. The
`### Risk classes` subsection header (line 217) is also being
removed as part of the block deletion, which §9 implies but does
not spell out explicitly — a one-word clarification would help
reviewers but is not a scope gap.

---

## 6. Scope creep check

Walked the proposal section-by-section against the answers doc
(Q1–Q5 + D1–D8 + the four scope anchors):

| Proposal section | Justified by |
| --- | --- |
| §2 schema (add `exhausted_at`, drop `quota_tight_routing`) | Q1, D1 |
| §3.1 `RiskClass` delete | scope anchor 4, Q5 |
| §3.2 `Selection` delete | scope anchor 4, D3 |
| §3.3 `BalanceError` delete | Q4 (no balancer hard error) |
| §3.4 `BalancerConfig` / thresholds delete | scope anchor 4 |
| §3.5 CLI risk-class surface delete | scope anchor 4 |
| §3.6 `quota_tight_routing` delete | scope anchor 4, D1 |
| §3.7 `TestModelResult` revert | D2 |
| §3.8 `ProviderEval` blocked flags / soft-degrade delete | scope anchor 3, Q4 |
| §3.9 test deletions | scope anchor 4 |
| §4 keep confirmations | scope anchor 3, problem §2 |
| §5 exhausted write path + `classify_exhaustion` helper | Q5, D6 (REPL deferral), D7 (heuristic unchanged) |
| §6 clear path (non-empty refresh branch only) | Q2 |
| §7 balancer filter with all-exhausted fallthrough | D4, Q4 |
| §8 test plan | scope anchor 4 + new §5–§7 (see §4 drift note) |
| §9 README update | anchored surface |
| §10 cross-cutting signature reverts + fixture cleanup | D3, D8 |
| §11 risk surface | meta (documents D6/D7 deferrals) |

**No scope creep detected.** Revision 2's additions (`classify_exhaustion`
as a pure heuristic wrapper; REPL write site deferred; `use RiskClass`
test-module imports handled mechanically) are all narrower than rev 1
and all justified by the newly added §D6/§D7/§D8.

### Rev 1 borderline item resolved

Rev 1 flagged "§5 REPL stderr plumbing" as implicitly scoped from Q5.
Rev 2 **explicitly removes** REPL stderr plumbing from scope via §D6.
Proposal §5 now documents the non-implementation with the one-extra-
failure consequence, matching the answers-doc decision. The rev 1
scope-boundary concern is resolved; the test-list drift in §4 above
is the only residue.

---

## 7. Observations (non-blocking)

### Observation 1 — `ProviderEval` plumbing after `hard_blocked` / `user_blocked` removal

Problem research §1.8 item 2 notes that recent-error avoidance
currently signals "skip" by constructing a `ProviderEval` with
`hard_blocked: true, user_blocked: true` (`balancer/mod.rs:146-160`).
With those fields deleted per §3.8, the proposal keeps recent-error
avoidance "as an eligibility/deprioritization path" but does not
spell out the replacement signal. Likely resolution: drop the
recent-error providers from the `evals` vector entirely, or use a
new `excluded` flag / binding-score sentinel. This is an
implementation-detail gap inside an in-scope item (§3.8), not a
missed scope item. Flagging so the phase-4 implementer doesn't
rediscover it mid-implementation.

Related: `binding_score = None` is currently computed from
`unlearned || hard_blocked || !scored_window`
(`balancer/mod.rs:201-205`). With `hard_blocked` gone, the condition
reduces to `unlearned || !scored_window`; the proposal does not
state this explicitly but the reduction is mechanical.

### Observation 2 — migration add-test symmetry

§8 tests the `quota_tight_routing` drop but not the `exhausted_at`
add. Both are idempotent `ALTER TABLE` statements in the same
schema-ensure block; a companion `exhausted_at_column_added_after_migration`
test would keep the migration behavior test coverage symmetric.
Non-blocking.

### Observation 3 — README subsection header

§9 says "delete the risk-class/threshold block." The subsection
header `### Risk classes` at `README.md:217` is part of that block
and should also be removed. Implied by §9 but not spelled out.
Non-blocking.

### Observation 4 — REPL test in §8 contradicts §D6

Primary rev-2 finding; see §4 above. Fix is to delete the
`run_repl_marks_provider_exhausted_on_quota_stderr` bullet from §8.
One-line correction, no further scope implications. Leaving this as
an observation rather than a blocker because §5 / §11 / §D6
unambiguously lock the deferral — the contradiction is between the
test list and its own proposal, not between the proposal and the
answers doc.

---

## 8. Summary

- **Coverage:** complete across all 9 delete categories and ~70
  concrete items.
- **Keep-list:** fully preserved; no accidental deletion; D7 anchors
  diagnostics heuristic untouched.
- **Single-PR boundary:** justified; every candidate split produces
  dead intermediate state or schema/code mismatch.
- **Tests:** all pinning tests named for deletion, all helpers
  mechanically updated; one residual rev-2 inconsistency
  (`run_repl_marks_provider_exhausted_on_quota_stderr` contradicts
  §D6 REPL deferral — delete the bullet); companion migration
  add-test still missing (non-blocking).
- **README:** scope correct at `:117-130` and `:217-234`; no other
  mentions.
- **Creep:** none. Rev 1's borderline "REPL stderr plumbing" is now
  explicitly deferred (§D6) and no longer implicit scope.
- **Minor implementation-detail gaps:** `ProviderEval` skip-signal
  replacement for recent-error avoidance after `hard_blocked` /
  `user_blocked` deletion needs explicit treatment during
  implementation.

**Verdict: LOW.** Proposal is correctly scoped for phase 4 given
the rev-2 addendum; the flagged test-list bullet is a one-line
correction, not a scoping issue.
