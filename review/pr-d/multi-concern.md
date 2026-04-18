# PR-D Multi-Concern Review

**Branch:** `feat/pr-d-sidechain-capture`
**Value under test:** V16 — one PR per concern unless mutually load-bearing.

## The concerns in the bundle

| # | Concern | Files |
|---|---|---|
| 1 | Additive schema: `parent_turn_id`, `is_sidechain` + two indexes | `state/db.rs` |
| 2 | `ScriptTurn` widened with two optional fields | `sessions/mod.rs` |
| 3 | `SessionTurnIngest` named-struct refactor of batch insert signature | `state/db.rs`, `state/mod.rs`, `sessions/mod.rs` |
| 4 | `count_session_turns` + `SessionTurnCounts` DB method | `state/db.rs`, `state/mod.rs` |
| 5 | Trace integration: populate `turn_count` / `assistant_turn_count` / `sidechain_turn_count` | `trace/mod.rs` |
| 6 | Trace graceful-degradation: count-failure → warning + None counts | `trace/mod.rs` |
| 7 | `claude-code-turns` emits `parentUuid` + `isSidechain` | `scripts/claude-code-turns` |

## Could any ship independently?

**Concerns 1, 2, 4, 5, 7 form one dependency chain.** Each link is empty without the others:

- Schema alone (1) adds two dead columns — zero visible value, zero test surface beyond “ALTER ran.”
- Widened `ScriptTurn` (2) without schema (1) or consumer (5) is plumbing that nothing observes.
- `count_session_turns` (4) without adapter emission (7) returns `sidechain: 0` always — the column is always-false — which is indistinguishable from a bug until end-to-end data lands.
- Adapter update (7) without ingest (2, 3) drops the new fields on the floor.
- Trace integration (5) is the first point where user-visible JSON gains a populated `sidechain_turn_count`.

This fits V16’s “mutually load-bearing” exception. Splitting produces 3–4 PRs whose individual test plans would be hollow — each would need end-to-end fixtures that only work once the others land, or tests that pass vacuously (`assert_eq!(sidechain, 0)`). That is the “real coupling pain” V16 permits bundling around.

**Concern 6 (graceful degradation)** is correctly coupled to concern 5: it is the V10 posture for the new DB call introduced in this PR. It should not land ahead of the call it protects, and it shouldn’t land later — shipping 5 without 6 would mean a DB error aborts the whole trace, which is the failure mode V10 forbids. Coupling is load-bearing.

**Concern 3 (`SessionTurnIngest` named struct)** is the one concern that is *technically* separable — it refactors a 4-tuple into a struct. But it is triggered directly by widening to 6 fields (positional `parent_turn_id`/`role` are easy to swap), and V14 rejects transitional dual-path shims. Landing the struct first as a pure refactor, then widening, would mean two reviews of overlapping surface. Bundling is the cheaper path under V14.

## Separability opportunities the PR missed — or rightly declined

One option the spec already foreclosed: `turn_count` and `assistant_turn_count` could, in principle, be populated purely from the existing `role` column with no schema work — that slice could have landed inside PR-B. PR-B’s §12 contract explicitly deferred all three counts to PR-D, so this is a proposal-level choice, not a PR-D bundling mistake. Given the deferral, filling all three here is the right move: a standalone count PR between B and D would have touched `trace/mod.rs` twice for the same surface.

## Verdict

**No split recommended.** The seven concerns collapse to one concern — “within-session sidechain visibility” — plumbed through the only layers that can carry it (schema → struct → adapter → query → trace). The incidental refactor (`SessionTurnIngest`) and the V10 warning path are co-located with the call site they exist to serve. Bundle is within the V16 exception; ship as one PR.
