# Quota-Tier Load Balancing: Needs Synthesis

This document maps the problem research in
`research/03-load-balancing-tiers-problem.md` to the user's specific
operational context, resolves the open framing questions from that
document, and enumerates the design questions that phase 3 (proposal)
must answer. It does not select a design.

Phase gate: this is the phase 2 deliverable, written by the
orchestrator (`claude-opus`). Phase 3 (proposal) runs as `gpt-high`
against this document plus the phase 1 research.

## 1. Scope confirmed with user

### 1.1 What is in scope

Three defects, treated as a single initiative because they share
schema and scoring code:

| Defect | Anchor |
| --- | --- |
| A — tier quantities not weighted when aggregating windows into a per-provider score | `research/03-load-balancing-tiers-problem.md` §4 |
| B — between-refresh turn projection uses one pool-wide scalar across all windows of a provider | `research/03-load-balancing-tiers-problem.md` §5 |
| C — no caller risk class (user-facing vs background) distinguishes traffic that tolerates mid-call failure from traffic that does not | `research/03-load-balancing-tiers-problem.md` §6 |

Expected combined effect: the high-weekly account should stop winning
round-robin picks once its projected weekly runway falls below peers,
and interactive calls should stop being routed to a provider whose
runway is likely to exhaust before the call finishes.

### 1.2 What is explicitly out of scope

Recorded here so phase 3 does not absorb them:

| Out-of-scope item | Rationale |
| --- | --- |
| Rewriting the quota scraper contract | Scraper output shape is an input to this initiative, not a subject of it (`src-tauri/src/quota/mod.rs:100-127`, §2.2 of phase 1). |
| Rewriting session ingestion | Assistant-turn counts via `session_turns` remain the source of in-flight turn projection (`src-tauri/src/sessions/mod.rs:53-127`). |
| Two-DB split between CLI and Tauri paths | CLI one-shot, CLI repl, and Tauri `test_model` currently open different `state.db` files (phase 1 §6.4). Needs its own initiative; treat as one logical store for design purposes but assume both code paths share a schema. |
| Sub-agent traffic as a distinct class | Per user: every agent call is atomic. Failure = one agent call fails mid-execution. Lineage does not confer its own risk class. See §4.1. |

### 1.3 Pulled into scope after initial framing

Two items were initially framed as out-of-scope but user confirmed
they must be fixed as part of this initiative. Both are prerequisites
for the redesign being testable and observable in production.

| Item | Why it is in scope |
| --- | --- |
| `is_stale` empty-windows defect | A provider row with zero windows currently gets `MAX_TTL_SECS = 24h` (`src-tauri/src/quota/mod.rs:148-151`). This is the direct cause of `claude2` never being refreshed. Detailed diagnosis in §5.1. The redesign cannot be validated on `claude2` until this is fixed, and the bug independently causes production imbalance. |
| `chatgpt-usage` second-window drop | Upstream Codex CLI reports two tiers (5h + weekly — user observation 2026-04-21, matching user paste `96% left / 95% left`). Current `chatgpt-usage` scraper returns only one window. Multi-tier weighting cannot affect Codex providers until the second window is emitted. Detailed diagnosis in §5.2. |

These two fixes are small and localized, but they gate validation of
the scoring redesign. Phase 3 should sequence them as prerequisites,
not bundle them into the scoring change commit.

## 2. Axis A — tier quantity weighting, mapped to user context

User statement: "The smaller tiers are just slices of the large tiers."
This is a structural fact about the provider's quota model, not a
choice the balancer is free to make. The 5h budget is a sub-interval
of the weekly budget; a turn consumed now debits both.

Consequences for phase 3:

- The scorer must be able to compare two windows of a single provider
  on a commensurate quantity axis, not on normalized percents.
- The only commensurate quantity the system can learn from current
  data is "turns remaining in this window," because turns are the one
  unit we already ingest (`count_assistant_turns_since`, phase 1
  §2.3). Raw API tokens are not observable from the scraper contract.
- The current scorer cannot express "A at 80% weekly / 4% 5h vs B at
  10% weekly / 85% 5h" in a way that reflects runway — worked example
  in phase 1 §4.2 confirms the `min(density)` formula will pick the
  account with the pressed short tier whenever the weekly tier has
  the longer remaining hours.

What phase 3 has to decide (design questions, not answered here):

- Whether to store a learned capacity per window or to derive it on
  the fly per pick.
- Whether the slice relationship between short and long windows
  should be stored explicitly in the schema, or treated as a property
  of how capacity is derived (short window capacity will always be a
  smaller turn count than long window capacity if both are learned
  from the same observed deltas).
- Whether to expose tier weighting as a model-TOML option or hardcode
  it from the learned capacities.

## 3. Axis B — per-window burn rate

User statement: "We are accumulating [turns] at equal rates across all
tiers." This is accurate to current behavior. Phase 1 §5 traces the
cause: `last_delta_percent` / `last_delta_calls` is a single pair on
`provider_quotas`, learned from the longest window only
(`src-tauri/src/state/db.rs:1148-1182`), and the projection loop
multiplies one pool-wide average by the ingested turn count and adds
the same amount to every window (`src-tauri/src/balancer/mod.rs:120-130`).

Representative deltas (user confirmed 2026-04-21 that live DB values
are representative, not illustrative):

| Provider | Window kind | Stored `last_delta_percent` / `last_delta_calls` | Corresponds to |
| --- | --- | ---: | --- |
| `claude` | longest only | `0.01 / 22` | Long (7d) tier drift per turn on this account |
| `claude2` | longest only | `0.06 / 2194` | Long (7d) tier drift per turn on this account |
| `claude3` | longest only | `0.01 / 80` | Long (7d) tier drift per turn on this account |
| `codex` | longest only | `0.02 / 579` | Whatever single window `chatgpt-usage` currently returns |
| `codex2` | longest only | `0.02 / 305` | Same |

Consequences for phase 3:

- A single provider-level scalar cannot drive two different windows'
  projections. The short window's drift per turn is intrinsically
  larger than the long window's drift per turn for the same call
  rate.
- The representative deltas above are learned only against the
  longest window. There is no stored signal for how much a turn
  debits the short window, so phase 3 has to decide whether to:
  - learn a second per-window delta on refresh, or
  - derive short-window drift from long-window drift plus the ratio
    of window durations (as a fallback only), or
  - bootstrap short-window drift to a sentinel value until a refresh
    lands.
- Sharing learned burn rates across sibling providers on the same
  plan is worth considering — `claude`, `claude2`, `claude3` all hit
  the same account-class plan and their deltas should converge.
  Whether to share them is a phase 3 decision.

User-confirmed symptom this fix must extinguish: "one account is
getting very high on weekly usage while the other accounts are quite
low, but the high account keeps getting hit as turns get
accumulated." Phase 3 must be able to explain, in writing, how its
projection equation makes that stop happening against the
representative deltas above.

## 4. Axis C — risk class per call

### 4.1 User-confirmed category set

Only two risk classes:

| Class | Failure tolerance | Typical origin |
| --- | --- | --- |
| `User` | Interactive caller is waiting. A mid-call failure is visible to the user and unrecoverable for that prompt. | Interactive REPL, Tauri UI test, any CLI invocation driven by a human sitting at the terminal |
| `Background` | Failure is retryable at the workflow layer. The one agent call failing is the only cost. | Workflow-driven agent dispatch, automation |

User clarification 2026-04-21: "sub-agents of a workflow don't
contribute to background workflows. All work is split by agent calls,
so it is that an agent can fail mid-call." Interpretation: risk class
is a property of the individual agent call, not inherited from a
parent. A sub-agent launched from a background workflow is still a
`Background` call; a sub-agent launched from an interactive REPL is
still a `User` call. Lineage (`parent_invocation_id`) does not
determine class.

### 4.2 Two threshold classes the user named

| Threshold | Fires at (user-named) | Intended effect per user wording |
| --- | --- | --- |
| `user_prompt_exhaustion_risk` | ~70% projected | "Approaching risk territory where user prompting can start to fail if there's not enough runway left." Providers over this threshold should not be selected for `User` calls. |
| `mid_operation_failure_risk` | ~95% projected | "Agents failing mid-operation." Providers over this threshold should not be selected for any call. |

Both thresholds evaluate on **projected** usage (after adding
in-flight turns), not just the last refreshed percent. The projection
math in axis B applies.

Phase 3 decides the exact numbers; the user named 70 and 95 as the
conceptual anchor, not the final constants.

### 4.3 Where the class has to be plumbed

Three call sites today (phase 1 §6.1). Each has to label itself:

| Call site | Natural class | Notes |
| --- | --- | --- |
| CLI one-shot (`src-tauri/src/main.rs:589-645`) | Depends on argv origin | A one-shot invocation driven by a workflow is `Background`; a one-shot invocation typed by the user at a terminal is `User`. Cannot be inferred from argv alone — phase 3 must decide how the runner tells the two apart. |
| CLI interactive `repl` (`src-tauri/src/main.rs:430-526`) | `User` | The interactive mode exists specifically for human-in-the-loop. |
| Tauri `test_model` (`src-tauri/src/lib.rs:471-494`) | `User` | Driven from the app UI. |

The first row is the one phase 3 has to answer. Options that exist
without new plumbing: environment variable set by the workflow, a new
CLI flag, or inference from the presence of `OULIPOLY_PARENT_INVOCATION`
(that env var today means "you were launched from something that
tracked you" — it does not today imply background). Phase 3 picks an
approach; phase 1 §6.2 enumerates the metadata actually flowing.

### 4.4 Behavior when no provider passes the gate

Phase 3 must answer:

- If all providers fail the `User` gate (≥70% projected), is a `User`
  call refused with an explicit "quota-tight" error, or is it
  degraded to the `Background` gate?
- If all providers fail the `Background` gate (≥95% projected), is
  the call refused or round-robined?

The user's priority — per "we'd still want to hit the weekly in that
case because we are at the edge of having real failures" — suggests
the 95% gate is a hard block, and the 70% gate is a soft block that
may be lifted only when the user class has no alternative. Phase 3
confirms.

## 5. In-scope prerequisite defects

Both items in this section are now part of the initiative (§1.3) but
are diagnosed here rather than in phase 1 because they surfaced
during scope confirmation, after phase 1 was closed.

### 5.1 `claude2` is starved by an `is_stale` TTL inversion

Phase 1 §3 shows `claude2` has a `provider_quotas` row but zero
`provider_quota_windows` rows. Live probe on 2026-04-21:

```
$ anthropic-usage ~/.claude2/.credentials.json
{
  "windows": [
    {"used_percent": 47.0, "resets_at": "2026-04-23T19:00:00.208630+00:00"},
    {"used_percent": 29.0, "resets_at": "2026-04-21T12:00:00.208614+00:00"}
  ]
}
```

The scraper returns two windows correctly, so this is not a scraper
defect.

Defect location: `src-tauri/src/quota/mod.rs:148-151`.

```rust
pub fn dynamic_ttl_secs(windows: &[crate::state::QuotaWindow]) -> i64 {
    if windows.is_empty() {
        return MAX_TTL_SECS;  // 24 hours
    }
```

The fallback branch is inverted. A quotas row with zero windows is
inconsistent state — the record says a refresh succeeded but the
window array is missing. That is the case where refresh should be
forced, not deferred for 24 hours. The current code defers for 24
hours.

Full feedback loop observed on this machine:

1. At some earlier point, `claude2`'s windows became empty in the DB
   (root cause of the initial wipe not yet confirmed; candidates
   include a transient `{"windows": []}` from the scraper or a
   historical migration path — out of scope for this defect's fix,
   which is to make the system self-heal).
2. With zero windows, `dynamic_ttl_secs` returns `MAX_TTL_SECS` (24h)
   (`src-tauri/src/quota/mod.rs:148-151`).
3. `claude2.refreshed_at = 2026-04-20T16:43:16Z`. Age at capture
   time (2026-04-21T10:40Z) ≈ 17h 57m, which is less than 24h, so
   `is_stale` returns `false` (`src-tauri/src/quota/mod.rs:132-143`).
4. `select_provider` skips the refresh branch for `claude2`
   (`src-tauri/src/balancer/mod.rs:36-47`).
5. Because `claude2` still has zero windows, the pool fails
   `all_have_windows` and drops to invocation-count fallback
   (`src-tauri/src/balancer/mod.rs:62-69`).
6. In invocation-count fallback, `claude2` loses to `claude` and
   `claude3` because its lifetime `invocation_count` is higher (its
   `last_delta_calls = 2194` on the stored longest-window delta is
   consistent with a long historical usage streak).
7. `claude2` is never picked, never accumulates a turn that would
   trigger an out-of-band refresh via the executor's per-invocation
   increment, and stays wedged until the 24h timer elapses.
8. At the 24h mark, `is_stale` returns `true`, the refresh fires,
   the two windows are restored, and the system heals — until the
   next time an empty-windows state occurs.

Required behavior for the fix: `is_stale` must treat "quotas row
exists but windows array is empty" as a forced-stale condition.
This is a one-line semantic fix to `dynamic_ttl_secs` or `is_stale`
itself. Phase 3 decides placement.

Secondary investigation for phase 3: identify and close the path that
produces the zero-window state in the first place. The current
`upsert_quota_refresh` deletes then re-inserts; if the script ever
emits `{"windows": []}` it will wipe without replacement. Phase 3
must decide whether to reject empty-window writes, log them, or keep
current behavior and rely on the `is_stale` fix to heal quickly.

### 5.2 `chatgpt-usage` drops the second Codex window

User's Codex CLI output on 2026-04-21 shows:

```
 5h limit:     96% left (resets 07:27)
 Weekly limit: 95% left (resets 16:26 on 27 Apr)
```

Live probe of the configured quota script on the same day returned a
single flat window:

```
$ chatgpt-usage ~/.codex/auth.json
{"used_percent": 5, "resets_at": "2026-04-27T23:26:11Z"}
```

That matches the DB — `codex` has one window row on `2026-04-27` and
no 5h row (phase 1 §3). The scraper is dropping the 5h window. Codex
providers will not benefit from this initiative's tier weighting
until `chatgpt-usage` is fixed to emit both windows. Track as its own
scraper bug; do not absorb into this initiative.

## 6. Phase 3 design questions (not answered here)

Phase 3 (proposal, `gpt-high`) must answer each of these in its
design. They are written as questions, not as solutions.

1. What unit does the per-provider binding score use after the
   redesign? (Phase 1 shows percents/time is current; user framing
   implies turns/time; phase 3 confirms.)
2. How is per-window burn rate learned and where is it stored?
   Per-window column on `provider_quota_windows`, or a side table, or
   reconstructed from history?
3. How are burn rates bootstrapped before the first refresh-to-refresh
   observation lands for a window, given that the existing code only
   writes the longest window's delta?
4. Do sibling providers on the same plan class share learned burn
   rates, or each learn independently?
5. How is the in-flight turn count projected into each window between
   refreshes, once per-window burn rates exist?
6. How does a caller declare `User` vs `Background` class in the
   CLI one-shot path, where today no field carries it?
7. What are the final threshold numbers, and are they configurable
   per model/per pool, or pool-global?
8. What happens when all providers fail the `User` gate (soft-block
   or refuse)? What happens when all providers fail the `Background`
   gate (refuse or round-robin)?
9. Does the redesign change the fallback path that kicks in when any
   provider in a pool has zero windows, or does that fallback remain
   invocation-count as today (phase 1 §1)?
10. Sequencing: §5.1 (`is_stale` empty-windows) and §5.2
    (`chatgpt-usage` second-window) are both in scope (§1.3). The
    proposal must sequence them as prerequisite commits before the
    scoring redesign, not bundle them in. What order, and do they
    share a PR or ship separately?
11. For §5.1 specifically, should the fix also reject empty-window
    writes in `upsert_quota_refresh`, or only correct the
    `is_stale` branch? Each has different failure semantics — one
    prevents the wipe, the other makes the wipe self-heal rapidly.

## 7. Deliverable readiness

With §1 through §6 resolved at the framing level:

- Phase 1 (problem research) is closed.
- Phase 2 (this document) is the input to phase 3.
- Phase 3 (proposal, `gpt-high`) can begin once the user confirms the
  scope mapping in §1.2 (especially the out-of-scope list) and the
  risk-class framing in §4.

Phase 3 output will land in `proposals/03-load-balancing-tiers.md`.
Phase 4 (3x risk assessment, `claude-opus` in parallel) will run
against that proposal before any implementation starts.
