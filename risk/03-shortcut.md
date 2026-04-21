# Shortcut Risk Assessment: proposals/03-load-balancing-tiers.md

## Verdict: LOW

Revision 2 reads cleanly against the no-compat-shims policy and
tightens several near-shortcuts flagged in the revision 1 audit. The
provider-level delta columns are still dropped in the same PR that
introduces the window-level columns, with no dual-write and no
transitional reader (§4.2 migrations `M_03_02` add-before-drop
`M_03_03`). The numerical shortcut from revision 1 — a hardcoded
`EPS_BURN_RATE = 1e-9` floor on the bootstrap output — is
**removed**: `bootstrap_burn_rate` now returns `Option<f64>`, and
`None` marks a provider window as ineligible for density scoring
rather than papering over it with an epsilon that would outrank
learned siblings by ~1e6× (§4.6, §4.7). The first-run empty-pool
case now falls through to `round_robin_fallback` explicitly, which
matches pre-PR-3 behavior and avoids both a scalar-floor shortcut
and a spurious `Exhausted` error. The other residual shortcut
surfaces — the §4.5 carry-forward rule, the §3.3 empty-write
reject-and-advance-`refreshed_at` dance, the heuristic risk-class
cascade in §4.4, and `EPS_HOURS` — are principled trade-offs with
stated rationale and explicit bounding, not symptom-masking. The
remaining hardcoded constant `EPS_HOURS = 1.0 / 60.0` is carried
over unchanged from current code with the motivation explicit.
`scripts/chatgpt-usage` follows the tracked `scripts/anthropic-usage`
convention and is not source-vs-artifact confusion. §4.10 enumerates
the canonical shortcut patterns (dual-write, compatibility aliases,
TODO-gated rollout, feature flags, hidden scalar fallback) and
refuses each of them explicitly. No finding rises to MEDIUM or HIGH.

## Findings (severity >= medium)

None.

## Shortcut-indicator grep

Searched `proposals/03-load-balancing-tiers.md` for the canonical
shortcut-indicator flags: `compat`, `shim`, `backward`, `legacy`,
`transitional`, `dual-write`, `feature flag`, `for now`, `in the
future`, `TODO`, `FIXME`, `workaround`, `temporary`, `graceful
degrade`, `self-heal`, `bootstrap`, `carry-forward`, `carry forward`,
`audit column`, `followup`, `follow-up`, `defer`, `hack`, `magic`,
`placeholder`, `hardcode`, `hard-code`, `symptom`.

Hits and their disposition:

- `legacy` (4 hits, lines 9, 19, 65, 143). All four describe
  **current** code being replaced by this proposal, not new legacy
  code being introduced. §2.1 names the current `chatgpt-usage`
  one-window shape as the legacy contract. §2.2 notes the Rust
  parser's existing legacy fallback (pre-existing tolerance, not
  introduced here). §3.3 describes the pre-PR-2 `legacy_used = 0.0`
  behavior being fixed. §4.2 uses "legacy invocation rebuilds" to
  refer to the existing SQLite table-rebuild migration style at
  `src-tauri/src/state/db.rs:658-727`.
- `compat` / `compatibility` / `shim` — only appear inside §4.10's
  explicit **negation** ("no provider-level delta compatibility
  aliases") and §5.4's explicit refusal to add a rollback shim
  ("do not add compatibility shims or dual-write paths to make
  rollback transparent"). Both consistent with the no-compat-shims
  policy.
- `dual-write` — §4.10 ("no dual-write of deltas on both old and
  new tables") and §5.2 ("avoids needing any dual-write"). Both
  negations.
- `TODO` / `FIXME` — zero hits.
- `feature flag` — §4.10 explicit negation only.
- `transitional` / `for now` / `in the future` / `temporary` /
  `workaround` / `hack` / `magic` / `placeholder` — zero hits.
- `hardcode` / `hardcodes` — two hits (lines 212–213, `run_repl`
  and `test_model` hardcode `RiskClass::User`). These are
  legitimate: interactive call-sites have a static class by
  definition (needs §4.3 row 2–3 — `repl` "exists specifically for
  human-in-the-loop" and Tauri `test_model` is "driven from the
  app UI"). Not a shortcut.
- `defer` — one hit (§4.5, "a max-refreshes-without-update decay
  counter is deferred to a future iteration if observed workloads
  show drift"). This is a scoped deferral of a speculative
  refinement, not a deferral of required work (see root-cause check
  below). It is explicitly tied to an observable trigger rather
  than being open-ended.
- `bootstrap` — §4.6 "Bootstrap cascade" and §4.7 pseudocode.
  Three-tier cascade from answers §Q3; each tier has a defined
  transition condition and the terminal case is `None`, which the
  caller handles by falling through to `round_robin_fallback` (not
  by a silent floored scalar). Addressed under root-cause check.
- `self-heal` — §3.6 and §5.1, both describing that a window-less
  `claude2` row will force-refresh on the next `select_provider`
  call because `is_stale` now returns `true` on empty windows. This
  is the **root** fix, not self-healing-in-lieu-of-a-fix.
- `reject` / `rejection` — §3.1 title, §3.3, §3.7, §4.3 validation
  rules. All describe explicit refusal paths that close bug classes,
  not papering-over. Addressed under root-cause check.
- `audit column` — §3.7 (`last_empty_refresh_at`). Addressed under
  root-cause check.
- `carry-forward` / `carry forward` — §4.5 ("Otherwise carry
  forward that same prior window's previous delta"). Now explicitly
  scoped in a new paragraph: carry-forward applies only on
  `new ≤ prior`; any positive delta overwrites; staleness is
  bounded by the preserved windows' `resets_at`. Addressed under
  root-cause check.
- `symptom` — zero hits.

`EPS_BURN_RATE` appears only as a citation of the **superseded**
revision-1 plan ("supersedes the earlier floor-at-zero-plus-
`EPS_BURN_RATE` plan", line 258). The constant is not introduced by
this revision. Concern 6 below is therefore moot.

Policy anchor: the concrete no-compat-shims statement lives in the
orchestrator answers (`research/03-load-balancing-tiers-answers.md:62-63`)
citing "This repo's AGENTS.md and the operator conventions in
`~/work` and `~/projects/server-manager` forbid backward-
compatibility shims, so the columns on `provider_quotas` are deleted
rather than dual-written." Revision 2 follows that.

## Root-cause vs symptom check

**§3.2 `is_stale` forces-stale on empty windows** — root fix. The
current bug is that `dynamic_ttl_secs([])` returns `MAX_TTL_SECS`,
so a window-less row is treated as fresh for 24h
(`src-tauri/src/quota/mod.rs:148-151`). §3.2 inverts that by adding
`windows.is_empty() → stale` in `is_stale` itself, keeping
`dynamic_ttl_secs` as a pure TTL helper for non-empty lists. This is
the one-line semantic fix needs §5.1 required, not a bypass.

**§3.3 `upsert_quota_refresh` empty-input rejection** — root fix at
the correct layer. The scraper failure mode in data-a §Q11
(`anthropic-usage` synthesizes `{"windows":[]}` when both Anthropic
timer entries are absent from the API response) is one path into the
wipe bug; any third-party scraper can also emit empty arrays. Fixing
at the Rust write-path closes the general class. Fixing only at
`anthropic-usage` would leave other scrapers vulnerable. The scraper
does emit empty arrays through its happy path (the script's `if
.seven_day.resets_at then ... else empty end` / `if
.five_hour.resets_at then ... else empty end` construction can
legitimately produce `{"windows":[]}`), so treating the Rust layer
as the authoritative rejection site is consistent with the scraper
contract being an input, not a subject, of this initiative (needs
§1.2).

**§3.3 `last_empty_refresh_at` audit column** — legitimate
observability, not a logging compensation. Answers §Q11 explicitly
states: "Alternative of logging to stderr alone was considered —
rejected because CLI and Tauri paths have different log sinks
(data-a §8) and the DB is the one sink both see." The CLI writes to
`[diagnostics:]` stderr; Tauri ships structured JSON to the
frontend; the DB is the only observation point both runtime paths
share. Recording the empty-refresh timestamp alongside `refreshed_at`
is the minimum post-mortem signal, and §3.7 cites the
different-log-sinks rationale.

**§4.5 carry-forward of prior delta** — principled, with the
revision-2 bounding paragraph pinning it down. The burn rate is a
property of workload (`percent per assistant turn`), not of absolute
`used_percent`. When a window resets, `used_percent` drops to zero
and `dp = max(0, new - prior) = 0`. Without carry-forward, a
provider would lose its learned rate every time a 5h window rolled
over and wait hours to relearn. Carry-forward preserves it across
resets. The revision-2 paragraph (lines 227–231) explicitly bounds
the staleness: carry-forward applies only on
`new.used_percent ≤ prior.used_percent` (resets or flat
observations); any positive delta overwrites; the preserved rate
rides the preserved windows, which age out via their own
`resets_at`. When windows cross their reset time, `dynamic_ttl_secs`
floors to `MIN_TTL_SECS` (`src-tauri/src/quota/mod.rs:152-158`) and
forces aggressive re-refresh. The decay-counter deferral (also in
§4.5) is explicitly contingent on observed drift — not an
indefinite "will fix later," but a scoped-out refinement that
activates on a defined trigger. Acceptable.

**§4.6 bootstrap cascade** — principled and newly numerically
honest. The three tiers (own-window learned → pool-sibling average
→ duration-ratio scaled from a sibling's longer window) match
answers §Q3 exactly. The revision-2 change is that the terminal
case returns `Option::None` rather than a floored positive scalar;
callers treat `None` as "provider ineligible at this window,"
causing an unlearned provider to either be rescued by a learned
sibling through the pool-average or duration-ratio branch, or to
fall through to `round_robin_fallback` when **every** pool member
is unlearned. This removes the revision-1 numerical shortcut
(EPS-floored bootstrap that would have made an unlearned provider
outrank learned siblings by ~1e6×). Root fix, not a workaround.

**§4.7 scoring function** — principled. `ProviderEval` gives the
selection policy an explicit shape; hard/user-blocked flags are
computed per window before the binding-min; the `any_unlearned`
flag cleanly excludes a provider from scoring without injecting an
epsilon; the fresh-pool round-robin fallback only fires when every
provider is unlearned and none is hard-blocked, matching the
pre-PR-3 `all_have_windows`-fail path at
`src-tauri/src/balancer/mod.rs:62-69`. Hard refuse at 95% returns
`BalanceError::Exhausted`, not round-robin — closing the prior
bug where round-robin hid an exhausted state.

**§4.4 7-step risk-class cascade** — heuristic grounded in
observed data, not a hack. Data-b §6.5 enumerated 92 real
invocations across 17 files and clustered them structurally. Each
branch of the cascade maps to an observed cluster: `repl` →
interactive; `-f/--file` → clusters A, B, E, F, I, J, K, L (60 of
92); `OULIPOLY_PARENT_INVOCATION` → runner-from-runner; piped
stdin → cluster H (3 of 92) treated as Background because the
runner cannot distinguish scripted pipes from human pipes and the
92-invocation observation shows pipe-stdin is overwhelmingly
workflow; TTY + positional prompt → clusters C/G (16 of 92) → User.
The cascade is **also** only the default — explicit signals (CLI
flag `--risk-class`, env var `OULIPOLY_RISK_CLASS`) override. The
revision-2 reconciliation of the earlier cluster-H contradiction
(audit-risk finding 2 on the prior revision) tightens this to a
consistent rule. A defaulted classifier with overridable defaults
drawn from real traffic is not a hack.

**Concern 6: `EPS_BURN_RATE = 1e-9`** — **removed in revision 2**.
The constant is no longer part of the design; bootstrap returns
`Option<f64>` and the `br is None` branch handles the unlearned
case without division (`proposals/03-load-balancing-tiers.md:360`).
The concern is moot for the current proposal. Corroborated by the
revision-rerun addendum (`tmp/03-risk-rerun-addendum.md:17-21`).

**Concern 7: `EPS_HOURS = 1.0 / 60.0`** — pre-existing floor at
`src-tauri/src/balancer/mod.rs:125-129`. The comment in-code
documents the reason ("a window 1 second from reset doesn't produce
infinite density"). The proposal preserves it unchanged (§4.7 at
line 360, "the existing one-minute floor carried over unchanged").
This is a legitimate divide-by-zero guard on a physically-meaningful
quantity (hours-until-reset); the one-minute choice is a bounded
floor, not a magic number hiding a missing derivation. Appropriate
scope control — churning on a derivation of the floor would
exceed this initiative's scope.

**Concern 8: `scripts/chatgpt-usage` tracked copy** — proper
version control, not source-vs-artifact confusion. Verified:
`/home/nes/projects/agent-runner/scripts/` already tracks
`anthropic-usage`, `claude-code-locate-transcript`,
`claude-code-turns`, `codex-locate-transcript`, `codex-turns`,
`migrate-model-names.sh`, `zai-usage`, but **not** `chatgpt-usage`.
The installed `/home/nes/.local/bin/chatgpt-usage` is the only
representation today. `scripts/README.md` documents these scripts
as reference adapter scripts wired via TOML, deployed to
`$PATH` by the user (`install -m 755 claude-code-turns codex-turns
~/.local/bin/`). The installed script is not generated; it is
manually deployed from the tracked source. Adding a tracked
`scripts/chatgpt-usage` closes a gap in the repository's source-
of-truth for its own reference scripts, aligning with the existing
`scripts/anthropic-usage` → `~/.local/bin/anthropic-usage` pattern.

**Concern 9: Soft-degrade for User at 70%** — explicit desired
behavior, not symptom-masking. Needs §4.2 literally states the
priority: "we'd still want to hit the weekly in that case because
we are at the edge of having real failures." The user asked for
soft-degrade over refusal below the failure threshold. The
`quota_tight_routing` column persists the decision per invocation,
the stderr warning (`[warn: no provider below user_threshold;
routing via quota-tight path]`) makes it visible at call time, and
post-hoc analysis can correlate `quota_tight_routing = true` rows
with actual failures. Observability + persisted audit trail +
explicit user intent = principled.

**Concern 10: §5.4 rollback plan** — policy-aligned, not a
shortcut. Making rollback transparent would require exactly the
dual-write that §4.10 forbids, or a compensating reverse migration
that re-adds the dropped `provider_quotas` delta columns and
refills them from window-level rows. §5.4 explicitly offers the
manual-repair path ("run a deliberate repair migration that re-adds
the provider-level columns") as an available recovery while
refusing to auto-install it ("do not add compatibility shims or
dual-write paths to make rollback transparent"). This is the no-
compat-shims policy operating as intended — rollback is possible
via deliberate operator action, but the wire is not left hot by
default.

**Concern 11: §3.3 empty-input `refreshed_at` update timing** —
principled under state-transition analysis, not masking refresh
failure. Trace:

- T=0: prior windows at 40%/30%, `refreshed_at=T0`; dynamic TTL
  say 2h.
- T=2h: TTL expired → `is_stale=true` → refresh fires → scraper
  returns `{"windows":[]}` → empty-write branch updates
  `refreshed_at=T2h` and `last_empty_refresh_at=T2h`, preserves
  windows at 40%/30%.
- T=3h: `is_stale` re-checks. `windows` is non-empty (preserved),
  so §3.2's empty-window guard does not fire. `dynamic_ttl_secs`
  recomputes from the preserved windows. As the preserved windows
  approach their own `resets_at`, `(w.resets_at - now).max(0)`
  tightens; `dynamic_ttl_secs` clamps to `MIN_TTL_SECS`, so
  retries fire at the floor cadence exactly when the preserved
  data becomes meaningfully stale.
- If the scraper recovers, a non-empty refresh lands on the
  wholesale-replacement branch and re-establishes fresh windows.
- If the scraper never recovers, preserved windows eventually
  pass their `resets_at`. Once passed, the hours-remaining min
  hits zero and dynamic TTL floors to `MIN_TTL_SECS` —
  aggressive retry kicks in precisely when the preserved data
  becomes unreliable.

This is "prefer stale-but-plausible over empty" rather than
"mask the failure." Answers §Q11 chose this explicitly because
empty state would drop the pool into `round_robin_fallback` with
no tier awareness, which is worse than bounded-stale data. The
preservation is bounded in time by the `resets_at` fields of the
preserved windows — not unbounded.

## Low-severity observations

1. **Carry-forward staleness horizon is explicit but not
   numerically bounded (§4.5).** Revision 2 added the staleness-
   rationale paragraph (lines 227–231) and a contingent deferral
   of the decay counter ("if observed workloads show drift"). The
   reasoning — bounded by `resets_at` and naturally retired as
   windows pass their reset time — is sound. If a workload shifts
   shape while its reset is still far out (for example, a sudden
   jump from short bursts to long batch work mid-week), the prior
   learned rate keeps being used until the next positive `dp`
   arrives. In practice this converges quickly because any
   sustained workload generates positive deltas on refresh. The
   deferral criterion is observable ("if observed workloads show
   drift"), which is better than an open-ended followup. Worth
   watching in tests but not a blocker.

2. **Risk-class cascade step 4 ("Background when `-f/--file` is
   provided").** The branch most likely to drift later if a new
   interactive mode ships that reads its prompt from a file. The
   current cascade will default to Background in that case. The
   explicit CLI flag and env var override (steps 1 and 2) cover
   this, so the failure mode is "classifier chose a reasonable
   default, workflow author overrides with `--risk-class user` if
   they know better." Acceptable given data-b §6.5 clustering, but
   an in-code comment tying each cascade branch back to its data-b
   cluster would ease later maintenance. Not a proposal-level
   blocker.

3. **§2.3's installed-path language.** The sentence "Change
   `/home/nes/.local/bin/chatgpt-usage:36-46`" could read as
   prescribing edits to the installed artifact. The actual tracked
   commit is the new `scripts/chatgpt-usage` file; the installed
   path is the user's deployment target, consistent with the
   existing `scripts/anthropic-usage` → `~/.local/bin/anthropic-usage`
   pattern in `scripts/README.md`. Not a shortcut; a minor clarity
   issue. A one-line clarification that the commit touches
   `scripts/chatgpt-usage` and that deployment to `~/.local/bin/`
   is a separate install step would remove ambiguity, but is not
   required.

4. **§4.10's negation of "no hidden fallback to old scalar
   scoring" is verifiable and verified.** The old scalar
   (`global_avg_percent_per_call`) is removed (§4.7 at line 284);
   the replacement reads `burn_rate_w` per window from the §4.6
   cascade. There is no scalar path to hide behind. Flagging only
   because any "no X" claim should be checked; this one holds.
