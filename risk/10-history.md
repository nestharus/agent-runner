# Audit history — initiative 10 routing-claude-skipped (Phase 4 risk gate loop)

## Purpose

Track Phase 4 revise/review rounds for `proposals/10-routing-claude-skipped.md`.
The loop verdict each round is `continue` (revise + re-run gates) or `apply`
(all four gates LOW with no termination signal, advance to Phase 5).

## Artifact lineage

- Proposal: `proposals/10-routing-claude-skipped.md`
- Phase 0 RCA: `research/10-routing-claude-skipped-rca.md`
- Phase 0 reproduction harness: `src-tauri/tests/rca_routing_claude_skipped.rs`
- Phase 2.5 problem map: `research/10-routing-claude-skipped-problem-map.md`
- Risk artifacts (current round): `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md`

## Round summaries

### Round 1 — initial proposal reviewed

- Artifact under review: `proposals/10-routing-claude-skipped.md` at
  `b92fb14`.
- Round artifacts: `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md` (round-1 commits
  `697c2d9`, `8ae5c76`, `2911ec5`, `2ecb0da`).
- Report artifacts:
  - report index: none
  - PDFs: none
  - uploaded artifact URL: none
  - screenshots: none
  - non-UI evidence: prompt+log pairs in `.tmp/routing-claude-skipped-risk-*`
- Prior finding counters:
  - closed: 0
  - intact: 0
  - weakened: 0
  - regressed: 0
  - not closed: 0
- New findings:
  - `R1-F01` — medium; audit gate flagged that the migration section did
    not state error/rollback behavior on partial failure or unexpected
    `providers` shapes; ancestor chain: none; oscillation: none.
  - `R1-N01` — note (low); shortcut gate flagged that
    `last_error_at = MAX(finished_at)` over all terminal rows could
    accidentally pick the most recent successful timestamp instead of the
    most recent failed one; ancestor chain: none; oscillation: none.
  - `R1-N02` — note (low); supported-surface gate flagged that
    `src-tauri/examples/quota_check.rs:123` calls `get_provider(name, i)`
    with an index argument and will fail to compile after the signature
    change; ancestor chain: none; oscillation: none.
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 0
- Decompose trigger: not fired (single MEDIUM finding plus two
  documentation/precision notes — well within revise scope).
- Watch signals: migration error/rollback paragraph and its tests must
  hold across rounds; no further index-keyed surface may resurface.
- Verdict / determination: `continue` — revise the proposal, re-run all
  four gates per Phase 4 rule "any MEDIUM or HIGH report means revise the
  proposal and re-run all four."
- Role outputs:
  - audit (gpt-high): `risk/10-audit.md` MEDIUM
  - scope (claude-opus): `risk/10-scope.md` LOW
  - shortcut (claude-opus): `risk/10-shortcut.md` LOW
  - supported-surface (claude-opus): `risk/10-supported-surface.md` LOW
    (termination=none)
- Next handoff: round 2 risk-gate fanout reads the revised
  `proposals/10-routing-claude-skipped.md`, this audit-history file's
  Round 1 entry, and the round-1 risk artifacts. Each gate must verify
  whether `R1-F01`, `R1-N01`, and `R1-N02` are closed by the revision.

## Decision register

- Decision D1: keep aggregate identity at `(model_name, provider_name)`;
  reject Option B (composite with provider_index) because it would split
  history across reorders. Source: proposal §Design Rationale; affirmed by
  scope and shortcut gates.
- Decision D2: discard pre-fix `providers.last_error` snippets during
  migration; rebuild from `invocations.error_category`. Source: assumption
  A4; affirmed by supported-surface gate.
- Decision D3: do not introduce a `PRAGMA user_version` write path.
  Source: assumption A5; affirmed by shortcut gate.
- Decision D4: include `recent_error_count` identity fix in the same diff
  as the aggregate fix. Source: scope of fix is "both index-keyed surfaces
  named in the problem map"; affirmed by scope and shortcut gates.
- Decision D5 (round 1 → round 2 revision): make the migration error
  contract explicit; add explicit `last_error_at` failure-row semantics;
  add `examples/quota_check.rs` to the in-scope diff; add three new
  test-intent entries (unexpected shape rejection, transactional rollback
  via rename-conflict, last_error_at vs success). Source: closes
  `R1-F01`, `R1-N01`, `R1-N02`.

### Round 2 — revised proposal reviewed

- Artifact under review: `proposals/10-routing-claude-skipped.md` at
  `b782b4d`.
- Round artifacts: `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md` (round-2
  commits `ceb60ba`, `1948f5c`, `2fdf2db`, `ddad20e`).
- Report artifacts:
  - report index: none
  - PDFs: none
  - uploaded artifact URL: none
  - screenshots: none
  - non-UI evidence: prompt+log pairs in
    `.tmp/routing-claude-skipped-risk-*-round2.{md,log}`
- Prior finding counters:
  - closed: 3 (R1-F01, R1-N01, R1-N02 all closed)
  - intact: 0
  - weakened: 0
  - regressed: 0
  - not closed: 0
- New findings:
  - `R2-F01` — medium; shortcut gate flagged that the round-2
    "Migration rollback on failed rebuild" test (using a pre-existing
    `providers_legacy_index_keyed` to force the `RENAME` to fail) does
    not actually exercise transactional rollback because the
    rename-conflict failure happens before any rebuild SQL runs; an
    implementation that omits the transaction wrapper would still
    pass the test. Ancestor chain: none. Oscillation: fix-created
    (introduced by the round-1 → round-2 revision that tried to close
    `R1-F01`).
  - `R2-N01` — low note; shortcut gate flagged that the migration
    error contract did not document an operator-level recovery
    procedure when `StateDb::open` rejects a malformed `providers`
    shape. Ancestor chain: none.
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 1 (`R2-F01`)
- Decompose trigger: not fired (one MEDIUM finding plus one low note,
  both addressable by replacing one test entry and adding one
  documentation paragraph; well within revise scope).
- Watch signals: `WS-3` added — the rollback property of
  `ensure_providers_schema` is now an explicit residual verified by
  code review only; no future round may quietly re-promote it to a
  runtime test claim without specifying a viable failure-injection
  mechanism.
- Verdict / determination: `continue` — revise the proposal, re-run
  all four gates per Phase 4 rule "any MEDIUM or HIGH report means
  revise the proposal and re-run all four."
- Role outputs:
  - audit (gpt-high): `risk/10-audit.md` LOW (R1-F01 closed)
  - scope (claude-opus): `risk/10-scope.md` LOW
  - shortcut (claude-opus): `risk/10-shortcut.md` MEDIUM (R1-N01
    closed; new R2-F01 medium + R2-N01 low note)
  - supported-surface (claude-opus): `risk/10-supported-surface.md`
    LOW termination=none (R1-N02 closed)
- Next handoff: round 3 risk-gate fanout reads the further-revised
  `proposals/10-routing-claude-skipped.md`, this audit-history file's
  Round 2 entry, and the round-2 risk artifacts. Each gate must
  verify whether `R2-F01` and `R2-N01` are closed by the round-3
  revision, and that the round-1 closures (`R1-F01`, `R1-N01`,
  `R1-N02`) remain closed.

## Watch signals

- `WS-1`: any future round must re-check that the migration helper
  remains transactional and rejects unexpected shapes — it must not
  acquire heuristic recovery affordances.
- `WS-2`: any future round must re-check that no index-keyed reader
  alias has been kept on the routing-history surface
  (`providers`/`get_provider`/`recent_error_count`).
- `WS-3`: rollback during mid-rebuild failure is now an explicit
  residual verified by code review only. Future rounds must not
  silently re-promote it to a runtime test claim without specifying a
  viable failure-injection mechanism that does not require test-only
  product source.

### Round 3 — twice-revised proposal reviewed

- Artifact under review: `proposals/10-routing-claude-skipped.md` at
  `023635f`.
- Round artifacts: `risk/10-audit.md`, `risk/10-scope.md`,
  `risk/10-shortcut.md`, `risk/10-supported-surface.md` (round-3
  commits `84e4271`, `2900ba5`, `0d05077`, `7b7797a`; merged at
  `608fdcc`).
- Report artifacts:
  - report index: none
  - PDFs: none
  - uploaded artifact URL: none
  - screenshots: none
  - non-UI evidence: prompt+log pairs in
    `.tmp/routing-claude-skipped-risk-*-round3.{md,log}`
- Prior finding counters:
  - closed: 2 (R2-F01, R2-N01 both closed)
  - intact: 0
  - weakened: 0
  - regressed: 0
  - not closed: 0
- New findings: none
- Oscillation:
  - same-label: 0
  - same-family: 0
  - fix-created: 0
- Decompose trigger: not fired.
- Watch signals: WS-1 / WS-2 / WS-3 all upheld this round.
- Verdict / determination: `apply` — all four gates LOW with
  termination=none on the supported-surface gate. Phase 4 passes;
  advance to Phase 5 (hookpoint research).
- Role outputs:
  - audit (gpt-high): `risk/10-audit.md` LOW (R1-F01 still closed)
  - scope (claude-opus): `risk/10-scope.md` LOW
  - shortcut (claude-opus): `risk/10-shortcut.md` LOW (R2-F01 closed,
    R2-N01 closed)
  - supported-surface (claude-opus): `risk/10-supported-surface.md`
    LOW termination=none
- Next handoff: Phase 5 hookpoint research consumes the approved
  proposal, the problem map, and the four LOW risk reports.

## Process-tree dispatch evidence

Phase 4 was run as three rounds of four parallel risk-gate dispatches.
Each round:

- Each gate ran in its own git worktree under
  `.worktrees/rca-routing-risk-{audit,scope,shortcut,supported-surface}`,
  satisfying `~/ai/conventions/worktree-isolation.md`.
- Models followed `~/ai/models/roles.md`: audit → `gpt-high`;
  scope, shortcut, supported-surface → `claude-opus`.
- Prompt and log pairs are preserved at
  `.tmp/routing-claude-skipped-risk-{audit,scope,shortcut,supported-surface}-round{1,2,3}.{md,log}`.
- Round-1 risk artifacts were preserved as separate commits before
  octopus-merge into the parent branch; round-2 and round-3 artifacts
  overwrite the same paths but git history retains every prior round
  via the per-round commit chain.
- A formal `process-tree-auditor` invocation was not run because the
  pipeline was orchestrated from Claude Code rather than under a
  single `agents` root UUID; the dispatch evidence above is the
  equivalent record. If a future round requires it, the
  prompt+log+commit chain is sufficient to reconstruct the process
  tree.

## Watch signals

- `WS-1`: any future round must re-check that the migration helper
  remains transactional and rejects unexpected shapes — it must not
  acquire heuristic recovery affordances.
- `WS-2`: any future round must re-check that no index-keyed reader
  alias has been kept on the routing-history surface
  (`providers`/`get_provider`/`recent_error_count`).
- `WS-3`: rollback during mid-rebuild failure is now an explicit
  residual verified by code review only. Future rounds must not
  silently re-promote it to a runtime test claim without specifying a
  viable failure-injection mechanism that does not require test-only
  product source.

## Final state

Phase 4 passed at round 3 with all four gates LOW and
termination=none. Determination: `apply`. Pipeline advances to
Phase 5.
