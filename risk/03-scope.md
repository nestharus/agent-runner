# Scope Risk Assessment: proposals/03-load-balancing-tiers.md

## Verdict: LOW

The proposal is correctly scoped. Every needs-doc requirement maps to
a specific proposal hunk, and every proposal hunk traces back to an
answer in `research/03-load-balancing-tiers-answers.md` or to a
pulled-in-scope prerequisite in `research/03-load-balancing-tiers-needs.md`
§1.3. The three-PR split (`chatgpt-usage` → `is_stale`/empty-write →
scoring redesign) matches the orchestrator's Q10 sequencing exactly
and draws its boundaries along genuine fault lines. Revision 2
(per `tmp/03-risk-rerun-addendum.md`) tightened the bootstrap/scoring
semantics and the `run_repl` exhaustion surface, introduced an
explicit `ProviderEval` record, replaced the `EPS_BURN_RATE` floor
with an `Option<f64>` ineligibility rule, corrected the duration-ratio
direction, and added three ineligibility-related tests. Each revision
change lands inside an already-in-scope section — none expand the PR
envelope or drop a prior requirement. PR 3 cannot usefully be split
further without introducing dead code or half-wired intermediate
states. All four explicit out-of-scope items from §1.2 remain
honored. Residual concerns are minor (the two items carried forward
from the prior assessment: the tracked-script addition in §2.3 and
the `scripts/README.md` mention).

## Coverage matrix

| Needs requirement | Proposal section | Coverage |
|---|---|---|
| Axis A — tier quantities, commensurate quantity axis (needs §1.1, §2) | §4.1, §4.5, §4.6, §4.7 | complete — `min_w rate_{p,w}` in turns-per-hour with per-window burn rate and per-window projection |
| Axis B — per-window burn rate (needs §1.1, §3) | §4.5 (per-window delta learning, carry-forward scope), §4.6 (bootstrap cascade returning `Option<f64>`, corrected direction), §4.7 (per-window projection replaces scalar) | complete — `global_avg_percent_per_call` deleted |
| Axis C — risk class (needs §1.1, §4) | §4.3 `[balancer]` TOML, §4.4 `RiskClass` + CLI flag + env var + heuristic cascade (revised rule 6 reconciled), §4.7 thresholds in scoring, §4.8 error surfacing including explicit `run_repl` paragraph | complete |
| §1.2 out: no scraper rewrite | §2 preserves `QuotaScriptOutput`/`QuotaScriptWindow` contract; only the external script's emitter changes | honored |
| §1.2 out: no session-ingestion redesign | proposal does not touch `sessions/mod.rs`, still reads `count_assistant_turns_since` | honored |
| §1.2 out: no two-DB unification | §5.3 explicitly confirms both `StateDb::open` call sites receive the same migrations with no coupling change | honored |
| §1.2 out: sub-agent traffic not a distinct class | §4.4 defines exactly `RiskClass { User, Background }`; cascade propagates per-call without lineage coupling | honored |
| §1.3 pulled in: `is_stale` empty-windows fix | §3.2 (is_stale guard), §3.5 (tests) | complete |
| §1.3 pulled in: `chatgpt-usage` second-window | §2 entirely | complete |
| §4.1 two-class set only | §4.4 `RiskClass { User, Background }` | complete |
| §4.2 threshold anchors 70/95 on projected usage | §4.3 defaults `0.70`/`0.95`, §4.7 evaluates on `projected_used_w` | complete |
| §4.3 plumbing at three call sites | §4.4 adds to `Cli`; `run_repl` hardcodes `User` (§4.4 + §4.8 paragraph); Tauri `test_model` hardcodes `User` | complete |
| §4.4 behavior on all-fail gate | §4.7 selection policy over `ProviderEval`: hard refuse at 95%, soft-degrade User at 70% with `quota_tight_routing`; §4.8 surfaces errors (one-shot, repl, Tauri) | complete |
| Needs Q9 fallback unchanged for fresh pools | §4.7 `fresh_pool_falls_through_to_invocation_count_round_robin` branch + test | complete — preserved pre-PR-3 behavior |
| §5.1 `is_stale` empty-windows TTL inversion | §3.2 | complete |
| §5.2 `chatgpt-usage` 5h drop | §2.1–§2.3 | complete |
| Needs Q11 reject empty writes | §3.3 (three cases: prior>0, prior=0, nonempty) + `last_empty_refresh_at` audit column | complete |
| Answers Q1–Q8 decisions | §4.1 (Q1), §4.5 (Q2), §4.6 (Q3 cascade w/ corrected direction, `Option<f64>`), §4.6+§4.7 (Q4 sibling pool), §4.7 (Q5), §4.4 (Q6 revised cascade), §4.3 (Q7), §4.7–§4.8 (Q8) | complete |

## Creep findings

**F1 — LOW — §2.3: tracked `scripts/chatgpt-usage` copy.** The
proposal adds a new tracked script file that does not currently exist
under `scripts/` (verified 2026-04-21: `ls /home/nes/projects/agent-runner/scripts/`
returns no `chatgpt-usage`). Needs §5.2 scopes the fix to the
installed script; orchestrator answer Q10 deliberately left the
packaging location for the proposal to verify: *"and any packaging
location of that script if tracked in the repo — verify during
proposal."* The proposal models the add after the existing
`scripts/anthropic-usage` precedent (`scripts/anthropic-usage:1-54`).
A small packaging expansion, not a refactor — acceptable but worth
flagging. Carried forward unchanged from the prior revision.

**F2 — LOW — §4.3: `rejects_balancer_user_threshold_above_failure_threshold`
validator.** The proposal adds a validation rule not explicitly named
in the needs doc or answers doc: *"rejects `user_threshold >
failure_threshold` because the user gate must not be stricter than
the hard failure gate."* This is semantically required by the design
(answer Q7 defines 0.70 as soft, 0.95 as hard — inversion is
nonsense), but it is a proposal-introduced constraint. Small and
defensible; flagged only for transparency. Carried forward unchanged.

**No new creep introduced by revision 2.** The revision's new surface
— the `ProviderEval` record shape (§4.7), the explicit `run_repl`
exhaustion paragraph (§4.8), the `Option<f64>` return from bootstrap
(§4.6), the `round_robin_fallback`-on-fully-unlearned branch (§4.7),
the carry-forward staleness paragraph (§4.5), and the three new tests
(`bootstrap_returns_none_when_no_sibling_has_learned_rate`,
`unlearned_provider_is_ineligible_when_siblings_are_learned`,
`fresh_pool_falls_through_to_invocation_count_round_robin`,
`bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio`,
`risk_class_heuristic_classifies_piped_stdin_as_background`) — all
lie inside already-in-scope sections (§4.5, §4.6, §4.7, §4.8, §4.9)
and map to existing answers (Q3, Q6, Q9). None add a new concern.

## Gap findings

**G1 — LOW — `scripts/README.md` quota-scripts section not updated.**
`scripts/README.md:191-209` enumerates `anthropic-usage` and
`zai-usage` as reference quota adapters. If PR 1 adds
`scripts/chatgpt-usage`, the README's "Reference" list should name it
too, otherwise the repo documents a tracked script that isn't
discoverable from the scripts README. Proposal §2 does not mention a
README touch-up. Minor — follow-up during implementation is
sufficient; not a blocker. Carried forward unchanged.

**G2 — LOW — `anthropic-usage` window-order comment drift.** §4.6
notes that `/home/nes/.local/bin/anthropic-usage:16-18` comments
"Window order doesn't matter" because the old learner used the
longest window; it then says *"PR 3 should update that comment in any
tracked script copy if it becomes misleading, but not bundle script
behavior changes into PR 3."* This correctly excludes the comment
change from PR 3 but does not clearly assign it anywhere. Borderline
creep-or-gap depending on interpretation. Safe to leave out —
cosmetic only. Carried forward unchanged.

**No substantive gaps introduced by revision 2.** Every audit-risk
fix from the addendum has a corresponding proposal hunk and a test:

- HIGH finding 1 (duration ratio): §4.6 pseudocode + physical-
  intuition paragraph + test `bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio`.
- MEDIUM finding 2 (cluster-H contradiction): §4.4 rule 6 +
  test `risk_class_heuristic_classifies_piped_stdin_as_background`.
- MEDIUM finding 3 (`ProviderEval`): §4.7 explicit record definition +
  filter-based selection policy.
- MEDIUM finding 4 (`run_repl` exhaustion): §4.8 explicit paragraph
  on `Err(Exhausted)` and `Ok(quota_tight)` treatment.
- LOW findings 5–8 (Cargo.lock swap, EPS bias, carry-forward bound,
  off-by-one): §4.2, §4.6 `Option<f64>`, §4.5 paragraph, §4.2 line
  range — each a localized textual/semantic correction.

## PR boundary findings

**PR 1 (`chatgpt-usage` 5h + weekly windows).** Cleanly disjoint. The
only Rust-adjacent touch is the `QuotaScriptOutput` parser already
accepting the target shape (§2.2 cites `src-tauri/src/quota/mod.rs:65-84`,
`222-265`), so no Rust changes are needed. Test scope (§2.4) is
script-level jq emission; no overlap with PR 2 or PR 3. Genuinely
parallelizable. Keep as a single PR — splitting would produce a
three-line PR with no standalone value.

**PR 2 (`is_stale` + empty-write + audit column).** Coherent around
one concern: "zero-window state must self-heal and must not be
re-entered." Files limited to `quota/mod.rs`, `state/db.rs`, and
tests (§3.7). The `last_empty_refresh_at` audit column travels with
the empty-write reject because it exists to diagnose the exact event
being rejected (answer Q11). Keep as one PR — splitting the audit
column out would leave a PR that cannot be validated except by
"I promise the empty-write rejection happened."

**PR 3 (scoring redesign).** Largest PR. Re-evaluated three candidate
splits against the revised proposal:

1. *Schema migrations (§4.2) ship independently.* Rejected.
   Migrations `M_03_02`, `M_03_03`, `M_03_04` are read/written by the
   PR 3 code; isolating them buys nothing and introduces a dead
   middle state where old code would select nonexistent columns
   after `M_03_03`. Answers Q2 forbids dual-write shims.

2. *Risk-class plumbing (§4.4) ships without the scoring redesign.*
   Rejected. `select_provider` now returns `Result<Selection,
   BalanceError>` where `Selection.quota_tight_routing` has no
   producer without §4.7's new scoring. The `--risk-class` flag
   would be accepted but ignored; `invocations.quota_tight_routing`
   would never be written from ≠0. Dead plumbing.

3. *`[balancer]` TOML block (§4.3) ships independently.* Rejected.
   Thresholds are unreferenced without §4.7's projected-used gating.
   Would land as dormant config that §4.9's
   `balancer_toml_overrides_apply_per_model_pool` test cannot
   exercise.

Revision 2 did not change this calculus — if anything, the revision
made PR 3 more tightly coupled internally (explicit `ProviderEval`,
`Option<f64>` bootstrap, `run_repl` exhaustion all require each
other to be useful). Keep as one PR. This is correct scope
discipline given §4.10's "no TODO-gated rollout, no feature flags,
no hidden fallback to old scalar scoring" stance.

**Combining candidates.** PR 1 + PR 2 could theoretically be one PR
("prerequisites"), but they touch disjoint file sets (external
script vs. Rust crate) and neither depends on the other
structurally. Parallel shipping lowers queue time; keeping them
split is better. PR 2 + PR 3 could combine, but that would bundle
the self-heal fix with the scoring rewrite, which violates the
orchestrator's Q10 sequencing (*"so `claude2` has windows and
scoring redesign tests can validate on it"*) and forces reviewers to
evaluate two concerns at once. Keep split.

## Dependency graph correctness

- **PR 1 ⊥ PR 2 & PR 3.** Verified: PR 1 touches
  `scripts/chatgpt-usage` (new) and `/home/nes/.local/bin/chatgpt-usage`.
  No Rust files. §5.1 correctly classifies as parallel.
- **PR 2 ⊥ PR 1.** Verified: PR 2 files are `quota/mod.rs` +
  `state/db.rs`; PR 1 files are script-only. No overlap.
- **PR 3 depends on PR 2.** Verified at two levels:
  - *Code-level*: PR 3's §4.5 rewrite of `upsert_quota_refresh` to
    emit per-window deltas must be layered on PR 2's rewrite
    (no-wipe-on-empty, `last_empty_refresh_at`). Merging PR 3
    without PR 2 would reintroduce the empty-write wipe regression.
  - *Test-data level*: §5.1 calls out that scoring redesign tests
    need `claude2` to self-heal into a windowed state before pool
    density scoring exercises the new code path.
  The dependency is real, not ceremonial. Revision 2 did not change
  the dependency graph.

## Test plan scope

- PR 1 test plan (§2.4): all four tests map to the behavior change
  (normal two-window emission, secondary-only, primary-only,
  credential failure). No unrelated tests. Complete.
- PR 2 test plan (§3.5): 8 tests, each mapping to a named behavior
  change (`is_stale_forces_refresh_when_windows_empty`,
  `upsert_quota_refresh_preserves_windows_on_empty_input`, etc.). No
  creep. `is_stale_treats_missing_quota_row_as_stale` is a
  regression guard for existing behavior, not net-new scope —
  acceptable as a guard when the surrounding function is edited.
- PR 3 test plan (§4.9): now 23 tests after revision 2 (20 prior +
  3 new ineligibility tests from the addendum +
  `bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio` +
  `risk_class_heuristic_classifies_piped_stdin_as_background`,
  balanced by the existing coverage entries). All trace to §4.3–§4.8
  behaviors (threshold overrides, risk-class heuristics, burn-rate
  learning, bootstrap cascade direction, ineligibility semantics,
  fresh-pool round-robin, exhausted error, structured Tauri error,
  per-window delta write, carry-forward on reset). The four existing
  tests named for re-seeding (answer Q5) remain called out. No tests
  cover unrelated correctness. Complete and no creep.

## Rollback scope (§5.4)

The proposal's stance — *"extra columns tolerated until `M_03_03`
drops provider-level deltas; after that, old code that reads the
dropped columns fails; recovery is roll-forward or manual repair"* —
is appropriate scope. Answer Q2 forbids dual-write shims; a rollback
migration inside PR 3 would either (a) be a no-op because schema
changes were additive only (but then `M_03_03` cannot drop the old
columns), or (b) amount to re-adding a compatibility shim when
executed, which contradicts the repo's operator conventions. The
proposal acknowledges the one-way door in writing. No rollback-scope
change needed. Revision 2 did not alter the rollback surface.

## Cross-DB scope (§5.3)

Verified: `StateDb::open` is the single entry both the CLI default
DB path and the Tauri app DB path route through
(`src-tauri/src/state/db.rs:326-340`, §5.3 citations). Running the
CLI and the Tauri app against their different `state.db` files
requires no coupling change in this initiative. Needs §1.2's *"treat
as one logical store for design purposes but assume both code paths
share a schema"* is honored. No scope drift.

## Spot-checks verified

- `scripts/chatgpt-usage` does not exist in the repo today (`ls
  /home/nes/projects/agent-runner/scripts/`); §2.3's addition is a
  net-new file. Consistent with orchestrator Q10's "verify during
  proposal."
- `scripts/anthropic-usage:1-54` exists as the packaging precedent
  the proposal follows.
- `scripts/README.md:191-209` lists reference quota adapters
  (`anthropic-usage`, `zai-usage`) but does not mention
  `chatgpt-usage` — the G1 gap above.
- `research/03-load-balancing-tiers-answers.md:305-323` PR
  sequencing matches the proposal's three-PR structure in §2/§3/§4
  and §5.1.
- `research/03-load-balancing-tiers-needs.md:31-40` out-of-scope
  list is faithfully excluded (§5.3 + absence of scraper contract /
  session ingestion / sub-agent-class work in the proposal).
- Heuristic cascade §4.4 matches answer Q6's seven-step cascade
  (revised rule 6 `cat | agents` → `Background`).
- `select_provider` signature in the current code at
  `src-tauri/src/balancer/mod.rs:22-26` is `(model, state, ctx) ->
  usize`, matching the proposal's stated refactor target.
- The `round_robin_fallback` fall-through in §4.7 for fully-unlearned
  pools aligns with answer Q9's "fallback path remains
  invocation-count as today" and with pre-PR-3 behavior at
  `src-tauri/src/balancer/mod.rs:62-69`.
- Addendum-named test additions
  (`bootstrap_returns_none_when_no_sibling_has_learned_rate`,
  `unlearned_provider_is_ineligible_when_siblings_are_learned`,
  `fresh_pool_falls_through_to_invocation_count_round_robin`) are
  present in §4.9 of the revised proposal.
