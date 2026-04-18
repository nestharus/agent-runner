# PR-C Multi-Concern Review

## Summary

PR-C bundles eight items across configuration, executor, DB, lifecycle, trace, and reference scripts. The proposal's §12 carved this out as a single PR after deliberation (§13 Scope F1 cited V16). The eight items cluster cleanly into two groups; one group has independent user value at merge, the other does not. The bundle is defensible, but a 2-PR split would be a legitimate V16 refinement rather than a violation. No blocking finding.

## Cluster analysis

**Cluster A — capture**: `session_capture` field on `ProviderConfig`, executor capture-aware dispatch (FFV + JSON event), DB columns (`session_id`, `session_capture_method`, index), `update_session_capture` method, `main.rs` lifecycle wiring.

**Cluster B — locator**: `transcript_locator` field on `SessionSourceEntry`, `locate_transcript` in `sessions/mod.rs`, trace integration in `trace/mod.rs`, two reference locator scripts.

### Intra-cluster coupling (mutually load-bearing)

Cluster A pieces are genuinely coupled: the config field is dead without executor dispatch; dispatch output is discarded without DB columns; the DB method is never called without main.rs wiring. Splitting within cluster A would produce PRs with no user-visible effect at merge.

Cluster B pieces are similarly coupled: the config field feeds `locate_transcript`, which feeds trace integration; reference scripts demonstrate the contract the field/function accept. Splitting within cluster B would leave dangling infrastructure.

### Cross-cluster independence

- **Cluster A alone, shipped first**: `trace --json` gains real `session.id` and `capture_method = forced_flag_verified | stdout_json_event`. That is visible, independent user value against the PR-B baseline. `transcript_state` stays `unresolved` everywhere, but capture metadata becomes useful immediately (direct DB queries, future tooling, diagnostics). This passes the V16 "visible user value at merge" bar.
- **Cluster B alone, shipped first**: without Cluster A, every invocation row has `session_id = NULL`, so the trace resolver exits at the early `Unresolved` branch ([trace/mod.rs:278](/home/nes/projects/agent-runner/src-tauri/src/trace/mod.rs:278)) and `locate_transcript` is never invoked. Zero user-visible effect. Fails the V16 "independent value" bar unless reframed as a strict prerequisite for a follow-up.

The natural split would therefore be ordered (A → B), with B having prerequisite-only value. That is not the strongest V16 case for splitting.

## V16 weighing

Arguments to split (A / B):
- Total diff is ~2042 lines across 12 files; ~700-line Cluster A and ~500-line Cluster B would each be more comfortably reviewable in one pass.
- Two clearly separate failure domains (CLI dispatch vs. filesystem adapter).
- Test-quality and coverage-delta audits already flag gaps in both halves separately, suggesting reviewers naturally think of them as two concerns.

Arguments against splitting:
- Cluster B has no independent user value at merge; it only unlocks the transcript-state surface that was the whole point of shipping the locator at all. The V16 rubric favors "smallest PRs that each ship independent value" — a strict-prerequisite PR that doesn't itself deliver a visible effect is the weaker side of that tradeoff.
- The proposal's §13 already split the original monolithic proposal into four PRs via risk gate; PR-C is the "session correlation" concern as deliberated. Further subdivision goes past what the proposal approved.
- Bundle is coherent: capture writes the column, locator reads it at trace time. Review cost of understanding them together is low because they share vocabulary and data flow.
- Cluster A by itself already improves `trace` (populates `session.id` / `capture_method`), so deferring B doesn't strand A's value.

On balance, the bundle matches the risk-gate approval and avoids a prerequisite-only PR. The split would be legitimate but not clearly superior.

## Anti-scope check

PR-C correctly excludes PR-D sidechain work, README sweep, and `claude-code-turns` changes. No new Cargo dependencies. No trans-cluster scope leak (e.g. no transcript content copied to SQLite, no hardcoded provider paths in the runner). Reference scripts live entirely in `scripts/` and are user-replaceable.

## Finding

**Low — bundled clusters could be split A→B.** Cluster A (capture) has independent user value; Cluster B (locator) does not without A. Current bundle matches the proposal's approved decomposition and keeps prerequisite-only work out of its own PR. Not a blocker. If the team later finds PR-C review fatigue, the A/B boundary is the cut line to use.

## Verification

- `git diff main..HEAD --stat` — 12 files, ~2042 insertions, all within declared PR-C surface.
- Cross-referenced against `proposals/01-trace-inspection.md` §12 PR-C and `tmp/01-pr-c-contract.md` Files-expected-to-change list: no files changed outside the declared scope.
